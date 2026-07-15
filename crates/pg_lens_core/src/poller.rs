//! Snapshot poller: a background task publishing `Arc<DbSnapshot>` through a
//! `tokio::sync::watch` channel ("last value wins", N consumers).
//!
//! Two flavors share the exact same contract:
//! - [`spawn`] — the real data layer (Fase 3): tokio-postgres session,
//!   versioned queries prepared once, delta-derived metrics, reconnect with
//!   backoff, and errors carried inside the snapshot (`PollerStatus`).
//! - [`spawn_mock`] — fake data for development/e2e without a database.
//!
//! Both take the poll interval as a `watch::Receiver<Duration>`: frontends
//! adjust the cadence live (the TUI's `+`/`-` keys) by sending a new value —
//! no shared mutable state, just a message (Fase 4).
//!
//! Both also own a [`SnapshotHistory`] ring (Fase 4): one [`HistoryPoint`] is
//! pushed per poll — incremental, never rebuilt — and a clone of the ring
//! travels inside every snapshot envelope, so every consumer (TUI sparklines,
//! the future web's charts) sees the exact same series.
//!
//! This module is frontend-agnostic: it knows nothing about terminal
//! libraries or about any frontend's internal message types.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio_postgres::{Client, Statement};

use crate::history::{HistoryPoint, SnapshotHistory, epoch_ms_now};
use crate::models::{
    AdminActionResult, AdminCommand, AdminOutcome, BloatRow, DbSnapshot, PollerStatus,
    ReplicationInfo, SchemaSnapshot, SchemaStatus, ServerVitals, StatementRow, StatementsSnapshot,
    StatementsStatus,
};
use crate::services::{self, PasswordSource};
use crate::{db, queries};

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Default slow cadence of the Schema Lens collection (`--schema-interval`).
pub const SCHEMA_INTERVAL_DEFAULT: Duration = Duration::from_secs(60);
/// Floor for `--schema-interval`: the size/stats queries are too expensive
/// to run more often than this on purpose.
pub const SCHEMA_INTERVAL_MIN: Duration = Duration::from_secs(5);
/// The mock poller refreshes its fake schema every N ticks — short, so the
/// staleness UI can be exercised without waiting a minute in `--mock`.
const MOCK_SCHEMA_EVERY_TICKS: u64 = 5;

/// Everything the slow schema collection needs, owned by the poller task —
/// no Mutex: the force-refresh request arrives as a bumped counter on a
/// `watch` channel (frontends keep the sender; Fase S3 binds it to `R`).
struct SchemaState {
    interval: Duration,
    refresh_rx: watch::Receiver<u64>,
    /// When the last collection *attempt* ran (successes and failures both
    /// arm the timer — a failing schema query must not retry every 2s).
    last_attempt: Option<Instant>,
    /// Last collection (or error envelope), reused by every fast tick in
    /// between at `Arc` cost.
    current: Option<Arc<SchemaSnapshot>>,
}

impl SchemaState {
    fn new(interval: Duration, refresh_rx: watch::Receiver<u64>) -> Self {
        Self {
            interval: interval.max(SCHEMA_INTERVAL_MIN),
            refresh_rx,
            last_attempt: None,
            current: None,
        }
    }

    /// Should this tick collect schema stats, and if so, WITH the heavy
    /// estimated-bloat queries? `Some(true)` on a force refresh (`R`) — full
    /// collection including on-demand bloat; `Some(false)` when the auto
    /// cadence elapsed — table stats only, never the slow bloat queries;
    /// `None` when there is nothing to do. Consumes the pending refresh
    /// signal, if any.
    fn due(&mut self, now: Instant) -> Option<bool> {
        // `has_changed` errs when the sender is gone — no more force
        // refreshes then, the elapsed check still stands.
        let forced = self.refresh_rx.has_changed().unwrap_or(false);
        if forced {
            self.refresh_rx.borrow_and_update();
            return Some(true);
        }
        if cadence_elapsed(self.last_attempt, now, self.interval) {
            return Some(false);
        }
        None
    }

    /// Stores a collection whose table stats succeeded. Partial-failure
    /// semantics (Fase S2): when only the estimated-bloat queries failed,
    /// the fresh tables are stored (with a fresh `collected_at`), the
    /// previous bloat vectors are kept, and the status carries the error —
    /// table stats degrade gracefully instead of vanishing.
    fn store(&mut self, collection: SchemaCollection) {
        let previous = self.current.as_deref();
        let (table_bloat, index_bloat, status) = match collection.bloat {
            // Bloat not requested this cycle (auto tick): carry the last
            // on-demand estimate forward untouched.
            None => (
                previous.map(|p| p.table_bloat.clone()).unwrap_or_default(),
                previous.map(|p| p.index_bloat.clone()).unwrap_or_default(),
                SchemaStatus::Ok,
            ),
            Some(Ok((table_bloat, index_bloat))) => (table_bloat, index_bloat, SchemaStatus::Ok),
            Some(Err(msg)) => (
                previous.map(|p| p.table_bloat.clone()).unwrap_or_default(),
                previous.map(|p| p.index_bloat.clone()).unwrap_or_default(),
                SchemaStatus::Error(format!("estimated-bloat collection failed: {msg}")),
            ),
        };
        self.current = Some(Arc::new(SchemaSnapshot {
            collected_at_epoch_ms: epoch_ms_now(),
            tables: collection.tables,
            table_bloat,
            index_bloat,
            status,
        }));
    }

    /// Stores a failed collection: the last good data (and its original
    /// `collected_at`, so staleness stays honest) is kept, only the status
    /// flips — mirroring the activity pipeline's resilience pattern.
    fn store_error(&mut self, msg: String) {
        let previous = self.current.as_deref();
        self.current = Some(Arc::new(SchemaSnapshot {
            collected_at_epoch_ms: previous.map_or_else(epoch_ms_now, |p| p.collected_at_epoch_ms),
            tables: previous.map(|p| p.tables.clone()).unwrap_or_default(),
            table_bloat: previous.map(|p| p.table_bloat.clone()).unwrap_or_default(),
            index_bloat: previous.map(|p| p.index_bloat.clone()).unwrap_or_default(),
            status: SchemaStatus::Error(msg),
        }));
    }
}

/// Query Lens (pg_stat_statements) collection state, owned by the poller
/// task like [`SchemaState`] — but WITHOUT a timer of its own: statements
/// are collected in the same slow tick `SchemaState::due` grants (one shared
/// cadence; `R` force-refreshes both). Survives reconnects like the schema.
struct StatementsState {
    /// Last collection (or unavailable/error envelope), reused by every
    /// fast tick in between at `Arc` cost.
    current: Option<Arc<StatementsSnapshot>>,
}

impl StatementsState {
    fn new() -> Self {
        Self { current: None }
    }

    /// Stores a successful collection.
    fn store(&mut self, statements: Vec<StatementRow>) {
        self.current = Some(Arc::new(StatementsSnapshot {
            collected_at_epoch_ms: epoch_ms_now(),
            statements,
            status: StatementsStatus::Ok,
        }));
    }

    /// Stores the calm per-lens Unavailable state (extension missing / too
    /// old). No last-good data is kept: unavailable means the rows would be
    /// stale forever. Fresh `collected_at` — the decision just re-ran.
    fn store_unavailable(&mut self, reason: String) {
        self.current = Some(Arc::new(StatementsSnapshot {
            collected_at_epoch_ms: epoch_ms_now(),
            statements: Vec::new(),
            status: StatementsStatus::Unavailable(reason),
        }));
    }

