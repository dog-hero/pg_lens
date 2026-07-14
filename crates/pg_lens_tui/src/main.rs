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
    app.host = if cli.mock {
        "mock".to_string()
    } else {
        dsn_host(&cli.dsn)
    };

    let (tx, mut actions) = mpsc::channel::<Action>(64);
    let _input_task = event::spawn_input(tx.clone());
    // The poller reads its cadence live from this watch channel; the loop
    // below mirrors `app.refresh_interval` into it whenever `+`/`-` change
    // it. Message-passing only — no shared Mutex.
    let (interval_tx, interval_rx) = watch::channel(interval);
    let snapshots = if cli.mock {
        pg_lens_core::poller::spawn_mock(interval_rx)
    } else {
        pg_lens_core::poller::spawn(cli.dsn, interval_rx)
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

/// Extracts a display-safe host from the DSN for the header. Understands
/// both libpq `key=value` strings and `postgres://` URLs; never returns
/// credentials. Falls back to `localhost` (libpq's own default).
fn dsn_host(dsn: &str) -> String {
    let trimmed = dsn.trim();
    // URL form: postgres://user:pass@host:port/db?params
    if let Some(rest) = trimmed
        .strip_prefix("postgres://")
        .or_else(|| trimmed.strip_prefix("postgresql://"))
    {
        let authority = rest.split(['/', '?']).next().unwrap_or(rest);
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        let host = host_port.split(':').next().unwrap_or(host_port);
        if !host.is_empty() {
            return host.to_string();
        }
        return "localhost".to_string();
    }
    // key=value form: host=example.com port=5432 ...
    for pair in trimmed.split_whitespace() {
        if let Some(value) = pair.strip_prefix("host=") {
            let value = value.trim_matches('\'');
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    "localhost".to_string()
}

#[cfg(test)]
mod tests {
    use super::dsn_host;

    #[test]
    fn host_from_key_value_dsn() {
        assert_eq!(dsn_host("host=db.prod.internal user=app"), "db.prod.internal");
        assert_eq!(dsn_host("user=app host='10.0.0.7' port=6432"), "10.0.0.7");
    }

    #[test]
    fn host_from_url_dsn_without_leaking_credentials() {
        assert_eq!(dsn_host("postgres://alice:s3cret@db.example.com:5432/app"), "db.example.com");
        assert_eq!(dsn_host("postgresql://db.example.com/app?sslmode=require"), "db.example.com");
    }

    #[test]
    fn host_defaults_to_localhost() {
        assert_eq!(dsn_host("user=postgres"), "localhost");
        assert_eq!(dsn_host("postgres:///app"), "localhost");
    }
}
