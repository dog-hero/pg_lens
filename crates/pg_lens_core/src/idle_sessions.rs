//! Idle connection / connection-age census (v0.11).
//!
//! Severity tiers over `IdleSessionRow::idle_age_secs` — same shape as
//! [`crate::prepared_xacts`]/[`crate::xact_age`]/[`crate::lock_capacity`]:
//! pure, DB-free, unit-tested directly. The TUI renders it as the Micro
//! Lens's idle-census toggle (`I`); the web frontend ports the same
//! thresholds in `frontend/src/idle_sessions.ts`.
//!
//! Unlike an open transaction (`xact_age`) or an orphaned 2PC
//! (`prepared_xacts`), a plain idle connection is not itself dangerous — a
//! connection pool is SUPPOSED to hold idle backends. The tiers below exist
//! to separate "a normal pooled connection" (calm) from "sat idle long
//! enough that it's worth asking whether the pool is leaking connections or
//! simply oversized" (yellow) from "idle for hours — almost certainly a
//! forgotten/leaked connection eating the budget" (red).

use crate::models::IdleSessionRow;

/// Yellow past this age (30 minutes) — longer than almost any legitimate
/// connection-pool idle timeout, worth a second look.
pub const WARN_AGE_SECS: f64 = 1_800.0;
/// Red past this age (4 hours) — no ordinary pool keeps a connection idle
/// this long; this is the classic "leaked connection" suspect.
pub const BAD_AGE_SECS: f64 = 14_400.0;

/// Severity tier of one idle session's age.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Warn,
    Bad,
}

/// Severity of one idle-session age. Below [`WARN_AGE_SECS`] is calm — a
/// normal pooled connection between requests.
pub fn severity(idle_age_secs: f64) -> Severity {
    if idle_age_secs > BAD_AGE_SECS {
        Severity::Bad
    } else if idle_age_secs > WARN_AGE_SECS {
        Severity::Warn
    } else {
        Severity::Ok
    }
}

/// The oldest idle session in a set, with its severity tier — `None` when
/// `rows` is empty (nothing idle to headline).
pub struct OldestIdleSession<'a> {
    pub row: &'a IdleSessionRow,
    pub severity: Severity,
}

/// Find the oldest idle session — the one that has held its connection slot
/// the longest, the prime pool-exhaustion suspect. `rows` need not already
/// be sorted (the census query returns oldest-first, but this stays correct
/// either way, and is cheap enough at the query's own `IDLE_SESSIONS_LIMIT`
/// row cap).
pub fn oldest_idle_session(rows: &[IdleSessionRow]) -> Option<OldestIdleSession<'_>> {
    rows.iter()
        .max_by(|a, b| a.idle_age_secs.total_cmp(&b.idle_age_secs))
        .map(|row| OldestIdleSession {
            row,
            severity: severity(row.idle_age_secs),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: i32, idle_age_secs: f64) -> IdleSessionRow {
        IdleSessionRow {
            pid,
            application_name: "app".to_string(),
            database: "shop".to_string(),
            client: "10.0.0.1".to_string(),
            username: "app_rw".to_string(),
            idle_age_secs,
        }
    }

    #[test]
    fn severity_tiers_match_the_thresholds() {
        assert_eq!(severity(0.0), Severity::Ok);
        assert_eq!(severity(1_800.0), Severity::Ok, "boundary is not yet warn");
        assert_eq!(severity(1_800.1), Severity::Warn);
        assert_eq!(severity(14_400.0), Severity::Warn, "boundary is not yet bad");
        assert_eq!(severity(14_400.1), Severity::Bad);
        assert_eq!(severity(86_400.0), Severity::Bad);
    }

    #[test]
    fn oldest_idle_session_picks_the_largest_age() {
        let rows = vec![row(1, 100.0), row(2, 20_000.0), row(3, 500.0)];
        let oldest = oldest_idle_session(&rows).expect("rows is non-empty");
        assert_eq!(oldest.row.pid, 2);
        assert_eq!(oldest.severity, Severity::Bad);
    }

    #[test]
    fn oldest_idle_session_is_none_when_empty() {
        assert!(oldest_idle_session(&[]).is_none());
    }
}
