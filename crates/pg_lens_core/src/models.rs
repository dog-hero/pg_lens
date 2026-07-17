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
    /// `EXTRACT(epoch FROM (NOW() - xact_start))` — `None` when the backend
    /// has no open transaction. Drives the idle-in-transaction / xact-age
    /// hunter (see [`crate::xact_age`]): the age itself, plus `state`,
    /// decide the severity tier.
    pub xact_age_secs: Option<f64>,
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

/// Checkpointer/bgwriter stats (F4), refreshed every fast tick (same
/// essential transaction as [`ServerVitals`] — a cheap single-row catalog
/// read, never best-effort). Normalizes the PG17 `pg_stat_bgwriter` /
/// `pg_stat_checkpointer` catalog split into one shape: the cumulative
/// counters below are what the SQL reports either way (`buffers_backend`
/// aside, `None` on 17+ — moved to `pg_stat_io`), and the derived fields are
/// computed by the poller from tick-to-tick deltas, mirroring
/// `ServerVitals::tps`.
#[derive(Clone, Debug, Serialize)]
pub struct CheckpointerStats {
    // --- cumulative counters (since server start, or the last stats reset) ---
    pub checkpoints_timed: i64,
    pub checkpoints_req: i64,
    pub checkpoint_write_time_ms: f64,
    pub checkpoint_sync_time_ms: f64,
    pub buffers_checkpoint: i64,
    pub buffers_clean: i64,
    pub maxwritten_clean: i32,
    /// `None` on PG 17+ (moved to `pg_stat_io`).
    pub buffers_backend: Option<i64>,
    pub buffers_alloc: i64,

    // --- derived per-tick rates: `None` on the first poll of a session (no
    // delta window yet — same rule as `ServerVitals::tps`) ---
    pub checkpoints_per_min_timed: Option<f64>,
    pub checkpoints_per_min_req: Option<f64>,
    pub buffers_checkpoint_per_sec: Option<f64>,
    pub buffers_clean_per_sec: Option<f64>,
    /// `None` when no delta window yet, OR when `buffers_backend` itself is
    /// absent (PG 17+).
    pub buffers_backend_per_sec: Option<f64>,
    /// Average write/sync time per checkpoint, over ticks that saw at least
    /// one new checkpoint complete; `None` when no checkpoint has completed
    /// in the delta window (the common case — checkpoints are infrequent).
    pub avg_checkpoint_write_ms: Option<f64>,
    pub avg_checkpoint_sync_ms: Option<f64>,

