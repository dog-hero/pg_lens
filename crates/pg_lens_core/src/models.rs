//! Serializable data models shared by all pg_lens frontends.
//!
//! Every struct here derives `serde::Serialize` from day one: the future web
//! frontend (Fase 6) streams these exact types as JSON.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::history::{SnapshotHistory, epoch_ms_now};

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
    /// `current_database()` — the database this connection observes. The
    /// Schema Lens (per-database by construction) names it in its footer.
    pub database: String,
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

/// One row of the Schema Lens table-stats query
/// (`queries/table_stats_post_130000.sql`): `pg_stat_user_tables` counters
/// plus on-disk sizes, for one user table of the *connected database*.
#[derive(Clone, Debug, Serialize)]
pub struct TableStatRow {
    pub schema: String,
    pub name: String,
    /// `pg_total_relation_size(relid)` — heap + indexes + TOAST.
    pub total_bytes: i64,
    /// `pg_table_size(relid)` — heap + TOAST, no indexes.
    pub table_bytes: i64,
    /// `pg_indexes_size(relid)`.
    pub index_bytes: i64,
    pub seq_scan: i64,
    pub seq_tup_read: i64,
    /// NULL in `pg_stat_user_tables` when the table has no indexes — kept
    /// as `None` (distinct from "indexed but never scanned" = `Some(0)`).
    pub idx_scan: Option<i64>,
    pub idx_tup_fetch: Option<i64>,
    pub n_tup_ins: i64,
    pub n_tup_upd: i64,
    pub n_tup_del: i64,
    pub n_tup_hot_upd: i64,
    pub n_live_tup: i64,
    pub n_dead_tup: i64,
    pub n_mod_since_analyze: i64,
    /// PG 13+ (the reason the lens's floor is 13).
    pub n_ins_since_vacuum: i64,
    /// `EXTRACT(epoch FROM last_vacuum)::float8` — Unix epoch seconds;
    /// `None` = never (auto)vacuumed/analyzed since the stats began.
    pub last_vacuum_epoch_secs: Option<f64>,
    pub last_autovacuum_epoch_secs: Option<f64>,
    pub last_analyze_epoch_secs: Option<f64>,
    pub last_autoanalyze_epoch_secs: Option<f64>,
    pub vacuum_count: i64,
    pub autovacuum_count: i64,
    pub analyze_count: i64,
    pub autoanalyze_count: i64,
}

/// One estimated-bloat row (table or btree index), shaped after the output
/// of ioguix/pgsql-bloat-estimation. Defined in Fase S1 so the snapshot
/// schema is final; the vectors stay empty until Fase S2 runs the queries.
#[derive(Clone, Debug, Serialize)]
pub struct BloatRow {
    pub schema: String,
    /// Table name, or index name for `index_bloat` rows.
    pub name: String,
    /// For `index_bloat` rows: the table the index belongs to (so the
    /// Schema Lens detail can list "indexes of this table"). `None` for
    /// `table_bloat` rows, where `name` already is the table.
    pub table: Option<String>,
    /// Current on-disk size of the relation.
    pub real_bytes: i64,
    /// Estimated wasted bytes. `None` when the estimate is not applicable.
    pub bloat_bytes: Option<i64>,
    /// Estimated bloat percentage of `real_bytes`. `None` when `is_na`.
    pub bloat_pct: Option<f64>,
    pub fillfactor: Option<i32>,
    /// ioguix's "not applicable" flag: the estimate is unreliable (e.g.
    /// `name`-typed columns, missing statistics). UIs must show a marker,
    /// never a made-up number.
    pub is_na: bool,
}

/// Cluster-wide XID wraparound distance (F2): the worst `age(datfrozenxid)`
/// across every database in the cluster, plus the database that owns it.
/// Collected on the slow schema cadence (`queries/vacuum_cluster_age.sql`) —
/// cheap catalog read, but cluster-wide by nature, not per-connected-db like
/// the rest of the Schema Lens.
#[derive(Clone, Debug, Serialize)]
pub struct VacuumClusterAge {
    pub max_age_xids: i64,
    pub worst_database: String,
}

/// One table's XID age + dead-tuple ratio ("vacuum debt"), F2. Worst N of
/// the connected database's user tables, collected alongside `tables` on
/// the same slow cadence (`queries/vacuum_table_ages.sql`).
#[derive(Clone, Debug, Serialize)]
pub struct VacuumTableRow {
    pub schema: String,
    pub name: String,
    pub age_xids: i64,
    pub n_dead_tup: i64,
    pub n_live_tup: i64,
}

/// One in-flight `pg_stat_progress_vacuum` row, F2. Collected on the FAST
/// tick, best-effort (see [`DbSnapshot::vacuum_progress`]).
#[derive(Clone, Debug, Serialize)]
pub struct VacuumProgressRow {
    pub pid: i32,
    /// Target relation name, or `"?"` if it was dropped mid-scan.
    pub relation: String,
    /// `initializing`, `scanning heap`, `vacuuming indexes`, `vacuuming
    /// heap`, `cleaning up indexes`, `truncating heap`, `performing final
    /// cleanup`.
    pub phase: String,
    pub heap_blks_total: i64,
    pub heap_blks_scanned: i64,
}

