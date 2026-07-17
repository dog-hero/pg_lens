//! Time-series history of snapshot-derived metrics.
//!
//! [`SnapshotHistory`] is a fixed-capacity ring buffer (`VecDeque`) that the
//! **poller** owns and grows incrementally — one [`HistoryPoint`] per poll,
//! never rebuilt. A clone of the ring travels inside every [`DbSnapshot`]
//! envelope, so all consumers (the TUI's sparklines/trend arrows and the
//! web's uPlot charts) see the exact same series; that is why this lives in
//! the core and derives `Serialize`.
//!
//! [`DbSnapshot`]: crate::models::DbSnapshot

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Default ring capacity: 1800 samples = 1 hour at the 2s default poll (and
/// proportionally longer at slower cadences). Large enough that the chart
/// spans hours; combined with [`crate::history_store`] the series also
/// survives restarts. ~36 KB per snapshot clone — trivial at one clone/tick.
pub const DEFAULT_CAP: usize = 1800;

/// Wall-clock timestamp for a new [`HistoryPoint`], in Unix epoch
/// milliseconds. Falls back to `0` if the system clock predates the epoch.
pub fn epoch_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One time-series sample, taken by the poller as it publishes a snapshot.
/// `Deserialize` too, so [`crate::history_store`] can reload persisted points.
///
/// v0.14 widened this struct with four more per-tick scalars so the Macro
/// Lens trend arrows (`trend`, below) have a ~5-minute-ago baseline to
/// compare against, without any new SQL or cadence: every field here is
/// something the poller already had in hand for [`crate::models::ServerVitals`]
/// / [`crate::models::LockCapacity`] / the Schema Lens's vacuum-age
/// collection. All four are `#[serde(default)]` so history JSONL files
/// written before this widening keep loading (missing fields become their
/// default: `0` or `None`) instead of being dropped by
/// [`crate::history_store::HistoryStore::load`]'s tolerant parsing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryPoint {
    /// When the sample was taken (Unix epoch milliseconds).
    pub epoch_ms: u64,
    /// Transactions per second (delta-derived by the poller).
    pub tps: f64,
    /// Number of `active` backends at that moment.
    pub active_sessions: u32,
    /// `ServerVitals::connections_total` at that moment — the fast tick
    /// already reads this from `pg_stat_activity`.
    #[serde(default)]
    pub connections_total: u32,
    /// `ServerVitals::cache_hit_ratio` (delta-derived) expressed as a
    /// percentage (`0.0..=100.0`). `None` only for points loaded from a
    /// pre-widening JSONL file — the real poller always has a reading (even
    /// the first tick of a session falls back to the cumulative ratio, see
    /// `poll_once`).
    #[serde(default)]
    pub cache_hit_pct: Option<f32>,
    /// `LockCapacity::used_fraction` (fast tick, best-effort) as a
    /// percentage. `None` when the lock-capacity gauge itself was
    /// unavailable that tick (restricted role, etc.), not just on old data.
    #[serde(default)]
    pub lock_pressure_pct: Option<f32>,
    /// `VacuumClusterAge::max_age_xids` — the oldest-transaction-ID age
    /// driving wraparound risk. This is Schema Lens data, collected on the
    /// SLOW cadence (default 60s), not the fast 2s tick that produces every
    /// `HistoryPoint`. Rather than leave it `None` between refreshes (which
    /// would make the trend arrow flap to "no data" every tick), the poller
    /// carries the last known value forward across fast ticks — the series
    /// is a step function that updates every slow-cadence collection, which
    /// is exactly the resolution this metric changes at anyway. `None` only
    /// before the first successful schema collection of a session, or when
    /// the source view is unavailable.
    #[serde(default)]
    pub oldest_xid_age: Option<i64>,
}

/// Direction of a metric over a comparison window, with a deadband so
/// sampling noise doesn't flap the arrow between ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Trend {
    Up,
    Down,
    Flat,
}

