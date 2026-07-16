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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls, Transaction};

use crate::history::{DEFAULT_CAP, HistoryPoint, SnapshotHistory, epoch_ms_now};
use crate::history_store::HistoryStore;
use crate::index_advisor::{self, IndexCatalogRow};
use crate::models::{
    AdminActionResult, AdminCommand, AdminOutcome, BloatRow, CheckpointerStats, DatabaseRow,
    DbSnapshot, IndexRow, PollerStatus, ReplicationInfo, ReplicationSlotRow, SchemaSnapshot,
    SchemaStatus, ServerVitals, StatementRow, StatementsSnapshot, StatementsStatus,
    VacuumClusterAge, VacuumProgressRow, VacuumTableRow,
};
use crate::services::{self, PasswordSource};
use crate::{db, queries};

/// Recomputes the on-disk history file path for a given database name (U2:
/// the poller reconnects to a different `dbname`, and history is
/// per-database — see `SessionEnd::SwitchDatabase`). `None` = derive the
/// path from the base connection's own dbname (the classic, pre-U2
/// behavior, computed the very first time a session starts); `Some(db)` = a
/// switch just happened, so use `db` instead. Injected by the frontend
/// rather than a plain path: `pg_lens_core` never reads `std::env`/XDG
/// itself (see `main.rs::history_file_path`), and only the frontend knows
/// how to derive a state directory.
pub type HistoryPathFn = Arc<dyn Fn(Option<&str>) -> Option<PathBuf> + Send + Sync>;

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
            vacuum_cluster_age: collection.vacuum_cluster_age,
            vacuum_tables: collection.vacuum_tables,
            indexes: collection.indexes,
            stats_reset_epoch_secs: collection.stats_reset_epoch_secs,
            status,
        }));
    }

    /// U2: drops the collection entirely (no "keep the last good data" —
    /// unlike a poll error, a database switch means the OLD collection
    /// describes a different database's objects, and showing it under the
    /// new database's name would be actively misleading). `interval` and
    /// `refresh_rx` are untouched: the cadence and the `R` channel are
    /// connection-scoped, not database-scoped.
    fn reset(&mut self) {
        self.last_attempt = None;
        self.current = None;
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
            vacuum_cluster_age: previous.and_then(|p| p.vacuum_cluster_age.clone()),
            vacuum_tables: previous.map(|p| p.vacuum_tables.clone()).unwrap_or_default(),
            indexes: previous.map(|p| p.indexes.clone()).unwrap_or_default(),
            stats_reset_epoch_secs: previous.and_then(|p| p.stats_reset_epoch_secs),
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

/// Resolves once a shutdown has been requested (`true`), or if the sender is
/// dropped. Returns `()` — the borrowed `watch::Ref` is released before this
/// returns, so the awaiting future stays `Send` (a `Ref` held across an
/// `.await` would not be, breaking `tokio::spawn`).
async fn wait_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    let _ = shutdown_rx.wait_for(|&stop| stop).await;
}

/// Resolves once the frontend requests a different database (U2's database
/// picker, `d` in the TUI) — returns the desired `dbname`. A closed channel
/// (no frontend wired one up, e.g. `--mock`/`serve` today) never resolves
/// again after the one `None`, so the poller keeps its current session
/// indefinitely — same "no busy loop on a dropped sender" contract as
/// [`wait_shutdown`] and [`wait_interval`].
async fn wait_db_switch(db_switch_rx: &mut mpsc::Receiver<String>) -> String {
    match db_switch_rx.recv().await {
        Some(name) => name,
        None => std::future::pending().await,
    }
}

/// Per-statement safety ceiling, applied as `SET LOCAL` at the start of every
/// query transaction. A monitoring tool must never run an unbounded query
/// against a production server: a pathological plan (e.g. the on-demand bloat
/// estimate on a schema with thousands of relations) would otherwise hang the
/// poller's connection forever. `LOCAL` (transaction-scoped) is deliberate —
/// unlike a session `SET`, it survives a transaction-pooling proxy, where the
/// server backend changes between transactions. Generous: fast-tick queries
/// finish in milliseconds; anything that trips it degrades gracefully (bloat
/// and replication are best-effort; an essential query surfaces a clear
/// timeout instead of a silent stall).
const SET_LOCAL_TIMEOUT: &str = "SET LOCAL statement_timeout = 60000";

/// Opens a READ ONLY transaction and applies the per-statement timeout.
///
/// Every read the poller issues runs inside such a transaction, for two
/// reasons that both matter behind a transaction-pooling PgBouncer:
///   * prepare + execute (tokio-postgres's extended protocol) land on the
///     SAME server backend — a bare `client.query` splits them across two
///     implicit transactions, so the pooler can route them to different
///     backends and the second fails with "prepared statement does not
///     exist";
///   * `SET LOCAL statement_timeout` takes hold (a session `SET` would be
///     discarded when the backend is handed back to the pool).
///
/// On a direct connection this also gives the fast tick a single consistent
/// MVCC snapshot across its three queries. The [`Transaction`] rolls back on
/// drop, so any early return leaves the connection clean.
async fn begin_read(client: &mut Client) -> Result<Transaction<'_>, tokio_postgres::Error> {
    let tx = client.build_transaction().read_only(true).start().await?;
    tx.batch_execute(SET_LOCAL_TIMEOUT).await?;
    Ok(tx)
}

/// Opens a read-write transaction (for the admin actions, which call
/// `pg_cancel_backend`/`pg_terminate_backend`) and applies the same timeout.
async fn begin_write(client: &mut Client) -> Result<Transaction<'_>, tokio_postgres::Error> {
    let tx = client.transaction().await?;
    tx.batch_execute(SET_LOCAL_TIMEOUT).await?;
    Ok(tx)
}

