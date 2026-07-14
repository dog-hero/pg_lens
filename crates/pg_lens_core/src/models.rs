//! Serializable data models shared by all pg_lens frontends.
//!
//! Every struct here derives `serde::Serialize` from day one: the future web
//! frontend (Fase 6) streams these exact types as JSON.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::history::SnapshotHistory;

/// Monotonic counter so that every [`DbSnapshot::mock`] call produces visibly
/// different (but deterministic) data — the mock poller (Fase 2) relies on
/// this to prove that fresh snapshots actually reach the screen.
static MOCK_CALLS: AtomicU64 = AtomicU64::new(0);

/// Deterministic pseudo-random value in `0..range` (SplitMix64-style
/// scramble). No `rand` dependency needed for fake data.
fn jitter(seq: u64, salt: u64, range: u64) -> u64 {
    let mut z = seq
        .wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) % range
}

/// One row of `pg_stat_activity`, mirroring the columns produced by the
/// pg_activity reference query (`get_pg_activity_post_140000.sql`).
#[derive(Clone, Debug, Serialize)]
pub struct ActivityRow {
    pub pid: i32,
    pub application_name: String,
    pub database: String,
    pub client: String,
    /// `EXTRACT(epoch FROM (NOW() - query_start))`.
    pub duration_secs: f64,
    pub wait_event: Option<String>,
    pub username: String,
    pub state: String,
    pub query: String,
    /// `coalesce(leader_pid, pid)` — groups parallel workers under a leader.
    pub query_leader_pid: i32,
    pub is_parallel_worker: bool,
    /// Only present on PG 14+.
    pub query_id: Option<i64>,
}

/// One blocked session from the blocking query (`pg_blocking_pids` based):
/// which pid is blocked, by whom, and on what.
#[derive(Clone, Debug, Serialize)]
pub struct LockRow {
    /// The *blocked* backend.
    pub pid: i32,
    /// `pg_blocking_pids(pid)` — the backends holding it up.
    pub blocked_by: Vec<i32>,
    /// Lock mode being awaited (e.g. `ShareLock`), if a pg_locks row matched.
    pub mode: Option<String>,
    /// `pg_locks.locktype` (e.g. `transactionid`, `relation`).
    pub locktype: Option<String>,
    /// Relation name, when the awaited lock targets one.
    pub relation: Option<String>,
    /// How long the blocked query has been running.
    pub duration_secs: f64,
    /// The blocked query text.
    pub query: String,
}

/// Server-wide vitals feeding the Macro Lens dashboard.
#[derive(Clone, Debug, Serialize)]
pub struct ServerVitals {
    pub server_version: String,
    pub uptime_secs: u64,
    pub connections_total: u32,
    pub max_connections: u32,
    pub active: u32,
    pub idle: u32,
    pub idle_in_transaction: u32,
    pub waiting: u32,
    /// Δ(xact_commit + xact_rollback) / Δt, computed by the poller.
    pub tps: f64,
    /// blks_hit / (blks_hit + blks_read), in `0.0..=1.0` (delta-based after
    /// the first poll of a session).
    pub cache_hit_ratio: f64,
    /// Cumulative counters from `pg_stat_database` (sum over all databases);
    /// Fase 4 turns some of these into deltas/rates for display.
    pub tup_returned: i64,
    pub tup_fetched: i64,
    pub temp_files: i64,
    pub temp_bytes: i64,
    pub deadlocks: i64,
}

/// Health of the poller loop, carried inside every snapshot so that all
/// frontends can surface collection errors without a side channel.
#[derive(Clone, Debug, Serialize)]
pub enum PollerStatus {
    Ok,
    /// First connection attempt still in flight — no data yet.
    Connecting,
    Error(String),
}

/// One complete observation of the monitored server. Published by the real
/// poller (Fase 3) or, in `--mock` mode, by [`DbSnapshot::mock`].
#[derive(Clone, Debug, Serialize)]
pub struct DbSnapshot {
    pub vitals: ServerVitals,
    pub activity: Vec<ActivityRow>,
    /// Blocked sessions (who waits on whom). Empty when nothing is blocked.
    pub locks: Vec<LockRow>,
    /// Time series of poll-derived metrics (TPS, active sessions). Owned and
    /// grown **incrementally by the poller** — one push per poll, never
    /// rebuilt; a clone travels in every envelope so all consumers (TUI
    /// sparklines, future web charts) see the same series.
    pub history: SnapshotHistory,
    pub status: PollerStatus,
}

