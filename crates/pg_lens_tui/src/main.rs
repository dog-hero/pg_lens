//! pg_lens entry point: one binary, two frontends.
//!
//! `pg_lens tui` (the default when no subcommand is given — bare
//! `pg_lens --mock` keeps working) runs the terminal UI; `pg_lens serve`
//! (behind the default-on `web` cargo feature) runs the pg_lens_web axum
//! server. Both share the same connection flags and the exact same core
//! pipeline: `settings::resolve` → `poller::spawn` → `watch<Arc<DbSnapshot>>`.
//!
//! TUI pattern copied from the ratatui `event-driven-async` template:
//! `ratatui::init()` (which installs a panic hook that restores the
//! terminal) + `ratatui::restore()` around the async run loop. The crossterm
//! `EventStream` lives in its own task (`event::spawn_input`), the core
//! poller publishes through a `watch` channel, and a bridge task
//! (`event::spawn_snapshot_bridge`) converts snapshots into `Action`s.
//!
//! Connection resolution (Fase C1) happens in `pg_lens_core::settings`:
//! `--dsn` fields win, the libpq env vars (`PGHOST`, `PGPORT`, `PGDATABASE`,
//! `PGUSER`, `PGPASSWORD`, `PGAPPNAME`, `PGCONNECT_TIMEOUT`) fill the gaps,
//! and `host=localhost user=postgres` is the fallback. The environment is
//! captured *here*, once, and injected — the core never reads `std::env`.
//! The resolved `Config` (which may carry a password) is handed to the core
//! as-is and never logged; only the safe `ConnLabel` reaches any output.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use pg_lens_core::DbSnapshot;
use pg_lens_core::settings::{self, ConnSpec, Resolved, ServiceSummary};
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, watch};

use pg_lens_core::AdminCommand;

use crate::app::{Action, App, PickerEntry, PickerState, update};

mod app;
mod event;
mod ui;

/// A blazing-fast, modern TUI (and web server) for PostgreSQL observability.
#[derive(Debug, Parser)]
#[command(name = "pg_lens", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Back-compat: the pre-subcommand flat CLI (`pg_lens --mock`,
    /// `pg_lens --dsn ...`) still parses and means `tui`.
    #[command(flatten)]
    conn: ConnArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the terminal UI (the default when no subcommand is given).
    Tui(ConnArgs),

    /// Serve the web UI and JSON/SSE API over HTTP (read-only).
    #[cfg(feature = "web")]
    Serve(ServeArgs),
}

/// Connection flags shared by every subcommand.
#[derive(Debug, Args)]
struct ConnArgs {
    /// PostgreSQL connection string (`key=value` DSN or `postgres://` URL).
    /// Fields not set here fall back to the libpq env vars (PGHOST, PGPORT,
    /// PGDATABASE, PGUSER, PGPASSWORD, PGAPPNAME, PGCONNECT_TIMEOUT), then
    /// to `host=localhost user=postgres`.
    #[arg(long, env = "PG_LENS_DSN")]
    dsn: Option<String>,

    /// Connect using a named entry from the services file. Also read from
    /// the PG_LENS_SERVICE env var, falling back to PGSERVICE. Mutually
    /// exclusive with --dsn.
    #[arg(long, value_name = "NAME", conflicts_with = "dsn")]
    service: Option<String>,

    /// Path to the services file. Defaults to
    /// $XDG_CONFIG_HOME/pg_lens/services.toml (or
    /// ~/.config/pg_lens/services.toml); also read from the
    /// PG_LENS_SERVICES_FILE env var.
    #[arg(long, value_name = "PATH")]
    services_file: Option<PathBuf>,

    /// Print the services defined in the services file (names + host/user,
    /// never passwords) and exit.
    #[arg(long)]
    list_services: bool,

    /// Poll interval in seconds (minimum 0.5).
    #[arg(long, default_value_t = 2.0)]
    interval: f64,

    /// Schema Lens collection interval in seconds (minimum 5): table stats
    /// and sizes are expensive, so they run on this slow cadence — never on
    /// the fast tick.
    #[arg(long, value_name = "SECS", default_value_t = 60)]
    schema_interval: u64,

