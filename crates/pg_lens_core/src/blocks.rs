//! Hierarchical blocking tree and lock conflict derivation (v0.17).
//!
//! Turns flat `LockRow`s (from `pg_blocking_pids`) into a structured forest of
//! wait-for trees. Identifies root blockers (sessions blocking others while not
//! waiting on anyone) and cycles (deadlocks).
//!
//! Pure in-memory computation over `DbSnapshot.locks`, `DbSnapshot.activity`,
//! and `DbSnapshot.idle_sessions`.

use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::models::{ActivityRow, IdleSessionRow, LockRow};

/// One node in the hierarchical blocking tree.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BlockTreeNode {
    /// Backend process ID.
    pub pid: i32,
    /// True if this node is at the root of a blocking tree (the root blocker).
    pub is_root: bool,
    /// True if this node is involved in a circular wait (deadlock).
    pub is_deadlock: bool,
    pub usename: String,
    pub application_name: String,
    pub state: String,
    pub query: String,
    pub duration_secs: f64,
    pub xact_age_secs: Option<f64>,
    pub wait_event: Option<String>,
    pub mode: Option<String>,
    pub locktype: Option<String>,
    pub relation: Option<String>,
    pub blocked_by: Vec<i32>,
    pub children: Vec<BlockTreeNode>,
    /// Total count of all descendant sessions waiting behind this node.
    pub num_descendants: usize,
    /// Maximum query/wait duration among this node and all its descendants.
    pub max_descendant_duration: f64,
}

struct SessionInfo {
    usename: String,
    application_name: String,
    state: String,
    query: String,
    duration_secs: f64,
    xact_age_secs: Option<f64>,
    wait_event: Option<String>,
}