/// Compares `now` against `then` (typically ~5 minutes / ~150 ticks apart at
/// the 2s default cadence) and classifies the direction, in ONE pure
/// function shared by every frontend that draws a trend arrow. `deadband` is
/// a *relative* threshold (e.g. `0.05` = 5%): changes smaller than that
/// fraction of `then`'s magnitude are `Flat`, which absorbs normal
/// tick-to-tick noise on a metric like cache-hit percentage that otherwise
/// wobbles within a couple of points every poll.
///
/// When `then` is `0.0` the relative comparison is undefined, so this falls
/// back to an absolute deadband of `deadband` itself (still `Flat` for a
/// `0.0 -> 0.0` non-change).
pub fn trend(now: f64, then: f64, deadband: f64) -> Trend {
    let deadband = deadband.abs();
    let threshold = if then.abs() > f64::EPSILON {
        then.abs() * deadband
    } else {
        deadband
    };
    let delta = now - then;
    if delta.abs() < threshold {
        Trend::Flat
    } else if delta > 0.0 {
        Trend::Up
    } else {
        Trend::Down
    }
}

/// The default relative deadband for trend arrows: changes under 5% of the
/// baseline are noise, not a trend.
pub const TREND_DEADBAND: f64 = 0.05;

/// How far back "now vs a while ago" looks, in poll ticks — ~5 minutes at
/// the 2s default poll interval. [`SnapshotHistory::sample_for_trend`]
/// clamps to the oldest available point when the ring is younger than this.
pub const TREND_LOOKBACK_TICKS: usize = 150;

/// Ring buffer of [`HistoryPoint`]s: pushing beyond the capacity evicts the
/// oldest sample. Iteration order is oldest → newest.
#[derive(Clone, Debug, Serialize)]
pub struct SnapshotHistory {
    cap: usize,
    points: VecDeque<HistoryPoint>,
}

impl Default for SnapshotHistory {
    fn default() -> Self {
        Self::new(DEFAULT_CAP)
    }
}