    /// Use built-in mock data instead of a real database (dev/demo mode).
    #[arg(long)]
    mock: bool,
}

#[cfg(feature = "web")]
#[derive(Debug, Args)]
struct ServeArgs {
    #[command(flatten)]
    conn: ConnArgs,

    /// Address to bind. Non-loopback addresses are refused unless
    /// PG_LENS_AUTH_TOKEN is set (all /api routes then require
    /// `Authorization: Bearer <token>`).
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:8080")]
    listen: std::net::SocketAddr,
}

impl ConnArgs {
    /// Everything `settings::resolve` needs; the process environment is
    /// captured exactly once, here, and injected.
    fn spec(&self) -> ConnSpec {
        ConnSpec {
            dsn: self.dsn.clone(),
            service: self.service.clone(),
            services_file: self.services_file.clone(),
            env: std::env::vars().collect::<HashMap<_, _>>(),
        }
    }

    /// --list-services: plain stdout, no TUI/server. Names + host/user only
    /// — a password or password_cmd never reaches this output.
    fn list_services(&self) -> color_eyre::Result<()> {
        let (services, warnings) = settings::list_services(&self.spec())?;
        for warning in &warnings {
            eprintln!("warning: {warning}");
        }
        for service in services {
            println!(
                "{name}\thost={host}\tuser={user}",
                name = service.name,
                host = service.host.as_deref().unwrap_or("-"),
                user = service.user.as_deref().unwrap_or("-"),
            );
        }
        Ok(())
    }

    /// Resolves the connection (unless `--mock`), printing warnings to
    /// stderr. Shared verbatim by `tui` and `serve` — same resolution, same
    /// poller, per the "one core pipeline" invariant.
    fn resolve(&self) -> color_eyre::Result<Option<Resolved>> {
        if self.mock {
            return Ok(None);
        }
        let resolved = settings::resolve(&self.spec())?;
        for warning in &resolved.warnings {
            eprintln!("warning: {warning}");
        }
        Ok(Some(resolved))
    }

    fn interval(&self) -> Duration {
        Duration::from_secs_f64(self.interval.max(0.5))
    }

    /// `--schema-interval`, floored at the core's sanity minimum (5s).
    fn schema_interval(&self) -> Duration {
        Duration::from_secs(self.schema_interval)
            .max(pg_lens_core::poller::SCHEMA_INTERVAL_MIN)
    }
}

/// How `run` starts: with a connection already resolved (the classic path),
/// or in picker mode — no poller yet; the user chooses first.
enum Startup {
    // Boxed: `Resolved` is large next to the picker variant (clippy
    // large_enum_variant), and this value exists exactly once at startup.
    Connect(Box<Option<Resolved>>),
    Picker(Vec<PickerEntry>),
}

/// Env vars whose presence (non-empty — empty values count as unset,
/// consistent with `settings.rs`) expresses a connection intent that the
/// picker must not second-guess.
const PICKER_SUPPRESSING_ENV: [&str; 4] =
    ["PGHOST", "PGSERVICE", "PG_LENS_SERVICE", "PG_LENS_DSN"];

