//! pg_lens_web — read-only axum server exposing the poller's snapshots.
//!
//! This crate is a *consumer* of `pg_lens_core`: it takes the same
//! `watch::Receiver<Arc<DbSnapshot>>` the TUI uses and serves it over HTTP.
//! It never talks to Postgres itself, never sees a DSN or password, and has
//! no write/admin endpoints (Fase 6 is read-only by design).
//!
//! Routes:
//! - `GET /` — placeholder page (the TypeScript frontend arrives in Fase 6b).
//! - `GET /api/snapshot` — the current snapshot as JSON.
//! - `GET /api/stream` — Server-Sent Events: the current snapshot
//!   immediately, then one event per poller tick. Pattern copied from the
//!   official `axum/examples/sse` (axum 0.8): a handler returning
//!   `Sse<impl Stream<Item = Result<Event, Infallible>>>` with a keep-alive.
//!
//! Security: when a bearer token is configured, every `/api/*` route
//! requires `Authorization: Bearer <token>`, compared in constant time via
//! `subtle::ConstantTimeEq` (a naive `==` short-circuits on the first
//! differing byte, which leaks how much of a guess was right). Deciding
//! *whether* a token is mandatory for a given bind address is
//! [`ensure_listen_allowed`] — the CLI must call it before binding.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use futures_util::stream::Stream;
use pg_lens_core::DbSnapshot;
use subtle::ConstantTimeEq;
use tokio::sync::watch;

/// Shared state: the snapshot channel plus the (optional) bearer token.
///
/// The token lives in an `Arc<str>` so cloning the state per-request is
/// cheap and the secret is never `Debug`-printed (no derive on purpose).
#[derive(Clone)]
struct WebState {
    snapshots: watch::Receiver<Arc<DbSnapshot>>,
    token: Option<Arc<str>>,
}

/// Refuses non-loopback binds without an auth token (Fase 6 requirement:
/// "sem token definido, o servidor recusa bind fora de localhost").
///
/// Loopback without a token is allowed — local use stays friction-free —
/// but exposing the server is an explicit two-step decision: pick the
/// address *and* set `PG_LENS_AUTH_TOKEN`.
pub fn ensure_listen_allowed(addr: &SocketAddr, has_token: bool) -> Result<(), String> {
    if addr.ip().is_loopback() || has_token {
        Ok(())
    } else {
        Err(format!(
            "refusing to listen on non-loopback address {addr} without authentication: \
             set PG_LENS_AUTH_TOKEN (all /api requests will then require \
             `Authorization: Bearer <token>`), or bind to 127.0.0.1"
        ))
    }
}

/// Runs the server on an already-bound listener until Ctrl+C (SIGINT).
///
/// Lives here (not in the CLI) so frontends never need their own axum
/// dependency: bind a `TcpListener`, build a [`router`], hand both over.
pub async fn serve(listener: tokio::net::TcpListener, router: Router) -> std::io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            // If installing the signal handler fails, pending() would hang
            // shutdown forever — surface it and keep serving instead.
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("warning: Ctrl+C handler unavailable ({error}); stop with SIGTERM/kill");
                std::future::pending::<()>().await;
            }
        })
        .await
}

/// Builds the full router. `auth_token` comes from the CLI (which reads
/// `PG_LENS_AUTH_TOKEN`); when `Some`, every `/api/*` route demands it.
pub fn router(snapshots: watch::Receiver<Arc<DbSnapshot>>, auth_token: Option<String>) -> Router {
    let state = WebState {
        snapshots,
        token: auth_token.map(Arc::from),
    };
    let api = Router::new()
        .route("/snapshot", get(snapshot))
        .route("/stream", get(stream))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));
    Router::new()
        .route("/", get(index))
        .nest("/api", api)
        .with_state(state)
}

/// Bearer-token gate for `/api/*`. No token configured → open (the CLI only
/// permits that on loopback). Configured → compare in constant time.
async fn require_auth(
    State(state): State<WebState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(expected) = &state.token else {
        return next.run(request).await;
    };
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    // `ct_eq` on byte slices: length mismatch returns false immediately
    // (only the token's *length* can leak), equal lengths compare every
    // byte before deciding.
    let authorized = presented
        .map(|token| bool::from(token.as_bytes().ct_eq(expected.as_bytes())))
        .unwrap_or(false);
    if authorized {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
    }
}

/// `GET /api/snapshot` — the latest snapshot as JSON. Built by hand (not
/// `axum::Json`) so a serialization failure yields a clean 500 instead of a
/// panic; the payload is the same `serde_json` output the TUI models carry.
async fn snapshot(State(state): State<WebState>) -> Response {
    // Clone the Arc out of the borrow guard immediately — holding the watch
    // read lock across .await points would block the poller's sends.
    let snapshot = state.snapshots.borrow().clone();
    match serde_json::to_string(&*snapshot) {
        Ok(body) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Err(error) => {
            // The models are plain data (no maps with non-string keys, no
            // fallible custom Serialize), so this is unreachable in
            // practice. The error itself carries no connection info.
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("snapshot serialization failed: {error}"),
            )
                .into_response()
        }
    }
}

