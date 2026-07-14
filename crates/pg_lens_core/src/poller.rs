//! Snapshot poller: a background task publishing `Arc<DbSnapshot>` through a
//! `tokio::sync::watch` channel ("last value wins", N consumers).
//!
//! Two flavors share the exact same contract:
//! - [`spawn`] — the real data layer (Fase 3): tokio-postgres session,
//!   versioned queries prepared once, delta-derived metrics, reconnect with
//!   backoff, and errors carried inside the snapshot (`PollerStatus`).
//! - [`spawn_mock`] — fake data for development/e2e without a database.
//!
//! This module is frontend-agnostic: it knows nothing about terminal
//! libraries or about any frontend's internal message types.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio_postgres::{Client, Statement};

use crate::models::{DbSnapshot, PollerStatus, ServerVitals};
use crate::{db, queries};

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(10);
/// Ring cap for the TPS sparkline history (Fase 4 grows this into a proper
/// `SnapshotHistory`; a capped Vec is enough for now).
const TPS_HISTORY_CAP: usize = 120;

/// Spawns the real poller: connect to `dsn`, detect the server version, pick
/// the matching [`queries::QuerySet`], prepare the statements **once**, then
/// publish one [`DbSnapshot`] per `interval` tick.
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
/// Must be called from within a tokio runtime (it calls `tokio::spawn`), and
/// `interval` must be non-zero (`tokio::time::interval` requirement).
pub fn spawn(dsn: String, interval: Duration) -> watch::Receiver<Arc<DbSnapshot>> {
    let (tx, rx) = watch::channel(Arc::new(DbSnapshot::connecting()));
    tokio::spawn(run(dsn, interval, tx));
    rx
}

/// Outer reconnect loop: one [`session`] per connection, backoff in between.
async fn run(dsn: String, interval: Duration, tx: watch::Sender<Arc<DbSnapshot>>) {
    let mut backoff = BACKOFF_INITIAL;
    // Survives reconnects so the sparkline doesn't reset on a blip.
    let mut tps_history: Vec<u64> = Vec::new();
    loop {
        let mut polled_ok = false;
        match session(&dsn, interval, &tx, &mut tps_history, &mut polled_ok).await {
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
    interval: Duration,
    tx: &watch::Sender<Arc<DbSnapshot>>,
    tps_history: &mut Vec<u64>,
    polled_ok: &mut bool,
) -> SessionEnd {
    let (client, conn_handle) = match db::connect(dsn).await {
        Ok(pair) => pair,
        Err(e) => return SessionEnd::Error(format!("connect failed: {e}")),
    };
    let end = poll_loop(&client, interval, tx, tps_history, polled_ok).await;
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
    interval: Duration,
    tx: &watch::Sender<Arc<DbSnapshot>>,
    tps_history: &mut Vec<u64>,
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

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut deltas: Option<DeltaState> = None;

    loop {
        // First tick completes immediately: poll right after connecting.
        ticker.tick().await;
        if tx.is_closed() {
            return SessionEnd::Closed;
        }
        let snapshot = match poll_once(client, &stmts, &mut deltas, tps_history).await {
            Ok(s) => s,
            Err(msg) => return SessionEnd::Error(format!("poll failed: {msg}")),
        };
        *polled_ok = true;
        if tx.send(Arc::new(snapshot)).is_err() {
            return SessionEnd::Closed;
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
}

async fn poll_once(
    client: &Client,
    stmts: &Prepared,
    deltas: &mut Option<DeltaState>,
    tps_history: &mut Vec<u64>,
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

    tps_history.push(tps.max(0.0).round() as u64);
    if tps_history.len() > TPS_HISTORY_CAP {
        let excess = tps_history.len() - TPS_HISTORY_CAP;
        tps_history.drain(..excess);
    }

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
        tps_history: tps_history.clone(),
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
            published
                .push(serde_json::to_string(&*rx.borrow_and_update().clone()).expect("serialize"));
        }

        assert_ne!(published[0], initial);
        assert_ne!(published[1], initial);
        assert_ne!(published[0], published[1]);
    }

    /// An unreachable DSN must not panic: the real poller publishes an error
    /// snapshot (keeping the channel alive) instead.
    #[tokio::test]
    async fn spawn_real_reports_connect_errors_via_status() {
        // Port 1 on localhost: connection refused, immediately.
        let mut rx = spawn(
            "host=127.0.0.1 port=1 user=nobody connect_timeout=1".to_string(),
            Duration::from_millis(50),
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
    /// the Fase 3 e2e run against Docker).
    #[test]
    fn hit_ratio_guards_division_by_zero() {
        assert_eq!(hit_ratio(0, 0), 0.0);
        assert_eq!(hit_ratio(90, 10), 0.9);
    }
}
