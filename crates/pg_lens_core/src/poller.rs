//! Snapshot poller: a background task publishing `Arc<DbSnapshot>` through a
//! `tokio::sync::watch` channel ("last value wins", N consumers).
//!
//! Fase 2 ships the mock flavor only; Fase 3 replaces the data source with
//! real PostgreSQL queries while keeping this exact contract. This module is
//! frontend-agnostic: it knows nothing about terminal libraries or about any
//! frontend's internal message types.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::models::DbSnapshot;

/// Spawns a task that publishes a fresh [`DbSnapshot::mock`] every
/// `interval`, and returns the receiving side of the watch channel.
///
/// The channel starts pre-filled with one snapshot, so consumers can render
/// immediately with `Receiver::borrow` before the first `changed()` fires
/// (this is the documented `tokio::sync::watch` pattern: the initial value is
/// *not* marked as seen-changed).
///
/// The task ends on its own once every receiver (including clones) has been
/// dropped — `watch::Sender::send` returns `Err` when the channel is closed.
///
/// # Panics
///
/// Must be called from within a tokio runtime (it calls `tokio::spawn`), and
/// `interval` must be non-zero (`tokio::time::interval` requirement).
pub fn spawn_mock(interval: Duration) -> watch::Receiver<Arc<DbSnapshot>> {
    let (tx, rx) = watch::channel(Arc::new(DbSnapshot::mock()));

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // The first tick of `tokio::time::interval` completes immediately;
        // skip it so the pre-filled value stands for one full interval.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if tx.send(Arc::new(DbSnapshot::mock())).is_err() {
                // All receivers dropped: nobody is watching, stop polling.
                break;
            }
        }
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The poller must publish at least two snapshots that differ from each
    /// other (and from the initial value) — bounded by a timeout, no sleeps.
    #[tokio::test]
    async fn spawn_mock_publishes_distinct_snapshots() {
        let mut rx = spawn_mock(Duration::from_millis(10));

        let initial = serde_json::to_string(&*rx.borrow().clone()).expect("serialize");

        let mut published = Vec::new();
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(2), rx.changed())
                .await
                .expect("poller must publish within 2s")
                .expect("sender must still be alive");
            published.push(serde_json::to_string(&*rx.borrow_and_update().clone()).expect("serialize"));
        }

        assert_ne!(published[0], initial);
        assert_ne!(published[1], initial);
        assert_ne!(published[0], published[1]);
    }
}