/// Health of the *slow* schema collection, separate from [`PollerStatus`]:
/// a failing schema query must never taint the 2s activity pipeline.
#[derive(Clone, Debug, Serialize)]
pub enum SchemaStatus {
    Ok,
    Error(String),
}

/// The Schema Lens payload: table stats (+ estimated bloat from Fase S2 on)
/// of the connected database, collected on its own slow cadence (default
/// 60s). Wrapped in an `Arc` inside [`DbSnapshot`] so the fast ticks that
/// do *not* recollect it reuse the previous collection for free.
#[derive(Clone, Debug, Serialize)]
pub struct SchemaSnapshot {
    /// When this collection ran (Unix epoch milliseconds) — the staleness
    /// indicator frontends show ("collected Xs ago").
    pub collected_at_epoch_ms: u64,
    /// Top tables by total size (the query caps at 200 rows).
    pub tables: Vec<TableStatRow>,
    /// Estimated table bloat (empty until Fase S2).
    pub table_bloat: Vec<BloatRow>,
    /// Estimated btree index bloat (empty until Fase S2).
    pub index_bloat: Vec<BloatRow>,
    /// Cluster-wide XID wraparound headline (F2). `None` only before the
    /// first successful slow collection of a session (kept, like `tables`,
    /// across a collection error — see [`crate::poller`]).
    pub vacuum_cluster_age: Option<VacuumClusterAge>,
    /// Worst N tables by XID age, F2 (the query caps at 20 rows).
    pub vacuum_tables: Vec<VacuumTableRow>,
    pub status: SchemaStatus,
}

