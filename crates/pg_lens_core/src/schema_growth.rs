//! Bounded per-table size-growth ring (v0.14): "this table grew 40% in the
//! last hour" — a `Δ1h` for the Schema Lens's Tables view.
//!
//! DISTINCT from [`crate::history::HistoryPoint`]/[`crate::history::SnapshotHistory`]:
//! that ring is one SCALAR series per snapshot, pushed on every fast tick and
//! JSONL-persisted. This module tracks a small time series PER TABLE, fed
//! only by the SLOW schema cadence (default 60s) — a fast-tick push per
//! table would be both wasteful (the sizes are unchanged between schema
//! collections) and unbounded in a way the fast path must never be.
//!
//! ## Key: oid, not schema-qualified name
//!
//! [`SchemaGrowthTracker`] keys rings by `pg_class.oid`
//! ([`crate::models::TableStatRow::oid`]), not `(schema, name)`. A rename
//! keeps the oid, so the ring (correctly) keeps tracking the same table
//! through a rename. A `DROP`+`CREATE` (or `TRUNCATE ... ; ALTER TABLE ...
//! RENAME` swap, the classic "recreate the table" pattern) gets a *new*
//! oid, so the ring (correctly) starts fresh instead of splicing the new
//! table's sizes onto the old table's history — with a name-based key, a
//! recreated `order_items` reusing the old table's name would otherwise
//! report a nonsensical Δ (comparing the new, empty table's size against
//! the dropped table's last known size).
//!
//! ## Bounding
//!
//! Two independent caps, both enforced in [`SchemaGrowthTracker::update`]:
//! - **Table count**: at most [`MAX_TRACKED_TABLES`] rings, one per oid
//!   present in the latest schema collection. The `table_stats` SQL already
//!   caps its result at `queries::TABLE_STATS_LIMIT` (200, top-N by size),
//!   so this simply mirrors that cap defensively — never trusts the caller
//!   to have capped its input.
//! - **Ring depth**: each ring holds at most [`RING_CAP`] samples
//!   (`VecDeque`, oldest evicted first).
//! - **Eviction of vanished tables**: [`SchemaGrowthTracker::update`]
//!   rebuilds its map from scratch each call, keeping only oids present in
//!   the fresh collection — a dropped/renamed-away/recreated table's old
//!   ring is discarded immediately, never accumulating indefinitely.
//!
//! Worst-case memory: `MAX_TRACKED_TABLES (200) * RING_CAP (90) * size_of(SizeSample) (16 bytes)`
//! ≈ 288 KB, plus `VecDeque`/`HashMap` overhead (a small constant multiple)
//! — trivial, and it cannot grow further no matter how many tables the
//! database has or how long the session runs.
//!
//! ## Persistence: none (in-memory only)
//!
//! Unlike [`crate::history_store`], the growth ring is NOT persisted. A
//! restart loses up to an hour of per-table growth (`Δ1h` reads `None`/`—`
//! for a while and refills as the slow cadence collects again — a calm,
//! self-healing gap, not an error state). Persisting would mean a second
//! per-target JSONL format (per-table lines, distinct schema from the
//! scalar `HistoryPoint` file) for a feature whose value is fundamentally
//! about the *last hour*, not historical replay — the marginal benefit
//! (surviving a restart mid-hour) doesn't justify a new on-disk format and
//! its own tolerant-load/compaction machinery. If this changes (e.g. a
//! future "growth over a day" view), revisit.

use std::collections::{HashMap, VecDeque};

use crate::models::TableStatRow;

/// Mirrors `queries::TABLE_STATS_LIMIT` — the `table_stats` SQL's own row
/// cap — as a defensive ceiling on how many per-table rings this tracker
/// will ever hold, independent of what the caller passes in.
pub const MAX_TRACKED_TABLES: usize = 200;

