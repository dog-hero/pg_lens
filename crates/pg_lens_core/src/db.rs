//! Connection bootstrap + typed row extraction for the real data layer.
//!
//! Nothing here uses `unwrap`/`get` on database results: extraction goes
//! through `Row::try_get`, and every error bubbles up so the poller can turn
//! it into a `PollerStatus::Error`.

use tokio::task::JoinHandle;
use tokio_postgres::{Client, Config, NoTls, Row};

use crate::models::{ActivityRow, LockRow};

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
pub async fn server_version_num(client: &Client) -> Result<i32, tokio_postgres::Error> {
    let row = client
        .query_one("SELECT current_setting('server_version_num')::int", &[])
        .await?;
    row.try_get(0)
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
    })
}

/// `try_get` an optional text column, defaulting NULL to `""`.
fn opt_text(row: &Row, column: &str) -> Result<String, tokio_postgres::Error> {
    Ok(row.try_get::<_, Option<String>>(column)?.unwrap_or_default())
}