/// Interactive service picker trigger rule (TUI mode only — never `serve`,
/// never `--mock`; see README "Interactive service picker"). The picker
/// shows iff ALL of:
/// - no `--dsn` and no `--service` (empty strings count as unset), and
/// - none of PGHOST / PGSERVICE / PG_LENS_SERVICE / PG_LENS_DSN is set to a
///   non-empty value in the captured environment, and
/// - the services file (the default XDG path, or the `--services-file` /
///   `PG_LENS_SERVICES_FILE` override) exists, parses, passes the existing
///   permission checks, and defines at least one service.
///
/// Any failure of that chain returns `None`, which means EXACTLY the
/// pre-picker behavior: plain `settings::resolve` (localhost default) or
/// its pre-TUI error. Returns display-safe summaries only (names +
/// host/user — never a `password`/`password_cmd`) plus the loader's
/// non-fatal warnings.
fn picker_services(spec: &ConnSpec) -> Option<(Vec<ServiceSummary>, Vec<String>)> {
    if spec.dsn.as_deref().is_some_and(|d| !d.is_empty())
        || spec.service.as_deref().is_some_and(|s| !s.is_empty())
    {
        return None;
    }
    if PICKER_SUPPRESSING_ENV
        .iter()
        .any(|var| spec.env.get(*var).is_some_and(|v| !v.is_empty()))
    {
        return None;
    }
    // `list_services` runs the whole chain (locate → read → parse →
    // permission checks); any error degrades to the default behavior.
    match settings::list_services(spec) {
        Ok((services, warnings)) if !services.is_empty() => Some((services, warnings)),
        _ => None,
    }
}

/// Builds the picker rows: one per service, rendered exactly as the file
/// says (`?` for fields the entry leaves out — env/default fallbacks are
/// NOT applied here), plus the trailing `localhost — (default)` entry that
/// maps to the plain no-service resolution.
fn picker_entries(services: Vec<ServiceSummary>) -> Vec<PickerEntry> {
    let mut entries: Vec<PickerEntry> = services
        .into_iter()
        .map(|s| {
            let detail = format!(
                "{user}@{host}",
                user = s.user.as_deref().unwrap_or("?"),
                host = s.host.as_deref().unwrap_or("?"),
            );
            PickerEntry {
                name: s.name.clone(),
                detail,
                service: Some(s.name),
            }
        })
        .collect();
    entries.push(PickerEntry {
        name: "localhost".to_string(),
        detail: "(default)".to_string(),
        service: None,
    });
    entries
}

/// Spawns the poller (mock or real) and returns its snapshot channel plus a
/// display label. The interval sender rides along: the TUI feeds `+`/`-`
/// changes into it, `serve` just keeps it alive. `schema_refresh_rx` is the
/// force-recollection signal (a bumped counter): Fase S3 wires the TUI's `R`
/// key to its sender — until then callers just keep the sender alive.
fn spawn_poller(
    conn: Option<Resolved>,
    interval_rx: watch::Receiver<Duration>,
    schema_interval: Duration,
    schema_refresh_rx: watch::Receiver<u64>,
    admin_rx: mpsc::Receiver<AdminCommand>,
) -> (watch::Receiver<Arc<DbSnapshot>>, String) {
    match conn {
        None => (
            pg_lens_core::poller::spawn_mock(interval_rx, schema_refresh_rx, admin_rx),
            "mock".to_string(),
        ),
        Some(resolved) => {
            // The label is the only connection info any frontend sees —
            // host and user, never the password. When the resolution came
            // with a password_cmd, the poller re-runs it per (re)connection.
            let label = resolved.label.to_string();
            let snapshots = pg_lens_core::poller::spawn(
                resolved.config,
                resolved.password_source,
                interval_rx,
                schema_interval,
                schema_refresh_rx,
                admin_rx,
            );
            (snapshots, label)
        }
    }
}

/// Forwards admin commands queued by `update()` (the `y` of the confirm
/// modal) into the poller's channel. `try_send` on purpose: the UI loop
/// must never block — on a momentarily full channel the remaining commands
/// simply go out on the next pass through the loop.
fn drain_admin(app: &mut App, admin_tx: &mpsc::Sender<AdminCommand>) {
    while let Some(&cmd) = app.pending_admin.first() {
        match admin_tx.try_send(cmd) {
            Ok(()) => {
                app.pending_admin.remove(0);
            }
            // Full: retry next pass. Closed: the poller is gone — the loop
            // is about to end anyway; dropping the queue is harmless.
            Err(_) => break,
        }
    }
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = Cli::parse();
    color_eyre::install()?;

    match cli.command {
        // Bare `pg_lens [flags]` = the historical flat CLI = `tui`.
        None => run_tui(cli.conn).await,
        Some(Command::Tui(conn)) => run_tui(conn).await,
        #[cfg(feature = "web")]
        Some(Command::Serve(args)) => run_serve(args).await,
    }
}