impl SchemaSnapshot {
    /// Plausible fake data (same contract as [`DbSnapshot::mock`]): a few
    /// pgbench-flavoured tables, one of them visibly bloated, plus mock
    /// bloat rows so Fase S3's UI can be built entirely against `--mock`.
    pub fn mock() -> Self {
        let seq = MOCK_CALLS.load(Ordering::Relaxed);
        let churn = (seq as i64) * 37;
        let tables = vec![
            TableStatRow {
                schema: "public".to_string(),
                name: "pgbench_accounts".to_string(),
                total_bytes: 671_088_640,
                table_bytes: 549_453_824,
                index_bytes: 121_634_816,
                seq_scan: 12,
                seq_tup_read: 6_000_000,
                idx_scan: Some(48_211_390 + churn),
                idx_tup_fetch: Some(48_211_390 + churn),
                n_tup_ins: 500_000,
                n_tup_upd: 1_204_388 + churn,
                n_tup_del: 0,
                n_tup_hot_upd: 981_224 + churn,
                n_live_tup: 500_000,
                n_dead_tup: 14_205 + jitter(seq, 20, 9_000) as i64,
                n_mod_since_analyze: 52_180,
                n_ins_since_vacuum: 0,
                last_vacuum_epoch_secs: None,
                last_autovacuum_epoch_secs: Some(1_752_000_000.0),
                last_analyze_epoch_secs: None,
                last_autoanalyze_epoch_secs: Some(1_752_000_300.0),
                vacuum_count: 0,
                autovacuum_count: 42,
                analyze_count: 1,
                autoanalyze_count: 40,
            },
            // The bloated-looking one: dead tuples rival live ones and
            // autovacuum has not caught up.
            TableStatRow {
                schema: "public".to_string(),
                name: "order_items".to_string(),
                total_bytes: 219_152_384,
                table_bytes: 187_695_104,
                index_bytes: 31_457_280,
                seq_scan: 3_310,
                seq_tup_read: 940_115_002,
                idx_scan: Some(88_202),
                idx_tup_fetch: Some(87_990),
                n_tup_ins: 2_400_000,
                n_tup_upd: 5_512_930 + churn,
                n_tup_del: 1_100_000,
                n_tup_hot_upd: 210_004,
                n_live_tup: 1_300_000,
                n_dead_tup: 1_050_000 + churn,
                n_mod_since_analyze: 2_205_818,
                n_ins_since_vacuum: 402_119,
                last_vacuum_epoch_secs: Some(1_751_400_000.0),
                last_autovacuum_epoch_secs: Some(1_751_400_000.0),
                last_analyze_epoch_secs: Some(1_751_400_100.0),
                last_autoanalyze_epoch_secs: Some(1_751_400_100.0),
                vacuum_count: 2,
                autovacuum_count: 7,
                analyze_count: 2,
                autoanalyze_count: 6,
            },
            TableStatRow {
                schema: "public".to_string(),
                name: "pgbench_branches".to_string(),
                total_bytes: 933_888,
                table_bytes: 892_928,
                index_bytes: 40_960,
                seq_scan: 1_204_400 + churn,
                seq_tup_read: 6_022_000 + churn * 5,
                idx_scan: Some(0),
                idx_tup_fetch: Some(0),
                n_tup_ins: 5,
                n_tup_upd: 1_204_388 + churn,
                n_tup_del: 0,
                n_tup_hot_upd: 1_100_301 + churn,
                n_live_tup: 5,
                n_dead_tup: 88,
                n_mod_since_analyze: 3_050,
                n_ins_since_vacuum: 0,
                last_vacuum_epoch_secs: None,
                last_autovacuum_epoch_secs: Some(1_752_000_420.0),
                last_analyze_epoch_secs: None,
                last_autoanalyze_epoch_secs: Some(1_752_000_425.0),
                vacuum_count: 0,
                autovacuum_count: 210,
                analyze_count: 0,
                autoanalyze_count: 204,
            },
            // A table with no indexes at all: idx_scan is NULL, exercising
            // the Option path end to end (SQL → model → JSON → UI).
            TableStatRow {
                schema: "audit".to_string(),
                name: "raw_events".to_string(),
                total_bytes: 96_468_992,
                table_bytes: 96_468_992,
                index_bytes: 0,
                seq_scan: 44,
                seq_tup_read: 12_007_113,
                idx_scan: None,
                idx_tup_fetch: None,
                n_tup_ins: 273_000 + churn,
                n_tup_upd: 0,
                n_tup_del: 0,
                n_tup_hot_upd: 0,
                n_live_tup: 273_000 + churn,
                n_dead_tup: 0,
                n_mod_since_analyze: 9_113,
                n_ins_since_vacuum: 9_113,
                last_vacuum_epoch_secs: None,
                last_autovacuum_epoch_secs: None,
                last_analyze_epoch_secs: None,
                last_autoanalyze_epoch_secs: Some(1_751_990_000.0),
                vacuum_count: 0,
                autovacuum_count: 0,
                analyze_count: 0,
                autoanalyze_count: 3,
            },
        ];
        let table_bloat = vec![
            BloatRow {
                schema: "public".to_string(),
                name: "order_items".to_string(),
                table: None,
                real_bytes: 187_695_104,
                bloat_bytes: Some(101_318_656),
                bloat_pct: Some(53.98),
                fillfactor: Some(100),
                is_na: false,
            },
            // Yellow tier (>30% and >1MB, but under the red >50%/>10MB bar).
            BloatRow {
                schema: "public".to_string(),
                name: "pgbench_history".to_string(),
                table: None,
                real_bytes: 14_680_064,
                bloat_bytes: Some(5_242_880),
                bloat_pct: Some(35.7),
                fillfactor: Some(100),
                is_na: false,
            },
            // Healthy: below both severity tiers (renders uncolored).
            BloatRow {
                schema: "public".to_string(),
                name: "pgbench_accounts".to_string(),
                table: None,
                real_bytes: 549_453_824,
                bloat_bytes: Some(23_068_672),
                bloat_pct: Some(4.2),
                fillfactor: Some(100),
                is_na: false,
            },
            // is_na: a `name`-typed column makes the estimate unreliable.
            BloatRow {
                schema: "audit".to_string(),
                name: "raw_events".to_string(),
                table: None,
                real_bytes: 96_468_992,
                bloat_bytes: None,
                bloat_pct: None,
                fillfactor: Some(100),
                is_na: true,
            },
        ];
        let index_bloat = vec![BloatRow {
            schema: "public".to_string(),
            name: "order_items_pkey".to_string(),
            table: Some("order_items".to_string()),
            real_bytes: 31_457_280,
            bloat_bytes: Some(11_010_048),
            bloat_pct: Some(35.0),
            fillfactor: Some(90),
            is_na: false,
        }];
        // F2: cluster age deliberately sits just BELOW the yellow threshold
        // (200M) — a calm default so `--mock` doesn't open on an alarm; the
        // severity tiers themselves are unit-tested directly, not only
        // demoed. Per-table ages mirror the table-stats rows above, with
        // `order_items` (the bloated-looking one) also carrying the oldest
        // XID age, dead-tuple ratio included so the "vacuum debt" view reads
        // consistently with the Tables list.
        let vacuum_cluster_age = Some(VacuumClusterAge {
            max_age_xids: 182_400_000 + churn.max(0),
            worst_database: "shop".to_string(),
        });
        let vacuum_tables = vec![
            VacuumTableRow {
                schema: "public".to_string(),
                name: "order_items".to_string(),
                age_xids: 179_800_000 + churn.max(0),
                n_dead_tup: 1_050_000 + churn,
                n_live_tup: 1_300_000,
            },
            VacuumTableRow {
                schema: "public".to_string(),
                name: "pgbench_accounts".to_string(),
                age_xids: 96_200_000,
                n_dead_tup: 14_205 + jitter(seq, 20, 9_000) as i64,
                n_live_tup: 500_000,
            },
            VacuumTableRow {
                schema: "audit".to_string(),
                name: "raw_events".to_string(),
                age_xids: 41_050_000,
                n_dead_tup: 0,
                n_live_tup: 273_000 + churn,
            },
        ];
        Self {
            collected_at_epoch_ms: epoch_ms_now(),
            tables,
            table_bloat,
            index_bloat,
            vacuum_cluster_age,
            vacuum_tables,
            status: SchemaStatus::Ok,
        }
    }
}

