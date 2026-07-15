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
    pub cancel_backend: &'static str,
    pub terminate_backend: &'static str,
}

/// Row cap of the table-stats query (top N tables by total size). Kept as a
/// const so the SQL and any future flag stay in sync (asserted by a test).
pub const TABLE_STATS_LIMIT: usize = 200;

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
// Admin actions, adapted from dalibo/pg_activity's do_pg_cancel_backend.sql /
// do_pg_terminate_backend.sql. Version-independent (both functions predate
// PG 13), so one file each serves the whole supported range.
const DO_CANCEL_BACKEND: &str = include_str!("../queries/do_cancel_backend.sql");
const DO_TERMINATE_BACKEND: &str = include_str!("../queries/do_terminate_backend.sql");

/// Picks the SQL variants for a server version (`server_version_num` format,
/// e.g. `160003`). Below PG 13 there is no `leader_pid`, so pg_lens refuses.
pub fn for_version(server_version_num: i32) -> Result<QuerySet, String> {
    if server_version_num >= 140_000 {
        Ok(QuerySet {
            activity: ACTIVITY_POST_140000,
            blocking: BLOCKING_POST_140000,
            server_info: SERVER_INFO_POST_130000,
            table_stats: TABLE_STATS_POST_130000,
            bloat_tables: BLOAT_TABLES,
            bloat_indexes: BLOAT_INDEXES,
            cancel_backend: DO_CANCEL_BACKEND,
            terminate_backend: DO_TERMINATE_BACKEND,
        })
    } else if server_version_num >= 130_000 {
        Ok(QuerySet {
            activity: ACTIVITY_POST_130000,
            blocking: BLOCKING_POST_140000,
            server_info: SERVER_INFO_POST_130000,
            table_stats: TABLE_STATS_POST_130000,
            bloat_tables: BLOAT_TABLES,
            bloat_indexes: BLOAT_INDEXES,
            cancel_backend: DO_CANCEL_BACKEND,
            terminate_backend: DO_TERMINATE_BACKEND,
        })
    } else {
        Err(format!(
            "unsupported PostgreSQL version (server_version_num={server_version_num}): \
             pg_lens requires PostgreSQL 13+"
        ))
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
    fn pre_pg13_is_rejected() {
        let err = for_version(120_017).expect_err("PG 12 unsupported");
        assert!(err.contains("PostgreSQL 13+"));
    }
}
