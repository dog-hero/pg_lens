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
use pg_lens_core::settings::{self, ConnSpec, Resolved};
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, watch};

use crate::app::{Action, App, update};

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
}

/// Spawns the poller (mock or real) and returns its snapshot channel plus a
/// display label. The interval sender rides along: the TUI feeds `+`/`-`
/// changes into it, `serve` just keeps it alive.
fn spawn_poller(
    conn: Option<Resolved>,
    interval_rx: watch::Receiver<Duration>,
) -> (watch::Receiver<Arc<DbSnapshot>>, String) {
    match conn {
        None => (pg_lens_core::poller::spawn_mock(interval_rx), "mock".to_string()),
        Some(resolved) => {
            // The label is the only connection info any frontend sees —
            // host and user, never the password. When the resolution came
            // with a password_cmd, the poller re-runs it per (re)connection.
            let label = resolved.label.to_string();
            let snapshots =
                pg_lens_core::poller::spawn(resolved.config, resolved.password_source, interval_rx);
            (snapshots, label)
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
    // Resolve the connection *before* entering the alternate screen so a
    // bad DSN / env var / services file prints as a normal error (and
    // permission warnings land on stderr), not inside a raw terminal.
    let conn = conn_args.resolve()?;

    let terminal = ratatui::init();
    let result = run(terminal, &conn_args, conn).await;
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
    let (snapshots, label) = spawn_poller(conn, interval_rx);

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
    conn: Option<Resolved>,
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
    let (snapshots, label) = spawn_poller(conn, interval_rx);
    app.host = label;
    // Seed the app with the poller's initial snapshot before the first frame
    // (in real mode this avoids one frame of placeholder mock data).
    update(&mut app, Action::Snapshot(snapshots.borrow().clone()));
    let _bridge_task = event::spawn_snapshot_bridge(snapshots, tx);

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
    }

    Ok(())
}