/// One row of the Query Lens statements query (`queries/statements.sql`):
/// cumulative per-normalized-query counters from `pg_stat_statements`,
/// filtered to the connected database (the extension is cluster-wide).
#[derive(Clone, Debug, Serialize)]
pub struct StatementRow {
    /// `queryid::text` — shipped as TEXT on purpose: the raw int8 can exceed
    /// JavaScript's `Number.MAX_SAFE_INTEGER` (2^53-1), and the web frontend
    /// consumes this field from JSON. `None` = NULL queryid.
    pub query_id: Option<String>,
    /// Normalized query text (constants replaced by `$n`).
    pub query: String,
    /// Role that executed the statement (`pg_roles.rolname`).
    pub username: String,
    pub calls: i64,
    /// Total execution time, milliseconds (`total_exec_time`, ext >= 1.8).
    pub total_exec_ms: f64,
    /// Mean execution time, milliseconds.
    pub mean_exec_ms: f64,
    /// Total rows retrieved or affected.
    pub rows: i64,
    pub shared_blks_hit: i64,
    pub shared_blks_read: i64,
}

/// Health of the Query Lens collection. `Unavailable` is NOT an error: the
/// extension simply is not usable on this server (missing, or older than
/// 1.8 — the version that introduced `total_exec_time`, shipped with PG 13).
/// The string carries the human-readable reason/hint frontends render as a
/// calm per-lens explainer, never an error banner.
#[derive(Clone, Debug, Serialize)]
pub enum StatementsStatus {
    Ok,
    /// Extension missing or too old; the payload says why and what to do.
    Unavailable(String),
    /// The collection query failed (last good data retained, like schema).
    Error(String),
}

/// The Query Lens payload: top statements by total execution time of the
/// connected database. Collected on the SAME slow cadence as the Schema
/// Lens (one shared timer — `R` force-refreshes both) and wrapped in an
/// `Arc` inside [`DbSnapshot`] so fast ticks reuse it at pointer cost.
#[derive(Clone, Debug, Serialize)]
pub struct StatementsSnapshot {
    /// When this collection ran (Unix epoch ms) — staleness indicator.
    pub collected_at_epoch_ms: u64,
    /// Top statements by `total_exec_time` (the query caps at 100 rows).
    pub statements: Vec<StatementRow>,
    pub status: StatementsStatus,
}

impl StatementsSnapshot {
    /// Plausible fake data (same contract as [`DbSnapshot::mock`]): a mix of
    /// SELECT/UPDATE/INSERT with realistic timings, calls climbing between
    /// collections so the slow cadence is observable in `--mock`.
    pub fn mock() -> Self {
        let seq = MOCK_CALLS.load(Ordering::Relaxed);
        let churn = (seq as i64) * 211;
        let row = |query_id: i64,
                   query: &str,
                   username: &str,
                   calls: i64,
                   total_exec_ms: f64,
                   rows: i64,
                   hit: i64,
                   read: i64| StatementRow {
            query_id: Some(query_id.to_string()),
            query: query.to_string(),
            username: username.to_string(),
            calls,
            total_exec_ms,
            mean_exec_ms: if calls > 0 {
                total_exec_ms / calls as f64
            } else {
                0.0
            },
            rows,
            shared_blks_hit: hit,
            shared_blks_read: read,
        };
        let statements = vec![
            row(
                3_004_918_872_215_881_003,
                "UPDATE pgbench_accounts SET abalance = abalance + $1 WHERE aid = $2",
                "bench",
                1_204_388 + churn,
                189_442.7 + churn as f64 * 0.16,
                1_204_388 + churn,
                44_212_190,
                122_408,
            ),
            row(
                -8_231_734_902_117_431_882,
                "SELECT o.id, o.total FROM orders o WHERE o.customer_id = $1 ORDER BY \
                 o.created_at DESC LIMIT $2",
                "app_rw",
                488_210 + churn,
                92_881.4 + churn as f64 * 0.19,
                9_522_101,
                18_309_441,
                2_205_118,
            ),
            row(
                551_202_998_310_442_781,
                "SELECT date_trunc($1, created_at) AS day, count(*) FROM events GROUP BY \
                 $2 ORDER BY $3",
                "analytics_ro",
                42,
                61_204.9,
                18_230,
                1_202_312,
                8_814_209,
            ),
            row(
                7_113_940_012_385_720_114,
                "INSERT INTO order_items (order_id, product_id, qty, price) VALUES \
                 ($1, $2, $3, $4)",
                "app_rw",
                240_119 + churn,
                12_407.1 + churn as f64 * 0.05,
                240_119 + churn,
                4_401_202,
                18_411,
            ),
            row(
                -1_400_233_881_002_117_555,
                "UPDATE pgbench_branches SET bbalance = bbalance + $1 WHERE bid = $2",
                "bench",
                1_204_388 + churn,
                9_302.6 + churn as f64 * 0.01,
                1_204_388 + churn,
                6_020_101,
                0,
            ),
            row(
                6_882_004_113_902_778_231,
                "SELECT p.sku, p.price FROM products p JOIN categories c ON c.id = \
                 p.category_id WHERE c.slug = $1",
                "app_ro",
                88_204,
                4_411.9,
                1_764_080,
                9_213_808,
                44_021,
            ),
            row(
                2_004_113_679_120_881_442,
                "INSERT INTO events (kind, payload, created_at) VALUES ($1, $2, now())",
                "collector",
                730_112 + churn,
                3_209.4 + churn as f64 * 0.02,
                730_112 + churn,
                2_204_119,
                1_202,
            ),
            // Zero shared blocks touched: exercises the Hit% "—" path.
            row(
                -3_310_224_887_664_190_007,
                "SELECT pg_sleep($1)",
                "leonardo",
                3,
                45_002.1,
                3,
                0,
                0,
            ),
        ];
        Self {
            collected_at_epoch_ms: epoch_ms_now(),
            statements,
            status: StatementsStatus::Ok,
        }
    }
}