/// Ring depth: 90 samples at the default 60s schema cadence covers 1h30m —
/// 50% margin over the 1h [`GROWTH_LOOKBACK_MS`] window so a jittered
/// cadence (a slow collection occasionally skipped, e.g. behind a busy
/// bloat-estimate tick) still has a sample at or before the lookback edge.
pub const RING_CAP: usize = 90;

/// The Δ window: compare the newest sample against the oldest sample within
/// the last hour.
pub const GROWTH_LOOKBACK_MS: u64 = 60 * 60 * 1000;

/// Below this absolute byte delta, a table's growth reads as flat — normal
/// catalog/page-allocation noise (a single autovacuum truncate/extend can
/// nudge `pg_total_relation_size` by a page or two) must not paint a table
/// green/yellow/red for doing nothing. 64 KiB is a handful of 8 KiB pages,
/// well under anything an operator would call "growth".
pub const GROWTH_DEADBAND_BYTES: i64 = 65_536;

/// Yellow tint floor: `growth_1h_pct` past this AND `total_bytes` past
/// [`SEVERITY_MIN_TABLE_BYTES`] — the "40% in an hour" headline case.
pub const WARN_GROWTH_PCT: f32 = 10.0;
/// Red tint floor.
pub const BAD_GROWTH_PCT: f32 = 25.0;
/// Severity tinting only applies to tables at least this big — a 2 KB
/// scratch table doubling in size is not an incident.
pub const SEVERITY_MIN_TABLE_BYTES: i64 = 10 * 1024 * 1024;

/// One (timestamp, size) sample in a table's ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeSample {
    pub epoch_ms: u64,
    pub total_bytes: i64,
}

/// Fixed-capacity ring of [`SizeSample`]s for one table (keyed by oid in
/// [`SchemaGrowthTracker`]).
#[derive(Clone, Debug, Default)]
pub struct TableGrowthRing {
    samples: VecDeque<SizeSample>,
}

impl TableGrowthRing {
    fn push(&mut self, sample: SizeSample) {
        if self.samples.len() == RING_CAP {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Oldest → newest, exposed for tests.
    pub fn iter(&self) -> impl Iterator<Item = &SizeSample> {
        self.samples.iter()
    }
}

/// A computed size delta over a lookback window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrowthDelta {
    /// Signed byte delta (newest minus oldest-in-window). Zeroed by the
    /// deadband, not the raw noise value, when `|raw delta| <
    /// GROWTH_DEADBAND_BYTES`. Negative is a valid, meaningful shrink
    /// (VACUUM FULL, TRUNCATE, partition drop) — never clamped to 0.
    pub bytes: i64,
    /// `bytes / oldest_bytes * 100`. `None` only when the oldest sample in
    /// the window was itself `0` bytes (percentage undefined, not "no
    /// growth") — `bytes` still carries the real (deadbanded) delta.
    pub pct: Option<f32>,
}

/// Pure Δ computation over one table's ring: the newest sample vs. the
/// oldest sample that is still within `lookback_ms` of `now_epoch_ms` (or
/// the ring's actual oldest sample, if the ring is younger than the
/// lookback window — same clamp-to-oldest convention as
/// [`crate::history::SnapshotHistory::sample_for_trend`]).
///
/// `None` when the ring has fewer than 2 samples (nothing to compare yet —
/// a fresh session, or a table that just appeared), or when the only sample
/// within the window and the newest sample are the same point.
pub fn growth(ring: &TableGrowthRing, now_epoch_ms: u64, lookback_ms: u64) -> Option<GrowthDelta> {
    if ring.samples.len() < 2 {
        return None;
    }
    // Safe: length checked above.
    let newest = ring.samples.back()?;
    let cutoff = now_epoch_ms.saturating_sub(lookback_ms);
    let oldest = ring
        .samples
        .iter()
        .find(|s| s.epoch_ms >= cutoff)
        .unwrap_or_else(|| ring.samples.front().expect("non-empty, checked above"));
    if oldest.epoch_ms == newest.epoch_ms {
        return None;
    }
    let raw_delta = newest.total_bytes - oldest.total_bytes;
    let bytes = if raw_delta.abs() < GROWTH_DEADBAND_BYTES {
        0
    } else {
        raw_delta
    };
    let pct = if oldest.total_bytes > 0 {
        Some((bytes as f64 / oldest.total_bytes as f64 * 100.0) as f32)
    } else {
        None
    };
    Some(GrowthDelta { bytes, pct })
}

/// Severity tint of one growth reading, `None` (calm) when the table is too
/// small for the reading to matter ([`SEVERITY_MIN_TABLE_BYTES`]) or growth
/// is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Bad,
}