impl DbSnapshot {
    /// Plausible fake data for developing the frontends before the real data
    /// layer exists (Fases 1–2). Every call is deterministically *different*
    /// from the previous one (jittered TPS/counters, scrolling TPS history,
    /// growing durations) so a screen fed by the mock poller visibly changes.
    pub fn mock() -> Self {
        let seq = MOCK_CALLS.fetch_add(1, Ordering::Relaxed);

        let tps = 900.0 + jitter(seq, 1, 700) as f64;

        let active = 4 + jitter(seq, 3, 8) as u32;
        let idle_in_transaction = 1 + jitter(seq, 4, 4) as u32;
        let connections_total = 30 + jitter(seq, 2, 25) as u32;
        let idle = connections_total.saturating_sub(active + idle_in_transaction);

        let vitals = ServerVitals {
            server_version: "16.3 (mock)".to_string(),
            uptime_secs: 3 * 86_400 + 4 * 3_600 + 27 * 60 + seq * 2,
            connections_total,
            max_connections: 100,
            active,
            idle,
            idle_in_transaction,
            waiting: jitter(seq, 5, 4) as u32,
            tps,
            cache_hit_ratio: 0.95 + jitter(seq, 6, 50) as f64 / 1_000.0,
            tup_returned: 9_000_000 + (seq as i64) * 1_500,
            tup_fetched: 7_400_000 + (seq as i64) * 1_200,
            temp_files: 3,
            temp_bytes: 48 * 1024 * 1024,
            deadlocks: 0,
        };

        // Long-running sessions keep aging between snapshots.
        let age = seq as f64 * 2.0;

        let activity = vec![
            ActivityRow {
                pid: 4821,
                application_name: "checkout-api".to_string(),
                database: "shop".to_string(),
                client: "10.0.4.12".to_string(),
                // Short-lived OLTP query: fresh duration every snapshot.
                duration_secs: 0.02 + jitter(seq, 7, 80) as f64 / 1_000.0,
                wait_event: None,
                username: "app_rw".to_string(),
                state: "active".to_string(),
                query: "SELECT o.id, o.total FROM orders o WHERE o.customer_id = $1 ORDER BY \
                        o.created_at DESC LIMIT 20"
                    .to_string(),
                query_leader_pid: 4821,
                is_parallel_worker: false,
                query_id: Some(-8_231_734_902_117_431_882),
            },
            ActivityRow {
                pid: 4977,
                application_name: "pgbench".to_string(),
                database: "bench".to_string(),
                client: "10.0.4.99".to_string(),
                duration_secs: 12.7 + age,
                wait_event: Some("Lock:transactionid".to_string()),
                username: "bench".to_string(),
                state: "active".to_string(),
                query: "UPDATE pgbench_branches SET bbalance = bbalance + $1 WHERE bid = $2"
                    .to_string(),
                query_leader_pid: 4977,
                is_parallel_worker: false,
                query_id: Some(3_004_918_872_215_881_003),
            },
            ActivityRow {
                pid: 5010,
                application_name: "reporting".to_string(),
                database: "warehouse".to_string(),
                client: "10.0.7.3".to_string(),
                duration_secs: 384.2 + age,
                wait_event: Some("IO:DataFileRead".to_string()),
                username: "analytics_ro".to_string(),
                state: "active".to_string(),
                query: "SELECT date_trunc('day', created_at) AS day, count(*) FROM events \
                        GROUP BY 1 ORDER BY 1"
                    .to_string(),
                query_leader_pid: 5010,
                is_parallel_worker: false,
                query_id: Some(551_202_998_310_442_781),
            },
            ActivityRow {
                pid: 5011,
                application_name: "reporting".to_string(),
                database: "warehouse".to_string(),
                client: "10.0.7.3".to_string(),
                duration_secs: 384.2 + age,
                wait_event: Some("IPC:MessageQueueSend".to_string()),
                username: "analytics_ro".to_string(),
                state: "active".to_string(),
                query: "SELECT date_trunc('day', created_at) AS day, count(*) FROM events \
                        GROUP BY 1 ORDER BY 1"
                    .to_string(),
                query_leader_pid: 5010,
                is_parallel_worker: true,
                query_id: Some(551_202_998_310_442_781),
            },
            ActivityRow {
                pid: 4312,
                application_name: "psql".to_string(),
                database: "shop".to_string(),
                client: "local".to_string(),
                duration_secs: 1_922.0 + age,
                wait_event: Some("Client:ClientRead".to_string()),
                username: "leonardo".to_string(),
                state: "idle in transaction".to_string(),
                query: "UPDATE products SET price = price * 1.1 WHERE category = 'books'"
                    .to_string(),
                query_leader_pid: 4312,
                is_parallel_worker: false,
                query_id: None,
            },
            ActivityRow {
                pid: 4650,
                application_name: "vacuumdb".to_string(),
                database: "shop".to_string(),
                client: "local".to_string(),
                duration_secs: 88.4 + age,
                wait_event: None,
                username: "postgres".to_string(),
                state: "active".to_string(),
                query: "autovacuum: VACUUM ANALYZE public.order_items".to_string(),
                query_leader_pid: 4650,
                is_parallel_worker: false,
                query_id: None,
            },
        ];

        // Matches the story above: pid 4977 waits on a transactionid lock
        // held by the idle-in-transaction psql session (pid 4312).
        let locks = vec![LockRow {
            pid: 4977,
            blocked_by: vec![4312],
            mode: Some("ShareLock".to_string()),
            locktype: Some("transactionid".to_string()),
            relation: None,
            duration_secs: 12.7 + age,
            query: "UPDATE pgbench_branches SET bbalance = bbalance + $1 WHERE bid = $2"
                .to_string(),
        }];

        Self {
            vitals,
            activity,
            locks,
            // Empty on purpose: the ring is owned and grown by the poller
            // ([`crate::poller`]), which stamps its clone onto each envelope.
            history: SnapshotHistory::default(),
            status: PollerStatus::Ok,
        }
    }

