//! Connection bootstrap + typed row extraction for the real data layer.
//!
//! Nothing here uses `unwrap`/`get` on database results: extraction goes
//! through `Row::try_get`, and every error bubbles up so the poller can turn
//! it into a `PollerStatus::Error`.

use tokio::task::JoinHandle;
use tokio_postgres::{Client, Config, NoTls, Row, Transaction};

use crate::models::{
    ActivityRow, BloatRow, LockRow, StatementRow, TableStatRow, WalReceiverRow, WalSenderRow,
};

/// Connects to PostgreSQL and — mandatory per docs.rs/tokio-postgres — moves
/// the `Connection` onto its own task: it performs the actual I/O, and no
/// query completes unless it is polled concurrently.
///
/// Takes a resolved [`Config`] (see [`crate::settings::resolve`]) rather
/// than a DSN string, so passwords resolved from the environment are never
/// re-interpolated into text.
pub async fn connect(config: &Config) -> Result<(Client, JoinHandle<()>), tokio_postgres::Error> {
    let (client, connection) = config.connect(NoTls).await?;
    let handle = tokio::spawn(async move {
        // A connection error also surfaces as an error on the Client side,
        // where the poller reports it through PollerStatus — nothing to do
        // with it here (and the core has no logging facility by design).
        let _ = connection.await;
    });
    Ok((client, handle))
}

/// `SELECT current_setting('server_version_num')::int` — e.g. `160003`.
/// Runs inside the caller's transaction so it is safe behind a
/// transaction-pooling proxy (a bare prepare+execute would otherwise split
/// across two server backends).
pub async fn server_version_num(tx: &Transaction<'_>) -> Result<i32, tokio_postgres::Error> {
    let row = tx
        .query_one("SELECT current_setting('server_version_num')::int", &[])
        .await?;
    row.try_get(0)
}

/// Identifies the poller's own session, run once right after connect, so
/// operators can see who is connected in `pg_stat_activity` instead of an
/// anonymous backend. This is a session-level `SET` (reverts on disconnect);
/// behind a transaction-pooling proxy it will not persist, which is harmless
/// — the per-statement safety timeout is applied as `SET LOCAL` inside each
/// query transaction instead (see the poller), so it holds in both modes.
pub async fn configure_session(client: &Client) -> Result<(), tokio_postgres::Error> {
    client
        .batch_execute("SET application_name = 'pg_lens'")
        .await
}

/// Maps one row of `queries/activity_post_*.sql` onto [`ActivityRow`].
/// Nullable text columns collapse to `""` (`"local"` for a NULL client
/// address, i.e. a Unix-socket connection).
pub fn activity_from_row(row: &Row) -> Result<ActivityRow, tokio_postgres::Error> {
    Ok(ActivityRow {
        pid: row.try_get("pid")?,
        application_name: opt_text(row, "application_name")?,
        database: opt_text(row, "database")?,
        client: row
            .try_get::<_, Option<String>>("client")?
            .unwrap_or_else(|| "local".to_string()),
        duration_secs: row.try_get::<_, Option<f64>>("duration")?.unwrap_or(0.0),
        wait_event: row.try_get("wait")?,
        username: opt_text(row, "usename")?,
        state: opt_text(row, "state")?,
        query: opt_text(row, "query")?,
        query_leader_pid: row.try_get("query_leader_pid")?,
        is_parallel_worker: row.try_get("is_parallel_worker")?,
        query_id: row.try_get("query_id")?,
    })
}

/// Maps one row of `queries/blocking_post_*.sql` onto [`LockRow`].
pub fn lock_from_row(row: &Row) -> Result<LockRow, tokio_postgres::Error> {
    Ok(LockRow {
        pid: row.try_get("pid")?,
        blocked_by: row
            .try_get::<_, Option<Vec<i32>>>("blocked_by")?
            .unwrap_or_default(),
        mode: row.try_get("mode")?,
        locktype: row.try_get("locktype")?,
        relation: row.try_get("relation")?,
        duration_secs: row.try_get::<_, Option<f64>>("duration")?.unwrap_or(0.0),
        query: opt_text(row, "query")?,
    })
}

