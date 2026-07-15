//! pg_lens_web — read-only axum server exposing the poller's snapshots.
//!
//! This crate is a *consumer* of `pg_lens_core`: it takes the same
//! `watch::Receiver<Arc<DbSnapshot>>` the TUI uses and serves it over HTTP.
//! It never talks to Postgres itself, never sees a DSN or password, and has
//! no write/admin endpoints (Fase 6 is read-only by design).
//!
//! Routes:
//! - `GET /` + static assets — the built TypeScript frontend, embedded in
//!   the binary via `rust-embed` (never served from disk in release).
//! - `GET /api/snapshot` — the current snapshot as JSON.
//! - `GET /api/stream` — Server-Sent Events: the current snapshot
//!   immediately, then one event per poller tick. Pattern copied from the
//!   official `axum/examples/sse` (axum 0.8): a handler returning
//!   `Sse<impl Stream<Item = Result<Event, Infallible>>>` with a keep-alive.
//!
//! Security: when a bearer token is configured, every `/api/*` route
//! requires `Authorization: Bearer <token>`, compared in constant time via
//! `subtle::ConstantTimeEq` (a naive `==` short-circuits on the first
//! differing byte, which leaks how much of a guess was right). Because the
//! browser `EventSource` API cannot set request headers, `/api/*` also
//! accepts `?token=<token>` as an equivalent credential (same constant-time
//! comparison) — see [`require_auth`] for the trade-off. Deciding *whether*
//! a token is mandatory for a given bind address is
//! [`ensure_listen_allowed`] — the CLI must call it before binding.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::stream::Stream;
use pg_lens_core::{AdminCommand, DbSnapshot};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, watch};

/// Shared state: the snapshot channel, the (optional) bearer token, and the
/// two control channels into the poller (Fase #24 web parity).
///
/// The token lives in an `Arc<str>` so cloning the state per-request is
/// cheap and the secret is never `Debug`-printed (no derive on purpose).
#[derive(Clone)]
struct WebState {
    snapshots: watch::Receiver<Arc<DbSnapshot>>,
    token: Option<Arc<str>>,
    /// Bumped by `POST /api/schema/refresh` — the poller recollects the
    /// Schema Lens on its next tick (the web twin of the TUI's `R`).
    schema_refresh: watch::Sender<u64>,
    /// Admin commands (cancel/terminate) go here; the poller (sole DB owner)
    /// executes them. Only reachable when a token is configured.
    admin: mpsc::Sender<AdminCommand>,
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
/// `schema_refresh` and `admin` are the control channels into the poller;
/// admin routes additionally require a configured token (see [`admin`]).
pub fn router(
    snapshots: watch::Receiver<Arc<DbSnapshot>>,
    schema_refresh: watch::Sender<u64>,
    admin: mpsc::Sender<AdminCommand>,
    auth_token: Option<String>,
) -> Router {
    let state = WebState {
        snapshots,
        token: auth_token.map(Arc::from),
        schema_refresh,
        admin,
    };
    let api = Router::new()
        .route("/snapshot", get(snapshot))
        .route("/stream", get(stream))
        // Web parity with the TUI: recollect the Schema Lens (like `R`) and
        // signal backends (like `c`/`K`). Admin is gated a second time inside
        // the handler — it must never work without an auth token.
        .route("/schema/refresh", post(schema_refresh_handler))
        .route("/admin/cancel/{pid}", post(admin_cancel))
        .route("/admin/terminate/{pid}", post(admin_terminate))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));
    Router::new()
        .nest("/api", api)
        // Everything that is not /api is a frontend asset (or 404). The
        // frontend itself is public even when a token is set — it contains
        // no data, and it is where the user *enters* the token.
        .fallback(static_assets)
        .with_state(state)
}