    /// The pre-filled value of the real poller's watch channel: no data yet,
    /// first connection attempt still in flight.
    pub fn connecting() -> Self {
        Self {
            vitals: ServerVitals {
                server_version: "?".to_string(),
                uptime_secs: 0,
                connections_total: 0,
                max_connections: 0,
                active: 0,
                idle: 0,
                idle_in_transaction: 0,
                waiting: 0,
                tps: 0.0,
                cache_hit_ratio: 0.0,
                tup_returned: 0,
                tup_fetched: 0,
                temp_files: 0,
                temp_bytes: 0,
                deadlocks: 0,
            },
            activity: Vec::new(),
            locks: Vec::new(),
            history: SnapshotHistory::default(),
            status: PollerStatus::Connecting,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_snapshot_is_plausible() {
        let snapshot = DbSnapshot::mock();
        assert!(!snapshot.activity.is_empty());
        assert!(snapshot.vitals.connections_total <= snapshot.vitals.max_connections);
        assert!((0.0..=1.0).contains(&snapshot.vitals.cache_hit_ratio));
        assert!(matches!(snapshot.status, PollerStatus::Ok));
    }

    #[test]
    fn mock_varies_between_calls() {
        let a = serde_json::to_string(&DbSnapshot::mock()).expect("serialize");
        let b = serde_json::to_string(&DbSnapshot::mock()).expect("serialize");
        assert_ne!(a, b, "consecutive mock snapshots must differ");
    }

    #[test]
    fn snapshot_serializes_to_json() {
        let snapshot = DbSnapshot::mock();
        let json = serde_json::to_string(&snapshot).expect("snapshot must serialize");
        assert!(json.contains("\"pid\":4821"));
        assert!(json.contains("\"max_connections\":100"));
        assert!(json.contains("\"blocked_by\":[4312]"));
    }

    #[test]
    fn connecting_snapshot_serializes_to_json() {
        let snapshot = DbSnapshot::connecting();
        let json = serde_json::to_string(&snapshot).expect("snapshot must serialize");
        assert!(json.contains("\"status\":\"Connecting\""));
        assert!(matches!(snapshot.status, PollerStatus::Connecting));
    }
}
