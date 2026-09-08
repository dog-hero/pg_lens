//! Connection bootstrap + typed row extraction for the real data layer.
//!
//! Nothing here uses `unwrap`/`get` on database results: extraction goes
//! through `Row::try_get`, and every error bubbles up so the poller can turn
//! it into a `PollerStatus::Error`.

use tokio::task::JoinHandle;
use tokio_postgres::{Client, Config, NoTls, Row, Transaction};

use crate::models::{
    ActiveLockRow, ActivityRow, BloatRow, DatabaseRow, DdlProgressRow, IdleSessionRow, LockRow,
    PreparedXactRow, PublicationRow, ReplicationSlotRow, SequenceRow, StatementRow, SubscriptionRow,
    TableDetailColumn, TableDetailConstraint, TableDetailIndex, TableStatRow, VacuumClusterAge,
    VacuumProgressRow, VacuumTableRow, WalReceiverRow, WalSenderRow, calculate_sequence_exhaustion,
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
        xact_age_secs: row.try_get("xact_age_seconds")?,
        wait_event: row.try_get("wait")?,
        username: opt_text(row, "usename")?,
        state: opt_text(row, "state")?,
        query: opt_text(row, "query")?,
        query_leader_pid: row.try_get("query_leader_pid")?,
        is_parallel_worker: row.try_get("is_parallel_worker")?,
        query_id: row.try_get("query_id")?,
        ssl: row.try_get::<_, Option<bool>>("ssl")?.unwrap_or(false),
        ssl_version: row.try_get("ssl_version")?,
        ssl_cipher: row.try_get("ssl_cipher")?,
    })
}

