//! pg_lens TUI entry point.
//!
//! Pattern copied from the ratatui `event-driven-async` template:
//! `ratatui::init()` (which installs a panic hook that restores the
//! terminal) + `ratatui::restore()` around the async run loop.
//!
//! Fase 2 pipeline: the crossterm `EventStream` lives in its own task
//! (`event::spawn_input`), the core mock poller publishes through a `watch`
//! channel, and a bridge task (`event::spawn_snapshot_bridge`) converts
//! snapshots into `Action`s. The loop below selects over the single
//! `mpsc<Action>` receiver and a render tick.

use std::time::Duration;

use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::app::{Action, App, update};

mod app;
mod event;
mod ui;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = run(terminal).await;
    ratatui::restore();
    result
}

async fn run(mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
    let mut app = App::new();

    let (tx, mut actions) = mpsc::channel::<Action>(64);
    let _input_task = event::spawn_input(tx.clone());
    let snapshots = pg_lens_core::poller::spawn_mock(app.refresh_interval);
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