    /// Stores a failed collection: last good rows (and their original
    /// `collected_at`, so staleness stays honest) are kept, only the status
    /// flips — mirroring [`SchemaState::store_error`]. Exception: the
    /// tell-tale "shared_preload_libraries" execution error (extension
    /// CREATEd but not preloaded) is really an unavailability, not a fault.
    fn store_error(&mut self, msg: String) {
        if msg.contains("shared_preload_libraries") {
            self.store_unavailable(format!(
                "pg_stat_statements is installed but not loaded \u{2014} add it to \
                 shared_preload_libraries and restart the server ({msg})"
            ));
            return;
        }
        let previous = self.current.as_deref();
        self.current = Some(Arc::new(StatementsSnapshot {
            collected_at_epoch_ms: previous.map_or_else(epoch_ms_now, |p| p.collected_at_epoch_ms),
            statements: previous.map(|p| p.statements.clone()).unwrap_or_default(),
            status: StatementsStatus::Error(msg),
        }));
    }
}

/// The pure elapsed check behind [`SchemaState::due`], factored out so the
/// slow-cadence scheduling is unit-testable without a database.
fn cadence_elapsed(last_attempt: Option<Instant>, now: Instant, interval: Duration) -> bool {
    match last_attempt {
        None => true,
        Some(at) => now.duration_since(at) >= interval,
    }
}

/// Sleeps for the interval currently held by `interval_rx`, waking early if
/// the value changes (so a `+`/`-` keypress takes effect immediately instead
/// of after the old interval elapses). If the interval sender is gone the
/// poller simply keeps the last known cadence.
async fn wait_interval(interval_rx: &mut watch::Receiver<Duration>) {
    let dur = *interval_rx.borrow_and_update();
    tokio::select! {
        _ = tokio::time::sleep(dur) => {}
        changed = interval_rx.changed() => {
            if changed.is_err() {
                // Sender dropped: never resolves again; finish the sleep so
                // this select cannot busy-loop.
                tokio::time::sleep(dur).await;
            }
        }
    }
}

/// Sleeps like [`wait_interval`], but ALSO wakes when an [`AdminCommand`]
/// arrives — the command is returned so the caller executes it and re-polls
/// immediately (a cancelled/terminated row should leave the screen fast).
/// This is the shape the feature spec calls the "poller select restructure":
/// the admin channel is one more branch of the tick sleep, not a new task —
/// the poller stays the only owner of the DB client.
///
/// A closed admin channel (a frontend without admin keys dropped the sender,
/// or never had one) degrades to the plain sleep — no busy loop.
async fn wait_interval_or_admin(
    interval_rx: &mut watch::Receiver<Duration>,
    admin_rx: &mut mpsc::Receiver<AdminCommand>,
) -> Option<AdminCommand> {
    tokio::select! {
        _ = wait_interval(interval_rx) => None,
        cmd = admin_rx.recv() => match cmd {
            Some(cmd) => Some(cmd),
            None => {
                // Sender dropped: `recv` resolves `None` forever; finish a
                // full sleep so this select cannot busy-loop.
                wait_interval(interval_rx).await;
                None
            }
        }
    }
}

/// Maps one executed [`AdminCommand`] + the query result onto the
/// serializable [`AdminActionResult`] stamped into snapshots. Pure — the
/// result-mapping rule is unit-testable without a database.
fn admin_result(cmd: AdminCommand, signalled: Result<bool, String>) -> AdminActionResult {
    AdminActionResult {
        kind: cmd.kind(),
        pid: cmd.pid(),
        outcome: match signalled {
            Ok(acknowledged) => AdminOutcome::Signalled(acknowledged),
            Err(msg) => AdminOutcome::Error(msg),
        },
        at_epoch_ms: epoch_ms_now(),
    }
}

/// Runs the prepared `pg_cancel_backend`/`pg_terminate_backend` statement
/// for `cmd`. Errors (usually privilege) become `AdminOutcome::Error` — an
/// admin failure must never tear the polling session down; a genuinely dead
/// connection will surface on the very next poll anyway.
async fn execute_admin(client: &Client, stmts: &Prepared, cmd: AdminCommand) -> AdminActionResult {
    let stmt = match cmd {
        AdminCommand::CancelBackend(_) => &stmts.cancel_backend,
        AdminCommand::TerminateBackend(_) => &stmts.terminate_backend,
    };
    let signalled = match client.query_one(stmt, &[&cmd.pid()]).await {
        Ok(row) => row.try_get::<_, bool>("is_stopped").map_err(|e| e.to_string()),
        // Prefer the server's own message ("permission denied to ...") over
        // tokio-postgres's generic "db error" wrapper — this text is the
        // frontend's feedback line.
        Err(e) => Err(e
            .as_db_error()
            .map(|db| db.message().to_string())
            .unwrap_or_else(|| e.to_string())),
    };
    admin_result(cmd, signalled)
}

/// Spawns the real poller: connect using `config`, detect the server
/// version, pick
/// the matching [`queries::QuerySet`], prepare the statements **once**, then
/// publish one [`DbSnapshot`] per poll. The cadence is read live from
/// `interval_rx` before every sleep.
///
/// On any connect/query error the poller publishes a snapshot carrying
/// `PollerStatus::Error(..)` while *keeping the last good data*, then
/// reconnects with exponential backoff (1s, 2s, 4s ... max 10s).
///
/// When `password_source` is `Some`, the password is (re-)resolved through
/// it — e.g. running `password_cmd` — before **every** connection attempt,
/// so rotating tokens stay fresh across reconnects. A failing command takes
/// the same resilience path as a DB error: `PollerStatus::Error` + backoff.
///
/// `schema_interval` sets the slow cadence of the Schema Lens collection
/// (floored at [`SCHEMA_INTERVAL_MIN`]); `schema_refresh_rx` is a bumped
/// counter — send any new value to force an immediate collection on the
/// next fast tick (the TUI's `R` key in Fase S3).
///
/// `admin_rx` is the frontend→poller admin channel: one [`AdminCommand`]
/// per cancel/terminate request. The poller (sole owner of the DB client)
/// executes it via prepared statements, stamps the [`AdminActionResult`]
/// into `last_admin_action` on every subsequent snapshot, and re-polls
/// immediately. Frontends without admin actions just drop the sender.
///
/// The channel starts pre-filled with a [`DbSnapshot::connecting`] value.
/// The task ends on its own once every receiver has been dropped.
///
/// # Panics
///
/// Must be called from within a tokio runtime (it calls `tokio::spawn`).
pub fn spawn(
    config: tokio_postgres::Config,
    password_source: Option<PasswordSource>,
    interval_rx: watch::Receiver<Duration>,
    schema_interval: Duration,
    schema_refresh_rx: watch::Receiver<u64>,
    admin_rx: mpsc::Receiver<AdminCommand>,
) -> watch::Receiver<Arc<DbSnapshot>> {
    let (tx, rx) = watch::channel(Arc::new(DbSnapshot::connecting()));
    let schema = SchemaState::new(schema_interval, schema_refresh_rx);
    tokio::spawn(run(
        config,
        password_source,
        interval_rx,
        schema,
        admin_rx,
        tx,
    ));
    rx
}

/// Outer reconnect loop: one [`session`] per connection, backoff in between.
async fn run(
    config: tokio_postgres::Config,
    password_source: Option<PasswordSource>,
    mut interval_rx: watch::Receiver<Duration>,
    mut schema: SchemaState,
    mut admin_rx: mpsc::Receiver<AdminCommand>,
    tx: watch::Sender<Arc<DbSnapshot>>,
) {
    let mut backoff = BACKOFF_INITIAL;
    // Survives reconnects so the sparklines don't reset on a blip.
    // (`schema` too: the last collection outlives a connection blip.
    // `last_admin` likewise: the result banner must not vanish on a blip.)
    let mut history = SnapshotHistory::default();
    // Statements share the schema's slow cadence and, like it, outlive a
    // connection blip (the last collection stays on screen).
    let mut statements = StatementsState::new();
    let mut last_admin: Option<AdminActionResult> = None;
    loop {
        let mut polled_ok = false;
        let end = session(
            &config,
            password_source.as_ref(),
            &mut interval_rx,
            &tx,
            &mut history,
            &mut schema,
            &mut statements,
            &mut admin_rx,
            &mut last_admin,
            &mut polled_ok,
        )
        .await;
        match end {
            SessionEnd::Closed => return,
            SessionEnd::Error(msg) => {
                if polled_ok {
                    // The session worked before failing: start backoff fresh.
                    backoff = BACKOFF_INITIAL;
                }
                publish_error(&tx, msg);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                if tx.is_closed() {
                    return;
                }
            }
        }
    }
}

