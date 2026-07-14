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
//! tick. The DSN string is handed to the core as-is and never logged.

use std::time::Duration;

use clap::Parser;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::app::{Action, App, update};

mod app;
mod event;
mod ui;

/// A blazing-fast, modern TUI for PostgreSQL observability.
#[derive(Debug, Parser)]
#[command(name = "pg_lens", version, about)]
struct Cli {
    /// PostgreSQL connection string (`key=value` DSN or `postgres://` URL).
    #[arg(
        long,
        env = "PG_LENS_DSN",
        default_value = "host=localhost user=postgres"
    )]
    dsn: String,

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
    let terminal = ratatui::init();
    let result = run(terminal, cli).await;
    ratatui::restore();
    result
}

async fn run(mut terminal: DefaultTerminal, cli: Cli) -> color_eyre::Result<()> {
    let interval = Duration::from_secs_f64(cli.interval.max(0.5));
    let mut app = App::new();
    app.refresh_interval = interval;

    let (tx, mut actions) = mpsc::channel::<Action>(64);
    let _input_task = event::spawn_input(tx.clone());
    let snapshots = if cli.mock {
        pg_lens_core::poller::spawn_mock(interval)
    } else {
        pg_lens_core::poller::spawn(cli.dsn, interval)
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
    }

    Ok(())
}
