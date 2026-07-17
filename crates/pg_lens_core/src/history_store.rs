//! Best-effort on-disk persistence of the [`SnapshotHistory`] ring.
//!
//! The ring lives in memory and dies with the process; this stores each
//! [`HistoryPoint`] as one line of JSON (JSONL) so the TPS/session chart
//! resumes with prior data after a restart and can span far more than a
//! single session. It is deliberately dumb and fault-tolerant: **every**
//! operation is best-effort, and any I/O or parse error leaves the in-memory
//! ring authoritative and never disturbs polling. The path is chosen by the
//! frontend (per connection target) and injected — the core reads no env.
//!
//! [`SnapshotHistory`]: crate::history::SnapshotHistory

use std::io::Write;
use std::path::PathBuf;

use crate::history::HistoryPoint;

/// Append-only JSONL store for one connection target's history.
pub struct HistoryStore {
    path: PathBuf,
}

impl HistoryStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Loads the most-recent `cap` points (oldest → newest). A missing file,
    /// unreadable file, or unparsable lines yield whatever parses (an empty
    /// vec in the worst case) — never an error.
    pub fn load(&self, cap: usize) -> Vec<HistoryPoint> {
        let Ok(contents) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let mut points: Vec<HistoryPoint> = contents
            .lines()
            .filter_map(|line| serde_json::from_str::<HistoryPoint>(line).ok())
            .collect();
        // Keep only the newest `cap` (the ring's capacity).
        if points.len() > cap {
            points.drain(0..points.len() - cap);
        }
        points
    }

    /// Appends one point as a JSON line. Best-effort — errors are swallowed
    /// (a monitoring tool must never fail a tick because its cache disk is
    /// full or read-only). Creates the parent directory on first write.
    pub fn append(&self, point: &HistoryPoint) {
        let Ok(line) = serde_json::to_string(point) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Rewrites the file with exactly `points`, bounding unbounded growth
    /// within a long-running session. Best-effort and atomic-ish (write a
    /// temp file, then rename over the target). Called periodically by the
    /// poller, not per tick.
    pub fn compact(&self, points: &[HistoryPoint]) {
        let mut body = String::with_capacity(points.len() * 48);
        for point in points {
            if let Ok(line) = serde_json::to_string(point) {
                body.push_str(&line);
                body.push('\n');
            }
        }
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
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
    fn append_then_load_roundtrips_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(dir.path().join("sub/history.jsonl"));
        for i in 0..5 {
            store.append(&point(i));
        }
        let loaded = store.load(10);
        assert_eq!(
            loaded.iter().map(|p| p.epoch_ms).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn load_keeps_only_the_newest_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(dir.path().join("history.jsonl"));
        for i in 0..10 {
            store.append(&point(i));
        }
        let loaded = store.load(3);
        assert_eq!(
            loaded.iter().map(|p| p.epoch_ms).collect::<Vec<_>>(),
            vec![7, 8, 9]
        );
    }

    #[test]
    fn missing_file_loads_empty_without_error() {
        let store = HistoryStore::new(PathBuf::from("/no/such/dir/history.jsonl"));
        assert!(store.load(10).is_empty());
    }

    #[test]
    fn compact_bounds_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(dir.path().join("history.jsonl"));
        for i in 0..100 {
            store.append(&point(i));
        }
        let keep: Vec<HistoryPoint> = (95..100).map(point).collect();
        store.compact(&keep);
        let loaded = store.load(1000);
        assert_eq!(
            loaded.iter().map(|p| p.epoch_ms).collect::<Vec<_>>(),
            vec![95, 96, 97, 98, 99]
        );
    }

    /// A JSONL file written entirely by a pre-v0.14 build (only the original
    /// three fields per line) must still load in full, with the new fields
    /// defaulting — a dropped-history regression on upgrade would be bad.
    #[test]
    fn old_format_jsonl_file_loads_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        std::fs::write(
            &path,
            "{\"epoch_ms\":1,\"tps\":1.0,\"active_sessions\":1}\n\
             {\"epoch_ms\":2,\"tps\":2.0,\"active_sessions\":2}\n",
        )
        .unwrap();
        let store = HistoryStore::new(path);
        let loaded = store.load(10);
        assert_eq!(loaded.len(), 2, "old-format lines must not be dropped");
        assert_eq!(loaded[0].epoch_ms, 1);
        assert_eq!(loaded[0].connections_total, 0);
        assert_eq!(loaded[0].cache_hit_pct, None);
        assert_eq!(loaded[0].lock_pressure_pct, None);
        assert_eq!(loaded[0].oldest_xid_age, None);
    }

    #[test]
    fn skips_corrupt_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        std::fs::write(
            &path,
            "not json\n{\"epoch_ms\":5,\"tps\":1.0,\"active_sessions\":2}\ngarbage\n",
        )
        .unwrap();
        let store = HistoryStore::new(path);
        let loaded = store.load(10);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].epoch_ms, 5);
    }
}