/// Why a polling session stopped.
enum SessionEnd {
    /// Every watch receiver is gone — stop polling entirely.
    Closed,
    /// Connect/prepare/query failure — reconnect after backoff.
    Error(String),
}

/// One connection worth of polling; ensures the spawned `Connection` task is
/// stopped on every exit path.
///
/// The password source (when present) is resolved here — once per
/// *connection attempt*, never per tick — so every reconnect re-runs
/// `password_cmd` and picks up rotated credentials.
#[allow(clippy::too_many_arguments)] // poller-internal plumbing, one call site
async fn session(
    config: &tokio_postgres::Config,
    password_source: Option<&PasswordSource>,
    interval_rx: &mut watch::Receiver<Duration>,
    tx: &watch::Sender<Arc<DbSnapshot>>,
    history: &mut SnapshotHistory,
    schema: &mut SchemaState,
    statements: &mut StatementsState,
    admin_rx: &mut mpsc::Receiver<AdminCommand>,
    last_admin: &mut Option<AdminActionResult>,
    polled_ok: &mut bool,
) -> SessionEnd {
    // The base config is never mutated: the resolved password goes into a
    // per-attempt clone (and is dropped with it).
    let mut config = config.clone();
    if let Some(PasswordSource::Command(cmd)) = password_source {
        match services::resolve_password_cmd(cmd).await {
            Ok(password) => {
                config.password(password);
            }
            // The error carries at most a stderr excerpt — never stdout.
            Err(e) => return SessionEnd::Error(e.to_string()),
        }
    }
    let (client, conn_handle) = match db::connect(&config).await {
        Ok(pair) => pair,
        Err(e) => return SessionEnd::Error(format!("connect failed: {e}")),
    };
    let end = poll_loop(
        &client,
        interval_rx,
        tx,
        history,
        schema,
        statements,
        admin_rx,
        last_admin,
        polled_ok,
    )
    .await;
    conn_handle.abort();
    end
}

/// The statements of a session, prepared once (never per tick). The first
/// three run every fast tick; `table_stats` and the two estimated-bloat
/// statements only on the slow cadence; the two admin statements only when
/// an [`AdminCommand`] arrives.
struct Prepared {
    activity: Statement,
    blocking: Statement,
    server_info: Statement,
    table_stats: Statement,
    bloat_tables: Statement,
    bloat_indexes: Statement,
    cancel_backend: Statement,
    terminate_backend: Statement,
    replication: Statement,
    wal_receiver: Statement,
}

#[allow(clippy::too_many_arguments)] // poller-internal plumbing, one call site
async fn poll_loop(
    client: &Client,
    interval_rx: &mut watch::Receiver<Duration>,
    tx: &watch::Sender<Arc<DbSnapshot>>,
    history: &mut SnapshotHistory,
    schema: &mut SchemaState,
    statements: &mut StatementsState,
    admin_rx: &mut mpsc::Receiver<AdminCommand>,
    last_admin: &mut Option<AdminActionResult>,
    polled_ok: &mut bool,
) -> SessionEnd {
    let version_num = match db::server_version_num(client).await {
        Ok(v) => v,
        Err(e) => return SessionEnd::Error(format!("version detection failed: {e}")),
    };
    let query_set = match queries::for_version(version_num) {
        Ok(q) => q,
        Err(msg) => return SessionEnd::Error(msg),
    };
    let stmts = match tokio::try_join!(
        client.prepare(query_set.activity),
        client.prepare(query_set.blocking),
        client.prepare(query_set.server_info),
        client.prepare(query_set.table_stats),
        client.prepare(query_set.bloat_tables),
        client.prepare(query_set.bloat_indexes),
        client.prepare(query_set.cancel_backend),
        client.prepare(query_set.terminate_backend),
        client.prepare(query_set.replication),
        client.prepare(query_set.wal_receiver),
    ) {
        Ok((
            activity,
            blocking,
            server_info,
            table_stats,
            bloat_tables,
            bloat_indexes,
            cancel_backend,
            terminate_backend,
            replication,
            wal_receiver,
        )) => Prepared {
            activity,
            blocking,
            server_info,
            table_stats,
            bloat_tables,
            bloat_indexes,
            cancel_backend,
            terminate_backend,
            replication,
            wal_receiver,
        },
        Err(e) => return SessionEnd::Error(format!("prepare failed: {e}")),
    };

    // Query Lens availability, decided once per SESSION (so every reconnect
    // re-checks it): the statements query is only prepared when the
    // extension is installed at >= 1.8. Unavailability is a calm per-lens
    // state, never a session error — activity polling continues untouched.
    let statements_stmt: Result<Statement, String> =
        match db::statements_extension_version(client).await {
            Ok(extversion) => match db::statements_availability(extversion.as_deref()) {
                Ok(()) => match client.prepare(query_set.statements).await {
                    Ok(stmt) => Ok(stmt),
                    Err(e) => Err(format!("failed to prepare the pg_stat_statements query: {e}")),
                },
                Err(reason) => Err(reason),
            },
            Err(e) => Err(format!("pg_stat_statements detection failed: {e}")),
        };

    let mut deltas: Option<DeltaState> = None;

    // First iteration polls immediately (right after connecting); every
    // later one sleeps for the *current* interval first.
    loop {
        if tx.is_closed() {
            return SessionEnd::Closed;
        }
        // Publish the fast tick FIRST so the dashboard appears immediately on
        // connect — the slow schema/statements collection never blocks the
        // first (or any) snapshot; its result rides the NEXT tick.
        let mut snapshot = match poll_once(client, &stmts, &mut deltas, history).await {
            Ok(s) => s,
            Err(msg) => return SessionEnd::Error(format!("poll failed: {msg}")),
        };
        // Ticks between collections reuse the last one at Arc-clone cost.
        snapshot.schema = schema.current.clone();
        snapshot.statements = statements.current.clone();
        // The most recent admin result rides in every envelope from then
        // on; frontends dedupe on its `at_epoch_ms`.
        snapshot.last_admin_action = last_admin.clone();
        *polled_ok = true;
        if tx.send(Arc::new(snapshot)).is_err() {
            return SessionEnd::Closed;
        }

        // Slow collection AFTER the fast snapshot is out. `due` returns
        // Some(with_bloat): table stats refresh on the auto cadence
        // (with_bloat = false), while the heavy estimated-bloat queries run
        // ONLY on an explicit force refresh (`R`, with_bloat = true) — they
        // are too slow to run automatically. A failed collection stays inside
        // SchemaStatus (activity intact) and re-arms the timer.
        let now = Instant::now();
        if let Some(with_bloat) = schema.due(now) {
            schema.last_attempt = Some(now);
            match collect_schema(client, &stmts, with_bloat).await {
                Ok(collection) => schema.store(collection),
                Err(msg) => schema.store_error(format!("schema collection failed: {msg}")),
            }
            // Statements share the SAME slow tick — no third timer. The
            // unavailable decision was made at session start; a failing
            // query keeps the last good rows (status carries the error).
            match &statements_stmt {
                Ok(stmt) => match collect_statements(client, stmt).await {
                    Ok(rows) => statements.store(rows),
                    Err(msg) => {
                        statements.store_error(format!("statements collection failed: {msg}"));
                    }
                },
                Err(reason) => statements.store_unavailable(reason.clone()),
            }
        }
        // The tick sleep doubles as the admin-command listener: a command
        // wakes it, executes on this session's prepared statements, and
        // skips the rest of the sleep so the next poll (and the snapshot
        // carrying the result) happens immediately.
        if let Some(cmd) = wait_interval_or_admin(interval_rx, admin_rx).await {
            *last_admin = Some(execute_admin(client, &stmts, cmd).await);
        }
    }
}

