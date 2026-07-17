//! Prepared-transaction (orphaned 2PC) watch (v0.9).
//!
//! Severity tiers over `PreparedXactRow::age_seconds` — same shape as
//! [`crate::xact_age`]/[`crate::waits`]: pure, DB-free, unit-tested directly.
//! The TUI calls [`severity`] at render time inside the Schema Lens's Vacuum
//! sub-view; the web frontend ports the same thresholds in
//! `frontend/src/prepared_xacts.ts`.
//!
//! Unlike a normal long-running transaction, ANY orphaned prepared
//! transaction is already a problem the moment the client that PREPAREd it
//! disconnects without following up — there is no "acceptable" baseline age.
//! The tiers below exist to separate "just prepared, the 2PC coordinator is
//! about to COMMIT/ROLLBACK it" (calm) from "very likely abandoned" (yellow)
//! from "definitely orphaned, blocking vacuum" (red).

use crate::models::PreparedXactRow;

/// Yellow past this age — a 2PC coordinator that hasn't resolved a prepared
/// transaction in 5 minutes is almost certainly gone.
pub const WARN_AGE_SECS: f64 = 300.0;
/// Red past this age (1 hour) — no legitimate 2PC coordinator takes this
/// long; this is an orphan holding locks and pinning wraparound.
pub const BAD_AGE_SECS: f64 = 3_600.0;

/// Severity tier of one prepared transaction's age.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Warn,
    Bad,
}

/// Severity of one prepared-transaction age. Below [`WARN_AGE_SECS`] is
/// still `Ok` — a 2PC coordinator resolving within the transaction's normal
/// window is unremarkable — but every row past it deserves a visible marker,
/// since there is no scenario where a healthy application leaves one open
/// for long.
pub fn severity(age_seconds: f64) -> Severity {
    if age_seconds > BAD_AGE_SECS {
        Severity::Bad
    } else if age_seconds > WARN_AGE_SECS {
        Severity::Warn
    } else {
        Severity::Ok
    }
}

/// The oldest prepared transaction in a set, with its severity tier —
/// `None` when `rows` is empty (nothing orphaned to headline).
pub struct OldestPreparedXact<'a> {
    pub row: &'a PreparedXactRow,
    pub severity: Severity,
}

/// Find the oldest prepared transaction — the one that has held its locks
/// (and pinned the wraparound horizon) the longest.
pub fn oldest_prepared_xact(rows: &[PreparedXactRow]) -> Option<OldestPreparedXact<'_>> {
    rows.iter()
        .max_by(|a, b| a.age_seconds.total_cmp(&b.age_seconds))
        .map(|row| OldestPreparedXact {
            row,
            severity: severity(row.age_seconds),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(gid: &str, age_seconds: f64) -> PreparedXactRow {
        PreparedXactRow {
            gid: gid.to_string(),
            owner: "app_rw".to_string(),
            database: "shop".to_string(),
            age_seconds,
        }
    }

    #[test]
    fn severity_tiers_match_the_thresholds() {
        assert_eq!(severity(0.0), Severity::Ok);
        assert_eq!(severity(300.0), Severity::Ok, "boundary is not yet warn");
        assert_eq!(severity(300.1), Severity::Warn);
        assert_eq!(severity(3_600.0), Severity::Warn, "boundary is not yet bad");
        assert_eq!(severity(3_600.1), Severity::Bad);
        assert_eq!(severity(86_400.0), Severity::Bad);
    }

    #[test]
    fn oldest_prepared_xact_picks_the_largest_age() {
        let rows = vec![row("a", 100.0), row("b", 9_000.0), row("c", 500.0)];
        let oldest = oldest_prepared_xact(&rows).expect("rows is non-empty");
        assert_eq!(oldest.row.gid, "b");
        assert_eq!(oldest.severity, Severity::Bad);
    }

    #[test]
    fn oldest_prepared_xact_is_none_when_empty() {
        assert!(oldest_prepared_xact(&[]).is_none());
    }
}