/// Maps one row of `queries/progress_ddl.sql` onto [`DdlProgressRow`].
pub fn ddl_progress_from_row(row: &Row) -> Result<DdlProgressRow, tokio_postgres::Error> {
    Ok(DdlProgressRow {
        pid: row.try_get("pid")?,
        command: opt_text(row, "command")?,
        relation: opt_text(row, "relation")?,
        phase: opt_text(row, "phase")?,
        progress_pct: row.try_get("progress_pct")?,
        current_step: row.try_get::<_, Option<i64>>("current_step")?.unwrap_or(0),
        total_step: row.try_get::<_, Option<i64>>("total_step")?.unwrap_or(0),
        detail: opt_text(row, "detail")?,
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

/// Maps one row of `queries/locks_active.sql` onto [`ActiveLockRow`].
pub fn active_lock_from_row(row: &Row) -> Result<ActiveLockRow, tokio_postgres::Error> {
    Ok(ActiveLockRow {
        pid: row.try_get("pid")?,
        locktype: row.try_get("locktype")?,
        relation: row.try_get("relation")?,
        schema: row.try_get("schema")?,
        mode: row.try_get("mode")?,
        granted: row.try_get("granted")?,
        fastpath: row.try_get("fastpath")?,
        duration_secs: row.try_get::<_, Option<f64>>("duration_secs")?.unwrap_or(0.0),
        usename: row.try_get("usename")?,
        application_name: row.try_get("application_name")?,
        query: row.try_get("query")?,
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

/// The single row of `queries/bgwriter_post_*.sql` (F4), still raw
/// cumulative counters — the poller turns them into per-tick rates and the
/// session-window requested/timed ratio, exactly like `ServerInfoRow`'s
/// TPS/cache-hit treatment.
#[derive(Clone, Debug)]
pub struct BgwriterRow {
    pub checkpoints_timed: i64,
    pub checkpoints_req: i64,
    pub checkpoint_write_time_ms: f64,
    pub checkpoint_sync_time_ms: f64,
    pub buffers_checkpoint: i64,
    pub buffers_clean: i64,
    pub maxwritten_clean: i32,
    /// `None` on PG 17+ (moved to `pg_stat_io`, the query sends a typed
    /// NULL there — see `bgwriter_post_170000.sql`).
    pub buffers_backend: Option<i64>,
    pub buffers_alloc: i64,
}

pub fn bgwriter_from_row(row: &Row) -> Result<BgwriterRow, tokio_postgres::Error> {
    Ok(BgwriterRow {
        checkpoints_timed: row.try_get("checkpoints_timed")?,
        checkpoints_req: row.try_get("checkpoints_req")?,
        checkpoint_write_time_ms: row.try_get("checkpoint_write_time_ms")?,
        checkpoint_sync_time_ms: row.try_get("checkpoint_sync_time_ms")?,
        buffers_checkpoint: row.try_get("buffers_checkpoint")?,
        buffers_clean: row.try_get("buffers_clean")?,
        maxwritten_clean: row.try_get("maxwritten_clean")?,
        buffers_backend: row.try_get("buffers_backend")?,
        buffers_alloc: row.try_get("buffers_alloc")?,
    })
}

/// Maps one row of `queries/replication.sql` onto [`WalSenderRow`] (the
/// primary side: one connected streaming replica).
pub fn wal_sender_from_row(row: &Row) -> Result<WalSenderRow, tokio_postgres::Error> {
    Ok(WalSenderRow {
        application_name: row.try_get("application_name")?,
        client: row.try_get("client")?,
        state: row.try_get("state")?,
        sync_state: row.try_get("sync_state")?,
        sync_priority: row.try_get("sync_priority")?,
        sent_lag_bytes: row.try_get("sent_lag_bytes")?,
        write_lag_bytes: row.try_get("write_lag_bytes")?,
        flush_lag_bytes: row.try_get("flush_lag_bytes")?,
        replay_lag_bytes: row.try_get("replay_lag_bytes")?,
        total_lag_bytes: row.try_get("total_lag_bytes")?,
        write_lag_secs: row.try_get("write_lag_secs")?,
        flush_lag_secs: row.try_get("flush_lag_secs")?,
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
        is_paused: row.try_get("is_paused")?,
        pause_state: row.try_get("pause_state")?,
    })
}

/// Maps one row of `queries/replication_slots.sql` onto
/// [`ReplicationSlotRow`].
pub fn replication_slot_from_row(row: &Row) -> Result<ReplicationSlotRow, tokio_postgres::Error> {
    Ok(ReplicationSlotRow {
        slot_name: row.try_get("slot_name")?,
        plugin: row.try_get("plugin").ok().flatten(),
        slot_type: row.try_get("slot_type")?,
        database: row.try_get("database").ok().flatten(),
        temporary: row.try_get("temporary").unwrap_or(false),
        active: row.try_get("active")?,
        active_pid: row.try_get("active_pid").ok().flatten(),
        application_name: row.try_get("application_name").ok().flatten(),
        client_addr: row.try_get("client_addr").ok().flatten(),
        restart_lsn: row.try_get("restart_lsn").ok().flatten(),
        confirmed_flush_lsn: row.try_get("confirmed_flush_lsn").ok().flatten(),
        retained_wal_bytes: row.try_get("retained_wal_bytes")?,
        consumer_lag_bytes: row.try_get("consumer_lag_bytes").ok().flatten(),
        wal_status: row.try_get("wal_status")?,
        safe_wal_size: row.try_get("safe_wal_size")?,
        xmin_age: row.try_get("xmin_age")?,
        catalog_xmin_age: row.try_get("catalog_xmin_age")?,
        two_phase: row.try_get("two_phase").ok().flatten(),
        conflicting: row.try_get("conflicting").ok().flatten(),
        invalidated: row.try_get("invalidated")?,
    })
}

/// Maps one row of `queries/publications.sql` onto [`PublicationRow`] (v0.20).
pub fn publication_from_row(row: &Row) -> Result<PublicationRow, tokio_postgres::Error> {
    let published_tables_raw: Option<String> = row.try_get("published_tables").ok().flatten();
    let published_tables = match published_tables_raw {
        Some(s) if !s.is_empty() => s.split(", ").map(|p| p.to_string()).collect(),
        _ => Vec::new(),
    };
    Ok(PublicationRow {
        pubname: row.try_get("pubname")?,
        owner: row.try_get("owner")?,
        all_tables: row.try_get("puballtables")?,
        pubinsert: row.try_get("pubinsert")?,
        pubupdate: row.try_get("pubupdate")?,
        pubdelete: row.try_get("pubdelete")?,
        pubtruncate: row.try_get("pubtruncate")?,
        pubviaroot: row.try_get("pubviaroot")?,
        table_count: row.try_get("table_count")?,
        published_tables,
    })
}

/// Maps one row of `queries/subscriptions.sql` onto [`SubscriptionRow`] (v0.20).
pub fn subscription_from_row(row: &Row) -> Result<SubscriptionRow, tokio_postgres::Error> {
    let syncing_raw: Option<String> = row.try_get("syncing_table_names").ok().flatten();
    let syncing_table_names = match syncing_raw {
        Some(s) if !s.is_empty() => s.split(", ").map(|p| p.to_string()).collect(),
        _ => Vec::new(),
    };
    Ok(SubscriptionRow {
        subname: row.try_get("subname")?,
        owner: row.try_get("owner")?,
        enabled: row.try_get("subenabled")?,
        slot_name: row.try_get("subslotname")?,
        publications: row.try_get("subpublications")?,
        sync_commit: row.try_get("sync_commit").ok().flatten(),
        publisher_host: row.try_get("publisher_host").ok().flatten(),
        publisher_port: row.try_get("publisher_port").ok().flatten(),
        publisher_dbname: row.try_get("publisher_dbname").ok().flatten(),
        streaming_mode: row.try_get("streaming_mode").ok().flatten(),
        binary_mode: row.try_get("binary_mode").ok().flatten(),
        two_phase: row.try_get("two_phase").ok().flatten(),
        worker_pid: row.try_get("pid")?,
        received_lsn: row.try_get("received_lsn")?,
        last_msg_send_secs: row.try_get("last_msg_send_secs")?,
        last_msg_receipt_secs: row.try_get("last_msg_receipt_secs")?,
        latest_end_lsn: row.try_get("latest_end_lsn")?,
        latest_end_secs: row.try_get("latest_end_secs")?,
        sync_tables: row.try_get("sync_tables")?,
        ready_tables: row.try_get("ready_tables")?,
        total_tables: row.try_get("total_tables")?,
        syncing_table_names,
        apply_error_count: row.try_get("apply_error_count")?,
        sync_error_count: row.try_get("sync_error_count")?,
    })
}

/// Maps the single row of `queries/table_stats_total.sql` onto the TRUE
/// (uncapped) table count (v0.15).
pub fn table_stats_total_from_row(row: &Row) -> Result<i64, tokio_postgres::Error> {
    row.try_get("tables_total")
}

/// Maps one row of `queries/table_stats_post_130000.sql` onto
/// [`TableStatRow`]. Counters arrive already COALESCEd to 0 by the SQL —
/// except `idx_scan`/`idx_tup_fetch`, whose NULL ("table has no indexes")
/// is information the model keeps as `None`. `last_*` timestamps arrive as
/// epoch seconds `::float8` (NULL = never), per the repo convention.
pub fn table_stat_from_row(row: &Row) -> Result<TableStatRow, tokio_postgres::Error> {
    let table_bytes: i64 = row.try_get("table_bytes")?;
    let heap_bytes: i64 = row.try_get("heap_bytes").unwrap_or(table_bytes);
    let toast_bytes = table_bytes.saturating_sub(heap_bytes);

    let heap_blks_read: i64 = row.try_get("heap_blks_read").unwrap_or(0);
    let heap_blks_hit: i64 = row.try_get("heap_blks_hit").unwrap_or(0);
    let idx_blks_read: i64 = row.try_get("idx_blks_read").unwrap_or(0);
    let idx_blks_hit: i64 = row.try_get("idx_blks_hit").unwrap_or(0);
    let toast_blks_read: i64 = row.try_get("toast_blks_read").unwrap_or(0);
    let toast_blks_hit: i64 = row.try_get("toast_blks_hit").unwrap_or(0);

    let heap_cache_hit_pct = if heap_blks_hit + heap_blks_read > 0 {
        Some(((heap_blks_hit as f64 / (heap_blks_hit + heap_blks_read) as f64) * 100.0) as f32)
    } else {
        None
    };
    let idx_cache_hit_pct = if idx_blks_hit + idx_blks_read > 0 {
        Some(((idx_blks_hit as f64 / (idx_blks_hit + idx_blks_read) as f64) * 100.0) as f32)
    } else {
        None
    };
    let toast_cache_hit_pct = if toast_blks_hit + toast_blks_read > 0 {
        Some(((toast_blks_hit as f64 / (toast_blks_hit + toast_blks_read) as f64) * 100.0) as f32)
    } else {
        None
    };

    Ok(TableStatRow {
        oid: row.try_get("relid")?,
        schema: row.try_get("schemaname")?,
        name: row.try_get("relname")?,
        total_bytes: row.try_get("total_bytes")?,
        table_bytes,
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
        // Filled in by the poller from the size-growth ring after every row
        // is parsed (`crate::schema_growth`), never by the SQL itself.
        growth_1h_bytes: None,
        growth_1h_pct: None,
        is_partition: row.try_get("is_partition")?,
        parent_oid: row.try_get("parent_oid")?,
        // Only a synthesized partition-parent row carries this (see
        // `partition_parent_from_row`) — never a plain `table_stats` row.
        partition_count: None,
        // Filled in by the poller from the FAST-tick lock join after every
        // row is parsed (`poller::fold_relation_locks`), never by this SQL.
        lock_count: None,
        lock_waiters: None,
        heap_bytes,
        toast_bytes,
        heap_cache_hit_pct,
        idx_cache_hit_pct,
        toast_cache_hit_pct,
    })
}

/// Maps one row of `queries/partition_parents.sql` onto a SYNTHESIZED
/// [`TableStatRow`] (v0.15's partition collapsing): a partitioned PARENT has
/// no physical storage of its own, so this row's size/tuple fields are
/// SUMS over its leaf partitions, never a direct catalog reading. Vacuum/
/// analyze bookkeeping (`last_vacuum`, `vacuum_count`, ...) has no single
/// meaningful value for an aggregate of many leaves — each leaf is vacuumed
/// independently — so those fields stay at their honest "unknown" default
/// (`None`/`0`) rather than a made-up rollup. `idx_scan`/`idx_tup_fetch`
/// likewise stay `None` (not aggregated by the SQL) rather than a
/// misleading partial sum.
pub fn partition_parent_from_row(row: &Row) -> Result<TableStatRow, tokio_postgres::Error> {
    let table_bytes: i64 = row.try_get("table_bytes")?;
    let heap_bytes: i64 = row.try_get("heap_bytes").unwrap_or(table_bytes);
    let toast_bytes = table_bytes.saturating_sub(heap_bytes);

    let heap_blks_read: i64 = row.try_get("heap_blks_read").unwrap_or(0);
    let heap_blks_hit: i64 = row.try_get("heap_blks_hit").unwrap_or(0);
    let idx_blks_read: i64 = row.try_get("idx_blks_read").unwrap_or(0);
    let idx_blks_hit: i64 = row.try_get("idx_blks_hit").unwrap_or(0);
    let toast_blks_read: i64 = row.try_get("toast_blks_read").unwrap_or(0);
    let toast_blks_hit: i64 = row.try_get("toast_blks_hit").unwrap_or(0);

    let heap_cache_hit_pct = if heap_blks_hit + heap_blks_read > 0 {
        Some(((heap_blks_hit as f64 / (heap_blks_hit + heap_blks_read) as f64) * 100.0) as f32)
    } else {
        None
    };
    let idx_cache_hit_pct = if idx_blks_hit + idx_blks_read > 0 {
        Some(((idx_blks_hit as f64 / (idx_blks_hit + idx_blks_read) as f64) * 100.0) as f32)
    } else {
        None
    };
    let toast_cache_hit_pct = if toast_blks_hit + toast_blks_read > 0 {
        Some(((toast_blks_hit as f64 / (toast_blks_hit + toast_blks_read) as f64) * 100.0) as f32)
    } else {
        None
    };

    Ok(TableStatRow {
        oid: row.try_get("relid")?,
        schema: row.try_get("schemaname")?,
        name: row.try_get("relname")?,
        total_bytes: row.try_get("total_bytes")?,
        table_bytes,
        index_bytes: row.try_get("index_bytes")?,
        seq_scan: row.try_get("seq_scan")?,
        seq_tup_read: row.try_get("seq_tup_read")?,
        idx_scan: None,
        idx_tup_fetch: None,
        n_tup_ins: row.try_get("n_tup_ins")?,
        n_tup_upd: row.try_get("n_tup_upd")?,
        n_tup_del: row.try_get("n_tup_del")?,
        n_tup_hot_upd: row.try_get("n_tup_hot_upd")?,
        n_live_tup: row.try_get("n_live_tup")?,
        n_dead_tup: row.try_get("n_dead_tup")?,
        n_mod_since_analyze: 0,
        n_ins_since_vacuum: 0,
        last_vacuum_epoch_secs: None,
        last_autovacuum_epoch_secs: None,
        last_analyze_epoch_secs: None,
        last_autoanalyze_epoch_secs: None,
        vacuum_count: 0,
        autovacuum_count: 0,
        analyze_count: 0,
        autoanalyze_count: 0,
        growth_1h_bytes: None,
        growth_1h_pct: None,
        is_partition: false,
        parent_oid: None,
        partition_count: Some(row.try_get("partition_count")?),
        // Same fold-in-poller contract as `table_stat_from_row`.
        lock_count: None,
        lock_waiters: None,
        heap_bytes,
        toast_bytes,
        heap_cache_hit_pct,
        idx_cache_hit_pct,
        toast_cache_hit_pct,
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
/// (`"1.8.1"`); anything not starting `major.minor` is `None`. `pub` (not
/// just `pub(crate)`) so the poller can feed the parsed tuple straight into
/// `queries::statements_sql_for_extension` (v0.14) without re-implementing
/// the parse.
pub fn parse_extension_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Maps one row of `queries/statements.sql` (or its sibling extension-tier
/// variants, `statements_ext_1_9.sql`/`statements_ext_1_11.sql` — see that
/// file's header) onto [`StatementRow`]. The queryid arrives already
/// `::text` (JS-safe across the JSON boundary); NULLs collapse per column
/// semantics (`query`/`usename` to `""`, `queryid` kept as `None`).
///
/// `track_io_timing_on` (v0.14) is a per-row scalar (constant across every
/// row of one collection): when the GUC is off, `blk_read_time_ms`/
/// `blk_write_time_ms` read back as `0`, indistinguishable from "no time
/// spent" — so both collapse to `None` in that case rather than shipping a
/// misleading zero (mirrors the checkpointer's optional-timing pattern).
pub fn statement_from_row(row: &Row) -> Result<StatementRow, tokio_postgres::Error> {
    let track_io_timing_on: bool = row.try_get("track_io_timing_on")?;
    let blk_read_time_ms: f64 = row.try_get("blk_read_time_ms")?;
    let blk_write_time_ms: f64 = row.try_get("blk_write_time_ms")?;
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
        shared_blks_dirtied: row.try_get("shared_blks_dirtied")?,
        shared_blks_written: row.try_get("shared_blks_written")?,
        temp_blks_read: row.try_get("temp_blks_read")?,
        temp_blks_written: row.try_get("temp_blks_written")?,
        blk_read_time_ms: track_io_timing_on.then_some(blk_read_time_ms),
        blk_write_time_ms: track_io_timing_on.then_some(blk_write_time_ms),
        wal_bytes: row.try_get("wal_bytes")?,
    })
}

/// Maps the single row of `queries/vacuum_cluster_age.sql` onto
/// [`VacuumClusterAge`] — the cluster-wide XID wraparound headline.
pub fn vacuum_cluster_age_from_row(row: &Row) -> Result<VacuumClusterAge, tokio_postgres::Error> {
    Ok(VacuumClusterAge {
        max_age_xids: row.try_get("max_age_xids")?,
        worst_database: row.try_get("worst_database")?,
    })
}

/// Maps one row of `queries/vacuum_table_ages.sql` onto [`VacuumTableRow`].
pub fn vacuum_table_from_row(row: &Row) -> Result<VacuumTableRow, tokio_postgres::Error> {
    Ok(VacuumTableRow {
        schema: row.try_get("schemaname")?,
        name: row.try_get("relname")?,
        age_xids: row.try_get("age_xids")?,
        n_dead_tup: row.try_get("n_dead_tup")?,
        n_live_tup: row.try_get("n_live_tup")?,
    })
}

/// Maps one row of `queries/indexes.sql` onto
/// [`crate::index_advisor::IndexCatalogRow`]. The raw catalog signature
/// columns (`indkey`/`indclass`/`indcollation`/`indpred`) travel through
/// this struct only as far as `index_advisor::build_index_rows`, which
/// consumes them to compute each row's `IndexFinding` and never re-exposes
/// them.
pub fn index_catalog_from_row(
    row: &Row,
) -> Result<crate::index_advisor::IndexCatalogRow, tokio_postgres::Error> {
    Ok(crate::index_advisor::IndexCatalogRow {
        schema: row.try_get("schemaname")?,
        table: row.try_get("tablename")?,
        name: row.try_get("indexname")?,
        index_bytes: row.try_get("index_bytes")?,
        idx_scan: row.try_get("idx_scan")?,
        idx_tup_read: row.try_get("idx_tup_read")?,
        idx_tup_fetch: row.try_get("idx_tup_fetch")?,
        is_unique: row.try_get("is_unique")?,
        is_primary: row.try_get("is_primary")?,
        is_exclusion: row.try_get("is_exclusion")?,
        is_valid: row.try_get("is_valid")?,
        is_ready: row.try_get("is_ready")?,
        is_constraint: row.try_get("is_constraint")?,
        indexdef: row.try_get("indexdef")?,
        indkey: row.try_get("indkey")?,
        indclass: row.try_get("indclass")?,
        indcollation: row.try_get("indcollation")?,
        indpred: row.try_get("indpred")?,
    })
}

/// Maps the single row of `queries/db_stats_reset.sql` onto its epoch-second
/// value. `None` when the query returns no row (the connected database
/// vanished mid-poll — practically unreachable, but stay defensive rather
/// than `unwrap`).
pub fn db_stats_reset_from_row(row: &Row) -> Result<Option<f64>, tokio_postgres::Error> {
    row.try_get("stats_reset")
}

/// Maps one row of `queries/vacuum_progress.sql` onto [`VacuumProgressRow`].
pub fn vacuum_progress_from_row(row: &Row) -> Result<VacuumProgressRow, tokio_postgres::Error> {
    Ok(VacuumProgressRow {
        pid: row.try_get("pid")?,
        relation: row.try_get("relation")?,
        phase: row.try_get("phase")?,
        heap_blks_total: row.try_get("heap_blks_total")?,
        heap_blks_scanned: row.try_get("heap_blks_scanned")?,
    })
}

/// Maps one row of `queries/databases.sql` onto [`DatabaseRow`] (U2).
pub fn database_from_row(row: &Row) -> Result<DatabaseRow, tokio_postgres::Error> {
    Ok(DatabaseRow {
        name: row.try_get("datname")?,
        size_bytes: row.try_get("size_bytes")?,
    })
}

/// Maps one row of `queries/prepared_xacts.sql` onto [`PreparedXactRow`]
/// (v0.9's orphaned 2PC watch).
pub fn prepared_xact_from_row(row: &Row) -> Result<PreparedXactRow, tokio_postgres::Error> {
    Ok(PreparedXactRow {
        gid: row.try_get("gid")?,
        owner: row.try_get("owner")?,
        database: row.try_get("database")?,
        age_seconds: row.try_get("age_seconds")?,
    })
}

/// Raw row of `queries/lock_capacity.sql` (v0.11's lock-table pressure
/// gauge), before the poller turns it into a [`crate::models::LockCapacity`]
/// (which also carries the derived `capacity_slots`/`used_fraction` —
/// computed once in `poller::collect_lock_capacity` rather than duplicated
/// per-caller).
pub struct LockCapacityRow {
    pub locks_held: i64,
    pub max_locks_per_xact: i64,
    pub max_connections: i64,
    pub max_prepared_xacts: i64,
}

/// Maps the single row of `queries/lock_capacity.sql` onto [`LockCapacityRow`]
/// (v0.11's lock-table pressure gauge).
pub fn lock_capacity_from_row(row: &Row) -> Result<LockCapacityRow, tokio_postgres::Error> {
    Ok(LockCapacityRow {
        locks_held: row.try_get("locks_held")?,
        max_locks_per_xact: row.try_get("max_locks_per_xact")?,
        max_connections: row.try_get("max_connections")?,
        max_prepared_xacts: row.try_get("max_prepared_xacts")?,
    })
}

/// Raw row of `queries/locks_by_relation.sql` (v0.15's per-table lock
/// indicator), before the poller folds it onto each [`crate::models::TableStatRow`]
/// by `rel_oid` (`poller::fold_relation_locks`).
pub struct RelationLockRow {
    pub rel_oid: i64,
    pub locks: i64,
    pub waiting: i64,
}

/// Maps one row of `queries/locks_by_relation.sql` onto [`RelationLockRow`].
pub fn relation_lock_from_row(row: &Row) -> Result<RelationLockRow, tokio_postgres::Error> {
    Ok(RelationLockRow {
        rel_oid: row.try_get("rel_oid")?,
        locks: row.try_get("locks")?,
        waiting: row.try_get("waiting")?,
    })
}

/// Maps one row of `queries/idle_sessions.sql` onto [`IdleSessionRow`]
/// (v0.11's idle connection / connection-age census). `client` collapses a
/// NULL address to `"local"`, same convention as `activity_from_row`.
pub fn idle_session_from_row(row: &Row) -> Result<IdleSessionRow, tokio_postgres::Error> {
    Ok(IdleSessionRow {
        pid: row.try_get("pid")?,
        application_name: opt_text(row, "application_name")?,
        database: opt_text(row, "database")?,
        client: row
            .try_get::<_, Option<String>>("client")?
            .unwrap_or_else(|| "local".to_string()),
        username: opt_text(row, "usename")?,
        idle_age_secs: row.try_get("idle_age_seconds")?,
        ssl: row.try_get::<_, Option<bool>>("ssl")?.unwrap_or(false),
        ssl_version: row.try_get("ssl_version")?,
        ssl_cipher: row.try_get("ssl_cipher")?,
    })
}

/// `pg_attribute.attidentity` (a single character) to the friendly label
/// the detail overlay renders instead of the raw catalog letter — `''`
/// (not an identity column) maps to `None`.
fn identity_label(attidentity: &str) -> Option<&'static str> {
    match attidentity {
        "a" => Some("generated always as identity"),
        "d" => Some("generated by default as identity"),
        _ => None,
    }
}

/// Maps one row of `queries/table_detail_columns.sql` onto
/// [`TableDetailColumn`] (v0.15's on-demand `\d`-style table detail).
/// `default_expr` is `pg_get_expr`'s own `text` output — NULL (no default)
/// stays `None`, never a made-up empty string. `identity`/`generated` are
/// mapped to friendly labels here — see the SQL file's header for why a
/// `GENERATED ... AS IDENTITY` column has no `pg_attrdef` row (so `default`
/// stays `None` for those) while `GENERATED ... AS (...) STORED` DOES (so
/// `default` carries the stored generation expression, flagged by
/// `generated_stored`).
pub fn table_detail_column_from_row(row: &Row) -> Result<TableDetailColumn, tokio_postgres::Error> {
    let attidentity: String = row.try_get("identity")?;
    let attgenerated: String = row.try_get("generated")?;
    Ok(TableDetailColumn {
        name: row.try_get("name")?,
        data_type: row.try_get("data_type")?,
        not_null: row.try_get("not_null")?,
        default: row.try_get("default_expr")?,
        identity: identity_label(&attidentity).map(str::to_string),
        generated_stored: attgenerated == "s",
    })
}

/// `pg_constraint.contype` (a single character) to the friendly label the
/// detail overlay renders — never surfaces the raw catalog letter.
fn constraint_kind_label(contype: &str) -> &'static str {
    match contype {
        "p" => "PRIMARY KEY",
        "f" => "FOREIGN KEY",
        "u" => "UNIQUE",
        "c" => "CHECK",
        "x" => "EXCLUDE",
        _ => "OTHER",
    }
}

/// Maps one row of `queries/table_detail_constraints.sql` onto
/// [`TableDetailConstraint`] — the table's own constraints AND the
/// referencing-FK half of the same UNION ALL (see that file's header).
pub fn table_detail_constraint_from_row(
    row: &Row,
) -> Result<TableDetailConstraint, tokio_postgres::Error> {
    let contype: String = row.try_get("contype")?;
    Ok(TableDetailConstraint {
        name: row.try_get("name")?,
        kind: constraint_kind_label(&contype).to_string(),
        definition: row.try_get("definition")?,
        referencing_table: row.try_get("referencing_table")?,
    })
}

/// Maps one row of `queries/table_detail_indexdefs.sql` onto
/// [`TableDetailIndex`].
pub fn table_detail_index_from_row(row: &Row) -> Result<TableDetailIndex, tokio_postgres::Error> {
    Ok(TableDetailIndex {
        name: row.try_get("name")?,
        definition: row.try_get("definition")?,
    })
}

/// One raw (still-cumulative) row of `queries/io_post_160000.sql` (v0.16,
/// PG 16+): `pg_stat_io` aggregated by `backend_type` x `context`. The
/// poller turns these into [`crate::models::IoStatRow`] by computing
/// per-tick deltas against the previous slow-cadence collection, exactly
/// like `BgwriterRow` feeds `CheckpointerStats` — kept as a separate raw
/// struct (not the model itself) because the model additionally carries
/// derived rates that only the poller (which owns the delta window) can
/// compute.
#[derive(Clone, Debug)]
pub struct IoStatRawRow {
    pub backend_type: String,
    pub context: String,
    pub reads: i64,
    pub writes: i64,
    pub writebacks: i64,
    pub extends: i64,
    pub hits: i64,
    pub evictions: i64,
    pub reuses: i64,
    pub fsyncs: i64,
    pub read_time_ms: f64,
    pub write_time_ms: f64,
    pub track_io_timing_on: bool,
}

/// Maps one row of `queries/io_post_160000.sql` onto [`IoStatRawRow`].
pub fn io_stat_from_row(row: &Row) -> Result<IoStatRawRow, tokio_postgres::Error> {
    Ok(IoStatRawRow {
        backend_type: row.try_get("backend_type")?,
        context: row.try_get("context")?,
        reads: row.try_get("reads")?,
        writes: row.try_get("writes")?,
        writebacks: row.try_get("writebacks")?,
        extends: row.try_get("extends")?,
        hits: row.try_get("hits")?,
        evictions: row.try_get("evictions")?,
        reuses: row.try_get("reuses")?,
        fsyncs: row.try_get("fsyncs")?,
        read_time_ms: row.try_get("read_time_ms")?,
        write_time_ms: row.try_get("write_time_ms")?,
        track_io_timing_on: row.try_get("track_io_timing_on")?,
    })
}

/// One raw (still-cumulative) row of `queries/wal_stats_post_140000.sql`
/// (v0.16, PG 14+ only): the single-row `pg_stat_wal` catalog view. The
/// poller turns this into [`crate::models::WalStats`] by computing per-tick
/// deltas against the previous fast-tick collection, the same
/// raw-then-derive split `BgwriterRow` feeds `CheckpointerStats`.
#[derive(Clone, Debug)]
pub struct WalStatsRawRow {
    pub wal_records: i64,
    pub wal_fpi: i64,
    pub wal_bytes: i64,
    pub wal_buffers_full: i64,
    pub wal_write_time_ms: f64,
    pub wal_sync_time_ms: f64,
    pub track_wal_io_timing_on: bool,
}

/// Maps one row of `queries/wal_stats_post_140000.sql` onto
/// [`WalStatsRawRow`].
pub fn wal_stats_from_row(row: &Row) -> Result<WalStatsRawRow, tokio_postgres::Error> {
    Ok(WalStatsRawRow {
        wal_records: row.try_get("wal_records")?,
        wal_fpi: row.try_get("wal_fpi")?,
        wal_bytes: row.try_get("wal_bytes")?,
        wal_buffers_full: row.try_get("wal_buffers_full")?,
        wal_write_time_ms: row.try_get("wal_write_time_ms")?,
        wal_sync_time_ms: row.try_get("wal_sync_time_ms")?,
        track_wal_io_timing_on: row.try_get("track_wal_io_timing_on")?,
    })
}

/// Maps one row of `queries/sequences.sql` onto [`SequenceRow`].
pub fn sequence_from_row(row: &Row) -> Result<SequenceRow, tokio_postgres::Error> {
    let schema: String = row.try_get("schemaname")?;
    let sequence_name: String = row.try_get("sequencename")?;
    let data_type: String = row.try_get("data_type")?;
    let start_value: i64 = row.try_get("start_value")?;
    let min_value: i64 = row.try_get("min_value")?;
    let max_value: i64 = row.try_get("max_value")?;
    let increment_by: i64 = row.try_get("increment_by")?;
    let cycle: bool = row.try_get("cycle")?;
    let last_value: Option<i64> = row.try_get("last_value")?;
    let table_name: String = opt_text(row, "table_name")?;
    let column_name: String = opt_text(row, "column_name")?;

    let (percent_used, remaining_count, severity) =
        calculate_sequence_exhaustion(start_value, min_value, max_value, increment_by, last_value);

    Ok(SequenceRow {
        schema,
        sequence_name,
        data_type,
        start_value,
        min_value,
        max_value,
        increment_by,
        cycle,
        last_value,
        table_name,
        column_name,
        percent_used,
        remaining_count,
        severity,
    })
}

/// Raw row of `pg_stat_slru` (v0.19, `queries/slru_post_130000.sql`).
#[derive(Clone, Debug)]
pub struct SlruRawRow {
    pub name: String,
    pub blks_zeroed: i64,
    pub blks_hit: i64,
    pub blks_read: i64,
    pub blks_written: i64,
    pub blks_exists: i64,
    pub flushes: i64,
    pub truncates: i64,
}

/// Maps one row of `queries/slru_post_130000.sql` onto [`SlruRawRow`].
pub fn slru_from_row(row: &Row) -> Result<SlruRawRow, tokio_postgres::Error> {
    Ok(SlruRawRow {
        name: row.try_get("name")?,
        blks_zeroed: row.try_get("blks_zeroed")?,
        blks_hit: row.try_get("blks_hit")?,
        blks_read: row.try_get("blks_read")?,
        blks_written: row.try_get("blks_written")?,
        blks_exists: row.try_get("blks_exists")?,
        flushes: row.try_get("flushes")?,
        truncates: row.try_get("truncates")?,
    })
}

/// Raw row of `pg_stat_database_conflicts` (v0.19, `queries/replication_conflicts.sql`).
#[derive(Clone, Debug)]
pub struct ConflictsRawRow {
    pub datid: i64,
    pub datname: String,
    pub confl_tablespace: i64,
    pub confl_lock: i64,
    pub confl_snapshot: i64,
    pub confl_bufferpin: i64,
    pub confl_deadlock: i64,
}

/// Maps one row of `queries/replication_conflicts.sql` onto [`ConflictsRawRow`].
pub fn conflicts_from_row(row: &Row) -> Result<ConflictsRawRow, tokio_postgres::Error> {
    Ok(ConflictsRawRow {
        datid: row.try_get("datid")?,
        datname: row.try_get("datname")?,
        confl_tablespace: row.try_get("confl_tablespace")?,
        confl_lock: row.try_get("confl_lock")?,
        confl_snapshot: row.try_get("confl_snapshot")?,
        confl_bufferpin: row.try_get("confl_bufferpin")?,
        confl_deadlock: row.try_get("confl_deadlock")?,
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
    fn constraint_kind_label_maps_every_pg_constraint_contype() {
        assert_eq!(constraint_kind_label("p"), "PRIMARY KEY");
        assert_eq!(constraint_kind_label("f"), "FOREIGN KEY");
        assert_eq!(constraint_kind_label("u"), "UNIQUE");
        assert_eq!(constraint_kind_label("c"), "CHECK");
        assert_eq!(constraint_kind_label("x"), "EXCLUDE");
        assert_eq!(constraint_kind_label("t"), "OTHER");
    }

    #[test]
    fn identity_label_maps_every_pg_attribute_attidentity() {
        assert_eq!(identity_label("a"), Some("generated always as identity"));
        assert_eq!(identity_label("d"), Some("generated by default as identity"));
        assert_eq!(identity_label(""), None, "'' means not an identity column");
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