/// Cumulative counters from the previous tick — the basis for the derived
/// deltas (the plan mandates computing them in the poller, not in SQL).
struct DeltaState {
    at: Instant,
    xact_total: i64,
    blks_hit: i64,
    blks_read: i64,
    cache_hit_ratio: f64,
}

async fn poll_once(
    client: &Client,
    stmts: &Prepared,
    deltas: &mut Option<DeltaState>,
    history: &mut SnapshotHistory,
) -> Result<DbSnapshot, String> {
    // Five futures on one client: tokio-postgres pipelines them. The two
    // replication queries are a few rows each — one is empty depending on the
    // server's role, and `is_in_recovery` (from server_info) picks which side
    // to present.
    let (activity_rows, blocking_rows, info_rows, sender_rows, receiver_rows) = tokio::try_join!(
        client.query(&stmts.activity, &[]),
        client.query(&stmts.blocking, &[]),
        client.query(&stmts.server_info, &[]),
        client.query(&stmts.replication, &[]),
        client.query(&stmts.wal_receiver, &[]),
    )
    .map_err(|e| e.to_string())?;

    let mut activity = Vec::with_capacity(activity_rows.len());
    for row in &activity_rows {
        activity.push(db::activity_from_row(row).map_err(|e| e.to_string())?);
    }
    let mut locks = Vec::with_capacity(blocking_rows.len());
    for row in &blocking_rows {
        locks.push(db::lock_from_row(row).map_err(|e| e.to_string())?);
    }
    let info_row = info_rows
        .first()
        .ok_or_else(|| "server info query returned no rows".to_string())?;
    let info = db::server_info_from_row(info_row).map_err(|e| e.to_string())?;

    // Role decides which replication view to present. A primary lists its
    // connected replicas; a standby shows its single WAL receiver.
    let replication = if info.is_in_recovery {
        let receiver = match receiver_rows.first() {
            Some(row) => Some(db::wal_receiver_from_row(row).map_err(|e| e.to_string())?),
            None => None,
        };
        Some(ReplicationInfo::Standby { receiver })
    } else {
        let mut senders = Vec::with_capacity(sender_rows.len());
        for row in &sender_rows {
            senders.push(db::wal_sender_from_row(row).map_err(|e| e.to_string())?);
        }
        Some(ReplicationInfo::Primary { senders })
    };

    let now = Instant::now();
    let xact_total = info.xact_commit + info.xact_rollback;
    let cumulative_ratio = hit_ratio(info.blks_hit, info.blks_read);
    let (tps, cache_hit_ratio) = match deltas.as_ref() {
        Some(prev) => {
            let dt = now.duration_since(prev.at).as_secs_f64();
            let dx = xact_total - prev.xact_total;
            let tps = if dt > 0.0 && dx >= 0 {
                dx as f64 / dt
            } else {
                0.0
            };
            let dh = info.blks_hit - prev.blks_hit;
            let dr = info.blks_read - prev.blks_read;
            let ratio = if dh >= 0 && dr >= 0 {
                if dh + dr > 0 {
                    dh as f64 / (dh + dr) as f64
                } else {
                    // No block activity this tick: carry the last reading.
                    prev.cache_hit_ratio
                }
            } else {
                // Stats reset (pg_stat_reset / crash): back to cumulative.
                cumulative_ratio
            };
            (tps, ratio)
        }
        // First poll of a session: no delta window yet — cumulative ratio,
        // no TPS reading (plan: acceptable for the first snapshot).
        None => (0.0, cumulative_ratio),
    };
    *deltas = Some(DeltaState {
        at: now,
        xact_total,
        blks_hit: info.blks_hit,
        blks_read: info.blks_read,
        cache_hit_ratio,
    });

    // One incremental push per poll — the ring is never rebuilt.
    history.push(HistoryPoint {
        epoch_ms: epoch_ms_now(),
        tps: tps.max(0.0),
        active_sessions: info.active.max(0) as u32,
    });

    let vitals = ServerVitals {
        server_version: info.server_version,
        database: info.database,
        uptime_secs: info.uptime_secs.max(0.0) as u64,
        connections_total: info.connections_total.max(0) as u32,
        max_connections: info.max_connections.max(0) as u32,
        active: info.active.max(0) as u32,
        idle: info.idle.max(0) as u32,
        idle_in_transaction: info.idle_in_transaction.max(0) as u32,
        waiting: info.waiting.max(0) as u32,
        tps,
        cache_hit_ratio,
        tup_returned: info.tup_returned,
        tup_fetched: info.tup_fetched,
        temp_files: info.temp_files,
        temp_bytes: info.temp_bytes,
        deadlocks: info.deadlocks,
    };

    Ok(DbSnapshot {
        vitals,
        activity,
        locks,
        history: history.clone(),
        // All three stamped by the caller from poller-owned state.
        schema: None,
        statements: None,
        last_admin_action: None,
        replication,
        status: PollerStatus::Ok,
    })
}

/// One slow-cadence collection: table stats plus the two estimated-bloat
/// row sets. The bloat part is a `Result` of its own — a failing bloat
/// query must not take the table stats down with it (see
/// [`SchemaState::store`] for the partial-failure semantics).
/// Estimated table + index bloat from one on-demand collection, or the error
/// that collection hit.
type BloatEstimate = Result<(Vec<BloatRow>, Vec<BloatRow>), String>;

struct SchemaCollection {
    tables: Vec<crate::models::TableStatRow>,
    /// `None` when bloat was not requested this cycle (auto tick — keep the
    /// last on-demand estimate); `Some(_)` when a force refresh asked for a
    /// fresh estimate.
    bloat: Option<BloatEstimate>,
}

/// Runs the (slow) table-stats query, plus the HEAVY estimated-bloat queries
/// only when `with_bloat` (an explicit force refresh). Bloat never runs on
/// the auto cadence — it is too slow to hold up the dashboard, so it is
/// on-demand. Called from the tick loop only when the schema cadence grants.
async fn collect_schema(
    client: &Client,
    stmts: &Prepared,
    with_bloat: bool,
) -> Result<SchemaCollection, String> {
    // Table stats are the collection's backbone: their failure fails it all.
    let table_rows = client
        .query(&stmts.table_stats, &[])
        .await
        .map_err(|e| e.to_string())?;
    let mut tables = Vec::with_capacity(table_rows.len());
    for row in &table_rows {
        tables.push(db::table_stat_from_row(row).map_err(|e| e.to_string())?);
    }
    let bloat = if with_bloat {
        let (bloat_table_rows, bloat_index_rows) = tokio::join!(
            client.query(&stmts.bloat_tables, &[]),
            client.query(&stmts.bloat_indexes, &[]),
        );
        Some(
            bloat_table_rows
                .map_err(|e| e.to_string())
                .and_then(|rows| bloat_rows(&rows))
                .and_then(|table_bloat| {
                    let index_bloat = bloat_index_rows
                        .map_err(|e| e.to_string())
                        .and_then(|rows| bloat_rows(&rows))?;
                    Ok((table_bloat, index_bloat))
                }),
        )
    } else {
        None
    };
    Ok(SchemaCollection { tables, bloat })
}

