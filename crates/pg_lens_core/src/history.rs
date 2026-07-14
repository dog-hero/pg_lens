//! Time-series history of snapshot-derived metrics.
//!
//! [`SnapshotHistory`] is a fixed-capacity ring buffer (`VecDeque`) that the
//! **poller** owns and grows incrementally — one [`HistoryPoint`] per poll,
//! never rebuilt. A clone of the ring travels inside every [`DbSnapshot`]
//! envelope, so all consumers (TUI sparklines today, the web's uPlot charts
//! in Fase 6) see the exact same series; that is why this lives in the core
//! and derives `Serialize`.
//!
//! [`DbSnapshot`]: crate::models::DbSnapshot

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Default ring capacity (~120 samples = 4 minutes at the 2s default poll).
pub const DEFAULT_CAP: usize = 120;

/// Wall-clock timestamp for a new [`HistoryPoint`], in Unix epoch
/// milliseconds. Falls back to `0` if the system clock predates the epoch.
pub fn epoch_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One time-series sample, taken by the poller as it publishes a snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct HistoryPoint {
    /// When the sample was taken (Unix epoch milliseconds).
    pub epoch_ms: u64,
    /// Transactions per second (delta-derived by the poller).
    pub tps: f64,
    /// Number of `active` backends at that moment.
    pub active_sessions: u32,
}

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
        assert_eq!(DEFAULT_CAP, 120);
    }

    #[test]
    fn history_serializes_to_json() {
        let mut h = SnapshotHistory::new(2);
        h.push(point(7));
        let json = serde_json::to_string(&h).expect("history must serialize");
        assert!(json.contains("\"epoch_ms\":7"));
        assert!(json.contains("\"active_sessions\":7"));
    }
}