/// Builds the hierarchical blocking forest from the current snapshot's locks
/// and session activity.
pub fn build_blocking_tree(
    locks: &[LockRow],
    activity: &[ActivityRow],
    idle_sessions: Option<&[IdleSessionRow]>,
) -> Vec<BlockTreeNode> {
    if locks.is_empty() {
        return Vec::new();
    }

    // Index session info from activity and idle sessions
    let mut sessions: HashMap<i32, SessionInfo> = HashMap::new();

    if let Some(idle) = idle_sessions {
        for s in idle {
            sessions.insert(
                s.pid,
                SessionInfo {
                    usename: s.username.clone(),
                    application_name: s.application_name.clone(),
                    state: "idle".to_string(),
                    query: "(idle)".to_string(),
                    duration_secs: s.idle_age_secs,
                    xact_age_secs: None,
                    wait_event: None,
                },
            );
        }
    }

    for a in activity {
        sessions.insert(
            a.pid,
            SessionInfo {
                usename: a.username.clone(),
                application_name: a.application_name.clone(),
                state: a.state.clone(),
                query: a.query.clone(),
                duration_secs: a.duration_secs,
                xact_age_secs: a.xact_age_secs,
                wait_event: a.wait_event.clone(),
            },
        );
    }

    // Map blocked PIDs to their LockRow
    let lock_by_pid: HashMap<i32, &LockRow> = locks.iter().map(|l| (l.pid, l)).collect();

    // Map blocker -> list of blocked pids
    let mut waiters_by_blocker: HashMap<i32, Vec<i32>> = HashMap::new();
    for l in locks {
        for &blocker in &l.blocked_by {
            waiters_by_blocker.entry(blocker).or_default().push(l.pid);
        }
    }

    // Deduplicate waiters per blocker
    for waiters in waiters_by_blocker.values_mut() {
        waiters.sort_unstable();
        waiters.dedup();
    }

    let blocked_pids: HashSet<i32> = locks.iter().map(|l| l.pid).collect();
    let all_blockers: HashSet<i32> = waiters_by_blocker.keys().copied().collect();

    // Root blockers: pids that block someone, but are NOT themselves blocked
    let root_pids: Vec<i32> = all_blockers
        .iter()
        .copied()
        .filter(|pid| !blocked_pids.contains(pid))
        .collect();

    let mut visited_globally: HashSet<i32> = HashSet::new();
    let mut trees = Vec::new();

    // 1. Build trees from clear root blockers
    for root_pid in root_pids {
        let mut path = HashSet::new();
        if let Some(node) = build_node(
            root_pid,
            true,
            false,
            &lock_by_pid,
            &waiters_by_blocker,
            &sessions,
            &mut path,
            &mut visited_globally,
        ) {
            trees.push(node);
        }
    }

    // 2. Handle deadlock cycles: any blocked pid not yet visited is part of a cycle
    for l in locks {
        if !visited_globally.contains(&l.pid) {
            let mut path = HashSet::new();
            if let Some(node) = build_node(
                l.pid,
                true,
                true,
                &lock_by_pid,
                &waiters_by_blocker,
                &sessions,
                &mut path,
                &mut visited_globally,
            ) {
                trees.push(node);
            }
        }
    }

    // Sort trees: most impacted first (descendants count desc, then duration desc)
    trees.sort_by(|a, b| {
        b.num_descendants
            .cmp(&a.num_descendants)
            .then_with(|| {
                b.max_descendant_duration
                    .partial_cmp(&a.max_descendant_duration)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    trees
}

#[allow(clippy::too_many_arguments)]
fn build_node(
    pid: i32,
    is_root: bool,
    force_deadlock: bool,
    lock_by_pid: &HashMap<i32, &LockRow>,
    waiters_by_blocker: &HashMap<i32, Vec<i32>>,
    sessions: &HashMap<i32, SessionInfo>,
    path: &mut HashSet<i32>,
    visited_globally: &mut HashSet<i32>,
) -> Option<BlockTreeNode> {
    if path.contains(&pid) {
        // Cycle detected along current path
        return None;
    }

    path.insert(pid);
    visited_globally.insert(pid);

    let lock_row = lock_by_pid.get(&pid).copied();
    let session = sessions.get(&pid);

    let (mode, locktype, relation, blocked_by) = if let Some(lr) = lock_row {
        (
            lr.mode.clone(),
            lr.locktype.clone(),
            lr.relation.clone(),
            lr.blocked_by.clone(),
        )
    } else {
        (None, None, None, Vec::new())
    };

    let usename = session
        .map(|s| s.usename.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let application_name = session
        .map(|s| s.application_name.clone())
        .unwrap_or_default();
    let state = session
        .map(|s| s.state.clone())
        .unwrap_or_else(|| if lock_row.is_some() { "active".to_string() } else { "unknown".to_string() });
    let query = session
        .map(|s| s.query.clone())
        .or_else(|| lock_row.map(|lr| lr.query.clone()))
        .unwrap_or_default();
    let duration_secs = session
        .map(|s| s.duration_secs)
        .or_else(|| lock_row.map(|lr| lr.duration_secs))
        .unwrap_or(0.0);
    let xact_age_secs = session.and_then(|s| s.xact_age_secs);
    let wait_event = session.and_then(|s| s.wait_event.clone());

    let mut children = Vec::new();
    let mut num_descendants = 0;
    let mut max_descendant_duration = duration_secs;

    if let Some(waiters) = waiters_by_blocker.get(&pid) {
        for &child_pid in waiters {
            if let Some(child_node) = build_node(
                child_pid,
                false,
                false,
                lock_by_pid,
                waiters_by_blocker,
                sessions,
                path,
                visited_globally,
            ) {
                num_descendants += 1 + child_node.num_descendants;
                if child_node.max_descendant_duration > max_descendant_duration {
                    max_descendant_duration = child_node.max_descendant_duration;
                }
                children.push(child_node);
            }
        }
    }

    path.remove(&pid);

    Some(BlockTreeNode {
        pid,
        is_root,
        is_deadlock: force_deadlock,
        usename,
        application_name,
        state,
        query,
        duration_secs,
        xact_age_secs,
        wait_event,
        mode,
        locktype,
        relation,
        blocked_by,
        children,
        num_descendants,
        max_descendant_duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_lock(pid: i32, blocked_by: &[i32], rel: &str) -> LockRow {
        LockRow {
            pid,
            blocked_by: blocked_by.to_vec(),
            mode: Some("ExclusiveLock".to_string()),
            locktype: Some("relation".to_string()),
            relation: Some(rel.to_string()),
            duration_secs: 10.0,
            query: format!("UPDATE {rel} SET v = 1"),
        }
    }

    fn mock_activity(pid: i32, state: &str, query: &str, dur: f64) -> ActivityRow {
        ActivityRow {
            pid,
            application_name: "test_app".to_string(),
            database: "test_db".to_string(),
            client: "127.0.0.1".to_string(),
            duration_secs: dur,
            xact_age_secs: Some(dur + 5.0),
            wait_event: None,
            username: "postgres".to_string(),
            state: state.to_string(),
            query: query.to_string(),
            query_leader_pid: pid,
            is_parallel_worker: false,
            query_id: None,
            ssl: false,
            ssl_version: None,
            ssl_cipher: None,
        }
    }

    #[test]
    fn empty_locks_yields_empty_forest() {
        assert!(build_blocking_tree(&[], &[], None).is_empty());
    }

    #[test]
    fn single_blocker_with_two_waiters() {
        // PID 100 blocks 101 and 102
        let locks = vec![
            mock_lock(101, &[100], "users"),
            mock_lock(102, &[100], "users"),
        ];
        let activity = vec![
            mock_activity(100, "idle in transaction", "UPDATE users SET active = true", 25.0),
            mock_activity(101, "active", "UPDATE users SET email = 'a'", 15.0),
            mock_activity(102, "active", "ALTER TABLE users ADD COLUMN age int", 10.0),
        ];

        let forest = build_blocking_tree(&locks, &activity, None);
        assert_eq!(forest.len(), 1);

        let root = &forest[0];
        assert_eq!(root.pid, 100);
        assert!(root.is_root);
        assert!(!root.is_deadlock);
        assert_eq!(root.state, "idle in transaction");
        assert_eq!(root.num_descendants, 2);
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].pid, 101);
        assert_eq!(root.children[1].pid, 102);
    }

    #[test]
    fn multi_level_chain() {
        // 100 blocks 101, 101 blocks 102
        let locks = vec![
            mock_lock(101, &[100], "orders"),
            mock_lock(102, &[101], "orders"),
        ];
        let activity = vec![
            mock_activity(100, "active", "LOCK orders", 30.0),
            mock_activity(101, "active", "UPDATE orders", 20.0),
            mock_activity(102, "active", "SELECT * FROM orders FOR UPDATE", 10.0),
        ];

        let forest = build_blocking_tree(&locks, &activity, None);
        assert_eq!(forest.len(), 1);

        let root = &forest[0];
        assert_eq!(root.pid, 100);
        assert_eq!(root.num_descendants, 2);
        assert_eq!(root.children.len(), 1);

        let child1 = &root.children[0];
        assert_eq!(child1.pid, 101);
        assert_eq!(child1.num_descendants, 1);
        assert_eq!(child1.children.len(), 1);

        let child2 = &child1.children[0];
        assert_eq!(child2.pid, 102);
        assert_eq!(child2.num_descendants, 0);
    }

    #[test]
    fn deadlock_cycle_is_detected_and_bounded() {
        // 101 blocks 102, 102 blocks 101
        let locks = vec![
            mock_lock(101, &[102], "tbl1"),
            mock_lock(102, &[101], "tbl2"),
        ];
        let activity = vec![
            mock_activity(101, "active", "LOCK tbl1", 10.0),
            mock_activity(102, "active", "LOCK tbl2", 10.0),
        ];

        let forest = build_blocking_tree(&locks, &activity, None);
        assert_eq!(forest.len(), 1);
        assert!(forest[0].is_deadlock);
    }
}