/// The single row of `queries/server_info_post_130000.sql`, still raw:
/// cumulative counters that the poller turns into deltas (TPS, cache hit).
#[derive(Clone, Debug)]
pub struct ServerInfoRow {
    pub xact_commit: i64,
    pub xact_rollback: i64,
    pub blks_hit: i64,
    pub blks_read: i64,
    pub tup_returned: i64,
    pub tup_fetched: i64,
    pub temp_files: i64,
    pub temp_bytes: i64,
    pub deadlocks: i64,
    pub connections_total: i32,
    pub active: i32,
    pub idle: i32,
    pub idle_in_transaction: i32,
    pub waiting: i32,
    pub max_connections: i32,
    pub uptime_secs: f64,
    pub server_version: String,
    /// `current_database()` — the Schema Lens is per-database, so its
    /// header names which database the table stats belong to.
    pub database: String,
    /// `pg_is_in_recovery()` — true on a standby, false on a primary. Decides
    /// which replication view the Macro Lens presents.
    pub is_in_recovery: bool,
}

pub fn server_info_from_row(row: &Row) -> Result<ServerInfoRow, tokio_postgres::Error> {
    Ok(ServerInfoRow {
        xact_commit: row.try_get("xact_commit")?,
        xact_rollback: row.try_get("xact_rollback")?,
        blks_hit: row.try_get("blks_hit")?,
        blks_read: row.try_get("blks_read")?,
        tup_returned: row.try_get("tup_returned")?,
        tup_fetched: row.try_get("tup_fetched")?,
        temp_files: row.try_get("temp_files")?,
        temp_bytes: row.try_get("temp_bytes")?,
        deadlocks: row.try_get("deadlocks")?,
        connections_total: row.try_get("connections_total")?,
        active: row.try_get("active")?,
        idle: row.try_get("idle")?,
        idle_in_transaction: row.try_get("idle_in_transaction")?,
        waiting: row.try_get("waiting")?,
        max_connections: row.try_get("max_connections")?,
        uptime_secs: row.try_get("uptime_secs")?,
        server_version: row.try_get("server_version")?,
        database: row.try_get("database")?,
        is_in_recovery: row.try_get("is_in_recovery")?,
    })
}

/// Maps one row of `queries/replication.sql` onto [`WalSenderRow`] (the
/// primary side: one connected streaming replica). Lag columns are nullable —
/// `replay_lag` is NULL while a replica is idle, and the byte diff is NULL on
/// a cascading standby (guarded by the CASE in the SQL).
pub fn wal_sender_from_row(row: &Row) -> Result<WalSenderRow, tokio_postgres::Error> {
    Ok(WalSenderRow {
        application_name: row.try_get("application_name")?,
        client: row.try_get("client")?,
        state: row.try_get("state")?,
        sync_state: row.try_get("sync_state")?,
        replay_lag_bytes: row.try_get("replay_lag_bytes")?,
        replay_lag_secs: row.try_get("replay_lag_secs")?,
    })
}

/// Maps one row of `queries/wal_receiver.sql` onto [`WalReceiverRow`] (the
/// standby side). At most one row exists.
pub fn wal_receiver_from_row(row: &Row) -> Result<WalReceiverRow, tokio_postgres::Error> {
    Ok(WalReceiverRow {
        status: row.try_get("status")?,
        sender_host: row.try_get("sender_host")?,
        sender_port: row.try_get("sender_port")?,
        replay_lag_bytes: row.try_get("replay_lag_bytes")?,
        replay_lag_secs: row.try_get("replay_lag_secs")?,
    })
}

