//! Input/data tasks feeding the single `mpsc<Action>` channel of the UI.
//!
//! Two producers, one consumer (the `tokio::select!` loop in `main.rs`):
//! - [`spawn_input`]: crossterm `EventStream` → `Action::Key` / `Action::Resize`.
//! - [`spawn_snapshot_bridge`]: core `watch::Receiver<Arc<DbSnapshot>>` →
//!   `Action::Snapshot`. This is the only place where core data is converted
//!   into the TUI-internal `Action` type — the core never sees `Action`.

use std::sync::Arc;

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use pg_lens_core::DbSnapshot;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::app::Action;

/// Consumes the crossterm [`EventStream`] and forwards terminal events as
/// [`Action`]s. Ends when the stream closes/errors (sending `Action::Quit`)
/// or when the receiving side of the channel is dropped.
pub fn spawn_input(tx: mpsc::Sender<Action>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut events = EventStream::new();
        while let Some(event) = events.next().await {
            let action = match event {
                Ok(Event::Key(key)) => Action::Key(key),
                Ok(Event::Resize(_, _)) => Action::Resize,
                Ok(_) => continue,
                Err(_) => Action::Quit,
            };
            if tx.send(action).await.is_err() {
                return; // UI gone: stop reading input.
            }
        }
        // Input stream closed: nothing left to react to.
        let _ = tx.send(Action::Quit).await;
    })
}

/// Bridges the core's watch channel into the UI's mpsc: forwards the initial
/// snapshot immediately (the watch channel is born pre-filled), then one
/// `Action::Snapshot` per `changed()` notification. Ends when the poller
/// drops the sender or the UI drops the receiver.
pub fn spawn_snapshot_bridge(
    mut rx: watch::Receiver<Arc<DbSnapshot>>,
    tx: mpsc::Sender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let initial = rx.borrow_and_update().clone();
        if tx.send(Action::Snapshot(initial)).await.is_err() {
            return;
        }
        while rx.changed().await.is_ok() {
            let snapshot = rx.borrow_and_update().clone();
            if tx.send(Action::Snapshot(snapshot)).await.is_err() {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// watch → bridge → mpsc: the initial value and every subsequent
    /// `send` on the watch side must come out as `Action::Snapshot`.
    #[tokio::test]
    async fn bridge_forwards_initial_and_updated_snapshots() {
        let first = Arc::new(DbSnapshot::mock());
        let second = Arc::new(DbSnapshot::mock());
        let (watch_tx, watch_rx) = watch::channel(Arc::clone(&first));
        let (tx, mut rx) = mpsc::channel(8);

        let _bridge = spawn_snapshot_bridge(watch_rx, tx);

        async fn recv(rx: &mut mpsc::Receiver<Action>) -> Action {
            tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("bridge must forward within 2s")
                .expect("channel must stay open")
        }

        match recv(&mut rx).await {
            Action::Snapshot(snap) => assert!(Arc::ptr_eq(&snap, &first)),
            other => panic!("expected initial Snapshot, got {other:?}"),
        }

        watch_tx.send(Arc::clone(&second)).expect("bridge alive");
        match recv(&mut rx).await {
            Action::Snapshot(snap) => assert!(Arc::ptr_eq(&snap, &second)),
            other => panic!("expected updated Snapshot, got {other:?}"),
        }
    }
}