/// Parses one estimated-bloat result set (tables and indexes share the
/// exact same output shape).
fn bloat_rows(rows: &[tokio_postgres::Row]) -> Result<Vec<BloatRow>, String> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(db::bloat_from_row(row).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Runs the (slow) statements query — only reachable when the extension was
/// detected as available at session start, and only on the schema cadence.
async fn collect_statements(
    client: &Client,
    stmt: &Statement,
) -> Result<Vec<StatementRow>, String> {
    let rows = client.query(stmt, &[]).await.map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(db::statement_from_row(row).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn hit_ratio(hit: i64, read: i64) -> f64 {
    let total = hit + read;
    if total > 0 { hit as f64 / total as f64 } else { 0.0 }
}

/// Re-publishes the last snapshot with `status = Error(msg)`: frontends show
/// a banner while keeping the last good data on screen.
fn publish_error(tx: &watch::Sender<Arc<DbSnapshot>>, msg: String) {
    let mut snapshot: DbSnapshot = tx.borrow().as_ref().clone();
    snapshot.status = PollerStatus::Error(msg);
    let _ = tx.send(Arc::new(snapshot));
}

/// Stamps one [`HistoryPoint`] derived from `snapshot` onto `history`, then
/// clones the ring into the envelope — the same ownership rule as the real
/// poller (shared by the mock).
fn record_history(history: &mut SnapshotHistory, snapshot: &mut DbSnapshot) {
    history.push(HistoryPoint {
        epoch_ms: epoch_ms_now(),
        tps: snapshot.vitals.tps.max(0.0),
        active_sessions: snapshot.vitals.active,
    });
    snapshot.history = history.clone();
}

/// Spawns a task that publishes a fresh [`DbSnapshot::mock`] per interval
/// (read live from `interval_rx`, like the real poller), and returns the
/// receiving side of the watch channel.
///
/// The channel starts pre-filled with one snapshot (already carrying one
/// history point), so consumers can render immediately with
/// `Receiver::borrow` before the first `changed()` fires.
///
/// Like the real poller, the mock schema collection runs on its own slow
/// cadence — every [`MOCK_SCHEMA_EVERY_TICKS`] ticks (short on purpose, so
/// `--mock` exercises the staleness UI) or when `schema_refresh_rx` bumps —
/// and the ticks in between reuse the same `Arc<SchemaSnapshot>`.
///
/// The task ends on its own once every receiver (including clones) has been
/// dropped — `watch::Sender::send` returns `Err` when the channel is closed.
///
/// Admin commands (`admin_rx`) are simulated: every command "succeeds"
/// (`Signalled(true)`), a terminated pid disappears from subsequent
/// snapshots (activity and locks), a cancelled pid goes `idle`, and the
/// result is stamped like the real poller's — so the whole TUI flow is
/// demoable/e2e-testable without a database.
///
/// # Panics
///
/// Must be called from within a tokio runtime (it calls `tokio::spawn`).
pub fn spawn_mock(
    mut interval_rx: watch::Receiver<Duration>,
    mut schema_refresh_rx: watch::Receiver<u64>,
    mut admin_rx: mpsc::Receiver<AdminCommand>,
) -> watch::Receiver<Arc<DbSnapshot>> {
    // The mock poller owns the ring exactly like the real one.
    let mut history = SnapshotHistory::default();
    let mut first = DbSnapshot::mock();
    record_history(&mut history, &mut first);
    // DbSnapshot::mock() builds a fresh schema (and statements) per call;
    // the poller retains one of each and re-stamps them so the shared slow
    // cadence is observable (same Arc between slow ticks).
    let mut schema = first.schema.clone();
    let mut statements = first.statements.clone();
    let (tx, rx) = watch::channel(Arc::new(first));

    tokio::spawn(async move {
        let mut ticks: u64 = 0;
        let mut cancelled: std::collections::HashSet<i32> = std::collections::HashSet::new();
        let mut terminated: std::collections::HashSet<i32> = std::collections::HashSet::new();
        let mut last_admin: Option<AdminActionResult> = None;
        loop {
            // Same select shape as the real poller: an admin command wakes
            // the sleep, is applied, and the re-publish happens immediately.
            if let Some(cmd) = wait_interval_or_admin(&mut interval_rx, &mut admin_rx).await {
                match cmd {
                    AdminCommand::CancelBackend(pid) => cancelled.insert(pid),
                    AdminCommand::TerminateBackend(pid) => terminated.insert(pid),
                };
                last_admin = Some(admin_result(cmd, Ok(true)));
            }
            ticks += 1;
            let mut snapshot = DbSnapshot::mock();
            apply_mock_admin(&mut snapshot, &cancelled, &terminated);
            snapshot.last_admin_action = last_admin.clone();
            record_history(&mut history, &mut snapshot);
            let forced = schema_refresh_rx.has_changed().unwrap_or(false);
            if forced {
                schema_refresh_rx.borrow_and_update();
            }
            if forced || ticks.is_multiple_of(MOCK_SCHEMA_EVERY_TICKS) {
                schema = snapshot.schema.clone(); // fresh collection
                statements = snapshot.statements.clone(); // same shared tick
            } else {
                snapshot.schema = schema.clone(); // reuse, like real ticks
                snapshot.statements = statements.clone();
            }
            if tx.send(Arc::new(snapshot)).is_err() {
                // All receivers dropped: nobody is watching, stop polling.
                break;
            }
        }
    });

    rx
}

/// Applies simulated admin outcomes onto a fresh mock snapshot: terminated
/// pids vanish (activity + locks, as if the connection died); cancelled
/// pids stay connected but their query is gone (state `idle`, no wait, zero
/// duration) — mirroring what a real cancel/terminate does to
/// `pg_stat_activity`.
fn apply_mock_admin(
    snapshot: &mut DbSnapshot,
    cancelled: &std::collections::HashSet<i32>,
    terminated: &std::collections::HashSet<i32>,
) {
    snapshot.activity.retain(|row| !terminated.contains(&row.pid));
    snapshot
        .locks
        .retain(|lock| !terminated.contains(&lock.pid) && !cancelled.contains(&lock.pid));
    for row in &mut snapshot.activity {
        if cancelled.contains(&row.pid) {
            row.state = "idle".to_string();
            row.wait_event = None;
            row.duration_secs = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interval_rx(ms: u64) -> watch::Receiver<Duration> {
        let (tx, rx) = watch::channel(Duration::from_millis(ms));
        // Leak the sender so the interval stays adjustable-shaped but fixed;
        // wait_interval must tolerate a dropped sender anyway (tested below).
        std::mem::forget(tx);
        rx
    }

    fn refresh_rx() -> watch::Receiver<u64> {
        let (tx, rx) = watch::channel(0u64);
        std::mem::forget(tx);
        rx
    }

    /// An admin channel whose sender stays alive (leaked) — the shape every
    /// non-admin test wants: open but silent.
    fn admin_rx() -> mpsc::Receiver<AdminCommand> {
        let (tx, rx) = mpsc::channel(8);
        std::mem::forget(tx);
        rx
    }

    fn spawn_mock_default(ms: u64) -> watch::Receiver<Arc<DbSnapshot>> {
        spawn_mock(interval_rx(ms), refresh_rx(), admin_rx())
    }

    /// The poller must publish at least two snapshots that differ from each
    /// other (and from the initial value) — bounded by a timeout, no sleeps.
    #[tokio::test]
    async fn spawn_mock_publishes_distinct_snapshots() {
        let mut rx = spawn_mock_default(10);

        let initial = serde_json::to_string(&*rx.borrow().clone()).expect("serialize");

        let mut published = Vec::new();
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(2), rx.changed())
                .await
                .expect("poller must publish within 2s")
                .expect("sender must still be alive");
            published
                .push(serde_json::to_string(&*rx.borrow_and_update().clone()).expect("serialize"));
        }

        assert_ne!(published[0], initial);
        assert_ne!(published[1], initial);
        assert_ne!(published[0], published[1]);
    }

    /// The history ring must grow by exactly one point per publish (owned by
    /// the poller, incremental — never rebuilt or reset between envelopes).
    #[tokio::test]
    async fn spawn_mock_grows_history_incrementally() {
        let mut rx = spawn_mock_default(10);

        let first = rx.borrow().clone();
        assert_eq!(first.history.len(), 1, "pre-filled snapshot has one point");

        let mut expected_len = 1;
        for _ in 0..3 {
            tokio::time::timeout(Duration::from_secs(2), rx.changed())
                .await
                .expect("poller must publish within 2s")
                .expect("sender must still be alive");
            expected_len += 1;
            let snap = rx.borrow_and_update().clone();
            assert_eq!(snap.history.len(), expected_len);
            let latest = snap.history.latest().expect("non-empty history");
            assert_eq!(latest.tps, snap.vitals.tps.max(0.0));
            assert_eq!(latest.active_sessions, snap.vitals.active);
        }
    }

    /// Dropping the interval sender must not busy-loop or kill the poller:
    /// it keeps publishing at the last known cadence.
    #[tokio::test]
    async fn mock_poller_survives_dropped_interval_sender() {
        let (interval_tx, interval_rx) = watch::channel(Duration::from_millis(10));
        let mut rx = spawn_mock(interval_rx, refresh_rx(), admin_rx());
        drop(interval_tx);

        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(2), rx.changed())
                .await
                .expect("poller must keep publishing after interval sender drop")
                .expect("sender must still be alive");
            rx.borrow_and_update();
        }
    }

    /// Sending a new interval takes effect (the sleep wakes early).
    #[tokio::test]
    async fn interval_change_wakes_the_poller() {
        let (interval_tx, interval_rx) = watch::channel(Duration::from_secs(3600));
        let mut rx = spawn_mock(interval_rx, refresh_rx(), admin_rx());
        rx.borrow_and_update();

        // With a one-hour interval nothing would arrive in 2s — unless the
        // interval message wakes the sleep.
        interval_tx
            .send(Duration::from_millis(10))
            .expect("poller alive");
        tokio::time::timeout(Duration::from_secs(2), rx.changed())
            .await
            .expect("interval change must wake the poller")
            .expect("sender must still be alive");
    }

    /// An unreachable server must not panic: the real poller publishes an
    /// error snapshot (keeping the channel alive) instead.
    #[tokio::test]
    async fn spawn_real_reports_connect_errors_via_status() {
        // Port 1 on localhost: connection refused, immediately.
        let config: tokio_postgres::Config = "host=127.0.0.1 port=1 user=nobody connect_timeout=1"
            .parse()
            .expect("test DSN must parse");
        let mut rx = spawn(
            config,
            None,
            interval_rx(50),
            SCHEMA_INTERVAL_DEFAULT,
            refresh_rx(),
            admin_rx(),
        );

        assert!(matches!(rx.borrow().status, PollerStatus::Connecting));

        tokio::time::timeout(Duration::from_secs(5), rx.changed())
            .await
            .expect("poller must publish an error snapshot within 5s")
            .expect("sender must still be alive");
        let snapshot = rx.borrow_and_update().clone();
        match &snapshot.status {
            PollerStatus::Error(msg) => assert!(msg.contains("connect failed")),
            other => panic!("expected PollerStatus::Error, got {other:?}"),
        }
        // Last (empty) data is retained, not dropped.
        assert!(snapshot.activity.is_empty());
    }

    /// A failing `password_cmd` must surface as `PollerStatus::Error` (same
    /// resilience path as a DB error) — and the banner text must carry the
    /// command's stderr, never its stdout.
    #[tokio::test]
    async fn failing_password_cmd_reports_error_status_without_stdout_leak() {
        let config: tokio_postgres::Config = "host=127.0.0.1 port=1 user=nobody connect_timeout=1"
            .parse()
            .expect("test DSN must parse");
        let source = PasswordSource::Command(
            "echo topsecret-stdout; echo vault sealed >&2; exit 1".to_string(),
        );
        let mut rx = spawn(
            config,
            Some(source),
            interval_rx(50),
            SCHEMA_INTERVAL_DEFAULT,
            refresh_rx(),
            admin_rx(),
        );

        tokio::time::timeout(Duration::from_secs(5), rx.changed())
            .await
            .expect("poller must publish an error snapshot within 5s")
            .expect("sender must still be alive");
        let snapshot = rx.borrow_and_update().clone();
        match &snapshot.status {
            PollerStatus::Error(msg) => {
                assert!(msg.contains("password_cmd failed"), "got: {msg}");
                assert!(msg.contains("vault sealed"), "stderr must surface: {msg}");
                assert!(
                    !msg.contains("topsecret-stdout"),
                    "stdout must never leak: {msg}"
                );
            }
            other => panic!("expected PollerStatus::Error, got {other:?}"),
        }
    }

    /// The password command must be re-executed on *every* connection
    /// attempt (rotating tokens): each backoff retry appends one line to a
    /// side-effect file, which therefore has to keep growing.
    #[tokio::test]
    async fn password_cmd_is_reexecuted_per_connection_attempt() {
        let marker = tempfile::NamedTempFile::new().expect("temp file");
        let marker_path = marker.path().to_path_buf();
        // Command succeeds (so the flow reaches the connect step, which then
        // fails on port 1 and schedules a retry) while logging each run.
        let cmd = format!("echo ran >> '{}'; echo pw", marker_path.display());

        let config: tokio_postgres::Config = "host=127.0.0.1 port=1 user=nobody connect_timeout=1"
            .parse()
            .expect("test DSN must parse");
        let _rx = spawn(
            config,
            Some(PasswordSource::Command(cmd)),
            interval_rx(50),
            SCHEMA_INTERVAL_DEFAULT,
            refresh_rx(),
            admin_rx(),
        );

        // First attempt at ~0s, second after the 1s backoff. Poll the file
        // (bounded) until it shows at least two executions.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let runs = std::fs::read_to_string(&marker_path)
                .map(|s| s.lines().count())
                .unwrap_or(0);
            if runs >= 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "password_cmd ran only {runs} time(s) in 10s; expected a re-execution per retry"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Deriving TPS/cache hit: the delta math is exercised through
    /// `hit_ratio` here; the end-to-end path needs a live server (verified in
    /// the Fase 3/4 e2e runs against Docker).
    #[test]
    fn hit_ratio_guards_division_by_zero() {
        assert_eq!(hit_ratio(0, 0), 0.0);
        assert_eq!(hit_ratio(90, 10), 0.9);
    }

    // --- Fase S1: slow schema cadence ---------------------------------------

    /// The pure scheduling rule of the slow cadence: first tick collects,
    /// then only once the interval elapsed — never on the ticks in between.
    /// (The real poller's tick loop calls exactly this check; the live-DB
    /// timing evidence is in the Fase S1 verification run.)
    #[test]
    fn cadence_elapsed_gates_the_slow_collection() {
        let interval = Duration::from_secs(60);
        let t0 = Instant::now();
        // Never collected: due immediately.
        assert!(cadence_elapsed(None, t0, interval));
        // 2s fast ticks after a collection at t0: not due for 58s...
        for secs in [2, 30, 58] {
            assert!(!cadence_elapsed(
                Some(t0),
                t0 + Duration::from_secs(secs),
                interval
            ));
        }
        // ...due again at/after the full interval.
        assert!(cadence_elapsed(Some(t0), t0 + interval, interval));
        assert!(cadence_elapsed(
            Some(t0),
            t0 + Duration::from_secs(90),
            interval
        ));
    }

    /// A bumped refresh counter makes the very next tick due regardless of
    /// the elapsed time — and the signal is consumed (one refresh per bump).
    #[test]
    fn force_refresh_signal_makes_the_next_tick_due() {
        let (refresh_tx, refresh_rx) = watch::channel(0u64);
        let mut schema = SchemaState::new(Duration::from_secs(3600), refresh_rx);
        let t0 = Instant::now();
        schema.last_attempt = Some(t0); // just collected: not due for an hour
        assert_eq!(schema.due(t0 + Duration::from_secs(2)), None);

        refresh_tx.send(1).expect("receiver alive");
        assert_eq!(
            schema.due(t0 + Duration::from_secs(4)),
            Some(true),
            "bump forces due WITH bloat (on-demand)"
        );
        assert_eq!(
            schema.due(t0 + Duration::from_secs(6)),
            None,
            "signal consumed: no refresh loop"
        );
    }

    /// The auto cadence collects table stats only (`Some(false)`), never the
    /// heavy bloat queries — those are on-demand (force refresh → `Some(true)`).
    #[test]
    fn auto_cadence_skips_bloat_force_refresh_includes_it() {
        let (refresh_tx, refresh_rx) = watch::channel(0u64);
        let mut schema = SchemaState::new(Duration::from_secs(60), refresh_rx);
        let t0 = Instant::now();
        // Never collected → auto due, but table-stats only (no bloat).
        assert_eq!(schema.due(t0), Some(false));
        schema.last_attempt = Some(t0);
        // Cadence elapsed → still table-stats only.
        assert_eq!(schema.due(t0 + Duration::from_secs(61)), Some(false));
        // Force refresh → full, including bloat.
        refresh_tx.send(1).expect("receiver alive");
        assert_eq!(schema.due(t0 + Duration::from_secs(62)), Some(true));
    }

    /// An auto tick (`bloat: None`) keeps the last on-demand bloat estimate
    /// while refreshing the table stats.
    #[test]
    fn auto_tick_carries_bloat_forward() {
        let mut schema = SchemaState::new(Duration::from_secs(60), refresh_rx());
        let good = SchemaSnapshot::mock();
        schema.store(SchemaCollection {
            tables: good.tables.clone(),
            bloat: Some(Ok((good.table_bloat.clone(), good.index_bloat.clone()))),
        });
        let mut fresh = good.tables.clone();
        fresh.pop();
        schema.store(SchemaCollection {
            tables: fresh.clone(),
            bloat: None, // auto tick: no bloat this cycle
        });
        let after = schema.current.clone().expect("stored");
        assert_eq!(after.tables.len(), fresh.len(), "fresh tables");
        assert_eq!(
            after.table_bloat.len(),
            good.table_bloat.len(),
            "bloat carried forward"
        );
        assert!(matches!(after.status, SchemaStatus::Ok));
    }

    /// A failed slow collection keeps the last good tables (and their
    /// original collected_at, so staleness stays honest) — only the status
    /// flips to Error. Mirrors the activity pipeline's resilience.
    #[test]
    fn schema_error_keeps_last_good_tables() {
        let mut schema = SchemaState::new(Duration::from_secs(60), refresh_rx());
        let good = SchemaSnapshot::mock();
        schema.store(SchemaCollection {
            tables: good.tables.clone(),
            bloat: Some(Ok((good.table_bloat.clone(), good.index_bloat.clone()))),
        });
        let stored = schema.current.clone().expect("stored");
        let collected_at = stored.collected_at_epoch_ms;

        schema.store_error("permission denied for pg_stat_user_tables".to_string());
        let after = schema.current.clone().expect("still present");
        assert_eq!(after.tables.len(), good.tables.len(), "data kept");
        assert_eq!(after.table_bloat.len(), good.table_bloat.len(), "bloat kept");
        assert_eq!(after.index_bloat.len(), good.index_bloat.len(), "bloat kept");
        assert_eq!(after.collected_at_epoch_ms, collected_at, "staleness honest");
        assert!(matches!(after.status, SchemaStatus::Error(ref m)
            if m.contains("permission denied")));
    }

    /// Fase S2 partial-failure semantics: table stats succeeded but the
    /// estimated-bloat queries failed → fresh tables are stored, the
    /// previous bloat vectors are kept, and the status carries the error.
    #[test]
    fn bloat_failure_keeps_table_stats_and_previous_bloat() {
        let mut schema = SchemaState::new(Duration::from_secs(60), refresh_rx());
        let good = SchemaSnapshot::mock();
        schema.store(SchemaCollection {
            tables: good.tables.clone(),
            bloat: Some(Ok((good.table_bloat.clone(), good.index_bloat.clone()))),
        });

        let mut fresh_tables = good.tables.clone();
        fresh_tables.pop(); // observably different from the previous set
        schema.store(SchemaCollection {
            tables: fresh_tables.clone(),
            bloat: Some(Err("canceling statement due to statement timeout".to_string())),
        });

        let after = schema.current.clone().expect("stored");
        assert_eq!(after.tables.len(), fresh_tables.len(), "fresh tables win");
        assert_eq!(
            after.table_bloat.len(),
            good.table_bloat.len(),
            "previous table bloat kept"
        );
        assert_eq!(
            after.index_bloat.len(),
            good.index_bloat.len(),
            "previous index bloat kept"
        );
        assert!(matches!(after.status, SchemaStatus::Error(ref m)
            if m.contains("estimated-bloat collection failed")
            && m.contains("statement timeout")));
    }

    /// The mock poller reuses the SAME Arc<SchemaSnapshot> between slow
    /// collections and only swaps it on its mock cadence (every
    /// MOCK_SCHEMA_EVERY_TICKS ticks) — the observable contract the real
    /// poller shares.
    #[tokio::test]
    async fn spawn_mock_refreshes_schema_on_the_slow_cadence_only() {
        let mut rx = spawn_mock_default(10);
        let initial = rx.borrow().schema.clone().expect("mock carries schema");

        let mut swaps = Vec::new();
        for tick in 1..=(2 * MOCK_SCHEMA_EVERY_TICKS) {
            tokio::time::timeout(Duration::from_secs(2), rx.changed())
                .await
                .expect("poller must publish within 2s")
                .expect("sender alive");
            let schema = rx
                .borrow_and_update()
                .schema
                .clone()
                .expect("every envelope carries schema");
            if !Arc::ptr_eq(&schema, &initial)
                && !swaps.last().is_some_and(|(_, s)| Arc::ptr_eq(s, &schema))
            {
                swaps.push((tick, schema));
            }
        }
        let swap_ticks: Vec<u64> = swaps.iter().map(|(t, _)| *t).collect();
        assert_eq!(
            swap_ticks,
            vec![MOCK_SCHEMA_EVERY_TICKS, 2 * MOCK_SCHEMA_EVERY_TICKS],
            "schema must be rebuilt exactly on the slow cadence, reused otherwise"
        );
    }

    /// Bumping the refresh channel forces the mock poller to rebuild the
    /// schema on the very next tick (S3's `R` key contract).
    #[tokio::test]
    async fn spawn_mock_force_refresh_rebuilds_schema_immediately() {
        let (refresh_tx, refresh_rx) = watch::channel(0u64);
        let mut rx = spawn_mock(interval_rx(10), refresh_rx, admin_rx());
        let initial = rx.borrow().schema.clone().expect("mock carries schema");

        refresh_tx.send(1).expect("poller alive");
        tokio::time::timeout(Duration::from_secs(2), rx.changed())
            .await
            .expect("poller must publish within 2s")
            .expect("sender alive");
        let schema = rx
            .borrow_and_update()
            .schema
            .clone()
            .expect("schema present");
        assert!(
            !Arc::ptr_eq(&schema, &initial),
            "tick 1 would normally reuse the Arc; the bump must swap it"
        );
    }

    // --- Query Lens (pg_stat_statements) --------------------------------------

    /// A failed statements collection keeps the last good rows (and their
    /// original collected_at) — only the status flips. Same resilience
    /// contract as the schema.
    #[test]
    fn statements_error_keeps_last_good_rows() {
        let mut state = StatementsState::new();
        let good = StatementsSnapshot::mock();
        state.store(good.statements.clone());
        let stored = state.current.clone().expect("stored");
        let collected_at = stored.collected_at_epoch_ms;

        state.store_error("permission denied for view pg_stat_statements".to_string());
        let after = state.current.clone().expect("still present");
        assert_eq!(after.statements.len(), good.statements.len(), "data kept");
        assert_eq!(after.collected_at_epoch_ms, collected_at, "staleness honest");
        assert!(matches!(after.status, StatementsStatus::Error(ref m)
            if m.contains("permission denied")));
    }

    /// The tell-tale not-preloaded execution error becomes the calm
    /// Unavailable state (with the preload hint), not an error banner.
    #[test]
    fn statements_preload_error_degrades_to_unavailable() {
        let mut state = StatementsState::new();
        state.store(StatementsSnapshot::mock().statements.clone());
        state.store_error(
            "pg_stat_statements must be loaded via \"shared_preload_libraries\"".to_string(),
        );
        let after = state.current.clone().expect("present");
        assert!(after.statements.is_empty(), "no stale rows behind an explainer");
        assert!(matches!(after.status, StatementsStatus::Unavailable(ref m)
            if m.contains("shared_preload_libraries")));
    }

    #[test]
    fn statements_unavailable_carries_the_reason() {
        let mut state = StatementsState::new();
        state.store_unavailable("extension missing".to_string());
        let current = state.current.clone().expect("present");
        assert!(current.statements.is_empty());
        assert!(current.collected_at_epoch_ms > 0);
        assert!(matches!(current.status, StatementsStatus::Unavailable(ref m)
            if m == "extension missing"));
    }

    /// The mock poller swaps the statements Arc on the SAME slow tick as the
    /// schema (one shared cadence — no third timer) and reuses it otherwise.
    #[tokio::test]
    async fn spawn_mock_refreshes_statements_on_the_shared_schema_cadence() {
        let mut rx = spawn_mock_default(10);
        let initial_schema = rx.borrow().schema.clone().expect("schema");
        let initial_statements = rx.borrow().statements.clone().expect("statements");

        for tick in 1..=MOCK_SCHEMA_EVERY_TICKS {
            tokio::time::timeout(Duration::from_secs(2), rx.changed())
                .await
                .expect("poller must publish within 2s")
                .expect("sender alive");
            let snap = rx.borrow_and_update().clone();
            let schema = snap.schema.clone().expect("schema in every envelope");
            let statements = snap.statements.clone().expect("statements too");
            let schema_swapped = !Arc::ptr_eq(&schema, &initial_schema);
            let statements_swapped = !Arc::ptr_eq(&statements, &initial_statements);
            assert_eq!(
                schema_swapped, statements_swapped,
                "tick {tick}: statements must swap exactly when the schema does"
            );
            if tick < MOCK_SCHEMA_EVERY_TICKS {
                assert!(!statements_swapped, "tick {tick}: reuse between slow ticks");
            } else {
                assert!(statements_swapped, "slow tick swaps both");
            }
        }
    }

    /// The R force-refresh rebuilds statements together with the schema.
    #[tokio::test]
    async fn spawn_mock_force_refresh_rebuilds_statements_with_the_schema() {
        let (refresh_tx, refresh_rx) = watch::channel(0u64);
        let mut rx = spawn_mock(interval_rx(10), refresh_rx, admin_rx());
        let initial = rx.borrow().statements.clone().expect("statements");

        refresh_tx.send(1).expect("poller alive");
        tokio::time::timeout(Duration::from_secs(2), rx.changed())
            .await
            .expect("poller must publish within 2s")
            .expect("sender alive");
        let statements = rx
            .borrow_and_update()
            .statements
            .clone()
            .expect("statements present");
        assert!(
            !Arc::ptr_eq(&statements, &initial),
            "the bump must swap the statements Arc too"
        );
    }

    // --- admin actions (cancel/terminate) ------------------------------------

    /// The pure result-mapping rule: function return → Signalled(bool),
    /// query error → Error(msg), pid/kind preserved, timestamp stamped.
    #[test]
    fn admin_result_maps_outcomes_and_stamps_the_time() {
        let ok = admin_result(AdminCommand::CancelBackend(4977), Ok(true));
        assert_eq!(ok.kind, crate::models::AdminKind::Cancel);
        assert_eq!(ok.pid, 4977);
        assert_eq!(ok.outcome, AdminOutcome::Signalled(true));
        assert!(ok.at_epoch_ms > 0);

        let gone = admin_result(AdminCommand::TerminateBackend(1), Ok(false));
        assert_eq!(gone.kind, crate::models::AdminKind::Terminate);
        assert_eq!(gone.outcome, AdminOutcome::Signalled(false));

        let err = admin_result(
            AdminCommand::TerminateBackend(2),
            Err("permission denied to terminate process".to_string()),
        );
        assert!(matches!(err.outcome, AdminOutcome::Error(ref m)
            if m.contains("permission denied")));
    }

    /// A terminate command wakes the mock poller immediately (1h interval —
    /// only the admin branch can publish within 2s), the result rides in the
    /// snapshot, and the pid is gone from activity AND locks.
    #[tokio::test]
    async fn mock_terminate_wakes_publishes_result_and_removes_the_row() {
        let (admin_tx, admin_rx) = mpsc::channel(8);
        let (interval_tx, interval_rx) = watch::channel(Duration::from_secs(3600));
        let mut rx = spawn_mock(interval_rx, refresh_rx(), admin_rx);
        let initial = rx.borrow_and_update().clone();
        assert!(initial.activity.iter().any(|r| r.pid == 4312), "mock pid");
        assert!(initial.last_admin_action.is_none());

        admin_tx
            .send(AdminCommand::TerminateBackend(4312))
            .await
            .expect("poller alive");
        tokio::time::timeout(Duration::from_secs(2), rx.changed())
            .await
            .expect("admin command must wake the poller for an immediate re-poll")
            .expect("sender alive");
        let snap = rx.borrow_and_update().clone();
        let result = snap.last_admin_action.as_ref().expect("result stamped");
        assert_eq!(result.kind, crate::models::AdminKind::Terminate);
        assert_eq!(result.pid, 4312);
        assert_eq!(result.outcome, AdminOutcome::Signalled(true));
        assert!(
            snap.activity.iter().all(|r| r.pid != 4312),
            "terminated backend must vanish from subsequent snapshots"
        );
        drop(interval_tx);
    }

    /// A cancel command keeps the session connected but idles its query
    /// (and clears its lock wait) — like a real pg_cancel_backend.
    #[tokio::test]
    async fn mock_cancel_idles_the_query_but_keeps_the_session() {
        let (admin_tx, admin_rx) = mpsc::channel(8);
        let mut rx = spawn_mock(interval_rx(10), refresh_rx(), admin_rx);
        rx.borrow_and_update();

        admin_tx
            .send(AdminCommand::CancelBackend(4977))
            .await
            .expect("poller alive");
        // The wake is immediate, but tolerate one in-flight tick racing it.
        let mut cancelled_seen = false;
        for _ in 0..3 {
            tokio::time::timeout(Duration::from_secs(2), rx.changed())
                .await
                .expect("poller publishes")
                .expect("sender alive");
            let snap = rx.borrow_and_update().clone();
            let Some(result) = snap.last_admin_action.as_ref() else {
                continue;
            };
            assert_eq!(result.kind, crate::models::AdminKind::Cancel);
            assert_eq!(result.pid, 4977);
            assert_eq!(result.outcome, AdminOutcome::Signalled(true));
            let row = snap
                .activity
                .iter()
                .find(|r| r.pid == 4977)
                .expect("cancelled session stays connected");
            assert_eq!(row.state, "idle");
            assert!(row.wait_event.is_none());
            assert_eq!(row.duration_secs, 0.0);
            assert!(
                snap.locks.iter().all(|l| l.pid != 4977),
                "cancelled query no longer waits on a lock"
            );
            cancelled_seen = true;
            break;
        }
        assert!(cancelled_seen, "result must surface within a few ticks");
    }

    /// Dropping the admin sender must not busy-loop or kill the poller —
    /// same resilience contract as the interval sender.
    #[tokio::test]
    async fn mock_poller_survives_dropped_admin_sender() {
        let (admin_tx, admin_rx) = mpsc::channel::<AdminCommand>(8);
        let mut rx = spawn_mock(interval_rx(10), refresh_rx(), admin_rx);
        drop(admin_tx);

        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(2), rx.changed())
                .await
                .expect("poller must keep publishing after admin sender drop")
                .expect("sender must still be alive");
            rx.borrow_and_update();
        }
    }
}