/// Maps one row of `queries/table_stats_post_130000.sql` onto
/// [`TableStatRow`]. Counters arrive already COALESCEd to 0 by the SQL —
/// except `idx_scan`/`idx_tup_fetch`, whose NULL ("table has no indexes")
/// is information the model keeps as `None`. `last_*` timestamps arrive as
/// epoch seconds `::float8` (NULL = never), per the repo convention.
pub fn table_stat_from_row(row: &Row) -> Result<TableStatRow, tokio_postgres::Error> {
    Ok(TableStatRow {
        schema: row.try_get("schemaname")?,
        name: row.try_get("relname")?,
        total_bytes: row.try_get("total_bytes")?,
        table_bytes: row.try_get("table_bytes")?,
        index_bytes: row.try_get("index_bytes")?,
        seq_scan: row.try_get("seq_scan")?,
        seq_tup_read: row.try_get("seq_tup_read")?,
        idx_scan: row.try_get("idx_scan")?,
        idx_tup_fetch: row.try_get("idx_tup_fetch")?,
        n_tup_ins: row.try_get("n_tup_ins")?,
        n_tup_upd: row.try_get("n_tup_upd")?,
        n_tup_del: row.try_get("n_tup_del")?,
        n_tup_hot_upd: row.try_get("n_tup_hot_upd")?,
        n_live_tup: row.try_get("n_live_tup")?,
        n_dead_tup: row.try_get("n_dead_tup")?,
        n_mod_since_analyze: row.try_get("n_mod_since_analyze")?,
        n_ins_since_vacuum: row.try_get("n_ins_since_vacuum")?,
        last_vacuum_epoch_secs: row.try_get("last_vacuum")?,
        last_autovacuum_epoch_secs: row.try_get("last_autovacuum")?,
        last_analyze_epoch_secs: row.try_get("last_analyze")?,
        last_autoanalyze_epoch_secs: row.try_get("last_autoanalyze")?,
        vacuum_count: row.try_get("vacuum_count")?,
        autovacuum_count: row.try_get("autovacuum_count")?,
        analyze_count: row.try_get("analyze_count")?,
        autoanalyze_count: row.try_get("autoanalyze_count")?,
    })
}

/// Maps one row of `queries/bloat_tables.sql` / `bloat_indexes.sql` (both
/// share the exact same output shape) onto [`BloatRow`]. Column wire types
/// were verified live (Fase S2): text, text, int8, int8, float8, int4, bool
/// — the casts live in the SQL per the repo's type-trap convention.
///
/// `is_na` gating happens HERE, not only in the SQL: a row ioguix flags as
/// "not applicable" must never carry a number into the models (`None`, not
/// `0.0`) — see [`na_gate`].
pub fn bloat_from_row(row: &Row) -> Result<BloatRow, tokio_postgres::Error> {
    let is_na: bool = row.try_get("is_na")?;
    let (bloat_bytes, bloat_pct) = na_gate(
        is_na,
        row.try_get("bloat_bytes")?,
        row.try_get("bloat_pct")?,
    );
    Ok(BloatRow {
        schema: row.try_get("schema")?,
        name: row.try_get("name")?,
        // Only bloat_indexes.sql outputs `tblname` (the owning table);
        // bloat_tables.sql has no such column, so the lookup collapses to
        // `None` there — one parser serves both shapes.
        table: row.try_get("tblname").ok().flatten(),
        real_bytes: row.try_get("real_bytes")?,
        bloat_bytes,
        bloat_pct,
        fillfactor: row.try_get("fillfactor")?,
        is_na,
    })
}

/// The is_na rule as a pure (unit-testable) function: an unreliable
/// estimate carries no numbers at all.
fn na_gate(
    is_na: bool,
    bloat_bytes: Option<i64>,
    bloat_pct: Option<f64>,
) -> (Option<i64>, Option<f64>) {
    if is_na {
        (None, None)
    } else {
        (bloat_bytes, bloat_pct)
    }
}

/// `SELECT extversion FROM pg_extension WHERE extname = 'pg_stat_statements'`
/// — `None` when the extension is not installed in the connected database.
/// Run once per session (and therefore re-run on every reconnect).
pub async fn statements_extension_version(
    tx: &Transaction<'_>,
) -> Result<Option<String>, tokio_postgres::Error> {
    let rows = tx
        .query(
            "SELECT extversion::text FROM pg_extension WHERE extname = 'pg_stat_statements'",
            &[],
        )
        .await?;
    match rows.first() {
        Some(row) => Ok(Some(row.try_get(0)?)),
        None => Ok(None),
    }
}

