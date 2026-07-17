//! Versioned SQL, embedded at compile time and selected by
//! `server_version_num` (pg_activity's `post_NNNNNN` convention).

/// The statements one poller session prepares (once). The first three run
/// per fast tick; `table_stats` + the two estimated-bloat queries run only
/// on the slow schema cadence; the two admin statements run only when a
/// frontend sends an [`crate::models::AdminCommand`].
#[derive(Clone, Copy, Debug)]
pub struct QuerySet {
    pub activity: &'static str,
    pub blocking: &'static str,
    pub server_info: &'static str,
    pub table_stats: &'static str,
    pub bloat_tables: &'static str,
    pub bloat_indexes: &'static str,
    /// Query Lens (pg_stat_statements). Only PREPARED when the extension is
    /// installed at >= 1.8 (see `db::statements_availability`) — the column
    /// set follows the EXTENSION version, not the server version. Runs on
    /// the slow schema cadence, never the fast tick.
    pub statements: &'static str,
    pub cancel_backend: &'static str,
    pub terminate_backend: &'static str,
    /// Streaming replicas of a primary (`pg_stat_replication`). Runs per fast
    /// tick; a few rows, cheap. Empty on a standby.
    pub replication: &'static str,
    /// WAL receiver of a standby (`pg_stat_wal_receiver`). Runs per fast tick.
    /// Empty on a primary.
    pub wal_receiver: &'static str,
    /// Cluster-wide XID wraparound distance (`pg_database.datfrozenxid`
    /// age), F2. Runs on the slow schema cadence, same essential
    /// transaction as `table_stats`.
    pub vacuum_cluster_age: &'static str,
    /// Per-table XID age + dead-tuple ratio ("vacuum debt"), F2. Same
    /// cadence/transaction as `vacuum_cluster_age`.
    pub vacuum_table_ages: &'static str,
    /// In-flight `pg_stat_progress_vacuum`, F2. Runs on the fast tick,
    /// best-effort (absent on any failure, like `replication`).
    pub vacuum_progress: &'static str,
    /// `pg_replication_slots`, F2.5. Runs on the fast tick, best-effort like
    /// `replication` — but unlike senders/receiver, slots exist on BOTH a
    /// primary and a standby, so this query always runs regardless of role.
    pub replication_slots: &'static str,
    /// Index advisor (F3): per-index usage + catalog signature for
    /// unused/duplicate detection (`index_advisor::classify`). Same slow
    /// cadence/transaction as `table_stats` (fail-together, like the F2
    /// vacuum-age queries).
    pub indexes: &'static str,
    /// Freshness of the connected database's cumulative stats (F3 header
    /// caveat). Same transaction as `indexes`.
    pub db_stats_reset: &'static str,
    /// Checkpointer/bgwriter counters (F4): `pg_stat_bgwriter` on 13-16,
    /// `pg_stat_checkpointer` + `pg_stat_bgwriter` joined on 17+ (columns
    /// aliased to the same names either way — see
    /// `bgwriter_post_130000.sql`/`bgwriter_post_170000.sql`). Cheap
    /// single-row catalog read, runs in the same essential transaction as
    /// `server_info` on the fast tick.
    pub bgwriter: &'static str,
    /// Databases available on the cluster (U2, the database picker). Runs
    /// on the fast tick, best-effort like `replication_slots` — a cheap
    /// catalog read, but never allowed to fail the poll.
    pub databases: &'static str,
    /// Orphaned two-phase-commit watch (v0.9, `pg_prepared_xacts`). Runs on
    /// the fast tick, best-effort like `databases` — a tiny system-view
    /// read, but never allowed to fail the poll.
    pub prepared_xacts: &'static str,
    /// Lock-table pressure gauge (v0.11): `pg_locks` count vs. the
    /// `max_locks_per_transaction * (max_connections +
    /// max_prepared_transactions)` capacity formula. Runs on the fast tick,
    /// best-effort like `databases`/`prepared_xacts` — a cheap single-row
    /// aggregate + scalar settings read, never allowed to fail the poll.
    pub lock_capacity: &'static str,
    /// Idle connection / connection-age census (v0.11): `pg_stat_activity`
    /// rows with `state = 'idle'`, oldest `state_change` first, capped at
    /// `IDLE_SESSIONS_LIMIT` rows — a separate query from `activity` (Option
    /// B, see `queries/idle_sessions.sql`'s header) so the hot activity
    /// payload/consumers stay untouched. Runs on the fast tick, best-effort
    /// like `databases`/`prepared_xacts`.
    pub idle_sessions: &'static str,
}