/// An administrative request a frontend sends TO the poller task (which owns
/// the DB client) over a `tokio::sync::mpsc` channel — the reverse direction
/// of the snapshot `watch`, same message-passing-only rule. TUI-only today:
/// the web frontend stays read-only by design (its API has no such channel).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AdminCommand {
    /// `SELECT pg_cancel_backend($1)` — cancel the backend's current query.
    CancelBackend(i32),
    /// `SELECT pg_terminate_backend($1)` — kill the whole connection.
    TerminateBackend(i32),
}

impl AdminCommand {
    pub fn pid(self) -> i32 {
        match self {
            AdminCommand::CancelBackend(pid) | AdminCommand::TerminateBackend(pid) => pid,
        }
    }

    pub fn kind(self) -> AdminKind {
        match self {
            AdminCommand::CancelBackend(_) => AdminKind::Cancel,
            AdminCommand::TerminateBackend(_) => AdminKind::Terminate,
        }
    }
}

/// Which admin function ran (mirrors [`AdminCommand`], minus the pid).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AdminKind {
    Cancel,
    Terminate,
}

/// What `pg_cancel_backend`/`pg_terminate_backend` said.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum AdminOutcome {
    /// The function's boolean return: `true` = signal sent; `false` = the
    /// PID no longer exists (pg_* may also return false without the
    /// same-user/pg_signal_backend privilege, depending on version/paths).
    Signalled(bool),
    /// The query itself failed — most commonly a privilege error
    /// (`must be a member of the role whose process is being ...`).
    Error(String),
}

/// The result of one [`AdminCommand`], reported back INSIDE the snapshot
/// envelope (no side channel): the poller stamps its most recent result on
/// every snapshot it publishes; frontends dedupe by `at_epoch_ms`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AdminActionResult {
    pub kind: AdminKind,
    pub pid: i32,
    pub outcome: AdminOutcome,
    /// When the command executed (Unix epoch ms) — the dedupe/aging key.
    pub at_epoch_ms: u64,
}

/// One streaming replica connected to a primary (`pg_stat_replication`,
/// `queries/replication.sql`). Lag is reported both ways because either can
/// matter: bytes for how much WAL is outstanding, seconds for how stale the
/// replica's view is.
#[derive(Clone, Debug, Serialize)]
pub struct WalSenderRow {
    /// `application_name` the replica reports (often its `cluster_name`).
    pub application_name: String,
    /// `client_addr::text`, or `"local"` when NULL (Unix-socket replica).
    pub client: String,
    /// `streaming`, `catchup`, `backup`, `startup`, …
    pub state: String,
    /// `async`, `sync`, `quorum`, `potential`.
    pub sync_state: String,
    /// WAL bytes the replica's replay is behind the primary's current LSN.
    /// `None` on a cascading standby (LSN diff is guarded during recovery).
    pub replay_lag_bytes: Option<i64>,
    /// `replay_lag` interval in seconds; `None` while the replica is idle.
    pub replay_lag_secs: Option<f64>,
}

/// The standby side (`pg_stat_wal_receiver` + last replay position,
/// `queries/wal_receiver.sql`).
#[derive(Clone, Debug, Serialize)]
pub struct WalReceiverRow {
    /// `streaming`, `waiting`, `stopping`, …
    pub status: String,
    pub sender_host: Option<String>,
    pub sender_port: Option<i32>,
    /// Received-but-not-yet-replayed WAL bytes on this standby.
    pub replay_lag_bytes: Option<i64>,
    /// Seconds since the last replayed transaction's commit timestamp;
    /// `None` when nothing has been replayed yet.
    pub replay_lag_secs: Option<f64>,
}