/// The Query Lens availability rule, pure and unit-testable: the extension
/// must be installed AND at version >= 1.8 — the release that renamed
/// `total_time` to `total_exec_time` (shipped with PG 13). The decision
/// follows the EXTENSION version, never the server version: an upgraded
/// cluster can carry an older extension. `Err` carries the human-readable
/// reason/hint frontends show as the calm `StatementsStatus::Unavailable`.
pub fn statements_availability(extversion: Option<&str>) -> Result<(), String> {
    match extversion {
        None => Err(
            "the pg_stat_statements extension is not installed in this database. \
             Run: CREATE EXTENSION pg_stat_statements; \
             (requires shared_preload_libraries = 'pg_stat_statements' \
             and a server restart)"
                .to_string(),
        ),
        Some(version) => match parse_extension_version(version) {
            Some((major, minor)) if (major, minor) >= (1, 8) => Ok(()),
            Some(_) => Err(format!(
                "pg_stat_statements extension version {version} is too old \u{2014} \
                 pg_lens needs 1.8+ (the total_exec_time columns, shipped with \
                 PostgreSQL 13). Run: ALTER EXTENSION pg_stat_statements UPDATE;"
            )),
            None => Err(format!(
                "could not parse pg_stat_statements extension version {version:?} \
                 \u{2014} pg_lens needs 1.8+"
            )),
        },
    }
}

/// `"1.8"` → `(1, 8)`; `"1.10"` → `(1, 10)`. Tolerates a patch component
/// (`"1.8.1"`); anything not starting `major.minor` is `None`.
fn parse_extension_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Maps one row of `queries/statements.sql` onto [`StatementRow`]. The
/// queryid arrives already `::text` (JS-safe across the JSON boundary);
/// NULLs collapse per column semantics (`query`/`usename` to `""`,
/// `queryid` kept as `None`).
pub fn statement_from_row(row: &Row) -> Result<StatementRow, tokio_postgres::Error> {
    Ok(StatementRow {
        query_id: row.try_get("queryid")?,
        query: opt_text(row, "query")?,
        username: opt_text(row, "usename")?,
        calls: row.try_get("calls")?,
        total_exec_ms: row.try_get("total_exec_time")?,
        mean_exec_ms: row.try_get("mean_exec_time")?,
        rows: row.try_get("rows")?,
        shared_blks_hit: row.try_get("shared_blks_hit")?,
        shared_blks_read: row.try_get("shared_blks_read")?,
    })
}

/// `try_get` an optional text column, defaulting NULL to `""`.
fn opt_text(row: &Row, column: &str) -> Result<String, tokio_postgres::Error> {
    Ok(row.try_get::<_, Option<String>>(column)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan's hard rule: `is_na = true` → both estimates are `None`
    /// (never 0.0 / 0), even when the SQL produced numbers for the row.
    /// The availability decision maps extension versions (not server
    /// versions) to Ok / Unavailable-with-hint.
    #[test]
    fn statements_availability_requires_extension_1_8_plus() {
        // Missing: Unavailable with the CREATE EXTENSION + preload hint.
        let err = statements_availability(None).expect_err("missing = unavailable");
        assert!(err.contains("CREATE EXTENSION pg_stat_statements"));
        assert!(err.contains("shared_preload_libraries"));

        // Too old (pre-1.8 schema, e.g. an upgraded cluster on PG 13+ that
        // never ran ALTER EXTENSION ... UPDATE): says so.
        for old in ["1.6", "1.7"] {
            let err = statements_availability(Some(old)).expect_err("old = unavailable");
            assert!(err.contains("too old"), "got: {err}");
            assert!(err.contains("ALTER EXTENSION pg_stat_statements UPDATE"));
        }

        // 1.8 (PG 13) through 1.11/1.12 (PG 17/18), incl. two-digit minors
        // — "1.10" must compare as (1,10) > (1,8), not lexicographically.
        for ok in ["1.8", "1.9", "1.10", "1.11", "1.12", "2.0", "1.8.1"] {
            assert!(statements_availability(Some(ok)).is_ok(), "{ok} must be ok");
        }

        // Garbage: refused with a clear message, never a panic.
        let err = statements_availability(Some("banana")).expect_err("unparsable");
        assert!(err.contains("could not parse"));
    }

    #[test]
    fn na_gate_nulls_unreliable_estimates() {
        assert_eq!(na_gate(true, Some(1_048_576), Some(42.5)), (None, None));
        assert_eq!(na_gate(true, None, None), (None, None));
        assert_eq!(
            na_gate(false, Some(1_048_576), Some(42.5)),
            (Some(1_048_576), Some(42.5))
        );
        assert_eq!(na_gate(false, None, None), (None, None));
    }
}
