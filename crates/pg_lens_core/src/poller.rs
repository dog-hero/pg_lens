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

use tokio::sync::watch;
use tokio_postgres::{Client, Statement};

use crate::history::{HistoryPoint, SnapshotHistory, epoch_ms_now};
use crate::models::{DbSnapshot, PollerStatus, ServerVitals};
use crate::{db, queries};

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

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

/// Spawns the real poller: connect to `dsn`, detect the server version, pick
/// the matching [`queries::QuerySet`], prepare the statements **once**, then
/// publish one [`DbSnapshot`] per poll. The cadence is read live from
/// `interval_rx` before every sleep.
///
/// On any connect/query error the poller publishes a snapshot carrying
/// `PollerStatus::Error(..)` while *keeping the last good data*, then
/// reconnects with exponential backoff (1s, 2s, 4s ... max 10s).
///
/// The channel starts pre-filled with a [`DbSnapshot::connecting`] value.
/// The task ends on its own once every receiver has been dropped.
///
/// # Panics
///
/// Must be called from within a tokio runtime (it calls `tokio::spawn`).
pub fn spawn(
    dsn: String,
    interval_rx: watch::Receiver<Duration>,
) -> watch::Receiver<Arc<DbSnapshot>> {
    let (tx, rx) = watch::channel(Arc::new(DbSnapshot::connecting()));
    tokio::spawn(run(dsn, interval_rx, tx));
    rx
}

/// Outer reconnect loop: one [`session`] per connection, backoff in between.
async fn run(
    dsn: String,
    mut interval_rx: watch::Receiver<Duration>,
    tx: watch::Sender<Arc<DbSnapshot>>,
) {
    let mut backoff = BACKOFF_INITIAL;
    // Survives reconnects so the sparklines don't reset on a blip.
    let mut history = SnapshotHistory::default();
    loop {
        let mut polled_ok = false;
        match session(&dsn, &mut interval_rx, &tx, &mut history, &mut polled_ok).await {
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
async fn session(
    dsn: &str,
    interval_rx: &mut watch::Receiver<Duration>,
    tx: &watch::Sender<Arc<DbSnapshot>>,
    history: &mut SnapshotHistory,
    polled_ok: &mut bool,
) -> SessionEnd {
    let (client, conn_handle) = match db::connect(dsn).await {
        Ok(pair) => pair,
        Err(e) => return SessionEnd::Error(format!("connect failed: {e}")),
    };
    let end = poll_loop(&client, interval_rx, tx, history, polled_ok).await;
    conn_handle.abort();
    end
}

/// The three statements of a session, prepared once (never per tick).
struct Prepared {
    activity: Statement,
    blocking: Statement,
    server_info: Statement,
}

async fn poll_loop(
    client: &Client,
    interval_rx: &mut watch::Receiver<Duration>,
    tx: &watch::Sender<Arc<DbSnapshot>>,
    history: &mut SnapshotHistory,
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
    ) {
        Ok((activity, blocking, server_info)) => Prepared {
            activity,
            blocking,
            server_info,
        },
        Err(e) => return SessionEnd::Error(format!("prepare failed: {e}")),
    };

    let mut deltas: Option<DeltaState> = None;

    // First iteration polls immediately (right after connecting); every
    // later one sleeps for the *current* interval first.
    loop {
        if tx.is_closed() {
            return SessionEnd::Closed;
        }
        let snapshot = match poll_once(client, &stmts, &mut deltas, history).await {
            Ok(s) => s,
            Err(msg) => return SessionEnd::Error(format!("poll failed: {msg}")),
        };
        *polled_ok = true;
        if tx.send(Arc::new(snapshot)).is_err() {
            return SessionEnd::Closed;
        }
        wait_interval(interval_rx).await;
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
    // Three futures on one client: tokio-postgres pipelines them.
    let (activity_rows, blocking_rows, info_rows) = tokio::try_join!(
        client.query(&stmts.activity, &[]),
        client.query(&stmts.blocking, &[]),
        client.query(&stmts.server_info, &[]),
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
        status: PollerStatus::Ok,
    })
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
/// The task ends on its own once every receiver (including clones) has been
/// dropped — `watch::Sender::send` returns `Err` when the channel is closed.
///
/// # Panics
///
/// Must be called from within a tokio runtime (it calls `tokio::spawn`).
pub fn spawn_mock(
    mut interval_rx: watch::Receiver<Duration>,
) -> watch::Receiver<Arc<DbSnapshot>> {
    // The mock poller owns the ring exactly like the real one.
    let mut history = SnapshotHistory::default();
    let mut first = DbSnapshot::mock();
    record_history(&mut history, &mut first);
    let (tx, rx) = watch::channel(Arc::new(first));

    tokio::spawn(async move {
        loop {
            wait_interval(&mut interval_rx).await;
            let mut snapshot = DbSnapshot::mock();
            record_history(&mut history, &mut snapshot);
            if tx.send(Arc::new(snapshot)).is_err() {
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

    fn interval_rx(ms: u64) -> watch::Receiver<Duration> {
        let (tx, rx) = watch::channel(Duration::from_millis(ms));
        // Leak the sender so the interval stays adjustable-shaped but fixed;
        // wait_interval must tolerate a dropped sender anyway (tested below).
        std::mem::forget(tx);
        rx
    }

    /// The poller must publish at least two snapshots that differ from each
    /// other (and from the initial value) — bounded by a timeout, no sleeps.
    #[tokio::test]
    async fn spawn_mock_publishes_distinct_snapshots() {
        let mut rx = spawn_mock(interval_rx(10));

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
        let mut rx = spawn_mock(interval_rx(10));

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
        let mut rx = spawn_mock(interval_rx);
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
        let mut rx = spawn_mock(interval_rx);
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

    /// An unreachable DSN must not panic: the real poller publishes an error
    /// snapshot (keeping the channel alive) instead.
    #[tokio::test]
    async fn spawn_real_reports_connect_errors_via_status() {
        // Port 1 on localhost: connection refused, immediately.
        let mut rx = spawn(
            "host=127.0.0.1 port=1 user=nobody connect_timeout=1".to_string(),
            interval_rx(50),
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

    /// Deriving TPS/cache hit: the delta math is exercised through
    /// `hit_ratio` here; the end-to-end path needs a live server (verified in
    /// the Fase 3/4 e2e runs against Docker).
    #[test]
    fn hit_ratio_guards_division_by_zero() {
        assert_eq!(hit_ratio(0, 0), 0.0);
        assert_eq!(hit_ratio(90, 10), 0.9);
    }
}