/// Bearer-token gate for `/api/*`. No token configured → open (the CLI only
/// permits that on loopback). Configured → compare in constant time.
///
/// Two ways to present the token:
/// - `Authorization: Bearer <token>` — preferred; used by curl and by the
///   frontend's `fetch` calls.
/// - `?token=<token>` query parameter — exists **only** because the browser
///   `EventSource` API cannot set request headers, so `/api/stream` has no
///   other way to authenticate. Trade-off (documented in the README): a
///   token in the URL can land in reverse-proxy access logs and browser
///   history. Mitigations: this server never logs request URLs, the token
///   is a revocable shared secret (rotate by restarting with a new
///   `PG_LENS_AUTH_TOKEN`), and remote deploys are expected to sit behind
///   TLS. The standard alternative — a session cookie minted from the
///   bearer token — buys CSRF surface for little gain on a read-only,
///   single-secret API.
async fn require_auth(
    State(state): State<WebState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(expected) = &state.token else {
        return next.run(request).await;
    };
    let header_token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    let query_token = request
        .uri()
        .query()
        .and_then(|query| {
            form_urlencoded::parse(query.as_bytes()).find(|(key, _)| key == "token")
        })
        .map(|(_, value)| value.into_owned());
    // `ct_eq` on byte slices: length mismatch returns false immediately
    // (only the token's *length* can leak), equal lengths compare every
    // byte before deciding.
    let authorized = header_token
        .or(query_token)
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

/// `POST /api/schema/refresh` — bump the schema-refresh counter so the poller
/// recollects the Schema Lens (estimated bloat included) on its next tick.
/// The web twin of the TUI's `R`. Gated by [`require_auth`] like any `/api`
/// route; harmless enough to allow on open loopback.
async fn schema_refresh_handler(State(state): State<WebState>) -> Response {
    state.schema_refresh.send_modify(|n| *n += 1);
    (StatusCode::ACCEPTED, "schema refresh requested").into_response()
}

async fn admin_cancel(state: State<WebState>, pid: Path<i32>) -> Response {
    admin(state.0, AdminCommand::CancelBackend(pid.0)).await
}

async fn admin_terminate(state: State<WebState>, pid: Path<i32>) -> Response {
    admin(state.0, AdminCommand::TerminateBackend(pid.0)).await
}

/// Queues an admin command for the poller. **Requires a configured token**
/// even on loopback: signalling backends is destructive, so the web never
/// exposes it unauthenticated (the TUI, a local interactive tool, is the
/// unauthenticated path). The command's RESULT travels back in the normal
/// snapshot stream as `last_admin_action`, which the frontend surfaces.
async fn admin(state: WebState, command: AdminCommand) -> Response {
    if state.token.is_none() {
        return (
            StatusCode::FORBIDDEN,
            "admin actions require PG_LENS_AUTH_TOKEN to be set",
        )
            .into_response();
    }
    match state.admin.try_send(command) {
        Ok(()) => (StatusCode::ACCEPTED, "admin action queued").into_response(),
        // Channel full or the poller is gone — never block the request.
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "poller unavailable; try again",
        )
            .into_response(),
    }
}

/// The Vite-built frontend (`frontend/dist`), embedded at compile time.
/// `build.rs` guarantees the folder exists with a clear error otherwise.
/// In release builds the files live inside the binary (the Fase 6
/// anti-pattern list forbids serving the frontend from disk in production);
/// debug builds read the same folder from disk for fast iteration.
#[derive(rust_embed::Embed)]
#[folder = "frontend/dist"]
struct Assets;

