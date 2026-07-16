//! Top-waits aggregation: "what is everyone stuck on right now".
//!
//! Pure in-memory derivation over the activity rows already present in every
//! [`DbSnapshot`](crate::DbSnapshot) — no SQL, no poller involvement. It is
//! deliberately NOT stored in `DbSnapshot`: the ranking is a pure function of
//! `activity`, so shipping it in the envelope would only duplicate data every
//! frontend can compute in microseconds (and would drag in poller wiring,
//! mock wiring and a web type for zero information gain). The TUI calls
//! [`top_waits`] at render time; the web frontend ports the same fold in
//! `frontend/src/waits.ts` (the way `sql.ts` mirrors `ui/sql.rs`).

use std::collections::BTreeMap;

use crate::models::ActivityRow;

/// The ranked wait picture of one snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitSummary {
    /// Sessions with a non-null `wait_event` (the ones actually stuck).
    pub waiting: usize,
    /// All sessions in the snapshot — `waiting`/`total` is the headline ratio.
    pub total: usize,
    /// `(wait_event, count)` pairs, most frequent first; ties break
    /// alphabetically so the order is deterministic between polls.
    pub ranked: Vec<(String, usize)>,
}

impl WaitSummary {
    /// True when nothing is waiting — frontends hide the strip entirely.
    pub fn is_empty(&self) -> bool {
        self.ranked.is_empty()
    }
}

/// Aggregate `wait_event` (already formatted `Type:Event` by the activity
/// query) across a snapshot's sessions into a ranked list.
///
/// Sessions with `wait_event == None` are running, not waiting — they are
/// excluded from the ranking but still counted in `total`, so the
/// `waiting`/`total` ratio says how much of the server is stuck.
///
/// Callers must pass the FULL activity set, never a filtered subset: the
/// question this answers is "what is the *server* stuck on", and a Micro
/// Lens display filter must not change that answer.
pub fn top_waits(activity: &[ActivityRow]) -> WaitSummary {
    // BTreeMap iterates name-ascending; the later sort by count is stable,
    // so equal counts keep that alphabetical order — deterministic ties.
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut waiting = 0;
    for row in activity {
        if let Some(wait) = row.wait_event.as_deref() {
            waiting += 1;
            *counts.entry(wait).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(wait, count)| (wait.to_string(), count))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    WaitSummary {
        waiting,
        total: activity.len(),
        ranked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal row builder: only `wait_event` matters to the fold.
    fn row(wait: Option<&str>) -> ActivityRow {
        ActivityRow {
            pid: 1,
            application_name: String::new(),
            database: String::new(),
            client: String::new(),
            duration_secs: 0.0,
            wait_event: wait.map(str::to_string),
            username: String::new(),
            state: "active".to_string(),
            query: String::new(),
            query_leader_pid: 1,
            is_parallel_worker: false,
            query_id: None,
        }
    }

    #[test]
    fn ranks_by_count_descending() {
        let activity = vec![
            row(Some("IO:DataFileRead")),
            row(Some("Lock:transactionid")),
            row(Some("Lock:transactionid")),
            row(Some("Lock:transactionid")),
            row(Some("IO:DataFileRead")),
            row(Some("Client:ClientRead")),
        ];
        let summary = top_waits(&activity);
        assert_eq!(
            summary.ranked,
            vec![
                ("Lock:transactionid".to_string(), 3),
                ("IO:DataFileRead".to_string(), 2),
                ("Client:ClientRead".to_string(), 1),
            ]
        );
    }

    #[test]
    fn ties_break_alphabetically_and_deterministically() {
        let activity = vec![
            row(Some("Lock:transactionid")),
            row(Some("Client:ClientRead")),
            row(Some("IO:DataFileRead")),
        ];
        let summary = top_waits(&activity);
        assert_eq!(
            summary.ranked,
            vec![
                ("Client:ClientRead".to_string(), 1),
                ("IO:DataFileRead".to_string(), 1),
                ("Lock:transactionid".to_string(), 1),
            ]
        );
        // Same input, same output — no map-iteration nondeterminism.
        assert_eq!(top_waits(&activity), summary);
    }

    #[test]
    fn running_sessions_are_excluded_but_counted_in_total() {
        let activity = vec![
            row(None),
            row(Some("Lock:relation")),
            row(None),
            row(Some("Lock:relation")),
        ];
        let summary = top_waits(&activity);
        assert_eq!(summary.waiting, 2);
        assert_eq!(summary.total, 4);
        assert_eq!(summary.ranked, vec![("Lock:relation".to_string(), 2)]);
    }

    #[test]
    fn empty_and_all_running_yield_an_empty_summary() {
        assert!(top_waits(&[]).is_empty());
        assert_eq!(top_waits(&[]).total, 0);

        let all_running = vec![row(None), row(None)];
        let summary = top_waits(&all_running);
        assert!(summary.is_empty());
        assert_eq!(summary.waiting, 0);
        assert_eq!(summary.total, 2);
    }

    #[test]
    fn mock_snapshot_produces_a_non_trivial_ranking() {
        // --mock must demo the strip: several distinct wait kinds, including
        // the Lock:* (red) and IO:* (yellow) severity paths.
        let snapshot = crate::DbSnapshot::mock();
        let summary = top_waits(&snapshot.activity);
        assert!(summary.ranked.len() >= 3, "several distinct waits");
        assert!(summary.waiting < summary.total, "some sessions run");
        let names: Vec<&str> = summary.ranked.iter().map(|(w, _)| w.as_str()).collect();
        assert!(names.iter().any(|w| w.starts_with("Lock:")));
        assert!(names.iter().any(|w| w.starts_with("IO:")));
    }
}