/// Runs the `pg_cancel_backend`/`pg_terminate_backend` statement for `cmd`
/// inside a transaction (pooler-safe). Errors (usually privilege) become
/// `AdminOutcome::Error` — an admin failure must never tear the polling
/// session down; a genuinely dead connection will surface on the next poll.
async fn execute_admin(client: &mut Client, q: &queries::QuerySet, cmd: AdminCommand) -> AdminActionResult {
    let sql = match cmd {
        AdminCommand::CancelBackend(_) => q.cancel_backend,
        AdminCommand::TerminateBackend(_) => q.terminate_backend,
    };
    // Map any tokio-postgres error to the server's own message ("permission
    // denied to ...") over its generic "db error" Display — this text is the
    // frontend's feedback line.
    let server_msg = |e: tokio_postgres::Error| {
        e.as_db_error()
            .map(|db| db.message().to_string())
            .unwrap_or_else(|| e.to_string())
    };
    let signalled = async {
        let tx = begin_write(client).await.map_err(server_msg)?;
        let row = tx.query_one(sql, &[&cmd.pid()]).await.map_err(server_msg)?;
        let stopped = row.try_get::<_, bool>("is_stopped").map_err(|e| e.to_string())?;
        tx.commit().await.map_err(server_msg)?;
        Ok(stopped)
    }
    .await;
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
/// `db_switch_rx` is U2's frontend→poller database-switch channel: one
/// dbname per request (the TUI's `d` picker). The poller ends the current
/// session (cancelling any in-flight query, exactly like shutdown) and
/// reconnects with `dbname` swapped in — PostgreSQL cannot switch databases
/// in-session. Per-database state (history, Schema/Query Lens) resets; see
/// [`SessionEnd::SwitchDatabase`]. Frontends without the picker (`--mock`,
/// `serve` today) just drop the sender.
///
/// `history_path_fn`, when present, is called with `None` once at the first
/// session and with `Some(dbname)` on every subsequent database switch, to
/// (re)derive the on-disk history file path for whichever database is now
/// connected (see [`HistoryPathFn`]).
///
/// The channel starts pre-filled with a [`DbSnapshot::connecting`] value.
/// The task ends on its own once every receiver has been dropped.
///
/// # Panics
///
/// Must be called from within a tokio runtime (it calls `tokio::spawn`).
#[allow(clippy::too_many_arguments)] // one call site (spawn_poller)
pub fn spawn(
    config: tokio_postgres::Config,
    password_source: Option<PasswordSource>,
    interval_rx: watch::Receiver<Duration>,
    schema_interval: Duration,
    schema_refresh_rx: watch::Receiver<u64>,
    admin_rx: mpsc::Receiver<AdminCommand>,
    history_path_fn: Option<HistoryPathFn>,
    shutdown_rx: watch::Receiver<bool>,
    db_switch_rx: mpsc::Receiver<String>,
) -> (watch::Receiver<Arc<DbSnapshot>>, JoinHandle<()>) {
    let (tx, rx) = watch::channel(Arc::new(DbSnapshot::connecting()));
    let schema = SchemaState::new(schema_interval, schema_refresh_rx);
    let handle = tokio::spawn(run(
        config,
        password_source,
        interval_rx,
        schema,
        admin_rx,
        tx,
        history_path_fn,
        shutdown_rx,
        db_switch_rx,
    ));
    (rx, handle)
}

/// Outer reconnect loop: one [`session`] per connection, backoff in between.
#[allow(clippy::too_many_arguments)] // poller-internal plumbing, one call site
async fn run(
    mut config: tokio_postgres::Config,
    password_source: Option<PasswordSource>,
    mut interval_rx: watch::Receiver<Duration>,
    mut schema: SchemaState,
    mut admin_rx: mpsc::Receiver<AdminCommand>,
    tx: watch::Sender<Arc<DbSnapshot>>,
    history_path_fn: Option<HistoryPathFn>,
    mut shutdown_rx: watch::Receiver<bool>,
    mut db_switch_rx: mpsc::Receiver<String>,
) {
    let mut backoff = BACKOFF_INITIAL;
    // Survives reconnects so the sparklines don't reset on a blip.
    // (`schema` too: the last collection outlives a connection blip.
    // `last_admin` likewise: the result banner must not vanish on a blip.)
    let mut history = SnapshotHistory::default();
    // Persistence (best-effort): seed the ring from disk so the chart resumes
    // after a restart, and append/compact as new points arrive. `None`
    // (e.g. --mock, or no derivable state dir) simply keeps everything
    // in-memory as before. `None` here (the base config's own dbname) is the
    // classic pre-U2 call; a later database switch recomputes it with
    // `Some(dbname)` instead (see `SessionEnd::SwitchDatabase` below).
    let mut store = history_path_fn
        .as_ref()
        .and_then(|f| f(None))
        .map(HistoryStore::new);
    if let Some(store) = &store {
        for point in store.load(DEFAULT_CAP) {
            history.push(point);
        }
    }
    let mut appends_since_compact: usize = 0;
    // Statements share the schema's slow cadence and, like it, outlive a
    // connection blip (the last collection stays on screen).
    let mut statements = StatementsState::new();
    let mut last_admin: Option<AdminActionResult> = None;
    loop {
        // Shutdown requested (the app is quitting): stop before reconnecting.
        if *shutdown_rx.borrow() {
            return;
        }
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
            store.as_ref(),
            &mut appends_since_compact,
            &mut shutdown_rx,
            &mut db_switch_rx,
        )
        .await;
        match end {
            SessionEnd::Closed => return,
            SessionEnd::SwitchDatabase(name) => {
                // Deliberate user action, not a fault: reconnect immediately
                // (no backoff), with `dbname` swapped in the base config so
                // every future reconnect (including error retries) stays on
                // the newly picked database until the next switch.
                config.dbname(&name);
                // Per-database state must not carry over: the Schema/Query
                // Lens and history all describe THIS database's objects and
                // activity — showing the old database's data under the new
                // name would be actively misleading, not merely stale.
                history = SnapshotHistory::default();
                appends_since_compact = 0;
                schema.reset();
                statements = StatementsState::new();
                last_admin = None;
                store = history_path_fn
                    .as_ref()
                    .and_then(|f| f(Some(&name)))
                    .map(HistoryStore::new);
                if let Some(store) = &store {
                    for point in store.load(DEFAULT_CAP) {
                        history.push(point);
                    }
                }
            }
            SessionEnd::Error(msg) => {
                if polled_ok {
                    // The session worked before failing: start backoff fresh.
                    backoff = BACKOFF_INITIAL;
                }
                publish_error(&tx, msg);
                // Interruptible backoff: a shutdown during the wait ends the
                // poller immediately instead of after the full delay.
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = wait_shutdown(&mut shutdown_rx) => return,
                }
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
    /// U2: the frontend requested a different database — reconnect
    /// immediately (no backoff) with this dbname.
    SwitchDatabase(String),
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
    store: Option<&HistoryStore>,
    appends_since_compact: &mut usize,
    shutdown_rx: &mut watch::Receiver<bool>,
    db_switch_rx: &mut mpsc::Receiver<String>,
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
    let (mut client, conn_handle) = match db::connect(&config).await {
        Ok(pair) => pair,
        Err(e) => return SessionEnd::Error(format!("connect failed: {e}")),
    };
    // Captured up front (owned — no borrow held), so the shutdown/switch
    // branches can cancel whatever query poll_loop has in flight without
    // conflicting with its `&mut client`.
    let cancel = client.cancel_token();
    // Race the poll loop against a shutdown request AND a database-switch
    // request. Either way, send a CancelRequest so a running server-side
    // query (notably the heavy bloat estimate) stops immediately instead of
    // lingering until it finishes or hits statement_timeout — the socket
    // dropping alone does NOT cancel it. `biased`: shutdown wins over a
    // switch request racing it at the exact same instant.
    let end = tokio::select! {
        biased;
        _ = wait_shutdown(shutdown_rx) => {
            let _ = cancel.cancel_query(NoTls).await;
            SessionEnd::Closed
        }
        name = wait_db_switch(db_switch_rx) => {
            let _ = cancel.cancel_query(NoTls).await;
            SessionEnd::SwitchDatabase(name)
        }
        end = poll_loop(
            &mut client,
            interval_rx,
            tx,
            history,
            schema,
            statements,
            admin_rx,
            last_admin,
            polled_ok,
            store,
            appends_since_compact,
        ) => end,
    };
    conn_handle.abort();
    end
}

#[allow(clippy::too_many_arguments)] // poller-internal plumbing, one call site
async fn poll_loop(
    client: &mut Client,
    interval_rx: &mut watch::Receiver<Duration>,
    tx: &watch::Sender<Arc<DbSnapshot>>,
    history: &mut SnapshotHistory,
    schema: &mut SchemaState,
    statements: &mut StatementsState,
    admin_rx: &mut mpsc::Receiver<AdminCommand>,
    last_admin: &mut Option<AdminActionResult>,
    polled_ok: &mut bool,
    store: Option<&HistoryStore>,
    appends_since_compact: &mut usize,
) -> SessionEnd {
    // Identify our session (application_name). Best-effort and session-level:
    // it never blocks the dashboard, and behind a pooler it simply won't
    // persist (harmless — the safety timeout rides SET LOCAL per transaction).
    let _ = db::configure_session(client).await;

    // Session init, in one read transaction (pooler-safe): the server version
    // (which picks the SQL variants) and whether the Query Lens is available.
    // The statements decision is made once per session — so every reconnect
    // re-checks it — and unavailability is a calm per-lens state, never a
    // session error: activity polling continues untouched.
    let (version_num, statements_extversion) = {
        let init_tx = match begin_read(client).await {
            Ok(t) => t,
            Err(e) => return SessionEnd::Error(db_error_message("version detection failed", &e)),
        };
        let version_num = match db::server_version_num(&init_tx).await {
            Ok(v) => v,
            Err(e) => return SessionEnd::Error(db_error_message("version detection failed", &e)),
        };
        let extversion = match db::statements_extension_version(&init_tx).await {
            Ok(v) => Ok(v),
            Err(e) => Err(db_error_message("pg_stat_statements detection failed", &e)),
        };
        (version_num, extversion)
    };
    let query_set = match queries::for_version(version_num) {
        Ok(q) => q,
        Err(msg) => return SessionEnd::Error(msg),
    };
    let statements_available: Result<(), String> = match statements_extversion {
        Ok(extversion) => db::statements_availability(extversion.as_deref()),
        Err(msg) => Err(msg),
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
        let mut snapshot = match poll_once(client, &query_set, &mut deltas, history).await {
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
        // Persist this tick's point (best-effort). Compact every DEFAULT_CAP
        // appends so a long-running session's file stays bounded to ~2× the
        // ring; the rewrite keeps exactly the current ring.
        if let Some(store) = store
            && let Some(point) = history.latest()
        {
            store.append(point);
            *appends_since_compact += 1;
            if *appends_since_compact >= DEFAULT_CAP {
                store.compact(&history.iter().cloned().collect::<Vec<_>>());
                *appends_since_compact = 0;
            }
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
            match collect_schema(client, &query_set, with_bloat).await {
                Ok(collection) => schema.store(collection),
                Err(msg) => schema.store_error(format!("schema collection failed: {msg}")),
            }
            // Statements share the SAME slow tick — no third timer. The
            // unavailable decision was made at session start; a failing
            // query keeps the last good rows (status carries the error).
            match &statements_available {
                Ok(()) => match collect_statements(client, query_set.statements).await {
                    Ok(rows) => statements.store(rows),
                    Err(msg) => {
                        statements.store_error(format!("statements collection failed: {msg}"));
                    }
                },
                Err(reason) => statements.store_unavailable(reason.clone()),
            }
        }
        // The tick sleep doubles as the admin-command listener: a command
        // wakes it, executes inside its own transaction, and skips the rest
        // of the sleep so the next poll (and the snapshot carrying the
        // result) happens immediately.
        if let Some(cmd) = wait_interval_or_admin(interval_rx, admin_rx).await {
            *last_admin = Some(execute_admin(client, &query_set, cmd).await);
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
    checkpointer: CheckpointerDeltaState,
}

/// F4's previous-tick checkpointer/bgwriter counters, plus the
/// session-window baseline for the requested/timed ratio — captured once at
/// the first poll of a session and never updated again unless a stats reset
/// is detected (see [`derive_checkpointer_stats`]). Checkpoints are
/// infrequent, so a per-tick delta would mostly be 0/0 noise; the ratio
/// needs the longer session window instead.
#[derive(Clone, Copy)]
struct CheckpointerDeltaState {
    at: Instant,
    checkpoints_timed: i64,
    checkpoints_req: i64,
    checkpoint_write_time_ms: f64,
    checkpoint_sync_time_ms: f64,
    buffers_checkpoint: i64,
    buffers_clean: i64,
    buffers_backend: Option<i64>,
    session_baseline_timed: i64,
    session_baseline_req: i64,
}

/// Derives F4's per-tick rates + session-window requested/timed ratio from
/// raw cumulative counters. Pure and DB-free (unit-tested directly).
/// `prev` is `None` on the first poll of a session — no delta window yet,
/// same "acceptable first-snapshot gap" rule as `ServerVitals::tps`.
fn derive_checkpointer_stats(
    raw: &db::BgwriterRow,
    now: Instant,
    prev: Option<&CheckpointerDeltaState>,
) -> (CheckpointerStats, CheckpointerDeltaState) {
    // A cumulative counter going backwards means pg_stat_reset() or a server
    // restart — the whole delta window (and the session baseline) restarts
    // from this tick, exactly like TPS falls back to the cumulative ratio.
    let stats_reset = prev.is_some_and(|p| {
        raw.checkpoints_timed < p.checkpoints_timed || raw.checkpoints_req < p.checkpoints_req
    });
    let usable_prev = prev.filter(|_| !stats_reset);

    let (session_baseline_timed, session_baseline_req) = match usable_prev {
        Some(p) => (p.session_baseline_timed, p.session_baseline_req),
        None => (raw.checkpoints_timed, raw.checkpoints_req),
    };

    let mut stats = CheckpointerStats {
        checkpoints_timed: raw.checkpoints_timed,
        checkpoints_req: raw.checkpoints_req,
        checkpoint_write_time_ms: raw.checkpoint_write_time_ms,
        checkpoint_sync_time_ms: raw.checkpoint_sync_time_ms,
        buffers_checkpoint: raw.buffers_checkpoint,
        buffers_clean: raw.buffers_clean,
        maxwritten_clean: raw.maxwritten_clean,
        buffers_backend: raw.buffers_backend,
        buffers_alloc: raw.buffers_alloc,
        checkpoints_per_min_timed: None,
        checkpoints_per_min_req: None,
        buffers_checkpoint_per_sec: None,
        buffers_clean_per_sec: None,
        buffers_backend_per_sec: None,
        avg_checkpoint_write_ms: None,
        avg_checkpoint_sync_ms: None,
        requested_ratio_session: None,
    };

    if let Some(p) = usable_prev {
        let dt = now.duration_since(p.at).as_secs_f64();
        if dt > 0.0 {
            let d_timed = raw.checkpoints_timed - p.checkpoints_timed;
            let d_req = raw.checkpoints_req - p.checkpoints_req;
            stats.checkpoints_per_min_timed = Some(d_timed as f64 / dt * 60.0);
            stats.checkpoints_per_min_req = Some(d_req as f64 / dt * 60.0);

            let d_buf_checkpoint = raw.buffers_checkpoint - p.buffers_checkpoint;
            if d_buf_checkpoint >= 0 {
                stats.buffers_checkpoint_per_sec = Some(d_buf_checkpoint as f64 / dt);
            }
            let d_buf_clean = raw.buffers_clean - p.buffers_clean;
            if d_buf_clean >= 0 {
                stats.buffers_clean_per_sec = Some(d_buf_clean as f64 / dt);
            }
            if let (Some(a), Some(b)) = (raw.buffers_backend, p.buffers_backend) {
                let d = a - b;
                if d >= 0 {
                    stats.buffers_backend_per_sec = Some(d as f64 / dt);
                }
            }

            // Average write/sync time per checkpoint, only over ticks that
            // actually saw one complete — otherwise it would divide by zero.
            let d_checkpoints = d_timed + d_req;
            if d_checkpoints > 0 {
                let d_write_ms = raw.checkpoint_write_time_ms - p.checkpoint_write_time_ms;
                let d_sync_ms = raw.checkpoint_sync_time_ms - p.checkpoint_sync_time_ms;
                if d_write_ms >= 0.0 {
                    stats.avg_checkpoint_write_ms = Some(d_write_ms / d_checkpoints as f64);
                }
                if d_sync_ms >= 0.0 {
                    stats.avg_checkpoint_sync_ms = Some(d_sync_ms / d_checkpoints as f64);
                }
            }
        }
    }

    let sd_timed = raw.checkpoints_timed - session_baseline_timed;
    let sd_req = raw.checkpoints_req - session_baseline_req;
    if sd_timed >= 0 && sd_req >= 0 && sd_timed + sd_req > 0 {
        stats.requested_ratio_session = Some(sd_req as f64 / (sd_timed + sd_req) as f64);
    }

    let next = CheckpointerDeltaState {
        at: now,
        checkpoints_timed: raw.checkpoints_timed,
        checkpoints_req: raw.checkpoints_req,
        checkpoint_write_time_ms: raw.checkpoint_write_time_ms,
        checkpoint_sync_time_ms: raw.checkpoint_sync_time_ms,
        buffers_checkpoint: raw.buffers_checkpoint,
        buffers_clean: raw.buffers_clean,
        buffers_backend: raw.buffers_backend,
        session_baseline_timed,
        session_baseline_req,
    };
    (stats, next)
}

async fn poll_once(
    client: &mut Client,
    q: &queries::QuerySet,
    deltas: &mut Option<DeltaState>,
    history: &mut SnapshotHistory,
) -> Result<DbSnapshot, String> {
    // Essential queries: four futures pipelined inside ONE read-only
    // transaction — a single consistent snapshot, and pooler-safe (prepare +
    // execute stay on the same backend). Their failure is a real fault, so it
    // is fatal to the poll, surfaced with the actual server message
    // (tokio-postgres's raw Display is just "db error"). `bgwriter` (F4) rides
    // here rather than best-effort: pg_stat_bgwriter/checkpointer are
    // readable by everyone, same class as pg_stat_database, and the query is
    // a single cheap catalog row.
    let (activity_rows, blocking_rows, info_rows, bgwriter_rows) = {
        let etx = begin_read(client)
            .await
            .map_err(|e| db_error_message("poll failed", &e))?;
        let rows = tokio::try_join!(
            etx.query(q.activity, &[]),
            etx.query(q.blocking, &[]),
            etx.query(q.server_info, &[]),
            etx.query(q.bgwriter, &[]),
        )
        .map_err(|e| db_error_message("poll failed", &e))?;
        etx.commit()
            .await
            .map_err(|e| db_error_message("poll failed", &e))?;
        rows
    };

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
    let bgwriter_row = bgwriter_rows
        .first()
        .ok_or_else(|| "bgwriter/checkpointer query returned no rows".to_string())?;
    let bgwriter_raw = db::bgwriter_from_row(bgwriter_row).map_err(|e| e.to_string())?;

    // Replication is BEST-EFFORT: a restricted or managed server (RDS,
    // Cloud SQL, …) may forbid the WAL views/functions. A failure here
    // degrades to "no replication panel" — it must NEVER take the whole poll
    // (and the dashboard) down. Only the query for the server's actual role
    // runs.
    let (replication, replication_slots) =
        collect_replication(client, q, info.is_in_recovery).await;

    // Vacuum progress (F2) is likewise best-effort on the fast tick: absent
    // (`None`) on any failure, never a poll fault — see
    // `collect_vacuum_progress`.
    let vacuum_progress = collect_vacuum_progress(client, q).await;

    // Databases (U2) are best-effort on the fast tick too — the picker
    // simply has nothing to show this tick on any failure.
    let databases = collect_databases(client, q).await;

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
    let (checkpointer_stats, checkpointer_delta) = derive_checkpointer_stats(
        &bgwriter_raw,
        now,
        deltas.as_ref().map(|d| &d.checkpointer),
    );

    *deltas = Some(DeltaState {
        at: now,
        xact_total,
        blks_hit: info.blks_hit,
        blks_read: info.blks_read,
        cache_hit_ratio,
        checkpointer: checkpointer_delta,
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
        replication_slots,
        vacuum_progress,
        checkpointer: Some(checkpointer_stats),
        databases,
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
    /// F2 cluster-wide wraparound headline, collected alongside `tables` in
    /// the same essential transaction (a missing row would mean an empty
    /// `pg_database`, which cannot happen).
    vacuum_cluster_age: Option<VacuumClusterAge>,
    /// F2 per-table XID ages, same essential transaction as `tables`.
    vacuum_tables: Vec<VacuumTableRow>,
    /// F3 index advisor rows, same essential transaction as `tables` — a
    /// role that can read `pg_stat_user_tables` can read
    /// `pg_stat_user_indexes`/`pg_index` too, so this fails together with
    /// the rest of the collection (same choice F2's vacuum ages made).
    indexes: Vec<IndexRow>,
    /// F3 freshness header, same transaction as `indexes`.
    stats_reset_epoch_secs: Option<f64>,
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
    client: &mut Client,
    q: &queries::QuerySet,
    with_bloat: bool,
) -> Result<SchemaCollection, String> {
    // One read-only transaction for the whole collection (pooler-safe).
    let stx = begin_read(client).await.map_err(|e| e.to_string())?;
    // Table stats are the collection's backbone: their failure fails it all.
    let table_rows = stx
        .query(q.table_stats, &[])
        .await
        .map_err(|e| e.to_string())?;
    let mut tables = Vec::with_capacity(table_rows.len());
    for row in &table_rows {
        tables.push(db::table_stat_from_row(row).map_err(|e| e.to_string())?);
    }

    // Vacuum health / XID wraparound (F2): cheap catalog reads, run in the
    // same essential transaction as table_stats — their failure fails the
    // whole schema collection just like table_stats (a role that can read
    // pg_stat_user_tables can read pg_database/pg_class too).
    let cluster_age_rows = stx
        .query(q.vacuum_cluster_age, &[])
        .await
        .map_err(|e| e.to_string())?;
    let vacuum_cluster_age = cluster_age_rows
        .first()
        .map(db::vacuum_cluster_age_from_row)
        .transpose()
        .map_err(|e| e.to_string())?;
    let vacuum_table_rows = stx
        .query(q.vacuum_table_ages, &[])
        .await
        .map_err(|e| e.to_string())?;
    let mut vacuum_tables = Vec::with_capacity(vacuum_table_rows.len());
    for row in &vacuum_table_rows {
        vacuum_tables.push(db::vacuum_table_from_row(row).map_err(|e| e.to_string())?);
    }

    // Index advisor (F3): same essential transaction, same fail-together
    // choice as the vacuum-age queries above.
    let index_rows = stx.query(q.indexes, &[]).await.map_err(|e| e.to_string())?;
    let mut index_catalog: Vec<IndexCatalogRow> = Vec::with_capacity(index_rows.len());
    for row in &index_rows {
        index_catalog.push(db::index_catalog_from_row(row).map_err(|e| e.to_string())?);
    }
    let indexes = index_advisor::build_index_rows(index_catalog);
    let stats_reset_rows = stx
        .query(q.db_stats_reset, &[])
        .await
        .map_err(|e| e.to_string())?;
    let stats_reset_epoch_secs = stats_reset_rows
        .first()
        .map(db::db_stats_reset_from_row)
        .transpose()
        .map_err(|e| e.to_string())?
        .flatten();

    let bloat = if with_bloat {
        let (bloat_table_rows, bloat_index_rows) = tokio::join!(
            stx.query(q.bloat_tables, &[]),
            stx.query(q.bloat_indexes, &[]),
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
    // A failed bloat query leaves the transaction aborted, so committing would
    // error — but `tables` were already materialized before it ran, and the
    // bloat error is captured in `bloat: Some(Err(_))` (partial-failure
    // semantics). In that case skip the commit and let the transaction roll
    // back on drop; otherwise commit normally.
    if !matches!(&bloat, Some(Err(_))) {
        stx.commit().await.map_err(|e| e.to_string())?;
    }
    Ok(SchemaCollection {
        tables,
        vacuum_cluster_age,
        vacuum_tables,
        indexes,
        stats_reset_epoch_secs,
        bloat,
    })
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
    client: &mut Client,
    sql: &str,
) -> Result<Vec<StatementRow>, String> {
    let tx = begin_read(client).await.map_err(|e| e.to_string())?;
    let rows = tx.query(sql, &[]).await.map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(db::statement_from_row(row).map_err(|e| e.to_string())?);
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(out)
}

/// Best-effort replication view for the server's current role, PLUS its
/// replication slots (F2.5) — both ride the same read-only transaction as
/// the sender/receiver query. Either half returns `None` independently on
/// its own query/parse failure (replication and slots are each optional: a
/// restricted or managed server may forbid one view but not the other), and
/// neither failure can fail the poll. Only the sender/receiver query
/// matching the role runs; the slots query always runs — slots exist on
/// BOTH a primary and a standby, unlike senders/receiver.
async fn collect_replication(
    client: &mut Client,
    q: &queries::QuerySet,
    is_in_recovery: bool,
) -> (Option<ReplicationInfo>, Option<Vec<ReplicationSlotRow>>) {
    let Ok(tx) = begin_read(client).await else {
        return (None, None);
    };

    let info = if is_in_recovery {
        match tx.query(q.wal_receiver, &[]).await {
            Ok(rows) => {
                let receiver = rows.first().and_then(|r| db::wal_receiver_from_row(r).ok());
                Some(ReplicationInfo::Standby { receiver })
            }
            Err(_) => None,
        }
    } else {
        match tx.query(q.replication, &[]).await {
            Ok(rows) => {
                let senders = rows
                    .iter()
                    .filter_map(|r| db::wal_sender_from_row(r).ok())
                    .collect();
                Some(ReplicationInfo::Primary { senders })
            }
            Err(_) => None,
        }
    };

    let slots = match tx.query(q.replication_slots, &[]).await {
        Ok(rows) => {
            let mut out = Vec::with_capacity(rows.len());
            let mut all_parsed = true;
            for row in &rows {
                match db::replication_slot_from_row(row) {
                    Ok(slot) => out.push(slot),
                    Err(_) => {
                        all_parsed = false;
                        break;
                    }
                }
            }
            if all_parsed { Some(out) } else { None }
        }
        Err(_) => None,
    };

    // Best-effort: a failed commit just means no panels this tick.
    if tx.commit().await.is_err() {
        return (None, None);
    }
    (info, slots)
}

/// Best-effort in-flight vacuum progress (F2, `pg_stat_progress_vacuum`),
/// refreshed every fast tick. Returns `None` on ANY query or parse failure —
/// a restricted role or a server that hides the view degrades to "no panel
/// this tick", exactly like [`collect_replication`]; it must never fail the
/// poll. `Some(vec![])` (the common case) means the collection succeeded and
/// found nothing running — the calm "no vacuum running" state, not an error.
async fn collect_vacuum_progress(
    client: &mut Client,
    q: &queries::QuerySet,
) -> Option<Vec<VacuumProgressRow>> {
    let tx = begin_read(client).await.ok()?;
    let rows = tx.query(q.vacuum_progress, &[]).await.ok()?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(db::vacuum_progress_from_row(row).ok()?);
    }
    // Best-effort: a failed commit just means no panel this tick.
    tx.commit().await.ok()?;
    Some(out)
}

/// Best-effort database list (U2, `queries/databases.sql`), refreshed every
/// fast tick. Returns `None` on ANY query or parse failure — the picker
/// simply has nothing to show this tick, same contract as
/// [`collect_vacuum_progress`]; it must never fail the poll.
async fn collect_databases(client: &mut Client, q: &queries::QuerySet) -> Option<Vec<DatabaseRow>> {
    let tx = begin_read(client).await.ok()?;
    let rows = tx.query(q.databases, &[]).await.ok()?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(db::database_from_row(row).ok()?);
    }
    tx.commit().await.ok()?;
    Some(out)
}

/// Turns a tokio-postgres error into the richest message available. The raw
/// `Display` is frequently just "db error", which hides the actual server
/// message — pull it (and the SQLSTATE) out of the `DbError` when present.
fn db_error_message(context: &str, e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        format!("{context}: {} ({})", db.message(), db.code().code())
    } else {
        format!("{context}: {e}")
    }
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

    /// A shutdown receiver that never fires (sender leaked): the poller runs
    /// as if the app is staying open.
    fn no_shutdown() -> watch::Receiver<bool> {
        let (tx, rx) = watch::channel(false);
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

    /// A database-switch channel whose sender stays alive (leaked) — the
    /// shape every non-switch test wants: open but silent (U2).
    fn db_switch_rx() -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel(4);
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
        let (mut rx, _h) = spawn(
            config,
            None,
            interval_rx(50),
            SCHEMA_INTERVAL_DEFAULT,
            refresh_rx(),
            admin_rx(),
            None,
            no_shutdown(),
            db_switch_rx(),
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
        let (mut rx, _h) = spawn(
            config,
            Some(source),
            interval_rx(50),
            SCHEMA_INTERVAL_DEFAULT,
            refresh_rx(),
            admin_rx(),
            None,
            no_shutdown(),
            db_switch_rx(),
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
        let (_rx, _h) = spawn(
            config,
            Some(PasswordSource::Command(cmd)),
            interval_rx(50),
            SCHEMA_INTERVAL_DEFAULT,
            refresh_rx(),
            admin_rx(),
            None,
            no_shutdown(),
            db_switch_rx(),
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

    // --- U2: database switch channel -----------------------------------------

    /// The happy path: a sent dbname resolves the wait immediately.
    #[tokio::test]
    async fn wait_db_switch_resolves_with_the_sent_name() {
        let (tx, mut rx) = mpsc::channel::<String>(1);
        tx.send("warehouse".to_string()).await.expect("receiver alive");
        assert_eq!(wait_db_switch(&mut rx).await, "warehouse");
    }

    /// A dropped sender (no picker wired up — `--mock`/`serve` today) must
    /// never resolve again, so it can never win a `tokio::select!` against
    /// the real work — same "no busy loop" contract as `wait_shutdown`.
    #[tokio::test]
    async fn wait_db_switch_never_resolves_after_the_sender_drops() {
        let (tx, mut rx) = mpsc::channel::<String>(1);
        drop(tx);
        let outcome = tokio::time::timeout(Duration::from_millis(50), wait_db_switch(&mut rx)).await;
        assert!(outcome.is_err(), "must stay pending, not resolve");
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
            vacuum_cluster_age: good.vacuum_cluster_age.clone(),
            vacuum_tables: good.vacuum_tables.clone(),
            indexes: good.indexes.clone(),
            stats_reset_epoch_secs: good.stats_reset_epoch_secs,
            bloat: Some(Ok((good.table_bloat.clone(), good.index_bloat.clone()))),
        });
        let mut fresh = good.tables.clone();
        fresh.pop();
        schema.store(SchemaCollection {
            tables: fresh.clone(),
            vacuum_cluster_age: good.vacuum_cluster_age.clone(),
            vacuum_tables: good.vacuum_tables.clone(),
            indexes: good.indexes.clone(),
            stats_reset_epoch_secs: good.stats_reset_epoch_secs,
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

    /// U2: unlike a poll error, a database switch must DROP the last
    /// collection entirely — the old data described a different database's
    /// objects, so carrying it forward under the new name would mislead.
    #[test]
    fn schema_state_reset_drops_the_last_collection() {
        let mut schema = SchemaState::new(Duration::from_secs(60), refresh_rx());
        let good = SchemaSnapshot::mock();
        schema.store(SchemaCollection {
            tables: good.tables.clone(),
            vacuum_cluster_age: good.vacuum_cluster_age.clone(),
            vacuum_tables: good.vacuum_tables.clone(),
            indexes: good.indexes.clone(),
            stats_reset_epoch_secs: good.stats_reset_epoch_secs,
            bloat: Some(Ok((good.table_bloat.clone(), good.index_bloat.clone()))),
        });
        assert!(schema.current.is_some(), "collected before the switch");
        let now = Instant::now();
        schema.last_attempt = Some(now);

        schema.reset();

        assert!(schema.current.is_none(), "old database's data must not survive");
        assert!(schema.last_attempt.is_none(), "cadence timer restarts too");
        // The next tick is due immediately (no need to wait out the old
        // database's cadence before the new one gets its first collection).
        assert_eq!(schema.due(now), Some(false));
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
            vacuum_cluster_age: good.vacuum_cluster_age.clone(),
            vacuum_tables: good.vacuum_tables.clone(),
            indexes: good.indexes.clone(),
            stats_reset_epoch_secs: good.stats_reset_epoch_secs,
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
            vacuum_cluster_age: good.vacuum_cluster_age.clone(),
            vacuum_tables: good.vacuum_tables.clone(),
            indexes: good.indexes.clone(),
            stats_reset_epoch_secs: good.stats_reset_epoch_secs,
            bloat: Some(Ok((good.table_bloat.clone(), good.index_bloat.clone()))),
        });

        let mut fresh_tables = good.tables.clone();
        fresh_tables.pop(); // observably different from the previous set
        schema.store(SchemaCollection {
            tables: fresh_tables.clone(),
            vacuum_cluster_age: good.vacuum_cluster_age.clone(),
            vacuum_tables: good.vacuum_tables.clone(),
            indexes: good.indexes.clone(),
            stats_reset_epoch_secs: good.stats_reset_epoch_secs,
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

    // --- F4: checkpointer / bgwriter delta derivation -----------------------

    fn bgwriter(
        timed: i64,
        req: i64,
        write_ms: f64,
        sync_ms: f64,
        buf_checkpoint: i64,
        buf_clean: i64,
        buf_backend: Option<i64>,
    ) -> db::BgwriterRow {
        db::BgwriterRow {
            checkpoints_timed: timed,
            checkpoints_req: req,
            checkpoint_write_time_ms: write_ms,
            checkpoint_sync_time_ms: sync_ms,
            buffers_checkpoint: buf_checkpoint,
            buffers_clean: buf_clean,
            maxwritten_clean: 0,
            buffers_backend: buf_backend,
            buffers_alloc: 0,
        }
    }

    #[test]
    fn first_poll_of_a_session_has_no_rates_but_carries_cumulative_counters() {
        let raw = bgwriter(100, 5, 50_000.0, 4_000.0, 900_000, 30_000, Some(20_000));
        let (stats, _delta) = derive_checkpointer_stats(&raw, Instant::now(), None);
        assert_eq!(stats.checkpoints_timed, 100);
        assert_eq!(stats.checkpoints_req, 5);
        assert_eq!(stats.buffers_backend, Some(20_000));
        assert!(stats.checkpoints_per_min_timed.is_none());
        assert!(stats.checkpoints_per_min_req.is_none());
        assert!(stats.buffers_checkpoint_per_sec.is_none());
        assert!(stats.avg_checkpoint_write_ms.is_none());
        // The session baseline captured on this first tick means the ratio
        // itself is also absent until a later tick sees a NEW checkpoint.
        assert!(stats.requested_ratio_session.is_none());
    }

    #[test]
    fn second_tick_derives_per_min_and_per_sec_rates() {
        let raw0 = bgwriter(100, 5, 50_000.0, 4_000.0, 900_000, 30_000, Some(20_000));
        let t0 = Instant::now();
        let (_stats0, delta0) = derive_checkpointer_stats(&raw0, t0, None);

        // 30s later: one more timed checkpoint completed, plus buffer churn.
        let raw1 = bgwriter(101, 5, 54_000.0, 4_200.0, 900_600, 30_300, Some(20_150));
        let t1 = t0 + Duration::from_secs(30);
        let (stats1, _delta1) = derive_checkpointer_stats(&raw1, t1, Some(&delta0));

        // 1 checkpoint / 30s = 2/min.
        assert!((stats1.checkpoints_per_min_timed.unwrap() - 2.0).abs() < 1e-9);
        assert!((stats1.checkpoints_per_min_req.unwrap() - 0.0).abs() < 1e-9);
        // 600 buffers / 30s = 20/s.
        assert!((stats1.buffers_checkpoint_per_sec.unwrap() - 20.0).abs() < 1e-9);
        // 300 buffers / 30s = 10/s.
        assert!((stats1.buffers_clean_per_sec.unwrap() - 10.0).abs() < 1e-9);
        // 150 buffers / 30s = 5/s.
        assert!((stats1.buffers_backend_per_sec.unwrap() - 5.0).abs() < 1e-9);
        // One checkpoint completed: avg write/sync = the full delta.
        assert!((stats1.avg_checkpoint_write_ms.unwrap() - 4_000.0).abs() < 1e-9);
        assert!((stats1.avg_checkpoint_sync_ms.unwrap() - 200.0).abs() < 1e-9);
    }

    #[test]
    fn no_new_checkpoint_this_tick_leaves_avg_write_sync_absent() {
        let raw0 = bgwriter(100, 5, 50_000.0, 4_000.0, 900_000, 30_000, None);
        let t0 = Instant::now();
        let (_stats0, delta0) = derive_checkpointer_stats(&raw0, t0, None);

        // Same checkpoint counts, only buffer churn — no division by zero.
        let raw1 = bgwriter(100, 5, 50_000.0, 4_000.0, 900_100, 30_050, None);
        let t1 = t0 + Duration::from_secs(10);
        let (stats1, _delta1) = derive_checkpointer_stats(&raw1, t1, Some(&delta0));
        assert!(stats1.avg_checkpoint_write_ms.is_none());
        assert!(stats1.avg_checkpoint_sync_ms.is_none());
        assert!(stats1.buffers_backend_per_sec.is_none(), "absent on 17+ style rows");
        assert!(stats1.buffers_checkpoint_per_sec.is_some());
    }

    #[test]
    fn session_ratio_accumulates_since_the_session_baseline_not_per_tick() {
        let raw0 = bgwriter(100, 5, 50_000.0, 4_000.0, 900_000, 30_000, None);
        let t0 = Instant::now();
        let (_stats0, delta0) = derive_checkpointer_stats(&raw0, t0, None);

        // Tick 2: one requested checkpoint completes (session delta: 0
        // timed, 1 requested since baseline) — ratio is 100% requested.
        let raw1 = bgwriter(100, 6, 50_500.0, 4_050.0, 900_100, 30_050, None);
        let t1 = t0 + Duration::from_secs(30);
        let (stats1, delta1) = derive_checkpointer_stats(&raw1, t1, Some(&delta0));
        assert!((stats1.requested_ratio_session.unwrap() - 1.0).abs() < 1e-9);

        // Tick 3: two timed checkpoints follow — session totals become 2
        // timed / 1 requested since baseline, ratio settles to 1/3, calm.
        let raw2 = bgwriter(102, 6, 51_500.0, 4_150.0, 900_300, 30_150, None);
        let t2 = t1 + Duration::from_secs(60);
        let (stats2, _delta2) = derive_checkpointer_stats(&raw2, t2, Some(&delta1));
        assert!((stats2.requested_ratio_session.unwrap() - (1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn a_backwards_counter_rebaselines_instead_of_going_negative() {
        let raw0 = bgwriter(500, 200, 900_000.0, 60_000.0, 5_000_000, 400_000, None);
        let t0 = Instant::now();
        let (_stats0, delta0) = derive_checkpointer_stats(&raw0, t0, None);

        // pg_stat_reset() or a restart: counters drop back near zero.
        let raw1 = bgwriter(1, 0, 100.0, 10.0, 500, 200, None);
        let t1 = t0 + Duration::from_secs(30);
        let (stats1, _delta1) = derive_checkpointer_stats(&raw1, t1, Some(&delta0));
        // No negative rates leak through — the tick is treated like a first
        // poll of a fresh session (baseline snaps to the new low counters).
        assert!(stats1.checkpoints_per_min_timed.is_none());
        assert!(stats1.buffers_checkpoint_per_sec.is_none());
        // The reset tick itself has no NEW checkpoint since its own
        // baseline yet, so the ratio is absent too.
        assert!(stats1.requested_ratio_session.is_none());
    }
}
