//! pg_lens TUI entry point.
//!
//! Pattern copied from the ratatui `event-driven-async` template:
//! `ratatui::init()` (which installs a panic hook that restores the
//! terminal) + `ratatui::restore()` around the async run loop.
//!
//! Pipeline: the crossterm `EventStream` lives in its own task
//! (`event::spawn_input`), the core poller (real tokio-postgres one, or the
//! mock with `--mock`) publishes through a `watch` channel, and a bridge task
//! (`event::spawn_snapshot_bridge`) converts snapshots into `Action`s. The
//! loop below selects over the single `mpsc<Action>` receiver and a render
//! tick.
//!
//! Connection resolution (Fase C1) happens in `pg_lens_core::settings`:
//! `--dsn` fields win, the libpq env vars (`PGHOST`, `PGPORT`, `PGDATABASE`,
//! `PGUSER`, `PGPASSWORD`, `PGAPPNAME`, `PGCONNECT_TIMEOUT`) fill the gaps,
//! and `host=localhost user=postgres` is the fallback. The environment is
//! captured *here*, once, and injected — the core never reads `std::env`.
//! The resolved `Config` (which may carry a password) is handed to the core
//! as-is and never logged; only the safe `ConnLabel` reaches the view.

use std::collections::HashMap;
use std::time::Duration;

use clap::Parser;
use pg_lens_core::settings::{self, ConnLabel, ConnSpec};
use pg_lens_core::tokio_postgres;
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, watch};

use crate::app::{Action, App, update};

mod app;
mod event;
mod ui;

/// A blazing-fast, modern TUI for PostgreSQL observability.
#[derive(Debug, Parser)]
#[command(name = "pg_lens", version, about)]
struct Cli {
    /// PostgreSQL connection string (`key=value` DSN or `postgres://` URL).
    /// Fields not set here fall back to the libpq env vars (PGHOST, PGPORT,
    /// PGDATABASE, PGUSER, PGPASSWORD, PGAPPNAME, PGCONNECT_TIMEOUT), then
    /// to `host=localhost user=postgres`.
    #[arg(long, env = "PG_LENS_DSN")]
    dsn: Option<String>,

    /// Poll interval in seconds (minimum 0.5).
    #[arg(long, default_value_t = 2.0)]
    interval: f64,

    /// Use built-in mock data instead of a real database (dev/demo mode).
    #[arg(long)]
    mock: bool,
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = Cli::parse();
    color_eyre::install()?;

    // Resolve the connection *before* entering the alternate screen so a
    // bad DSN / env var prints as a normal error, not inside a raw terminal.
    let conn = if cli.mock {
        None
    } else {
        let spec = ConnSpec {
            dsn: cli.dsn.clone(),
            // Captured exactly once; the core takes it by injection.
            env: std::env::vars().collect::<HashMap<_, _>>(),
        };
        Some(settings::resolve(&spec)?)
    };

    let terminal = ratatui::init();
    let result = run(terminal, &cli, conn).await;
    ratatui::restore();
    result
}

async fn run(
    mut terminal: DefaultTerminal,
    cli: &Cli,
    conn: Option<(tokio_postgres::Config, ConnLabel)>,
) -> color_eyre::Result<()> {
    let interval = Duration::from_secs_f64(cli.interval.max(0.5));
    let mut app = App::new();
    app.refresh_interval = interval;

    let (tx, mut actions) = mpsc::channel::<Action>(64);
    let _input_task = event::spawn_input(tx.clone());
    // The poller reads its cadence live from this watch channel; the loop
    // below mirrors `app.refresh_interval` into it whenever `+`/`-` change
    // it. Message-passing only — no shared Mutex.
    let (interval_tx, interval_rx) = watch::channel(interval);
    let snapshots = match conn {
        None => {
            app.host = "mock".to_string();
            pg_lens_core::poller::spawn_mock(interval_rx)
        }
        Some((config, label)) => {
            // The label is the only connection info the view ever sees —
            // host and user, never the password.
            app.host = label.to_string();
            pg_lens_core::poller::spawn(config, interval_rx)
        }
    };
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