    /// `requested / (requested + timed)` checkpoints, computed over the
    /// **poller session window** (since this connection's first poll, not
    /// per-tick — checkpoints are rare enough that a per-tick delta would be
    /// mostly 0/0 noise). `None` until at least one checkpoint has completed
    /// since the session began. A high share means checkpoint pressure
    /// (`max_wal_size` likely too small): the Macro Lens tints this yellow
    /// when `requested > timed`, calm otherwise.
    pub requested_ratio_session: Option<f64>,
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

/// One row of `pg_database` (U2, `queries/databases.sql`): the databases
/// available on this cluster, feeding the in-session database picker (`d`).
/// PostgreSQL cannot switch databases without reconnecting, so picking a row
/// asks the poller to reconnect with a different `dbname` rather than
/// running an in-place query.
#[derive(Clone, Debug, Serialize)]
pub struct DatabaseRow {
    pub name: String,
    /// `pg_database_size(datname)`, best-effort per row: `None` when the
    /// connected role lacks CONNECT privilege on this OTHER database (the
    /// function raises for those — the SQL guards it instead of failing the
    /// whole query, see `queries/databases.sql`).
    pub size_bytes: Option<i64>,
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

/// One orphaned two-phase-commit row (v0.9, `queries/prepared_xacts.sql`):
/// a `PREPARE TRANSACTION` left dangling holds its locks and pins the
/// wraparound horizon indefinitely, with no session in `pg_stat_activity` to
/// blame (the backend that prepared it already disconnected) — the classic
/// silent incident that blocks vacuum forever. Collected on the FAST tick,
/// best-effort (see [`DbSnapshot::prepared_xacts`]); severity tiers live in
/// `crate::prepared_xacts` (mirrored by the TUI/web frontends).
#[derive(Clone, Debug, Serialize)]
pub struct PreparedXactRow {
    pub gid: String,
    pub owner: String,
    pub database: String,
    /// `EXTRACT(epoch FROM (now() - prepared))::float8`.
    pub age_seconds: f64,
}

/// Lock-table pressure gauge (v0.11, `queries/lock_capacity.sql`): current
/// `pg_locks` row count against the documented shared-memory capacity
/// formula (`max_locks_per_transaction * (max_connections +
/// max_prepared_transactions)`) — the headroom before the classic "out of
/// shared memory, you might need to increase max_locks_per_transaction"
/// outage. `capacity_slots`/`used_fraction` are derived by
/// `crate::lock_capacity::compute`, never in SQL. Collected on the FAST
/// tick, best-effort (see [`DbSnapshot::lock_capacity`]); severity tiers
/// live in `crate::lock_capacity` (mirrored by the TUI/web frontends).
#[derive(Clone, Debug, Serialize)]
pub struct LockCapacity {
    pub locks_held: i64,
    pub max_locks_per_xact: i64,
    pub max_connections: i64,
    pub max_prepared_xacts: i64,
    /// `max_locks_per_xact * (max_connections + max_prepared_xacts)`.
    pub capacity_slots: i64,
    /// `locks_held / capacity_slots`, in `0.0..=1.0` (0.0 if `capacity_slots`
    /// is somehow 0 — never a NaN/inf).
    pub used_fraction: f64,
}

/// One idle connection (v0.11, `queries/idle_sessions.sql`): a
/// `pg_stat_activity` row with `state = 'idle'` — a backend holding a slot
/// in the connection budget without doing anything, the classic
/// pool-exhaustion suspect (`connections_total` near `max_connections` but
/// few active). Ranked oldest-first by `idle_age_secs`; severity tiers live
/// in `crate::idle_sessions` (mirrored by the TUI/web frontends).
#[derive(Clone, Debug, Serialize)]
pub struct IdleSessionRow {
    pub pid: i32,
    pub application_name: String,
    pub database: String,
    pub client: String,
    pub username: String,
    /// `EXTRACT(epoch FROM (now() - state_change))::float8` — how long this
    /// backend has sat idle since its last query finished.
    pub idle_age_secs: f64,
}

/// Health of the *slow* schema collection, separate from [`PollerStatus`]:
/// a failing schema query must never taint the 2s activity pipeline.
#[derive(Clone, Debug, Serialize)]
pub enum SchemaStatus {
    Ok,
    Error(String),
}

/// The Index Advisor's (F3) verdict for one index — PRD pillar 6 ("signal,
/// not verdict"): a flag plus, for duplicates, WHICH other index makes it
/// one, so the detail panel can show the evidence rather than a bare label.
/// Computed purely in [`crate::index_advisor::classify`], never in SQL.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub enum IndexFinding {
    /// `pg_index.indisvalid = false` or `indisready = false` — a `CREATE
    /// INDEX CONCURRENTLY` was interrupted (crash, cancel) and left a dead
    /// index behind: it never serves a query, still costs every write, and
    /// `\d` does not warn about it. The strongest signal (overrides every
    /// other finding): it is both dead weight AND evidence a concurrent
    /// build needs cleanup/retry.
    Invalid,
    /// `idx_scan = 0` and the index serves no constraint (never flagged:
    /// unique/primary/exclusion indexes exist for correctness, not reads).
    Unused,
    /// Same table, identical column list/opclasses/collations/predicate/
    /// uniqueness as `partner` — interchangeable, one is pure overhead.
    DuplicateExact { partner: String },
    /// This index's column list is a strict, order-respecting prefix of
    /// `partner`'s — a weaker signal than an exact duplicate: `partner`
    /// alone could serve every query this index serves.
    DuplicatePrefix { partner: String },
    /// Nothing to report — reads happen, or it uniquely serves a query
    /// shape no other index covers.
    None,
}

/// One row of the Index Advisor query (`queries/indexes.sql`), F3: usage
/// counters + on-disk size + constraint flags of one index of the connected
/// database, plus its computed [`IndexFinding`]. The raw catalog signature
/// used to derive `finding` (indkey/indclass/indcollation/indpred) is
/// deliberately NOT part of this model — see
/// [`crate::index_advisor::IndexCatalogRow`], which never survives past
/// `index_advisor::build_index_rows`.
#[derive(Clone, Debug, Serialize)]
pub struct IndexRow {
    pub schema: String,
    pub table: String,
    pub name: String,
    /// `pg_relation_size(indexrelid)`.
    pub index_bytes: i64,
    pub idx_scan: i64,
    pub idx_tup_read: i64,
    pub idx_tup_fetch: i64,
    pub is_unique: bool,
    pub is_primary: bool,
    pub is_exclusion: bool,
    /// `pg_index.indisvalid` — false means a `CREATE INDEX CONCURRENTLY`
    /// never finished building this index; it is silently skipped by the
    /// planner.
    pub is_valid: bool,
    /// `pg_index.indisready` — false means the index is not even being
    /// maintained on writes yet (an even earlier build stage than
    /// `is_valid`).
    pub is_ready: bool,
    /// True when a `pg_constraint` row backs this index (PK/UNIQUE/EXCLUDE
    /// constraint) — distinct from `is_unique` alone, which is also true
    /// for a manually created `CREATE UNIQUE INDEX` with no constraint.
    pub is_constraint: bool,
    /// `pg_get_indexdef(indexrelid)` — the full `CREATE INDEX` statement,
    /// shown verbatim in the detail panel (never reconstructed from parts).
    pub indexdef: String,
    pub finding: IndexFinding,
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
    /// Index advisor rows (F3), collected in the same essential transaction
    /// as `tables` (the query caps at 50 rows by size).
    pub indexes: Vec<IndexRow>,
    /// When the connected database's cumulative `pg_stat_*` counters were
    /// last zeroed (F3 freshness header) — `None` only if the row vanished
    /// mid-query (the database itself would have to be dropped), never a
    /// real "unknown" state on a healthy connection.
    pub stats_reset_epoch_secs: Option<f64>,
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
        // U3: six rows (not just the three the old compact footer showed) so
        // the Vacuum sub-view's worst-tables list has enough rows to demo
        // j/k scrolling under `--mock`. `pgbench_branches` joins a real
        // `tables` row (exercising the "last (auto)vacuum" join); the last
        // two are synthetic (no `tables` partner), exercising the "no join"
        // fallback the same way a table dropped from `table_stats`'s LIMIT
        // would.
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
                schema: "public".to_string(),
                name: "pgbench_branches".to_string(),
                age_xids: 61_500_000,
                n_dead_tup: 88,
                n_live_tup: 5,
            },
            VacuumTableRow {
                schema: "public".to_string(),
                name: "pgbench_history".to_string(),
                age_xids: 58_900_000,
                n_dead_tup: 2_014,
                n_live_tup: 812_400,
            },
            VacuumTableRow {
                schema: "audit".to_string(),
                name: "login_events".to_string(),
                age_xids: 50_100_000,
                n_dead_tup: 512,
                n_live_tup: 128_000,
            },
            VacuumTableRow {
                schema: "audit".to_string(),
                name: "raw_events".to_string(),
                age_xids: 41_050_000,
                n_dead_tup: 0,
                n_live_tup: 273_000 + churn,
            },
        ];
        // F3: run the SAME `index_advisor::classify` the live poller uses,
        // over a catalog fixture that demos all three findings at once —
        // one unused index, one exact-duplicate pair, and one prefix case —
        // so `--mock` exercises the real detection code, not a hand-typed
        // `finding` field that could drift from it.
        use crate::index_advisor::{IndexCatalogRow, build_index_rows};
        let catalog_index = |table: &str, name: &str, idx_scan: i64, is_unique: bool,
                              is_primary: bool, indkey: &str, indexdef: &str| {
            IndexCatalogRow {
                schema: "public".to_string(),
                table: table.to_string(),
                name: name.to_string(),
                index_bytes: 8_388_608,
                idx_scan,
                idx_tup_read: idx_scan * 3,
                idx_tup_fetch: idx_scan * 3,
                is_unique,
                is_primary,
                is_exclusion: false,
                is_valid: true,
                is_ready: true,
                is_constraint: is_unique || is_primary,
                indexdef: indexdef.to_string(),
                indkey: indkey.to_string(),
                indclass: indkey.split_whitespace().map(|_| "1978").collect::<Vec<_>>().join(" "),
                indcollation: indkey.split_whitespace().map(|_| "0").collect::<Vec<_>>().join(" "),
                indpred: String::new(),
            }
        };
        let index_catalog = vec![
            catalog_index(
                "order_items",
                "order_items_pkey",
                9_004_112 + churn.max(0),
                true,
                true,
                "1",
                "CREATE UNIQUE INDEX order_items_pkey ON public.order_items USING btree (id)",
            ),
            // Exact-duplicate pair: same column, same uniqueness — a
            // classic "created it twice, nobody noticed" finding.
            catalog_index(
                "order_items",
                "order_items_customer_idx",
                412,
                false,
                false,
                "3",
                "CREATE INDEX order_items_customer_idx ON public.order_items USING \
                 btree (customer_id)",
            ),
            catalog_index(
                "order_items",
                "order_items_customer_idx2",
                97,
                false,
                false,
                "3",
                "CREATE INDEX order_items_customer_idx2 ON public.order_items USING \
                 btree (customer_id)",
            ),
            // Unused: never scanned since the last stats reset, not backing
            // any constraint — pure write overhead.
            catalog_index(
                "order_items",
                "order_items_notes_idx",
                0,
                false,
                false,
                "6",
                "CREATE INDEX order_items_notes_idx ON public.order_items USING btree (notes)",
            ),
            catalog_index(
                "pgbench_accounts",
                "pgbench_accounts_pkey",
                48_211_390 + churn,
                true,
                true,
                "1",
                "CREATE UNIQUE INDEX pgbench_accounts_pkey ON public.pgbench_accounts USING \
                 btree (aid)",
            ),
            // Prefix-redundant: (bid) is a strict prefix of (bid, abalance)
            // — a weaker, dim-yellow signal.
            catalog_index(
                "pgbench_accounts",
                "pgbench_accounts_bid_idx",
                58,
                false,
                false,
                "2",
                "CREATE INDEX pgbench_accounts_bid_idx ON public.pgbench_accounts USING \
                 btree (bid)",
            ),
            catalog_index(
                "pgbench_accounts",
                "pgbench_accounts_bid_abalance_idx",
                7_310 + churn.max(0),
                false,
                false,
                "2 4",
                "CREATE INDEX pgbench_accounts_bid_abalance_idx ON public.pgbench_accounts \
                 USING btree (bid, abalance)",
            ),
            // Invalid: `indisvalid = false` — a `CREATE INDEX CONCURRENTLY`
            // that never finished (crash, cancel). Zero scans (it can never
            // be planned into a query) AND unrelated to any other index
            // here, so it demos the Invalid category in isolation.
            IndexCatalogRow {
                schema: "public".to_string(),
                table: "order_items".to_string(),
                name: "order_items_shipped_at_idx".to_string(),
                index_bytes: 3_145_728,
                idx_scan: 0,
                idx_tup_read: 0,
                idx_tup_fetch: 0,
                is_unique: false,
                is_primary: false,
                is_exclusion: false,
                is_valid: false,
                is_ready: true,
                is_constraint: false,
                indexdef: "CREATE INDEX order_items_shipped_at_idx ON public.order_items \
                           USING btree (shipped_at)"
                    .to_string(),
                indkey: "9".to_string(),
                indclass: "1978".to_string(),
                indcollation: "0".to_string(),
                indpred: String::new(),
            },
        ];
        let indexes = build_index_rows(index_catalog);
        Self {
            collected_at_epoch_ms: epoch_ms_now(),
            tables,
            table_bloat,
            index_bloat,
            vacuum_cluster_age,
            vacuum_tables,
            indexes,
            // A plausible "reset a couple weeks ago" freshness so the
            // header's "stats reset Nd ago" reads naturally in `--mock`.
            stats_reset_epoch_secs: Some(epoch_ms_now() as f64 / 1000.0 - 12.0 * 86_400.0),
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

/// One row of `pg_replication_slots` (F2.5, `queries/replication_slots.sql`).
/// Unlike [`WalSenderRow`]/[`WalReceiverRow`], slots exist on BOTH a primary
/// and a standby, so they are collected regardless of `is_in_recovery` and
/// carried on [`DbSnapshot`] alongside — not inside — [`ReplicationInfo`].
///
/// The point of the feature: an INACTIVE slot that keeps retaining WAL is
/// the classic full-disk incident (nothing is consuming it, so WAL piles up
/// in `pg_wal`). See `pg_lens_tui::ui::macro_lens` (and its web mirror) for
/// the severity rule.
#[derive(Clone, Debug, Serialize)]
pub struct ReplicationSlotRow {
    pub slot_name: String,
    /// `"physical"` or `"logical"`.
    pub slot_type: String,
    pub active: bool,
    /// `pg_wal_lsn_diff(pg_current_wal_lsn(), restart_lsn)` — `None` during
    /// recovery (the CASE guard in the SQL short-circuits before the
    /// recovery-only-erroring `pg_current_wal_lsn()` runs) or when
    /// `restart_lsn` itself is NULL (a logical slot never yet used).
    pub retained_wal_bytes: Option<i64>,
    /// `reserved` / `extended` / `unreserved` / `lost` (PG 13+).
    pub wal_status: Option<String>,
    /// Bytes of `max_slot_wal_keep_size` headroom still available before
    /// this slot's WAL is at risk of being removed (PG 13+); `None` when
    /// `max_slot_wal_keep_size` is unlimited (the default) or not
    /// applicable to this slot.
    pub safe_wal_size: Option<i64>,
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
    /// `pg_replication_slots` rows (F2.5), refreshed every fast tick,
    /// best-effort like `replication`: `None` when the collection failed
    /// this tick, `Some(vec![])` when it succeeded and simply found no
    /// slots (the common case on a server with no logical replication /
    /// standbys using slots — rendered as no extra rows, never an error).
    /// A sibling of `replication` rather than a field of `ReplicationInfo`
    /// because slots exist on BOTH a primary and a standby, unlike the
    /// role-specific senders/receiver.
    pub replication_slots: Option<Vec<ReplicationSlotRow>>,
    /// In-flight `pg_stat_progress_vacuum` rows (F2), refreshed every fast
    /// tick, best-effort like `replication`: `None` when the collection
    /// failed this tick (restricted role, hidden view, ...), `Some(vec![])`
    /// when it succeeded and simply found no vacuum running (the common
    /// case — rendered as a calm "no vacuum running", never an error).
    pub vacuum_progress: Option<Vec<VacuumProgressRow>>,
    /// Checkpointer/bgwriter stats (F4), refreshed every fast tick in the
    /// same essential transaction as `vitals` — NOT best-effort (readable by
    /// everyone, same class as `pg_stat_database`). `None` only before the
    /// first successful poll of a session.
    pub checkpointer: Option<CheckpointerStats>,
    /// Databases available on the cluster (U2), refreshed every fast tick,
    /// best-effort like `replication`: `None` when the collection failed
    /// this tick, `Some(vec![])` only if the cluster genuinely has none
    /// connectable (never happens in practice — at least the connected
    /// database always qualifies), never an error.
    pub databases: Option<Vec<DatabaseRow>>,
    /// Orphaned two-phase-commit watch (v0.9, `pg_prepared_xacts`), refreshed
    /// every fast tick, best-effort like `databases`: `None` when the
    /// collection failed this tick, `Some(vec![])` when it succeeded and
    /// simply found no dangling prepared transaction (the overwhelmingly
    /// common case — rendered calmly, never as an error).
    pub prepared_xacts: Option<Vec<PreparedXactRow>>,
    /// Lock-table pressure gauge (v0.11, `pg_locks` vs. capacity), refreshed
    /// every fast tick, best-effort like `databases`/`prepared_xacts`: `None`
    /// when the collection failed this tick (restricted role, a renamed
    /// GUC, ...), otherwise always present — every cluster has a lock table,
    /// so there is no "found nothing" empty case like the other best-effort
    /// sources.
    pub lock_capacity: Option<LockCapacity>,
    /// Idle connection / connection-age census (v0.11, `pg_stat_activity`
    /// `state = 'idle'`), refreshed every fast tick, best-effort like
    /// `databases`/`prepared_xacts`: `None` when the collection failed this
    /// tick, `Some(vec![])` when it succeeded and simply found no idle
    /// sessions (a fully busy or freshly-started server — calm, never an
    /// error). Oldest (most suspect) first, capped at
    /// `queries::IDLE_SESSIONS_LIMIT` rows.
    pub idle_sessions: Option<Vec<IdleSessionRow>>,
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
                // Implicit single-statement transaction: xact age tracks
                // the query duration.
                xact_age_secs: Some(0.02 + jitter(seq, 7, 80) as f64 / 1_000.0),
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
                xact_age_secs: Some(12.7 + age),
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
                xact_age_secs: Some(384.2 + age),
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
                xact_age_secs: Some(384.2 + age),
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
                // U3.9: deliberately the oldest open transaction in the
                // mock set AND idle-in-transaction — the exact story the
                // xact-age hunter's headline exists to catch. Comfortably
                // past `xact_age::IDLE_IN_XACT_AGE_BAD_SECS` (900s) so
                // `--mock` opens on a visible red marker, not a calm one.
                xact_age_secs: Some(2_450.0 + age),
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
                xact_age_secs: Some(88.4 + age),
                wait_event: None,
                username: "postgres".to_string(),
                state: "active".to_string(),
                query: "autovacuum: VACUUM ANALYZE public.order_items".to_string(),
                query_leader_pid: 4650,
                is_parallel_worker: false,
                query_id: None,
            },
            // v0.9: third link of the blocking chain — waits on pid 4977,
            // which is itself waiting on pid 4312. See `locks` below: this
            // is what makes `--mock`/the PTY e2e exercise a real 3-level
            // wait-for chain (A blocks B blocks C), not just a single pair.
            ActivityRow {
                pid: 5104,
                application_name: "checkout-worker".to_string(),
                database: "bench".to_string(),
                client: "10.0.4.51".to_string(),
                duration_secs: 6.1 + age,
                xact_age_secs: Some(6.1 + age),
                wait_event: Some("Lock:tuple".to_string()),
                username: "bench".to_string(),
                state: "active".to_string(),
                query: "UPDATE pgbench_branches SET bbalance = bbalance - $1 WHERE bid = $2"
                    .to_string(),
                query_leader_pid: 5104,
                is_parallel_worker: false,
                query_id: Some(3_004_918_872_215_881_003),
            },
        ];

        // Matches the story above: pid 4977 waits on a transactionid lock
        // held by the idle-in-transaction psql session (pid 4312), and pid
        // 5104 in turn waits on a row 4977 has touched — a 3-level chain
        // (5104 -> 4977 -> 4312) so the Blocking Chain feature has real
        // multi-hop data to render under `--mock`.
        let locks = vec![
            LockRow {
                pid: 4977,
                blocked_by: vec![4312],
                mode: Some("ShareLock".to_string()),
                locktype: Some("transactionid".to_string()),
                relation: None,
                duration_secs: 12.7 + age,
                query: "UPDATE pgbench_branches SET bbalance = bbalance + $1 WHERE bid = $2"
                    .to_string(),
            },
            LockRow {
                pid: 5104,
                blocked_by: vec![4977],
                mode: Some("RowExclusiveLock".to_string()),
                locktype: Some("tuple".to_string()),
                relation: Some("pgbench_branches".to_string()),
                duration_secs: 6.1 + age,
                query: "UPDATE pgbench_branches SET bbalance = bbalance - $1 WHERE bid = $2"
                    .to_string(),
            },
        ];

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
            // F2.5: two slots so `--mock` demos both severity tiers — one
            // healthy (active physical replica, fully reserved, nothing to
            // worry about) and one INACTIVE logical slot retaining a couple
            // GB of WAL with wal_status "extended": the classic
            // slowly-filling-disk warning sign, yellow in the panel.
            replication_slots: Some(vec![
                ReplicationSlotRow {
                    slot_name: "replica_1_slot".to_string(),
                    slot_type: "physical".to_string(),
                    active: true,
                    retained_wal_bytes: Some(4 * 1024 * 1024 + jitter(seq, 11, 512 * 1024) as i64),
                    wal_status: Some("reserved".to_string()),
                    safe_wal_size: None,
                },
                ReplicationSlotRow {
                    slot_name: "analytics_cdc".to_string(),
                    slot_type: "logical".to_string(),
                    active: false,
                    retained_wal_bytes: Some(2_600_000_000 + (seq as i64) * 1_048_576),
                    wal_status: Some("extended".to_string()),
                    safe_wal_size: Some(1_400_000_000),
                },
            ]),
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
            // F4: healthy checkpointer — requested share stays well under
            // timed (calm session ratio), everything else jittered so
            // `--mock` visibly moves.
            checkpointer: Some(CheckpointerStats {
                checkpoints_timed: 812 + (seq as i64) / 3,
                checkpoints_req: 46 + jitter(seq, 20, 6) as i64,
                checkpoint_write_time_ms: 245_000.0 + jitter(seq, 21, 5_000) as f64,
                checkpoint_sync_time_ms: 18_400.0 + jitter(seq, 22, 800) as f64,
                buffers_checkpoint: 1_240_000 + (seq as i64) * 37,
                buffers_clean: 88_500 + (seq as i64) * 4,
                maxwritten_clean: 12 + jitter(seq, 23, 3) as i32,
                buffers_backend: Some(305_000 + (seq as i64) * 9),
                buffers_alloc: 5_600_000 + (seq as i64) * 120,
                checkpoints_per_min_timed: Some(0.31 + jitter(seq, 24, 20) as f64 / 1_000.0),
                checkpoints_per_min_req: Some(0.02 + jitter(seq, 25, 10) as f64 / 1_000.0),
                buffers_checkpoint_per_sec: Some(210.0 + jitter(seq, 26, 40) as f64),
                buffers_clean_per_sec: Some(35.0 + jitter(seq, 27, 15) as f64),
                buffers_backend_per_sec: Some(58.0 + jitter(seq, 28, 20) as f64),
                avg_checkpoint_write_ms: Some(4_200.0 + jitter(seq, 29, 600) as f64),
                avg_checkpoint_sync_ms: Some(310.0 + jitter(seq, 30, 80) as f64),
                requested_ratio_session: Some(0.13 + jitter(seq, 31, 10) as f64 / 100.0),
            }),
            // U2: a small cluster with the connected database ("shop", per
            // `vitals.database` above) plus two others — one whose size the
            // connected role can read, one it cannot (the `--` path), so
            // `--mock` demos the picker's current-db marker and the
            // best-effort size dash in one fixture.
            databases: Some(vec![
                DatabaseRow {
                    name: "shop".to_string(),
                    size_bytes: Some(3_400_000_000 + (seq as i64) * 1_048_576),
                },
                DatabaseRow {
                    name: "warehouse".to_string(),
                    size_bytes: Some(48_600_000_000),
                },
                DatabaseRow {
                    name: "analytics".to_string(),
                    size_bytes: None,
                },
            ]),
            // v0.9: one dangling 2PC transaction, comfortably past
            // `prepared_xacts::BAD_AGE_SECS` (1h) — a classic orphaned
            // incident, left there from a client that PREPAREd and vanished,
            // so `--mock` opens on a visible red row in the Vacuum sub-view.
            prepared_xacts: Some(vec![PreparedXactRow {
                gid: "payment_batch_2026_07_14".to_string(),
                owner: "app_rw".to_string(),
                database: "shop".to_string(),
                age_seconds: 5_412.0 + age,
            }]),
            // v0.11: max_locks_per_transaction=64, max_connections=100,
            // max_prepared_transactions=0 => 6400 slots capacity; held
            // count jitters around ~92% so `--mock` opens on a visible red
            // gauge (past `lock_capacity::BAD_FRACTION`) — a batch job or a
            // burst of long transactions eating the lock table, exactly the
            // precursor this gauge exists to surface before the "out of
            // shared memory" outage.
            lock_capacity: Some(crate::lock_capacity::compute(crate::db::LockCapacityRow {
                locks_held: 5_850 + jitter(seq, 32, 200) as i64,
                max_locks_per_xact: 64,
                max_connections: 100,
                max_prepared_xacts: 0,
            })),
            // v0.11: a handful of idle connections at varying ages — the
            // pool-exhaustion suspects. Oldest-first, matching the SQL's own
            // ORDER BY, including one comfortably past
            // `idle_sessions::BAD_AGE_SECS` (4h) at ~4h12m so `--mock` opens
            // on a visible red row without waiting for a live incident.
            idle_sessions: Some(vec![
                IdleSessionRow {
                    pid: 6104,
                    application_name: "reporting-pool".to_string(),
                    database: "warehouse".to_string(),
                    client: "10.0.7.9".to_string(),
                    username: "analytics_ro".to_string(),
                    idle_age_secs: 15_120.0 + age,
                },
                IdleSessionRow {
                    pid: 6205,
                    application_name: "checkout-api".to_string(),
                    database: "shop".to_string(),
                    client: "10.0.4.14".to_string(),
                    username: "app_rw".to_string(),
                    idle_age_secs: 5_402.0 + age,
                },
                IdleSessionRow {
                    pid: 6301,
                    application_name: "checkout-api".to_string(),
                    database: "shop".to_string(),
                    client: "10.0.4.15".to_string(),
                    username: "app_rw".to_string(),
                    idle_age_secs: 612.0 + age,
                },
                IdleSessionRow {
                    pid: 6402,
                    application_name: "psql".to_string(),
                    database: "shop".to_string(),
                    client: "local".to_string(),
                    username: "leonardo".to_string(),
                    idle_age_secs: 42.0 + jitter(seq, 33, 30) as f64,
                },
            ]),
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
            replication_slots: None,
            vacuum_progress: None,
            checkpointer: None,
            databases: None,
            prepared_xacts: None,
            lock_capacity: None,
            idle_sessions: None,
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

    /// F2.5: the mock carries exactly the two slots the spec asks for — one
    /// calm active/reserved physical slot, one inactive logical slot
    /// retaining several GB with wal_status "extended" (severity fixture
    /// for the TUI/web panel tests).
    #[test]
    fn mock_snapshot_carries_replication_slots() {
        let snapshot = DbSnapshot::mock();
        let slots = snapshot
            .replication_slots
            .as_ref()
            .expect("mock must carry replication slots");
        assert_eq!(slots.len(), 2);
        let active = slots.iter().find(|s| s.active).expect("one active slot");
        assert_eq!(active.slot_type, "physical");
        assert_eq!(active.wal_status.as_deref(), Some("reserved"));
        let inactive = slots.iter().find(|s| !s.active).expect("one inactive slot");
        assert_eq!(inactive.slot_type, "logical");
        assert_eq!(inactive.wal_status.as_deref(), Some("extended"));
        assert!(
            inactive.retained_wal_bytes.unwrap_or(0) > 2_000_000_000,
            "the inactive slot must retain a couple GB, per the spec"
        );
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

    /// U2: the mock carries the connected database plus at least one other,
    /// and demonstrates the per-row best-effort size (one `None`).
    #[test]
    fn mock_snapshot_carries_databases_including_the_current_one() {
        let snapshot = DbSnapshot::mock();
        let databases = snapshot.databases.as_ref().expect("mock must carry databases");
        assert!(databases.len() >= 2);
        assert!(
            databases.iter().any(|d| d.name == snapshot.vitals.database),
            "the connected database must be among the choices"
        );
        assert!(
            databases.iter().any(|d| d.size_bytes.is_none()),
            "one row must demo the best-effort size dash"
        );
    }

    /// v0.9: the mock carries one dangling prepared transaction, old enough
    /// to land in the red tier — the Vacuum sub-view has real data to show
    /// under `--mock` without waiting for a live incident.
    #[test]
    fn mock_snapshot_carries_a_prepared_xact() {
        let snapshot = DbSnapshot::mock();
        let rows = snapshot
            .prepared_xacts
            .as_ref()
            .expect("mock must carry prepared_xacts");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert!(!row.gid.is_empty());
        assert!(!row.owner.is_empty());
        assert_eq!(row.database, snapshot.vitals.database);
        assert!(
            row.age_seconds > crate::prepared_xacts::BAD_AGE_SECS,
            "mock row must land in the red tier, got {}",
            row.age_seconds
        );
    }

    /// v0.11: the mock carries several idle sessions, oldest-first, at
    /// least one comfortably past the red tier — the census has real data
    /// to show under `--mock` without waiting for a live incident.
    #[test]
    fn mock_snapshot_carries_idle_sessions() {
        let snapshot = DbSnapshot::mock();
        let rows = snapshot
            .idle_sessions
            .as_ref()
            .expect("mock must carry idle_sessions");
        assert!(rows.len() >= 3);
        for pair in rows.windows(2) {
            assert!(
                pair[0].idle_age_secs >= pair[1].idle_age_secs,
                "idle_sessions must be oldest-first, like the SQL's ORDER BY"
            );
        }
        assert!(
            rows.iter().any(|r| r.idle_age_secs > crate::idle_sessions::BAD_AGE_SECS),
            "mock must carry at least one row in the red tier"
        );
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