/// `GET /api/stream` — SSE feed. The current snapshot is sent immediately
/// (via `mark_changed`), then one `data:` event per `watch.changed()`, i.e.
/// per poller tick. The stream ends when the poller drops the channel; the
/// keep-alive comment keeps proxies from timing out idle connections.
async fn stream(
    State(state): State<WebState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut receiver = state.snapshots.clone();
    // Force the first `changed()` to resolve right away so a new subscriber
    // renders without waiting up to one full poll interval.
    receiver.mark_changed();
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        loop {
            // `Err` = poller gone (sender dropped) → end the stream cleanly.
            receiver.changed().await.ok()?;
            let snapshot = receiver.borrow_and_update().clone();
            match serde_json::to_string(&*snapshot) {
                // Compact JSON has no raw newlines, so it is SSE-safe as a
                // single `data:` line.
                Ok(json) => return Some((Ok(Event::default().data(json)), receiver)),
                // Unreachable for our plain-data models; skip rather than
                // kill the stream if it ever happens.
                Err(_) => continue,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `GET /` — placeholder until Fase 6b embeds the built frontend.
async fn index() -> Html<&'static str> {
    Html(
        "<!doctype html>\
         <html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>pg_lens</title></head><body>\
         <h1>pg_lens web</h1>\
         <p>The web frontend arrives in phase 6b. Until then:</p>\
         <ul>\
         <li><a href=\"/api/snapshot\"><code>GET /api/snapshot</code></a> — current snapshot (JSON)</li>\
         <li><code>GET /api/stream</code> — live snapshots (Server-Sent Events)</li>\
         </ul>\
         </body></html>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Router over a fresh mock snapshot; returns the sender so tests keep
    /// the channel alive (and could publish updates if needed).
    fn mock_router(token: Option<&str>) -> (watch::Sender<Arc<DbSnapshot>>, Router) {
        let (tx, rx) = watch::channel(Arc::new(DbSnapshot::mock()));
        (tx, router(rx, token.map(str::to_string)))
    }

    async fn get_response(router: Router, uri: &str, bearer: Option<&str>) -> Response {
        let mut request = Request::builder().uri(uri);
        if let Some(token) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        router
            .oneshot(request.body(Body::empty()).expect("request builds"))
            .await
            .expect("infallible service")
    }

    #[tokio::test]
    async fn snapshot_returns_json_without_token_when_open() {
        let (_tx, router) = mock_router(None);
        let response = get_response(router, "/api/snapshot", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert!(value.get("vitals").is_some(), "snapshot carries vitals");
        assert!(value.get("activity").is_some(), "snapshot carries activity");
        // Security: no connection fields leak into the payload.
        let text = String::from_utf8_lossy(&body).to_lowercase();
        assert!(!text.contains("password"), "no password field in payload");
        assert!(!text.contains("dsn"), "no dsn field in payload");
    }

    #[tokio::test]
    async fn api_requires_token_when_configured() {
        let (_tx, router) = mock_router(Some("sekret"));
        let response = get_response(router, "/api/snapshot", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_rejects_wrong_token() {
        let (_tx, router) = mock_router(Some("sekret"));
        let response = get_response(router, "/api/snapshot", Some("nope")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_accepts_correct_token() {
        let (_tx, router) = mock_router(Some("sekret"));
        let response = get_response(router, "/api/snapshot", Some("sekret")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn stream_is_sse_and_gated_by_token() {
        let (_tx, router) = mock_router(Some("sekret"));
        let response = get_response(router.clone(), "/api/stream", Some("sekret")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );

        let unauthorized = get_response(router, "/api/stream", None).await;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn stream_delivers_current_snapshot_immediately() {
        let (_tx, router) = mock_router(None);
        let response = get_response(router, "/api/stream", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        // The first frame must arrive without any poller tick (mark_changed).
        let frame = response
            .into_body()
            .into_data_stream()
            .frame()
            .await
            .expect("one frame")
            .expect("frame ok");
        let data = frame.into_data().expect("data frame");
        let text = String::from_utf8_lossy(&data);
        assert!(text.starts_with("data:"), "SSE data event, got: {text}");
    }

    #[tokio::test]
    async fn index_is_open_html_even_with_token() {
        let (_tx, router) = mock_router(Some("sekret"));
        let response = get_response(router, "/", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("phase 6b"));
    }

    #[test]
    fn listen_rules() {
        let loopback: SocketAddr = "127.0.0.1:8080".parse().expect("addr");
        let loopback6: SocketAddr = "[::1]:8080".parse().expect("addr");
        let public: SocketAddr = "0.0.0.0:8080".parse().expect("addr");
        assert!(ensure_listen_allowed(&loopback, false).is_ok());
        assert!(ensure_listen_allowed(&loopback6, false).is_ok());
        assert!(ensure_listen_allowed(&public, true).is_ok());
        let error = ensure_listen_allowed(&public, false).expect_err("must refuse");
        assert!(error.contains("PG_LENS_AUTH_TOKEN"), "error names the env var");
    }
}
