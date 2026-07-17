//! Lock-table pressure gauge (v0.11).
//!
//! The shared-memory lock table is a FIXED-size array, sized at postmaster
//! start from `max_locks_per_transaction * (max_connections +
//! max_prepared_transactions)` slots (the documented capacity formula).
//! Once every slot is in use, the next backend that requests a lock does not
//! wait — it gets a hard `ERROR: out of shared memory` (with the classic hint
//! to raise `max_locks_per_transaction`), which typically means an incident,
//! not graceful degradation. This module turns the raw
//! [`crate::db::LockCapacityRow`] into a [`crate::models::LockCapacity`] with
//! the derived fraction, and carries the severity tiers over that fraction —
//! same shape as [`crate::prepared_xacts`]/[`crate::xact_age`]/[`crate::waits`]:
//! pure, DB-free, unit-tested directly. The TUI renders it in the Macro
//! Lens vitals strip; the web frontend ports the same thresholds in
//! `frontend/src/lock_capacity.ts`.

use crate::db::LockCapacityRow;
use crate::models::LockCapacity;

/// Yellow past this fraction of the lock table's capacity — comfortably
/// before exhaustion, but worth watching (a burst of long transactions or a
/// batch job taking many locks can climb fast).
pub const WARN_FRACTION: f64 = 0.6;
/// Red past this fraction — close enough to "out of shared memory" that an
/// operator should act now, not just note it.
pub const BAD_FRACTION: f64 = 0.85;

/// Severity tier of the lock table's used fraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Warn,
    Bad,
}

/// Severity of one used-fraction reading. Below [`WARN_FRACTION`] is calm;
/// above [`BAD_FRACTION`] is the outage-precursor zone.
pub fn severity(used_fraction: f64) -> Severity {
    if used_fraction > BAD_FRACTION {
        Severity::Bad
    } else if used_fraction > WARN_FRACTION {
        Severity::Warn
    } else {
        Severity::Ok
    }
}

/// Turns a raw `lock_capacity.sql` row into the [`LockCapacity`] model,
/// computing the documented capacity formula and the used fraction. A
/// `capacity_slots` of 0 (a misconfigured/garbage `current_setting` value)
/// yields `used_fraction = 0.0` rather than a division-by-zero `NaN`/`inf` —
/// the gauge renders empty instead of panicking or lying.
pub fn compute(raw: LockCapacityRow) -> LockCapacity {
    let capacity_slots =
        raw.max_locks_per_xact * (raw.max_connections + raw.max_prepared_xacts);
    let used_fraction = if capacity_slots > 0 {
        raw.locks_held as f64 / capacity_slots as f64
    } else {
        0.0
    };
    LockCapacity {
        locks_held: raw.locks_held,
        max_locks_per_xact: raw.max_locks_per_xact,
        max_connections: raw.max_connections,
        max_prepared_xacts: raw.max_prepared_xacts,
        capacity_slots,
        used_fraction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(locks_held: i64, max_locks_per_xact: i64, max_connections: i64, max_prepared_xacts: i64) -> LockCapacityRow {
        LockCapacityRow {
            locks_held,
            max_locks_per_xact,
            max_connections,
            max_prepared_xacts,
        }
    }

    #[test]
    fn compute_derives_capacity_and_fraction() {
        // 64 * (100 + 0) = 6400 capacity slots; 3200 held => 50%.
        let lc = compute(raw(3_200, 64, 100, 0));
        assert_eq!(lc.capacity_slots, 6_400);
        assert!((lc.used_fraction - 0.5).abs() < 1e-9);
    }

    #[test]
    fn compute_includes_prepared_transactions_in_capacity() {
        // 64 * (100 + 20) = 7680 capacity slots.
        let lc = compute(raw(0, 64, 100, 20));
        assert_eq!(lc.capacity_slots, 7_680);
    }

    #[test]
    fn compute_never_divides_by_zero() {
        let lc = compute(raw(10, 0, 0, 0));
        assert_eq!(lc.capacity_slots, 0);
        assert_eq!(lc.used_fraction, 0.0);
    }

    #[test]
    fn severity_tiers_match_the_thresholds() {
        assert_eq!(severity(0.0), Severity::Ok);
        assert_eq!(severity(0.6), Severity::Ok, "boundary is not yet warn");
        assert_eq!(severity(0.6001), Severity::Warn);
        assert_eq!(severity(0.85), Severity::Warn, "boundary is not yet bad");
        assert_eq!(severity(0.8501), Severity::Bad);
        assert_eq!(severity(1.0), Severity::Bad);
    }
}
