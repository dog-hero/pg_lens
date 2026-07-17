//! Idle-in-transaction / transaction-age hunter (v0.9).
//!
//! Pure in-memory derivation over the activity rows already present in every
//! [`DbSnapshot`](crate::DbSnapshot) — no new SQL, no poller involvement
//! (same shape as [`crate::waits`]). `ActivityRow::xact_age_secs` is the raw
//! datum (`pg_stat_activity.xact_start`, `None` when the backend has no
//! open transaction); this module turns it into a severity tier and finds
//! the single oldest open transaction, which is the one driving
//! XID-wraparound risk and lock retention. The TUI calls these functions at
//! render time; the web frontend ports the same logic in
//! `frontend/src/xact_age.ts` (the way `waits.ts` mirrors `waits.rs`).

use crate::models::ActivityRow;

/// Yellow past this age for a normally-running transaction (5 minutes).
pub const XACT_AGE_WARN_SECS: f64 = 300.0;
/// Red past this age for a normally-running transaction (30 minutes).
pub const XACT_AGE_BAD_SECS: f64 = 1_800.0;
/// `idle in transaction` sessions get HALF these thresholds: they hold
/// locks and pin the wraparound horizon while doing nothing, so the same
/// age is strictly worse than an actively-running transaction of equal age.
pub const IDLE_IN_XACT_AGE_WARN_SECS: f64 = XACT_AGE_WARN_SECS / 2.0;
pub const IDLE_IN_XACT_AGE_BAD_SECS: f64 = XACT_AGE_BAD_SECS / 2.0;

/// Severity tier of one open transaction's age.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Warn,
    Bad,
}

/// `state` values meaning "in a transaction but not doing anything" —
/// `pg_stat_activity.state` also reports the aborted variant, which is
/// exactly as lock/wraparound-hostile as the healthy one.
fn is_idle_in_transaction(state: &str) -> bool {
    state == "idle in transaction" || state == "idle in transaction (aborted)"
}

/// Severity of one transaction age, escalated for idle-in-transaction
/// sessions (see the module doc: half the thresholds).
pub fn xact_age_severity(age_secs: f64, state: &str) -> Severity {
    let (warn, bad) = if is_idle_in_transaction(state) {
        (IDLE_IN_XACT_AGE_WARN_SECS, IDLE_IN_XACT_AGE_BAD_SECS)
    } else {
        (XACT_AGE_WARN_SECS, XACT_AGE_BAD_SECS)
    };
    if age_secs > bad {
        Severity::Bad
    } else if age_secs > warn {
        Severity::Warn
    } else {
        Severity::Ok
    }
}

/// The oldest open transaction in a snapshot, with its severity tier —
/// `None` when nothing in `activity` has an open transaction.
pub struct OldestXact<'a> {
    pub row: &'a ActivityRow,
    pub severity: Severity,
}

/// Find the session with the largest `xact_age_secs` (rows with no open
/// transaction are skipped, never treated as age `0`). This is the headline
/// the Micro Lens surfaces: the single transaction most likely to be
/// pinning `datfrozenxid` or holding locks other sessions are queued on.
pub fn oldest_open_xact(activity: &[ActivityRow]) -> Option<OldestXact<'_>> {
    activity
        .iter()
        .filter_map(|row| row.xact_age_secs.map(|age| (row, age)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(row, age)| OldestXact {
            row,
            severity: xact_age_severity(age, &row.state),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &str, xact_age_secs: Option<f64>, pid: i32) -> ActivityRow {
        ActivityRow {
            pid,
            application_name: String::new(),
            database: String::new(),
            client: String::new(),
            duration_secs: 0.0,
            xact_age_secs,
            wait_event: None,
            username: String::new(),
            state: state.to_string(),
            query: String::new(),
            query_leader_pid: pid,
            is_parallel_worker: false,
            query_id: None,
        }
    }

    #[test]
    fn severity_tiers_for_a_normal_transaction() {
        assert_eq!(xact_age_severity(0.0, "active"), Severity::Ok);
        assert_eq!(xact_age_severity(300.0, "active"), Severity::Ok, "boundary is not yet warn");
        assert_eq!(xact_age_severity(300.1, "active"), Severity::Warn);
        assert_eq!(xact_age_severity(1_800.0, "active"), Severity::Warn, "boundary is not yet bad");
        assert_eq!(xact_age_severity(1_800.1, "active"), Severity::Bad);
    }

    #[test]
    fn idle_in_transaction_is_worse_than_an_equally_old_active_query() {
        // Same age, different state: idle-in-transaction crosses into a
        // worse tier well before an active session does.
        let age = 1_000.0;
        assert_eq!(xact_age_severity(age, "active"), Severity::Warn);
        assert_eq!(xact_age_severity(age, "idle in transaction"), Severity::Bad);
        // The aborted variant is exactly as bad.
        assert_eq!(
            xact_age_severity(age, "idle in transaction (aborted)"),
            Severity::Bad
        );
    }

    #[test]
    fn oldest_open_xact_skips_rows_with_no_transaction() {
        let activity = vec![
            row("active", None, 1),
            row("active", Some(50.0), 2),
            row("idle in transaction", Some(2_000.0), 3),
            row("active", Some(1_500.0), 4),
        ];
        let oldest = oldest_open_xact(&activity).expect("one row has an open transaction");
        assert_eq!(oldest.row.pid, 3);
        assert_eq!(oldest.severity, Severity::Bad);
    }

    #[test]
    fn oldest_open_xact_is_none_when_nothing_has_a_transaction() {
        let activity = vec![row("idle", None, 1), row("active", None, 2)];
        assert!(oldest_open_xact(&activity).is_none());
    }
}