/// Serves the embedded frontend: `/` → `index.html`, everything else by
/// path. MIME types come from rust-embed's `mime-guess` metadata (the
/// pattern shown in rust-embed's own axum example). Vite emits hashed
/// asset filenames, so `/assets/*` can be cached forever; `index.html`
/// (whose content references the hashes) must always revalidate.
async fn static_assets(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(file) => {
            let mime = file.metadata.mimetype().to_string();
            let cache = if path.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, cache.to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Router over a fresh mock snapshot; returns the snapshot sender plus the
    /// two control-channel ends so tests keep them alive AND can observe what
    /// the handlers pushed (the admin receiver, the schema-refresh counter).
    struct Harness {
        _snap_tx: watch::Sender<Arc<DbSnapshot>>,
        schema_refresh: watch::Receiver<u64>,
        admin_rx: mpsc::Receiver<AdminCommand>,
        router: Router,
    }

    fn mock_harness(token: Option<&str>) -> Harness {
        let (snap_tx, snap_rx) = watch::channel(Arc::new(DbSnapshot::mock()));
        let (schema_tx, schema_rx) = watch::channel(0u64);
        let (admin_tx, admin_rx) = mpsc::channel(8);
        let router = router(snap_rx, schema_tx, admin_tx, token.map(str::to_string));
        Harness {
            _snap_tx: snap_tx,
            schema_refresh: schema_rx,
            admin_rx,
            router,
        }
    }

    /// Back-compat shim for the read-only tests: just the sender + router.
    fn mock_router(token: Option<&str>) -> (watch::Sender<Arc<DbSnapshot>>, Router) {
        let h = mock_harness(token);
        (h._snap_tx, h.router)
    }

    async fn send(router: Router, method: &str, uri: &str, bearer: Option<&str>) -> Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        router
            .oneshot(request.body(Body::empty()).expect("request builds"))
            .await
            .expect("infallible service")
    }

    async fn get_response(router: Router, uri: &str, bearer: Option<&str>) -> Response {
        send(router, "GET", uri, bearer).await
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
    async fn api_accepts_token_query_param() {
        // EventSource cannot set headers, so ?token= must be equivalent.
        let (_tx, router) = mock_router(Some("sekret"));
        let ok = get_response(router.clone(), "/api/snapshot?token=sekret", None).await;
        assert_eq!(ok.status(), StatusCode::OK);
        let wrong = get_response(router.clone(), "/api/snapshot?token=nope", None).await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        let stream = get_response(router, "/api/stream?token=sekret", None).await;
        assert_eq!(stream.status(), StatusCode::OK);
        assert_eq!(stream.headers()[header::CONTENT_TYPE], "text/event-stream");
    }

    #[tokio::test]
    async fn index_is_open_html_even_with_token() {
        let (_tx, router) = mock_router(Some("sekret"));
        let response = get_response(router, "/", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .expect("ascii")
                .starts_with("text/html")
        );
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let html = String::from_utf8_lossy(&body);
        // The real Vite app: root div + hashed asset references.
        assert!(html.contains("id=\"app\""), "app root present");
        assert!(html.contains("/assets/"), "hashed asset links present");
    }

    #[tokio::test]
    async fn embedded_assets_serve_with_correct_mime_types() {
        let (_tx, router) = mock_router(None);
        // Discover the hashed asset names from the embedded index.html.
        let index = Assets::get("index.html").expect("index embedded");
        let html = String::from_utf8_lossy(&index.data);
        let js = extract_asset(&html, ".js").expect("index references a JS bundle");
        let css = extract_asset(&html, ".css").expect("index references a CSS bundle");

        let response = get_response(router.clone(), &format!("/{js}"), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .expect("ascii")
                .contains("javascript")
        );
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );

        let response = get_response(router.clone(), &format!("/{css}"), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/css");

        let missing = get_response(router, "/assets/nope.js", None).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    /// First `assets/…<ext>` path referenced by the built index.html.
    fn extract_asset(html: &str, ext: &str) -> Option<String> {
        html.match_indices("assets/").find_map(|(start, _)| {
            let candidate = &html[start..];
            let end = candidate.find('"')?;
            let path = &candidate[..end];
            path.ends_with(ext).then(|| path.to_string())
        })
    }

    #[tokio::test]
    async fn schema_refresh_bumps_the_counter() {
        let mut h = mock_harness(None);
        assert_eq!(*h.schema_refresh.borrow_and_update(), 0);
        let response = send(h.router.clone(), "POST", "/api/schema/refresh", None).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(h.schema_refresh.has_changed().unwrap());
        assert_eq!(*h.schema_refresh.borrow_and_update(), 1);
    }

    #[tokio::test]
    async fn admin_requires_a_configured_token() {
        // Open server (no token): admin is refused even though /api is open.
        let mut h = mock_harness(None);
        let response = send(h.router.clone(), "POST", "/api/admin/cancel/4242", None).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        // Nothing was queued.
        assert!(h.admin_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn admin_queues_command_with_token() {
        let mut h = mock_harness(Some("sekret"));
        // Wrong/no token is rejected by require_auth before the handler.
        let denied = send(h.router.clone(), "POST", "/api/admin/terminate/50", None).await;
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let ok = send(
            h.router.clone(),
            "POST",
            "/api/admin/terminate/50",
            Some("sekret"),
        )
        .await;
        assert_eq!(ok.status(), StatusCode::ACCEPTED);
        match h.admin_rx.try_recv() {
            Ok(AdminCommand::TerminateBackend(pid)) => assert_eq!(pid, 50),
            other => panic!("expected TerminateBackend(50), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn admin_cancel_carries_the_pid() {
        let mut h = mock_harness(Some("k"));
        let ok = send(h.router.clone(), "POST", "/api/admin/cancel/777", Some("k")).await;
        assert_eq!(ok.status(), StatusCode::ACCEPTED);
        assert!(matches!(
            h.admin_rx.try_recv(),
            Ok(AdminCommand::CancelBackend(777))
        ));
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