/// Row cap of the table-stats query (top N tables by total size). Kept as a
/// const so the SQL and any future flag stay in sync (asserted by a test).
pub const TABLE_STATS_LIMIT: usize = 200;

/// Row cap of the statements query (top N by total execution time).
pub const STATEMENTS_LIMIT: usize = 100;

/// Row cap of the vacuum per-table ages query (worst N by XID age).
pub const VACUUM_TABLES_LIMIT: usize = 20;

/// Row cap of the index-advisor query (worst N indexes by size).
pub const INDEXES_LIMIT: usize = 50;

/// Row cap of the idle-sessions census query (oldest N idle connections).
pub const IDLE_SESSIONS_LIMIT: usize = 100;

const ACTIVITY_POST_140000: &str = include_str!("../queries/activity_post_140000.sql");
const ACTIVITY_POST_130000: &str = include_str!("../queries/activity_post_130000.sql");
// Uses only 9.6+ features (pg_blocking_pids), so it serves PG 13 too.
const BLOCKING_POST_140000: &str = include_str!("../queries/blocking_post_140000.sql");
const SERVER_INFO_POST_130000: &str = include_str!("../queries/server_info_post_130000.sql");
// n_ins_since_vacuum is PG 13+, matching pg_lens's version floor.
const TABLE_STATS_POST_130000: &str = include_str!("../queries/table_stats_post_130000.sql");
// Estimated bloat, adapted from ioguix/pgsql-bloat-estimation
// (BSD-2-Clause — attribution kept in the SQL headers). The originals are
// 9.0/8.2-compatible, so one file serves the whole 13+ range (verified live
// on 13 and 16 in the Fase S2 run) — no post_NNNNNN variants needed.
const BLOAT_TABLES: &str = include_str!("../queries/bloat_tables.sql");
const BLOAT_INDEXES: &str = include_str!("../queries/bloat_indexes.sql");
// pg_stat_statements top statements. One SERVER-version-independent file
// serves 13+ because the lens requires EXTENSION >= 1.8 (the
// `total_exec_time` schema, shipped with PG 13) and refuses older
// extensions at detection time instead of carrying a pre-1.8 (`total_time`)
// variant. `QuerySet.statements` always holds this base (1.8) tier; the
// v0.14 I/O/temp-spill columns widen as the EXTENSION version climbs, a
// SEPARATE decision made at session-init time by
// `statements_sql_for_extension` (see statements.sql's header).
const STATEMENTS: &str = include_str!("../queries/statements.sql");
const STATEMENTS_EXT_1_9: &str = include_str!("../queries/statements_ext_1_9.sql");
const STATEMENTS_EXT_1_11: &str = include_str!("../queries/statements_ext_1_11.sql");
// Admin actions, adapted from dalibo/pg_activity's do_pg_cancel_backend.sql /
// do_pg_terminate_backend.sql. Version-independent (both functions predate
// PG 13), so one file each serves the whole supported range.
const DO_CANCEL_BACKEND: &str = include_str!("../queries/do_cancel_backend.sql");
const DO_TERMINATE_BACKEND: &str = include_str!("../queries/do_terminate_backend.sql");
// Replication (primary + standby sides). Adapted from dalibo/pg_activity;
// version-independent 10+ (replay_lag / pg_stat_wal_receiver), so one file
// each serves the whole supported range.
const REPLICATION: &str = include_str!("../queries/replication.sql");
const WAL_RECEIVER: &str = include_str!("../queries/wal_receiver.sql");
// Vacuum health / XID wraparound (F2). All version-independent 13+ (plain
// catalog + pg_stat_progress_vacuum, present since PG 9.6/13's stable
// shape), so one file each serves the whole supported range.
const VACUUM_CLUSTER_AGE: &str = include_str!("../queries/vacuum_cluster_age.sql");
const VACUUM_TABLE_AGES: &str = include_str!("../queries/vacuum_table_ages.sql");
const VACUUM_PROGRESS: &str = include_str!("../queries/vacuum_progress.sql");
// Replication slots (F2.5). Version-independent 13+ (wal_status /
// safe_wal_size shipped in PG 13), so one file serves the whole supported
// range — no post_NNNNNN variant needed.
const REPLICATION_SLOTS: &str = include_str!("../queries/replication_slots.sql");
// Index advisor (F3). Version-independent 13+ (pg_stat_user_indexes /
// pg_index / pg_constraint are stable across the whole supported range).
const INDEXES: &str = include_str!("../queries/indexes.sql");
const DB_STATS_RESET: &str = include_str!("../queries/db_stats_reset.sql");
// Checkpointer/bgwriter (F4). Split at the PG17 catalog reshuffle
// (pg_stat_checkpointer split out of pg_stat_bgwriter) — the two files alias
// their columns to the same names, so one parser serves both.
const BGWRITER_POST_130000: &str = include_str!("../queries/bgwriter_post_130000.sql");
const BGWRITER_POST_170000: &str = include_str!("../queries/bgwriter_post_170000.sql");
// Databases (U2). Version-independent 13+ (pg_database/has_database_privilege
// are stable across the whole supported range) — no post_NNNNNN variant.
const DATABASES: &str = include_str!("../queries/databases.sql");
// Orphaned 2PC watch (v0.9). pg_prepared_xacts is stable across the whole
// supported range — no post_NNNNNN variant needed.
const PREPARED_XACTS: &str = include_str!("../queries/prepared_xacts.sql");
// Lock-table pressure gauge (v0.11). pg_locks + current_setting() are
// stable across the whole supported range — no post_NNNNNN variant needed.
const LOCK_CAPACITY: &str = include_str!("../queries/lock_capacity.sql");
// Idle connection / connection-age census (v0.11). state_change is stable
// across the whole supported range — no post_NNNNNN variant needed.
const IDLE_SESSIONS: &str = include_str!("../queries/idle_sessions.sql");