/// Replication role and topology, refreshed every fast tick (the queries are
/// a few rows and cheap). Absent (`DbSnapshot.replication == None`) only
/// before the first successful poll of a session.
#[derive(Clone, Debug, Serialize)]
pub enum ReplicationInfo {
    /// This server is a primary; lists its connected replicas (may be empty,
    /// in which case the Macro Lens hides the panel).
    Primary { senders: Vec<WalSenderRow> },
    /// This server is a standby receiving WAL from an upstream. `receiver` is
    /// `None` if `pg_stat_wal_receiver` is momentarily empty (e.g. between
    /// reconnects to the primary).
    Standby { receiver: Option<WalReceiverRow> },
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
    /// Schema Lens payload, collected on its own slow cadence (default 60s
    /// — never on the 2s tick). `None` until the first slow collection ran.
    /// `Arc`: fast ticks that don't recollect reuse the last collection at
    /// pointer-clone cost.
    pub schema: Option<Arc<SchemaSnapshot>>,
    /// Query Lens payload (`pg_stat_statements`), collected on the SAME
    /// slow cadence as `schema` (one shared timer). `None` until the first
    /// slow collection ran; `Arc` for the same reuse-at-pointer-cost reason.
    pub statements: Option<Arc<StatementsSnapshot>>,
    /// Result of the most recent [`AdminCommand`] this poller executed
    /// (cancel/terminate), stamped on every snapshot from then on. `None`
    /// until the first admin action of the session. Frontends that expose
    /// admin actions (the TUI) dedupe on `at_epoch_ms`; read-only frontends
    /// (the web) simply never look at it.
    pub last_admin_action: Option<AdminActionResult>,
    /// Replication role and topology, refreshed every fast tick. `None` only
    /// before the first successful poll of a session.
    pub replication: Option<ReplicationInfo>,
    /// In-flight `pg_stat_progress_vacuum` rows (F2), refreshed every fast
    /// tick, best-effort like `replication`: `None` when the collection
    /// failed this tick (restricted role, hidden view, ...), `Some(vec![])`
    /// when it succeeded and simply found no vacuum running (the common
    /// case — rendered as a calm "no vacuum running", never an error).
    pub vacuum_progress: Option<Vec<VacuumProgressRow>>,
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
            database: "shop".to_string(),
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
            // The mock poller overrides this on its own slow cadence (so
            // staleness UI is exercisable); a bare mock() carries a fresh
            // collection so `--mock` always has schema data to render.
            schema: Some(Arc::new(SchemaSnapshot::mock())),
            // Same contract as `schema`: the mock poller re-stamps it on
            // its own slow cadence; a bare mock() carries a fresh one.
            statements: Some(Arc::new(StatementsSnapshot::mock())),
            // Stamped by the (mock) poller after it executes a command.
            last_admin_action: None,
            // Primary with two replicas: one healthy/async near-zero lag, one
            // lagging tens of MB — so the panel and its severity tiers show.
            replication: Some(ReplicationInfo::Primary {
                senders: vec![
                    WalSenderRow {
                        application_name: "replica-1".to_string(),
                        client: "10.0.8.21".to_string(),
                        state: "streaming".to_string(),
                        sync_state: "async".to_string(),
                        replay_lag_bytes: Some(196_608 + jitter(seq, 8, 131_072) as i64),
                        replay_lag_secs: Some(0.12 + jitter(seq, 9, 400) as f64 / 1_000.0),
                    },
                    WalSenderRow {
                        application_name: "replica-2-dr".to_string(),
                        client: "10.9.2.4".to_string(),
                        state: "streaming".to_string(),
                        sync_state: "async".to_string(),
                        replay_lag_bytes: Some(48 * 1024 * 1024 + (seq as i64) * 131_072),
                        replay_lag_secs: Some(14.5 + age),
                    },
                ],
            }),
            // F2: one in-flight autovacuum, progressing between snapshots so
            // `--mock` visibly moves — 60% at seq 0, wrapping back to a low
            // percentage as `heap_blks_scanned` cycles under the fixed total.
            vacuum_progress: Some(vec![VacuumProgressRow {
                pid: 4650,
                relation: "order_items".to_string(),
                phase: "vacuuming heap".to_string(),
                heap_blks_total: 24_000,
                heap_blks_scanned: (14_400 + (seq as i64) * 350) % 24_000,
            }]),
            status: PollerStatus::Ok,
        }
    }

    /// The pre-filled value of the real poller's watch channel: no data yet,
    /// first connection attempt still in flight.
    pub fn connecting() -> Self {
        Self {
            vitals: ServerVitals {
                server_version: "?".to_string(),
                database: "?".to_string(),
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
            schema: None,
            statements: None,
            last_admin_action: None,
            replication: None,
            vacuum_progress: None,
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

    /// F2: the mock's cluster age sits below the yellow threshold (200M) —
    /// a calm default, per-table ages are present and ordered by age like
    /// the SQL, and the one in-flight vacuum's progress is well-formed
    /// (scanned <= total).
    #[test]
    fn mock_snapshot_carries_calm_vacuum_data() {
        let snapshot = DbSnapshot::mock();
        let schema = snapshot.schema.as_ref().expect("mock must carry schema");
        let cluster_age = schema
            .vacuum_cluster_age
            .as_ref()
            .expect("mock must carry a cluster age");
        assert!(
            cluster_age.max_age_xids < 200_000_000,
            "calm default: below the yellow threshold, got {}",
            cluster_age.max_age_xids
        );
        assert!(!cluster_age.worst_database.is_empty());
        assert!(!schema.vacuum_tables.is_empty());
        for pair in schema.vacuum_tables.windows(2) {
            assert!(
                pair[0].age_xids >= pair[1].age_xids,
                "vacuum_tables must be worst-first, like the SQL's ORDER BY"
            );
        }

        let progress = snapshot
            .vacuum_progress
            .as_ref()
            .expect("mock must carry vacuum progress");
        assert_eq!(progress.len(), 1, "one in-flight autovacuum in the mock");
        let p = &progress[0];
        assert!(p.heap_blks_scanned <= p.heap_blks_total);
        assert!(!p.relation.is_empty());
        assert!(!p.phase.is_empty());
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
    fn mock_snapshot_carries_schema_tables() {
        let snapshot = DbSnapshot::mock();
        let schema = snapshot.schema.as_ref().expect("mock must carry schema");
        assert!(schema.tables.len() >= 3, "several tables expected");
        assert!(matches!(schema.status, SchemaStatus::Ok));
        assert!(schema.collected_at_epoch_ms > 0);
        // The bloated-looking table exists and looks bloated.
        let bloated = schema
            .tables
            .iter()
            .find(|t| t.name == "order_items")
            .expect("mock bloated table");
        assert!(bloated.n_dead_tup * 2 > bloated.n_live_tup);
        // Option path: at least one table has no indexes (idx_scan NULL).
        assert!(schema.tables.iter().any(|t| t.idx_scan.is_none()));
        // Mock bloat rows exist for Fase S3, including an is_na one.
        assert!(!schema.table_bloat.is_empty());
        assert!(!schema.index_bloat.is_empty());
        let na = schema
            .table_bloat
            .iter()
            .find(|b| b.is_na)
            .expect("one is_na bloat row");
        assert!(na.bloat_pct.is_none(), "is_na must not carry a number");
        assert!(na.bloat_bytes.is_none());
        // Index-bloat rows carry their owning table (S3 detail view joins
        // "indexes of this table" through it); table-bloat rows do not.
        assert!(schema.index_bloat.iter().all(|b| b.table.is_some()));
        assert!(schema.table_bloat.iter().all(|b| b.table.is_none()));
        // Fase S3's severity tiers are both exercisable from --mock alone:
        // red = >50% and >10MB; yellow = >30% and >1MB (but not red).
        let tier = |b: &&BloatRow, pct: f64, bytes: i64| {
            b.bloat_pct.is_some_and(|p| p > pct) && b.bloat_bytes.is_some_and(|by| by > bytes)
        };
        assert!(
            schema
                .table_bloat
                .iter()
                .any(|b| tier(&b, 50.0, 10 << 20)),
            "one red-tier table bloat row"
        );
        assert!(
            schema
                .table_bloat
                .iter()
                .any(|b| tier(&b, 30.0, 1 << 20) && !tier(&b, 50.0, 10 << 20)),
            "one yellow-tier table bloat row"
        );
        assert!(
            schema
                .index_bloat
                .iter()
                .any(|b| tier(&b, 30.0, 1 << 20)),
            "one flagged index bloat row"
        );
    }

    #[test]
    fn schema_snapshot_serializes_inside_the_envelope() {
        let snapshot = DbSnapshot::mock();
        let json = serde_json::to_value(&snapshot).expect("snapshot must serialize");
        let schema = json.get("schema").expect("schema field present");
        assert!(
            schema
                .get("tables")
                .and_then(|t| t.as_array())
                .is_some_and(|t| !t.is_empty()),
            "schema.tables serialized: {schema}"
        );
        assert_eq!(schema["status"], serde_json::json!("Ok"));
        // A NULL idx_scan crosses as JSON null, not 0.
        let no_index = schema["tables"]
            .as_array()
            .expect("array")
            .iter()
            .find(|t| t["name"] == "raw_events")
            .expect("raw_events row");
        assert!(no_index["idx_scan"].is_null());

        // `connecting` (pre-first-collection) serializes schema as null.
        let json = serde_json::to_value(DbSnapshot::connecting()).expect("serialize");
        assert!(json["schema"].is_null());
    }

    #[test]
    fn mock_snapshot_carries_statements() {
        let snapshot = DbSnapshot::mock();
        let statements = snapshot.statements.as_ref().expect("mock statements");
        assert!(statements.statements.len() >= 8, "several statements");
        assert!(matches!(statements.status, StatementsStatus::Ok));
        assert!(statements.collected_at_epoch_ms > 0);
        // A mix of SELECT/UPDATE/INSERT is present.
        for verb in ["SELECT", "UPDATE", "INSERT"] {
            assert!(
                statements.statements.iter().any(|s| s.query.starts_with(verb)),
                "mock must carry a {verb} statement"
            );
        }
        // The Hit% zero-division path is exercisable from --mock alone.
        assert!(
            statements
                .statements
                .iter()
                .any(|s| s.shared_blks_hit + s.shared_blks_read == 0),
            "one row with zero shared blocks"
        );
        // mean is consistent with total/calls.
        for s in &statements.statements {
            assert!(s.calls > 0);
            assert!((s.mean_exec_ms - s.total_exec_ms / s.calls as f64).abs() < 1e-9);
        }
    }

    #[test]
    fn statements_serialize_with_queryid_as_string() {
        let snapshot = DbSnapshot::mock();
        let json = serde_json::to_value(&snapshot).expect("serialize");
        let statements = json["statements"]["statements"]
            .as_array()
            .expect("statements array");
        assert!(!statements.is_empty());
        // The hard rule: query_id crosses the JSON boundary as a STRING —
        // the raw int8 can exceed JS Number.MAX_SAFE_INTEGER.
        for s in statements {
            assert!(
                s["query_id"].is_string(),
                "query_id must be a JSON string: {s}"
            );
        }
        assert_eq!(json["statements"]["status"], serde_json::json!("Ok"));

        // `connecting` (pre-first-collection) serializes statements as null.
        let json = serde_json::to_value(DbSnapshot::connecting()).expect("serialize");
        assert!(json["statements"].is_null());
    }

    #[test]
    fn statements_unavailable_status_serializes_with_its_reason() {
        let snapshot = StatementsSnapshot {
            collected_at_epoch_ms: 1,
            statements: Vec::new(),
            status: StatementsStatus::Unavailable(
                "pg_stat_statements is not installed".to_string(),
            ),
        };
        let json = serde_json::to_value(&snapshot).expect("serialize");
        assert_eq!(
            json["status"]["Unavailable"],
            serde_json::json!("pg_stat_statements is not installed")
        );
        assert!(json["statements"].as_array().is_some_and(Vec::is_empty));

        let err = StatementsSnapshot {
            collected_at_epoch_ms: 1,
            statements: Vec::new(),
            status: StatementsStatus::Error("permission denied".to_string()),
        };
        let json = serde_json::to_value(&err).expect("serialize");
        assert_eq!(
            json["status"]["Error"],
            serde_json::json!("permission denied")
        );
    }

    #[test]
    fn admin_action_result_serializes_inside_the_envelope() {
        // No action yet: the field crosses as JSON null (web renders nothing).
        let json = serde_json::to_value(DbSnapshot::mock()).expect("serialize");
        assert!(json["last_admin_action"].is_null());

        // Every outcome shape is JSON-representable.
        let mut snapshot = DbSnapshot::mock();
        snapshot.last_admin_action = Some(AdminActionResult {
            kind: AdminKind::Cancel,
            pid: 4977,
            outcome: AdminOutcome::Signalled(true),
            at_epoch_ms: 1_752_000_000_000,
        });
        let json = serde_json::to_value(&snapshot).expect("serialize");
        let action = &json["last_admin_action"];
        assert_eq!(action["kind"], serde_json::json!("Cancel"));
        assert_eq!(action["pid"], serde_json::json!(4977));
        assert_eq!(action["outcome"]["Signalled"], serde_json::json!(true));
        assert_eq!(action["at_epoch_ms"], serde_json::json!(1_752_000_000_000u64));

        let err = AdminActionResult {
            kind: AdminKind::Terminate,
            pid: 1,
            outcome: AdminOutcome::Error("permission denied".to_string()),
            at_epoch_ms: 1,
        };
        let json = serde_json::to_value(&err).expect("serialize");
        assert_eq!(json["outcome"]["Error"], serde_json::json!("permission denied"));
    }

    #[test]
    fn admin_command_exposes_pid_and_kind() {
        assert_eq!(AdminCommand::CancelBackend(42).pid(), 42);
        assert_eq!(AdminCommand::TerminateBackend(43).pid(), 43);
        assert_eq!(AdminCommand::CancelBackend(1).kind(), AdminKind::Cancel);
        assert_eq!(AdminCommand::TerminateBackend(1).kind(), AdminKind::Terminate);
    }

    #[test]
    fn connecting_snapshot_serializes_to_json() {
        let snapshot = DbSnapshot::connecting();
        let json = serde_json::to_string(&snapshot).expect("snapshot must serialize");
        assert!(json.contains("\"status\":\"Connecting\""));
        assert!(matches!(snapshot.status, PollerStatus::Connecting));
    }
}