async fn run_tui(conn_args: ConnArgs) -> color_eyre::Result<()> {
    if conn_args.list_services {
        return conn_args.list_services();
    }
    // Startup mode decision. Picker mode (see `picker_services` for the
    // full trigger rule) enters the TUI with no poller; otherwise resolve
    // the connection *before* entering the alternate screen so a bad DSN /
    // env var / services file prints as a normal error (and permission
    // warnings land on stderr), not inside a raw terminal.
    let startup = match picker_services(&conn_args.spec()) {
        Some((services, warnings)) if !conn_args.mock => {
            for warning in &warnings {
                eprintln!("warning: {warning}");
            }
            Startup::Picker(picker_entries(services))
        }
        _ => Startup::Connect(Box::new(conn_args.resolve()?)),
    };

    let terminal = ratatui::init();
    let result = run(terminal, &conn_args, startup).await;
    ratatui::restore();
    result
}

/// `pg_lens serve`: same resolution and poller as the TUI, but the watch
/// channel feeds pg_lens_web's router instead of ratatui. Runs until Ctrl+C.
#[cfg(feature = "web")]
async fn run_serve(args: ServeArgs) -> color_eyre::Result<()> {
    use color_eyre::eyre::eyre;

    if args.conn.list_services {
        return args.conn.list_services();
    }

    // Empty tokens count as unset: `PG_LENS_AUTH_TOKEN= pg_lens serve`
    // must not silently create a server "protected" by the empty string.
    let token = std::env::var("PG_LENS_AUTH_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    // Security gate: non-loopback bind without a token refuses to start.
    pg_lens_web::ensure_listen_allowed(&args.listen, token.is_some()).map_err(|e| eyre!(e))?;
    let auth_enabled = token.is_some();

    let conn = args.conn.resolve()?;
    // `serve` has no `+`/`-` keys; the sender only needs to outlive the
    // server so the poller keeps its cadence.
    let (_interval_tx, interval_rx) = watch::channel(args.conn.interval());
    // No re-collect endpoint in the read-only web UI (S4 backlog): the
    // sender only has to outlive the server, like the interval one.
    let (_schema_refresh_tx, schema_refresh_rx) = watch::channel(0u64);
    // The web stays read-only by design (plan's security posture): no route
    // ever sends an AdminCommand; the sender just outlives the server.
    let (_admin_tx, admin_rx) = mpsc::channel::<AdminCommand>(8);
    let (snapshots, label) = spawn_poller(
        conn,
        interval_rx,
        args.conn.schema_interval(),
        schema_refresh_rx,
        admin_rx,
    );

    let router = pg_lens_web::router(snapshots, token);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let addr = listener.local_addr()?;
    // Operator info on stderr: bound address + auth mode + safe label
    // (user@host — never the DSN or password).
    eprintln!("pg_lens: serving {label} on http://{addr}/");
    eprintln!(
        "pg_lens: auth {}",
        if auth_enabled {
            "enabled — /api requires `Authorization: Bearer <PG_LENS_AUTH_TOKEN>`"
        } else {
            "disabled (loopback bind without PG_LENS_AUTH_TOKEN)"
        }
    );
    pg_lens_web::serve(listener, router).await?;
    Ok(())
}

async fn run(
    mut terminal: DefaultTerminal,
    conn_args: &ConnArgs,
    startup: Startup,
) -> color_eyre::Result<()> {
    let interval = conn_args.interval();
    let mut app = App::new();
    app.refresh_interval = interval;

    let (tx, mut actions) = mpsc::channel::<Action>(64);
    let _input_task = event::spawn_input(tx.clone());
    // The poller reads its cadence live from this watch channel; the loop
    // below mirrors `app.refresh_interval` into it whenever `+`/`-` change
    // it. Message-passing only — no shared Mutex.
    let (interval_tx, interval_rx) = watch::channel(interval);
    // Schema force-refresh (Fase S3): the `R` key bumps
    // `app.schema_refresh_requests`; the loop below mirrors that counter
    // into this watch channel, and the poller recollects on the next tick.
    let (schema_refresh_tx, schema_refresh_rx) = watch::channel(0u64);
    // Admin actions (`c`/`K` + confirm): update() queues commands in
    // `app.pending_admin`; the loop below drains them into this channel and
    // the poller (sole owner of the DB client) executes them. Message
    // passing only, like every other TUI↔poller conversation.
    let (admin_tx, admin_rx) = mpsc::channel::<AdminCommand>(8);
    // Picker mode enters the loop with NO poller: the channels above exist
    // as usual, but their receivers are parked here until the user picks an
    // entry — the loop then spawns the poller (and its snapshot bridge)
    // lazily, reusing the exact same wiring as the classic path below.
    #[allow(clippy::type_complexity)]
    let mut parked_rx: Option<(
        watch::Receiver<Duration>,
        watch::Receiver<u64>,
        mpsc::Receiver<AdminCommand>,
    )> = None;
    match startup {
        Startup::Connect(conn) => {
            let (snapshots, label) = spawn_poller(
                *conn,
                interval_rx,
                conn_args.schema_interval(),
                schema_refresh_rx,
                admin_rx,
            );
            app.host = label;
            // Seed the app with the poller's initial snapshot before the
            // first frame (in real mode this avoids one frame of
            // placeholder mock data).
            update(&mut app, Action::Snapshot(snapshots.borrow().clone()));
            // Dropping the JoinHandle detaches the task; it keeps running.
            let _bridge_task = event::spawn_snapshot_bridge(snapshots, tx.clone());
        }
        Startup::Picker(entries) => {
            app.picker = Some(PickerState::new(entries));
            parked_rx = Some((interval_rx, schema_refresh_rx, admin_rx));
        }
    }

    let mut tick = tokio::time::interval(Duration::from_millis(250));

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(&mut app, frame))?;

        tokio::select! {
            maybe_action = actions.recv() => match maybe_action {
                Some(action) => update(&mut app, action),
                // All senders gone: nothing left to react to.
                None => update(&mut app, Action::Quit),
            },
            _ = tick.tick() => update(&mut app, Action::Tick),
        }

        // Push a `+`/`-` interval change to the poller (no-op otherwise;
        // receivers are only woken when the value actually changed).
        interval_tx.send_if_modified(|current| {
            if *current == app.refresh_interval {
                false
            } else {
                *current = app.refresh_interval;
                true
            }
        });
        // Same pattern for `R`: mirror the bumped request counter so the
        // poller force-recollects the schema stats on its next tick.
        schema_refresh_tx.send_if_modified(|current| {
            if *current == app.schema_refresh_requests {
                false
            } else {
                *current = app.schema_refresh_requests;
                true
            }
        });
        // Confirmed admin commands (the modal's `y`) go to the poller task,
        // which executes them and reports back inside the next snapshot.
        drain_admin(&mut app, &admin_tx);

        // Lazy poller spawn (picker mode): the first pass after Enter sees
        // `app.picked` plus the parked receivers and starts the exact same
        // pipeline `Startup::Connect` uses — the resolved label and the
        // initial Connecting snapshot go through update() (the sole
        // mutation point), so the very next draw is the connection splash
        // ("connecting to user@host…"), then the dashboard on first data.
        // Connection failures are NOT errors here: they surface as
        // `PollerStatus::Error` on the splash, retrying with backoff.
        if let Some(entry) = app.picked.clone()
            && let Some((interval_rx, schema_refresh_rx, admin_rx)) = parked_rx.take()
        {
            // Re-resolve with the chosen service (or none, for the default
            // entry). The file was readable moments ago at trigger time; if
            // it changed underneath us, exit the TUI with the plain error —
            // the same text the pre-TUI path would have printed. Non-fatal
            // warnings were already printed before the TUI took over.
            let mut spec = conn_args.spec();
            spec.dsn = None;
            spec.service = entry.service.clone();
            let resolved = settings::resolve(&spec)?;
            let (snapshots, label) = spawn_poller(
                Some(resolved),
                interval_rx,
                conn_args.schema_interval(),
                schema_refresh_rx,
                admin_rx,
            );
            update(&mut app, Action::HostLabel(label));
            update(&mut app, Action::Snapshot(snapshots.borrow().clone()));
            // Detaches on drop; the bridge lives as long as its channels.
            let _bridge_task = event::spawn_snapshot_bridge(snapshots, tx.clone());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A 0600 services file on disk, kept alive for the test's duration.
    fn services_file(contents: &str) -> tempfile::TempPath {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(contents.as_bytes()).expect("write");
        f.flush().expect("flush");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600))
                .expect("chmod");
        }
        f.into_temp_path()
    }

    const TWO_SERVICES: &str = r#"
        [services.prod]
        host = "db.prod.internal"
        user = "svc_ro"
        password_cmd = "echo hidden-secret"

        [services.dead]
        host = "dead.invalid"
    "#;

    fn spec_with_file(file: &tempfile::TempPath, env_pairs: &[(&str, &str)]) -> ConnSpec {
        ConnSpec {
            dsn: None,
            service: None,
            services_file: Some(file.to_path_buf()),
            env: env(env_pairs),
        }
    }

    // --- admin command drain ---------------------------------------------------

    /// update() queues on `y`; the loop's drain forwards the command into
    /// the poller channel — a test receiver observes exactly what a poller
    /// would (the "command surfaces on a test mpsc receiver" proof).
    #[tokio::test]
    async fn drain_admin_forwards_queued_commands_to_the_channel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = App::new();
        // Micro Lens, selected row, open the cancel modal, confirm with y.
        update(&mut app, Action::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        let pid = app.selected_row().expect("selection").pid;
        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)),
        );
        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
        );

        let (admin_tx, mut admin_rx) = mpsc::channel::<AdminCommand>(8);
        drain_admin(&mut app, &admin_tx);
        assert!(app.pending_admin.is_empty(), "queue drained");
        assert_eq!(
            admin_rx.try_recv().expect("command forwarded"),
            AdminCommand::CancelBackend(pid)
        );
        assert!(admin_rx.try_recv().is_err(), "exactly one command");
    }

    #[tokio::test]
    async fn drain_admin_keeps_commands_when_the_channel_is_full() {
        let mut app = App::new();
        app.pending_admin = vec![
            AdminCommand::CancelBackend(1),
            AdminCommand::TerminateBackend(2),
        ];
        // Capacity 1: the second command must survive for the next pass.
        let (admin_tx, mut admin_rx) = mpsc::channel::<AdminCommand>(1);
        drain_admin(&mut app, &admin_tx);
        assert_eq!(app.pending_admin, vec![AdminCommand::TerminateBackend(2)]);
        assert_eq!(
            admin_rx.try_recv().expect("first sent"),
            AdminCommand::CancelBackend(1)
        );
        // Next pass (channel drained): the rest goes out.
        drain_admin(&mut app, &admin_tx);
        assert!(app.pending_admin.is_empty());
        assert_eq!(
            admin_rx.try_recv().expect("second sent"),
            AdminCommand::TerminateBackend(2)
        );
    }

    // --- picker trigger rule -------------------------------------------------

    #[test]
    fn picker_triggers_with_no_hints_and_a_valid_services_file() {
        let file = services_file(TWO_SERVICES);
        let (services, _) =
            picker_services(&spec_with_file(&file, &[])).expect("picker must trigger");
        let names: Vec<_> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["dead", "prod"], "BTreeMap order");
    }

    #[test]
    fn picker_suppressed_by_dsn_or_service_flags() {
        let file = services_file(TWO_SERVICES);
        let mut spec = spec_with_file(&file, &[]);
        spec.dsn = Some("host=x".to_string());
        assert!(picker_services(&spec).is_none(), "--dsn suppresses");

        let mut spec = spec_with_file(&file, &[]);
        spec.service = Some("prod".to_string());
        assert!(picker_services(&spec).is_none(), "--service suppresses");
    }

    #[test]
    fn picker_suppressed_by_each_connection_env_var() {
        let file = services_file(TWO_SERVICES);
        for var in PICKER_SUPPRESSING_ENV {
            let spec = spec_with_file(&file, &[(var, "something")]);
            assert!(
                picker_services(&spec).is_none(),
                "{var} set must suppress the picker"
            );
        }
        // Unrelated env vars (even PGUSER/PGPORT) do NOT suppress it.
        let spec = spec_with_file(&file, &[("PGUSER", "monitor"), ("PGPORT", "5433")]);
        assert!(picker_services(&spec).is_some());
    }

    #[test]
    fn empty_string_values_count_as_unset_everywhere() {
        let file = services_file(TWO_SERVICES);
        // Empty env values: consistent with settings.rs, still a picker.
        let spec = spec_with_file(
            &file,
            &[
                ("PGHOST", ""),
                ("PGSERVICE", ""),
                ("PG_LENS_SERVICE", ""),
                ("PG_LENS_DSN", ""),
            ],
        );
        assert!(picker_services(&spec).is_some(), "empty env = unset");

        // Empty flag values too (clap env-fill can produce Some("")).
        let mut spec = spec_with_file(&file, &[]);
        spec.dsn = Some(String::new());
        spec.service = Some(String::new());
        assert!(picker_services(&spec).is_some(), "empty flags = unset");
    }

    #[test]
    fn picker_suppressed_when_no_services_file_exists() {
        // Default XDG location pointing into nowhere: no file, no picker.
        let spec = ConnSpec {
            env: env(&[("HOME", "/nonexistent-home-for-test")]),
            ..ConnSpec::default()
        };
        assert!(picker_services(&spec).is_none());

        // No derivable location at all (HOME/XDG unset): same.
        assert!(picker_services(&ConnSpec::default()).is_none());

        // Explicit override pointing at a missing file: same (the error
        // then surfaces through the normal resolve path, unchanged).
        let spec = ConnSpec {
            services_file: Some(PathBuf::from("/nonexistent/services.toml")),
            ..ConnSpec::default()
        };
        assert!(picker_services(&spec).is_none());
    }

    #[test]
    fn picker_suppressed_by_parse_error_or_zero_services() {
        let broken = services_file("this is [not really { toml");
        assert!(picker_services(&spec_with_file(&broken, &[])).is_none());

        let empty = services_file("");
        assert!(picker_services(&spec_with_file(&empty, &[])).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn picker_suppressed_by_insecure_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let file = services_file(TWO_SERVICES);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666))
            .expect("chmod 666");
        assert!(
            picker_services(&spec_with_file(&file, &[])).is_none(),
            "group/other-writable file must fail the permission check"
        );
    }

    // --- picker entries ------------------------------------------------------

    #[test]
    fn entries_show_what_the_file_says_plus_default_last_and_no_secrets() {
        let file = services_file(TWO_SERVICES);
        let (services, _) =
            picker_services(&spec_with_file(&file, &[])).expect("picker must trigger");
        let entries = picker_entries(services);

        assert_eq!(entries.len(), 3, "two services + the default");
        // Verbatim file fields; `?` where the entry is silent (no env or
        // localhost/postgres fallbacks applied here).
        assert_eq!(entries[0].name, "dead");
        assert_eq!(entries[0].detail, "?@dead.invalid");
        assert_eq!(entries[0].service.as_deref(), Some("dead"));
        assert_eq!(entries[1].name, "prod");
        assert_eq!(entries[1].detail, "svc_ro@db.prod.internal");
        // The default entry is LAST and maps to no-service resolution.
        assert_eq!(entries[2].name, "localhost");
        assert_eq!(entries[2].detail, "(default)");
        assert_eq!(entries[2].service, None);

        // Secrets canary: nothing password-shaped reaches the picker rows.
        let rendered = format!("{entries:?}");
        assert!(!rendered.contains("hidden-secret"), "got: {rendered}");
        assert!(!rendered.contains("password"), "got: {rendered}");
    }
}