/// Picks the SQL variants for a server version (`server_version_num` format,
/// e.g. `160003`). Below PG 13 there is no `leader_pid`, so pg_lens refuses.
pub fn for_version(server_version_num: i32) -> Result<QuerySet, String> {
    let bgwriter = if server_version_num >= 170_000 {
        BGWRITER_POST_170000
    } else {
        BGWRITER_POST_130000
    };
    if server_version_num >= 140_000 {
        Ok(QuerySet {
            activity: ACTIVITY_POST_140000,
            blocking: BLOCKING_POST_140000,
            server_info: SERVER_INFO_POST_130000,
            table_stats: TABLE_STATS_POST_130000,
            bloat_tables: BLOAT_TABLES,
            bloat_indexes: BLOAT_INDEXES,
            statements: STATEMENTS,
            cancel_backend: DO_CANCEL_BACKEND,
            terminate_backend: DO_TERMINATE_BACKEND,
            replication: REPLICATION,
            wal_receiver: WAL_RECEIVER,
            vacuum_cluster_age: VACUUM_CLUSTER_AGE,
            vacuum_table_ages: VACUUM_TABLE_AGES,
            vacuum_progress: VACUUM_PROGRESS,
            replication_slots: REPLICATION_SLOTS,
            indexes: INDEXES,
            db_stats_reset: DB_STATS_RESET,
            bgwriter,
            databases: DATABASES,
            prepared_xacts: PREPARED_XACTS,
            lock_capacity: LOCK_CAPACITY,
            idle_sessions: IDLE_SESSIONS,
        })
    } else if server_version_num >= 130_000 {
        Ok(QuerySet {
            activity: ACTIVITY_POST_130000,
            blocking: BLOCKING_POST_140000,
            server_info: SERVER_INFO_POST_130000,
            table_stats: TABLE_STATS_POST_130000,
            bloat_tables: BLOAT_TABLES,
            bloat_indexes: BLOAT_INDEXES,
            statements: STATEMENTS,
            cancel_backend: DO_CANCEL_BACKEND,
            terminate_backend: DO_TERMINATE_BACKEND,
            replication: REPLICATION,
            wal_receiver: WAL_RECEIVER,
            vacuum_cluster_age: VACUUM_CLUSTER_AGE,
            vacuum_table_ages: VACUUM_TABLE_AGES,
            vacuum_progress: VACUUM_PROGRESS,
            replication_slots: REPLICATION_SLOTS,
            indexes: INDEXES,
            db_stats_reset: DB_STATS_RESET,
            bgwriter,
            databases: DATABASES,
            prepared_xacts: PREPARED_XACTS,
            lock_capacity: LOCK_CAPACITY,
            idle_sessions: IDLE_SESSIONS,
        })
    } else {
        Err(format!(
            "unsupported PostgreSQL version (server_version_num={server_version_num}): \
             pg_lens requires PostgreSQL 13+"
        ))
    }
}