pub fn severity(total_bytes: i64, growth_pct: Option<f32>) -> Option<Severity> {
    if total_bytes < SEVERITY_MIN_TABLE_BYTES {
        return None;
    }
    let pct = growth_pct?.abs();
    if pct > BAD_GROWTH_PCT {
        Some(Severity::Bad)
    } else if pct > WARN_GROWTH_PCT {
        Some(Severity::Warn)
    } else {
        None
    }
}

/// Poller-owned tracker: one [`TableGrowthRing`] per table oid, fed only by
/// successful slow-cadence schema collections. See the module docs for the
/// oid-keying rationale and the bounding guarantees.
#[derive(Clone, Debug, Default)]
pub struct SchemaGrowthTracker {
    rings: HashMap<i64, TableGrowthRing>,
}

impl SchemaGrowthTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one fresh schema collection's table sizes into the tracker: a
    /// sample is pushed onto each table's ring (creating it if new), and any
    /// ring whose oid is NOT present in `tables` is dropped — this is the
    /// single place eviction happens, run every slow-cadence collection.
    pub fn update(&mut self, tables: &[TableStatRow], now_epoch_ms: u64) {
        let mut fresh: HashMap<i64, TableGrowthRing> = HashMap::with_capacity(
            tables.len().min(MAX_TRACKED_TABLES),
        );
        for row in tables.iter().take(MAX_TRACKED_TABLES) {
            let mut ring = self.rings.remove(&row.oid).unwrap_or_default();
            ring.push(SizeSample {
                epoch_ms: now_epoch_ms,
                total_bytes: row.total_bytes,
            });
            fresh.insert(row.oid, ring);
        }
        self.rings = fresh;
    }

    /// Populates `growth_1h_bytes`/`growth_1h_pct` on each row from its
    /// ring — the "rendered form" the poller ships inside the snapshot so
    /// every frontend stays dumb (no ring/deadband/severity logic anywhere
    /// but here and `crate::schema_growth`).
    pub fn apply(&self, tables: &mut [TableStatRow], now_epoch_ms: u64, lookback_ms: u64) {
        for row in tables.iter_mut() {
            let delta = self
                .rings
                .get(&row.oid)
                .and_then(|ring| growth(ring, now_epoch_ms, lookback_ms));
            row.growth_1h_bytes = delta.as_ref().map(|d| d.bytes);
            row.growth_1h_pct = delta.and_then(|d| d.pct);
        }
    }

    #[cfg(test)]
    fn ring(&self, oid: i64) -> Option<&TableGrowthRing> {
        self.rings.get(&oid)
    }

    #[cfg(test)]
    fn tracked_count(&self) -> usize {
        self.rings.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(oid: i64, name: &str, total_bytes: i64) -> TableStatRow {
        TableStatRow {
            oid,
            schema: "public".to_string(),
            name: name.to_string(),
            total_bytes,
            table_bytes: total_bytes,
            index_bytes: 0,
            seq_scan: 0,
            seq_tup_read: 0,
            idx_scan: None,
            idx_tup_fetch: None,
            n_tup_ins: 0,
            n_tup_upd: 0,
            n_tup_del: 0,
            n_tup_hot_upd: 0,
            n_live_tup: 0,
            n_dead_tup: 0,
            n_mod_since_analyze: 0,
            n_ins_since_vacuum: 0,
            last_vacuum_epoch_secs: None,
            last_autovacuum_epoch_secs: None,
            last_analyze_epoch_secs: None,
            last_autoanalyze_epoch_secs: None,
            vacuum_count: 0,
            autovacuum_count: 0,
            analyze_count: 0,
            autoanalyze_count: 0,
            growth_1h_bytes: None,
            growth_1h_pct: None,
        }
    }

    #[test]
    fn ring_bounds_depth_and_evicts_oldest() {
        let mut tracker = SchemaGrowthTracker::new();
        for i in 0..(RING_CAP as u64 + 5) {
            tracker.update(&[row(1, "t", 1000 + i as i64)], i * 60_000);
        }
        let ring = tracker.ring(1).expect("tracked");
        assert_eq!(ring.len(), RING_CAP);
        // The oldest 5 samples (total_bytes 1000..1005) were evicted.
        let first = ring.iter().next().expect("non-empty");
        assert_eq!(first.total_bytes, 1005);
    }

    #[test]
    fn update_evicts_vanished_tables() {
        let mut tracker = SchemaGrowthTracker::new();
        tracker.update(&[row(1, "a", 100), row(2, "b", 200)], 0);
        assert_eq!(tracker.tracked_count(), 2);
        // Table 2 dropped/renamed away: only table 1 appears next collection.
        tracker.update(&[row(1, "a", 110)], 60_000);
        assert_eq!(tracked_oids(&tracker), vec![1]);
    }

    fn tracked_oids(tracker: &SchemaGrowthTracker) -> Vec<i64> {
        let mut oids: Vec<i64> = (0..10).filter(|oid| tracker.ring(*oid).is_some()).collect();
        oids.sort_unstable();
        oids
    }

    #[test]
    fn update_caps_total_tracked_tables() {
        let mut tracker = SchemaGrowthTracker::new();
        let rows: Vec<TableStatRow> = (0..(MAX_TRACKED_TABLES as i64 + 20))
            .map(|oid| row(oid, "t", 1000))
            .collect();
        tracker.update(&rows, 0);
        assert_eq!(tracker.tracked_count(), MAX_TRACKED_TABLES);
    }

    #[test]
    fn growth_none_with_fewer_than_two_samples() {
        let mut ring = TableGrowthRing::default();
        assert!(growth(&ring, 0, GROWTH_LOOKBACK_MS).is_none());
        ring.push(SizeSample { epoch_ms: 0, total_bytes: 1000 });
        assert!(growth(&ring, 0, GROWTH_LOOKBACK_MS).is_none());
    }

    #[test]
    fn growth_positive_delta_and_percentage() {
        let mut ring = TableGrowthRing::default();
        ring.push(SizeSample { epoch_ms: 0, total_bytes: 100_000_000 });
        ring.push(SizeSample { epoch_ms: GROWTH_LOOKBACK_MS / 2, total_bytes: 140_000_000 });
        let d = growth(&ring, GROWTH_LOOKBACK_MS / 2, GROWTH_LOOKBACK_MS).expect("2 samples");
        assert_eq!(d.bytes, 40_000_000);
        assert!((d.pct.unwrap() - 40.0).abs() < 0.01);
    }

    #[test]
    fn growth_negative_delta_is_reported_not_clamped() {
        let mut ring = TableGrowthRing::default();
        ring.push(SizeSample { epoch_ms: 0, total_bytes: 100_000_000 });
        ring.push(SizeSample { epoch_ms: 1000, total_bytes: 40_000_000 });
        let d = growth(&ring, 1000, GROWTH_LOOKBACK_MS).expect("2 samples");
        assert_eq!(d.bytes, -60_000_000);
        assert!((d.pct.unwrap() + 60.0).abs() < 0.01);
    }

    #[test]
    fn growth_deadband_flattens_tiny_noise() {
        let mut ring = TableGrowthRing::default();
        ring.push(SizeSample { epoch_ms: 0, total_bytes: 100_000_000 });
        // A 4 KiB wobble is well under GROWTH_DEADBAND_BYTES.
        ring.push(SizeSample { epoch_ms: 1000, total_bytes: 100_004_096 });
        let d = growth(&ring, 1000, GROWTH_LOOKBACK_MS).expect("2 samples");
        assert_eq!(d.bytes, 0);
        assert_eq!(d.pct, Some(0.0));
    }

    #[test]
    fn growth_ignores_samples_older_than_the_lookback_window() {
        let mut ring = TableGrowthRing::default();
        // A very old sample (2 hours before "now") must not be picked as
        // the comparison baseline once a newer in-window sample exists.
        ring.push(SizeSample { epoch_ms: 0, total_bytes: 1 });
        ring.push(SizeSample {
            epoch_ms: 2 * GROWTH_LOOKBACK_MS + 100_000, // just inside the window
            total_bytes: 200_000_000,
        });
        ring.push(SizeSample {
            epoch_ms: 2 * GROWTH_LOOKBACK_MS + 200_000, // "now"
            total_bytes: 220_000_000,
        });
        let now = 2 * GROWTH_LOOKBACK_MS + 200_000;
        let d = growth(&ring, now, GROWTH_LOOKBACK_MS).expect("in-window samples");
        assert_eq!(d.bytes, 20_000_000);
    }

    #[test]
    fn growth_undefined_percentage_when_oldest_is_zero_bytes() {
        let mut ring = TableGrowthRing::default();
        ring.push(SizeSample { epoch_ms: 0, total_bytes: 0 });
        ring.push(SizeSample { epoch_ms: 1000, total_bytes: 500_000 });
        let d = growth(&ring, 1000, GROWTH_LOOKBACK_MS).expect("2 samples");
        assert_eq!(d.bytes, 500_000);
        assert_eq!(d.pct, None);
    }

    #[test]
    fn apply_populates_rows_from_rings_leaving_untracked_as_none() {
        let mut tracker = SchemaGrowthTracker::new();
        tracker.update(&[row(1, "a", 100_000_000)], 0);
        tracker.update(&[row(1, "a", 150_000_000)], 1_000_000);
        let mut rows = vec![row(1, "a", 150_000_000), row(2, "b", 500)];
        tracker.apply(&mut rows, 1_000_000, GROWTH_LOOKBACK_MS);
        assert_eq!(rows[0].growth_1h_bytes, Some(50_000_000));
        // Table 2 was never in a collection the tracker saw: no ring.
        assert_eq!(rows[1].growth_1h_bytes, None);
        assert_eq!(rows[1].growth_1h_pct, None);
    }

    #[test]
    fn severity_gates_on_absolute_size_floor() {
        // 50% growth but the table is tiny: no severity.
        assert_eq!(severity(1024, Some(50.0)), None);
        // 50% growth on a table above the floor: bad.
        assert_eq!(severity(SEVERITY_MIN_TABLE_BYTES, Some(50.0)), Some(Severity::Bad));
        // 15% growth on a table above the floor: warn.
        assert_eq!(severity(SEVERITY_MIN_TABLE_BYTES, Some(15.0)), Some(Severity::Warn));
        // 5% growth: calm.
        assert_eq!(severity(SEVERITY_MIN_TABLE_BYTES, Some(5.0)), None);
        // Unknown growth: calm (never guess a severity).
        assert_eq!(severity(SEVERITY_MIN_TABLE_BYTES, None), None);
        // A large shrink is also severity-worthy (abs value).
        assert_eq!(severity(SEVERITY_MIN_TABLE_BYTES, Some(-30.0)), Some(Severity::Bad));
    }
}