impl SnapshotHistory {
    /// A new ring holding at most `cap` samples (`cap` is floored at 1).
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            cap,
            points: VecDeque::with_capacity(cap),
        }
    }

    /// Appends one sample, evicting the oldest if the ring is full. O(1).
    pub fn push(&mut self, point: HistoryPoint) {
        if self.points.len() == self.cap {
            self.points.pop_front();
        }
        self.points.push_back(point);
    }

    /// Oldest → newest.
    pub fn iter(&self) -> impl Iterator<Item = &HistoryPoint> {
        self.points.iter()
    }

    /// The most recent sample, if any.
    pub fn latest(&self) -> Option<&HistoryPoint> {
        self.points.back()
    }

    /// The sample ~`lookback_ticks` before the latest one, clamped to the
    /// oldest available point when the ring holds fewer samples than that
    /// (e.g. right after a restart, or a session younger than 5 minutes) —
    /// used as the "then" side of [`trend`] by every frontend's trend arrow.
    /// `None` when the ring is empty.
    pub fn sample_for_trend(&self, lookback_ticks: usize) -> Option<&HistoryPoint> {
        if self.points.is_empty() {
            return None;
        }
        let idx = self.points.len().saturating_sub(1).saturating_sub(lookback_ticks);
        self.points.get(idx)
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(i: u64) -> HistoryPoint {
        HistoryPoint {
            epoch_ms: i,
            tps: i as f64,
            active_sessions: i as u32,
            connections_total: i as u32,
            cache_hit_pct: Some(i as f32),
            lock_pressure_pct: Some(i as f32),
            oldest_xid_age: Some(i as i64),
        }
    }

    #[test]
    fn push_is_incremental_and_capped() {
        let mut h = SnapshotHistory::new(3);
        assert!(h.is_empty());
        for i in 0..5 {
            h.push(point(i));
        }
        // Capacity 3: samples 0 and 1 were evicted, order is oldest→newest.
        assert_eq!(h.len(), 3);
        let seen: Vec<u64> = h.iter().map(|p| p.epoch_ms).collect();
        assert_eq!(seen, vec![2, 3, 4]);
        assert_eq!(h.latest().map(|p| p.epoch_ms), Some(4));
    }

    #[test]
    fn default_cap_matches_plan() {
        let h = SnapshotHistory::default();
        assert_eq!(h.cap, DEFAULT_CAP);
        // 1 hour at the 2s default poll.
        assert_eq!(DEFAULT_CAP, 1800);
    }

    #[test]
    fn history_serializes_to_json() {
        let mut h = SnapshotHistory::new(2);
        h.push(point(7));
        let json = serde_json::to_string(&h).expect("history must serialize");
        assert!(json.contains("\"epoch_ms\":7"));
        assert!(json.contains("\"active_sessions\":7"));
    }

    /// A JSONL line written before v0.14's widening (only the original three
    /// fields) must still deserialize, with the new fields defaulting rather
    /// than the line being dropped by `HistoryStore::load`'s tolerant parse.
    #[test]
    fn old_format_history_point_parses_with_defaults() {
        let line = r#"{"epoch_ms":123,"tps":4.5,"active_sessions":7}"#;
        let p: HistoryPoint = serde_json::from_str(line).expect("old-format line must parse");
        assert_eq!(p.epoch_ms, 123);
        assert_eq!(p.tps, 4.5);
        assert_eq!(p.active_sessions, 7);
        assert_eq!(p.connections_total, 0);
        assert_eq!(p.cache_hit_pct, None);
        assert_eq!(p.lock_pressure_pct, None);
        assert_eq!(p.oldest_xid_age, None);
    }

    #[test]
    fn sample_for_trend_clamps_to_oldest_when_ring_is_young() {
        let mut h = SnapshotHistory::new(10);
        for i in 0..5 {
            h.push(point(i));
        }
        // Only 5 samples exist; a 150-tick lookback clamps to index 0.
        let then = h.sample_for_trend(150).expect("non-empty ring");
        assert_eq!(then.epoch_ms, 0);
    }

    #[test]
    fn sample_for_trend_picks_exact_offset_when_available() {
        let mut h = SnapshotHistory::new(20);
        for i in 0..20 {
            h.push(point(i));
        }
        // latest is index 19; 5 ticks back is index 14.
        let then = h.sample_for_trend(5).expect("non-empty ring");
        assert_eq!(then.epoch_ms, 14);
    }

    #[test]
    fn sample_for_trend_empty_ring_is_none() {
        let h = SnapshotHistory::new(10);
        assert!(h.sample_for_trend(150).is_none());
    }

    #[test]
    fn trend_flat_within_deadband() {
        // 100 -> 103 is a 3% change, under the 5% deadband.
        assert_eq!(trend(103.0, 100.0, TREND_DEADBAND), Trend::Flat);
        // Exact equality is always Flat.
        assert_eq!(trend(50.0, 50.0, TREND_DEADBAND), Trend::Flat);
    }

    #[test]
    fn trend_up_and_down_outside_deadband() {
        // 100 -> 110 is a 10% rise, past the 5% deadband.
        assert_eq!(trend(110.0, 100.0, TREND_DEADBAND), Trend::Up);
        // 100 -> 88 is a 12% fall.
        assert_eq!(trend(88.0, 100.0, TREND_DEADBAND), Trend::Down);
    }

    #[test]
    fn trend_zero_baseline_falls_back_to_absolute_deadband() {
        // then == 0.0: relative comparison is undefined, so a tiny absolute
        // change stays Flat and a change past the deadband is Up.
        assert_eq!(trend(0.01, 0.0, TREND_DEADBAND), Trend::Flat);
        assert_eq!(trend(1.0, 0.0, TREND_DEADBAND), Trend::Up);
        assert_eq!(trend(-1.0, 0.0, TREND_DEADBAND), Trend::Down);
    }
}