/// Picks the Query Lens SQL variant for a PARSED `pg_stat_statements`
/// EXTENSION version (major, minor) — v0.14's I/O & temp-spill profile.
/// Independent of [`for_version`]'s server-version selection: these column
/// renames/additions track the extension, not the server (see
/// `queries/statements.sql`'s header for the full tier rundown). Only
/// called once the caller already knows the extension clears the >= 1.8
/// [`crate::db::statements_availability`] floor — below that this function
/// is never reached (the Query Lens is `Unavailable` instead).
pub fn statements_sql_for_extension(major: u32, minor: u32) -> &'static str {
    if (major, minor) >= (1, 11) {
        STATEMENTS_EXT_1_11
    } else if (major, minor) >= (1, 9) {
        STATEMENTS_EXT_1_9
    } else {
        STATEMENTS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg14_and_later_get_query_id() {
        let q = for_version(140_000).expect("PG 14 supported");
        assert!(q.activity.contains("a.query_id"));
        let q16 = for_version(160_003).expect("PG 16 supported");
        assert!(q16.activity.contains("a.query_id"));
    }

    #[test]
    fn pg13_gets_null_query_id_variant() {
        let q = for_version(130_011).expect("PG 13 supported");
        assert!(q.activity.contains("NULL::int8 AS query_id"));
        assert!(!q.activity.contains("a.query_id"));
    }

    #[test]
    fn table_stats_serves_pg13_and_up_with_the_row_cap() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.table_stats.contains("pg_stat_user_tables"));
            assert!(q.table_stats.contains("n_ins_since_vacuum"), "PG13+ set");
            assert!(
                q.table_stats.contains(&format!("LIMIT {TABLE_STATS_LIMIT}")),
                "SQL row cap must match TABLE_STATS_LIMIT"
            );
        }
    }

    #[test]
    fn bloat_queries_serve_pg13_and_up_with_cap_and_attribution() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            for (sql, marker) in [
                (q.bloat_tables, "table/table_bloat.sql"),
                (q.bloat_indexes, "btree/btree_bloat.sql"),
            ] {
                // BSD-2-Clause attribution must survive any future edit.
                assert!(sql.contains("ioguix/pgsql-bloat-estimation"));
                assert!(sql.contains("Jehan-Guillaume (ioguix) de Rorthais"));
                assert!(sql.contains(marker));
                // Same row-cap philosophy as table_stats.
                assert!(sql.contains(&format!("LIMIT {TABLE_STATS_LIMIT}")));
                // Estimates, never presented as measurements.
                assert!(sql.to_lowercase().contains("estimate"));
            }
            // The index variant names the owning table (S3 detail join).
            assert!(q.bloat_indexes.contains("tblname::text AS tblname"));
        }
    }

    #[test]
    fn admin_statements_serve_pg13_and_up_with_one_pid_param() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.cancel_backend.contains("pg_cancel_backend($1::int4)"));
            assert!(q.cancel_backend.contains("AS is_stopped"));
            assert!(q.terminate_backend.contains("pg_terminate_backend($1::int4)"));
            assert!(q.terminate_backend.contains("AS is_stopped"));
            // Attribution to the pg_activity originals must survive edits.
            assert!(q.cancel_backend.contains("do_pg_cancel_backend.sql"));
            assert!(q.terminate_backend.contains("do_pg_terminate_backend.sql"));
        }
    }

    #[test]
    fn statements_query_serves_pg13_and_up_with_cap_and_conventions() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            // Extension-1.8+ column names (never the pre-1.8 total_time).
            assert!(q.statements.contains("total_exec_time"));
            assert!(q.statements.contains("mean_exec_time"));
            assert!(!q.statements.contains("s.total_time"));
            // queryid ships as text (JS-safe) and the lens is current-db.
            assert!(q.statements.contains("queryid::text"));
            assert!(q.statements.contains("current_database()"));
            assert!(
                q.statements.contains(&format!("LIMIT {STATEMENTS_LIMIT}")),
                "SQL row cap must match STATEMENTS_LIMIT"
            );
        }
    }

    #[test]
    fn statements_ext_tiers_alias_to_the_same_column_names() {
        // v0.14: one Rust parser serves all three extension tiers — every
        // variant must expose the SAME output column names.
        for sql in [STATEMENTS, STATEMENTS_EXT_1_9, STATEMENTS_EXT_1_11] {
            for marker in [
                "AS temp_blks_read",
                "AS temp_blks_written",
                "AS shared_blks_dirtied",
                "AS shared_blks_written",
                "AS blk_read_time_ms",
                "AS blk_write_time_ms",
                "AS wal_bytes",
                "AS track_io_timing_on",
            ] {
                assert!(sql.contains(marker), "{marker} missing from a statements tier");
            }
        }
    }

    #[test]
    fn statements_sql_for_extension_picks_the_right_tier() {
        assert_eq!(statements_sql_for_extension(1, 8), STATEMENTS);
        assert!(statements_sql_for_extension(1, 8).contains("NULL::int8 AS wal_bytes"));
        assert_eq!(statements_sql_for_extension(1, 9), STATEMENTS_EXT_1_9);
        assert!(statements_sql_for_extension(1, 9).contains("s.wal_bytes::int8 AS wal_bytes"));
        assert_eq!(statements_sql_for_extension(1, 10), STATEMENTS_EXT_1_9);
        assert_eq!(statements_sql_for_extension(1, 11), STATEMENTS_EXT_1_11);
        assert!(statements_sql_for_extension(1, 11).contains("s.shared_blk_read_time::float8"));
        assert_eq!(statements_sql_for_extension(2, 0), STATEMENTS_EXT_1_11);
    }

    #[test]
    fn replication_queries_serve_pg13_and_up() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.replication.contains("pg_stat_replication"));
            // LSN diff guarded against evaluation during recovery.
            assert!(q.replication.contains("pg_is_in_recovery()"));
            assert!(q.wal_receiver.contains("pg_stat_wal_receiver"));
        }
    }

    #[test]
    fn vacuum_queries_serve_pg13_and_up_with_conventions() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.vacuum_cluster_age.contains("pg_database"));
            assert!(q.vacuum_cluster_age.contains("age(datfrozenxid)"));
            assert!(q.vacuum_table_ages.contains("pg_stat_user_tables"));
            assert!(q.vacuum_table_ages.contains("relfrozenxid"));
            assert!(
                q.vacuum_table_ages
                    .contains(&format!("LIMIT {VACUUM_TABLES_LIMIT}")),
                "SQL row cap must match VACUUM_TABLES_LIMIT"
            );
            assert!(q.vacuum_progress.contains("pg_stat_progress_vacuum"));
        }
    }

    #[test]
    fn replication_slots_query_serves_pg13_and_up_with_conventions() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.replication_slots.contains("pg_replication_slots"));
            // Retained-bytes LSN diff guarded against evaluation during
            // recovery, exactly like the sender/receiver queries.
            assert!(q.replication_slots.contains("pg_is_in_recovery()"));
            assert!(q.replication_slots.contains("wal_status"));
            assert!(q.replication_slots.contains("safe_wal_size"));
        }
    }

    #[test]
    fn index_advisor_query_serves_pg13_and_up_with_conventions() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.indexes.contains("pg_stat_user_indexes"));
            assert!(q.indexes.contains("pg_index"));
            assert!(q.indexes.contains("indisunique"));
            assert!(q.indexes.contains("indisprimary"));
            assert!(q.indexes.contains("indisexclusion"));
            assert!(q.indexes.contains("pg_get_indexdef"));
            assert!(
                q.indexes.contains(&format!("LIMIT {INDEXES_LIMIT}")),
                "SQL row cap must match INDEXES_LIMIT"
            );
            assert!(q.db_stats_reset.contains("pg_stat_database"));
            assert!(q.db_stats_reset.contains("current_database()"));
        }
    }

    #[test]
    fn pre_pg13_is_rejected() {
        let err = for_version(120_017).expect_err("PG 12 unsupported");
        assert!(err.contains("PostgreSQL 13+"));
    }

    #[test]
    fn bgwriter_query_uses_the_merged_view_below_pg17() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.bgwriter.contains("pg_stat_bgwriter"));
            assert!(!q.bgwriter.contains("pg_stat_checkpointer"));
            assert!(q.bgwriter.contains("checkpoints_timed"));
            assert!(q.bgwriter.contains("buffers_backend"));
        }
    }

    #[test]
    fn databases_query_serves_pg13_and_up_with_conventions() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.databases.contains("pg_database"));
            assert!(q.databases.contains("datallowconn"));
            assert!(q.databases.contains("has_database_privilege"));
        }
    }

    #[test]
    fn lock_capacity_query_serves_pg13_and_up_with_conventions() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.lock_capacity.contains("pg_catalog.pg_locks"));
            assert!(q.lock_capacity.contains("max_locks_per_transaction"));
            assert!(q.lock_capacity.contains("max_connections"));
            assert!(q.lock_capacity.contains("max_prepared_transactions"));
        }
    }

    #[test]
    fn idle_sessions_query_serves_pg13_and_up_with_conventions() {
        for version in [130_011, 140_000, 160_003] {
            let q = for_version(version).expect("supported");
            assert!(q.idle_sessions.contains("pg_stat_activity"));
            assert!(q.idle_sessions.contains("state = 'idle'"));
            assert!(q.idle_sessions.contains("state_change"));
            assert!(q.idle_sessions.contains("client_addr::text"));
            assert!(
                q.idle_sessions
                    .contains(&format!("LIMIT {IDLE_SESSIONS_LIMIT}")),
                "SQL row cap must match IDLE_SESSIONS_LIMIT"
            );
            // Never crosses idle-in-transaction sessions — those are the
            // v0.9 xact-age hunter's territory, not this census's.
            assert!(!q.idle_sessions.to_lowercase().contains("idle in transaction"));
        }
    }

    #[test]
    fn bgwriter_query_splits_the_checkpointer_out_on_pg17() {
        let q = for_version(170_000).expect("PG 17 supported");
        assert!(q.bgwriter.contains("pg_stat_checkpointer"));
        assert!(q.bgwriter.contains("pg_stat_bgwriter"));
        // Aliased to the same output columns as the pre-17 variant.
        assert!(q.bgwriter.contains("AS checkpoints_timed"));
        assert!(q.bgwriter.contains("AS checkpoints_req"));
        assert!(q.bgwriter.contains("AS buffers_backend"));
        assert!(q.bgwriter.contains("NULL::int8 AS buffers_backend"));
    }
}
