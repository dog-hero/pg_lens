//! Versioned SQL, embedded at compile time and selected by
//! `server_version_num` (pg_activity's `post_NNNNNN` convention).

/// The three statements one poller session prepares (once) and runs per tick.
#[derive(Clone, Copy, Debug)]
pub struct QuerySet {
    pub activity: &'static str,
    pub blocking: &'static str,
    pub server_info: &'static str,
}

const ACTIVITY_POST_140000: &str = include_str!("../queries/activity_post_140000.sql");
const ACTIVITY_POST_130000: &str = include_str!("../queries/activity_post_130000.sql");
// Uses only 9.6+ features (pg_blocking_pids), so it serves PG 13 too.
const BLOCKING_POST_140000: &str = include_str!("../queries/blocking_post_140000.sql");
const SERVER_INFO_POST_130000: &str = include_str!("../queries/server_info_post_130000.sql");

/// Picks the SQL variants for a server version (`server_version_num` format,
/// e.g. `160003`). Below PG 13 there is no `leader_pid`, so pg_lens refuses.
pub fn for_version(server_version_num: i32) -> Result<QuerySet, String> {
    if server_version_num >= 140_000 {
        Ok(QuerySet {
            activity: ACTIVITY_POST_140000,
            blocking: BLOCKING_POST_140000,
            server_info: SERVER_INFO_POST_130000,
        })
    } else if server_version_num >= 130_000 {
        Ok(QuerySet {
            activity: ACTIVITY_POST_130000,
            blocking: BLOCKING_POST_140000,
            server_info: SERVER_INFO_POST_130000,
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
    fn pre_pg13_is_rejected() {
        let err = for_version(120_017).expect_err("PG 12 unsupported");
        assert!(err.contains("PostgreSQL 13+"));
    }
}
