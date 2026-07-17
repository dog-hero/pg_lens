//! Blocking chain / lock-wait graph (v0.9).
//!
//! Pure in-memory derivation over `DbSnapshot.locks` (`LockRow::blocked_by`,
//! itself sourced from `queries/blocking_post_140000.sql` — no new SQL, same
//! shape as [`crate::waits`] / [`crate::xact_age`]). Given the blocked pid a
//! frontend has selected, this walks `blocked_by` transitively to the ROOT
//! blocker: the session at the head of the wait-for chain that is not itself
//! waiting on anyone (or, on a real deadlock, back to a pid already in the
//! chain). The TUI calls [`blocking_chain`] at render time; the web frontend
//! ports the same walk in `frontend/src/blocking.ts` (the way `waits.ts`
//! mirrors `waits.rs`).
//!
//! Each blocked backend can report several blockers (`pg_blocking_pids`
//! returns a `Vec`) — the chain follows the FIRST one at every step. That is
//! a deliberate simplification: the wait-for graph can genuinely branch, but
//! a linear "who do I act on first" chain is what the detail panel has room
//! for, and the head of any branch is still a real blocker worth surfacing.

use std::collections::{HashMap, HashSet};

use crate::models::LockRow;

/// The wait-for chain starting at a selected blocked pid, ending at either
/// the root blocker (a pid with no `LockRow` of its own — i.e. not itself
/// waiting) or, on `deadlock == true`, back at a pid already seen in the
/// chain (the cycle).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockingChain {
    /// Ordered pids: `chain[0]` is the pid this was built for, `chain.last()`
    /// is the root blocker (or the repeated pid that closes a cycle).
    pub chain: Vec<i32>,
    /// True when the walk revisited a pid already in `chain` — a genuine
    /// wait-for cycle (deadlock) rather than terminating at a free session.
    pub deadlock: bool,
}

impl BlockingChain {
    /// The pid to actually act on: the last link, which is either the root
    /// blocker or (on a deadlock) the pid that closes the cycle.
    pub fn root(&self) -> Option<i32> {
        self.chain.last().copied()
    }
}

/// Build the wait-for chain for `pid`, or `None` if `pid` is not itself
/// blocked (no matching `LockRow`) — the caller has nothing to show.
///
/// Bounded by construction: `visited` is checked before every step extends
/// the chain, so a cycle is detected and stops the walk on its first repeat
/// — never an infinite loop, regardless of how the data is shaped.
pub fn blocking_chain(pid: i32, locks: &[LockRow]) -> Option<BlockingChain> {
    let by_pid: HashMap<i32, &LockRow> = locks.iter().map(|l| (l.pid, l)).collect();
    by_pid.get(&pid)?;

    let mut chain = vec![pid];
    let mut visited: HashSet<i32> = HashSet::from([pid]);
    let mut current = pid;
    let mut deadlock = false;

    while let Some(row) = by_pid.get(&current) {
        let Some(&next) = row.blocked_by.first() else {
            break; // No blocker on record for this row — treat it as root.
        };
        if visited.contains(&next) {
            chain.push(next);
            deadlock = true;
            break;
        }
        chain.push(next);
        visited.insert(next);
        current = next;
    }

    Some(BlockingChain { chain, deadlock })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(pid: i32, blocked_by: &[i32]) -> LockRow {
        LockRow {
            pid,
            blocked_by: blocked_by.to_vec(),
            mode: Some("ShareLock".to_string()),
            locktype: Some("transactionid".to_string()),
            relation: None,
            duration_secs: 1.0,
            query: String::new(),
        }
    }

    #[test]
    fn a_free_pid_has_no_chain() {
        let locks = vec![lock(2, &[1])];
        assert!(blocking_chain(99, &locks).is_none());
    }

    #[test]
    fn direct_pair_chains_to_the_single_blocker() {
        let locks = vec![lock(2, &[1])];
        let chain = blocking_chain(2, &locks).expect("2 is blocked");
        assert_eq!(chain.chain, vec![2, 1]);
        assert!(!chain.deadlock);
        assert_eq!(chain.root(), Some(1));
    }

    #[test]
    fn three_level_chain_walks_to_the_root_blocker() {
        // C waits on B, B waits on A, A is free (no LockRow of its own).
        let locks = vec![lock(3, &[2]), lock(2, &[1])];
        let chain = blocking_chain(3, &locks).expect("3 is blocked");
        assert_eq!(chain.chain, vec![3, 2, 1]);
        assert!(!chain.deadlock);
        assert_eq!(chain.root(), Some(1));
    }

    #[test]
    fn deadlock_cycle_is_detected_and_bounded() {
        // A waits on B, B waits on A: a genuine wait-for cycle.
        let locks = vec![lock(1, &[2]), lock(2, &[1])];
        let chain = blocking_chain(1, &locks).expect("1 is blocked");
        assert!(chain.deadlock);
        // Terminates instead of looping forever: 1, 2, then the repeat of 1.
        assert_eq!(chain.chain, vec![1, 2, 1]);
    }

    #[test]
    fn self_referencing_row_is_a_trivial_deadlock() {
        let locks = vec![lock(1, &[1])];
        let chain = blocking_chain(1, &locks).expect("1 is blocked");
        assert!(chain.deadlock);
        assert_eq!(chain.chain, vec![1, 1]);
    }

    #[test]
    fn mock_snapshot_has_a_three_level_chain() {
        // v0.9 acceptance: --mock demos a multi-level chain (A blocks B
        // blocks C), not just a single blocked pair.
        let snapshot = crate::DbSnapshot::mock();
        let deepest = snapshot
            .locks
            .iter()
            .map(|l| blocking_chain(l.pid, &snapshot.locks).expect("blocked pid has a chain"))
            .max_by_key(|c| c.chain.len())
            .expect("mock has at least one blocked pid");
        assert!(
            deepest.chain.len() >= 3,
            "expected a >=3-level chain in mock data, got {:?}",
            deepest.chain
        );
        assert!(!deepest.deadlock, "mock data should not be a deadlock");
    }
}
