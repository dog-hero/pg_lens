//! pg_lens entry point: one binary, two frontends.
//!
//! `pg_lens tui` (the default when no subcommand is given — bare
//! `pg_lens --mock` keeps working) runs the terminal UI; `pg_lens serve`
//! (behind the default-on `web` cargo feature) runs the pg_lens_web axum
//! server. Both share the same connection flags and the exact same core
//! pipeline: `settings::resolve` → `poller::spawn` → `watch<Arc<DbSnapshot>>`.
//!
//! TUI pattern copied from the ratatui `event-driven-async` template:
//! `ratatui::init()` (which installs a panic hook that restores the
//! terminal) + `ratatui::restore()` around the async run loop. The crossterm
//! `EventStream` lives in its own task (`event::spawn_input`), the core
//! poller publishes through a `watch` channel, and a bridge task
//! (`event::spawn_snapshot_bridge`) converts snapshots into `Action`s.
//!
//! Connection resolution (Fase C1) happens in `pg_lens_core::settings`:
//! `--dsn` fields win, the libpq env vars (`PGHOST`, `PGPORT`, `PGDATABASE`,
//! `PGUSER`, `PGPASSWORD`, `PGAPPNAME`, `PGCONNECT_TIMEOUT`) fill the gaps,
//! and `host=localhost user=postgres` is the fallback. The environment is
//! captured *here*, once, and injected — the core never reads `std::env`.
//! The resolved `Config` (which may carry a password) is handed to the core
//! as-is and never logged; only the safe `ConnLabel` reaches any output.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use pg_lens_core::DbSnapshot;
use pg_lens_core::services::ServicesFile;
use pg_lens_core::settings::{self, ConnSpec, Resolved, ServiceSummary};
use ratatui::DefaultTerminal;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use pg_lens_core::AdminCommand;

use crate::app::{Action, App, PickerEntry, PickerState, update};

mod app;
mod event;
mod psql;
mod ui;

/// A blazing-fast, modern TUI (and web server) for PostgreSQL observability.
#[derive(Debug, Parser)]
#[command(name = "pg_lens", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Back-compat: the pre-subcommand flat CLI (`pg_lens --mock`,
    /// `pg_lens --dsn ...`) still parses and means `tui`.
    #[command(flatten)]
    conn: ConnArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the terminal UI (the default when no subcommand is given). The
    /// connection flags are global (see [`ConnArgs`]), so they live on the
    /// top-level `Cli` — this variant carries none of its own.
    Tui,

    /// Serve the web UI and JSON/SSE API over HTTP (read-only).
    #[cfg(feature = "web")]
    Serve(ServeArgs),
}

/// Connection flags shared by every subcommand. Every flag is `global`, so it
/// works in any position — `pg_lens --service x serve`, `pg_lens serve
/// --service x`, and the bare `pg_lens --service x` (= `tui`) all resolve the
/// same value. (Before, a flag placed BEFORE a subcommand bound to the
/// top-level back-compat copy and was silently ignored by the subcommand.)
#[derive(Debug, Args)]
struct ConnArgs {
    /// PostgreSQL connection string (`key=value` DSN or `postgres://` URL).
    /// Fields not set here fall back to the libpq env vars (PGHOST, PGPORT,
    /// PGDATABASE, PGUSER, PGPASSWORD, PGAPPNAME, PGCONNECT_TIMEOUT), then
    /// to `host=localhost user=postgres`.
    #[arg(long, env = "PG_LENS_DSN", global = true)]
    dsn: Option<String>,

    /// Connect using a named entry from the services file. Also read from
    /// the PG_LENS_SERVICE env var, falling back to PGSERVICE. Mutually
    /// exclusive with --dsn.
    #[arg(long, value_name = "NAME", conflicts_with = "dsn", global = true)]
    service: Option<String>,

    /// Path to the services file. Defaults to
    /// $XDG_CONFIG_HOME/pg_lens/services.toml (or
    /// ~/.config/pg_lens/services.toml); also read from the
    /// PG_LENS_SERVICES_FILE env var.
    #[arg(long, value_name = "PATH", global = true)]
    services_file: Option<PathBuf>,

    /// Print the services defined in the services file (names + host/user,
    /// never passwords) and exit.
    #[arg(long, global = true)]
    list_services: bool,

    /// Poll interval in seconds (minimum 0.5). [default: 2, or config.toml]
    #[arg(long, env = "PG_LENS_INTERVAL", global = true)]
    interval: Option<f64>,

    /// Schema Lens collection interval in seconds (minimum 5): table stats
    /// and sizes are expensive, so they run on this slow cadence — never on
    /// the fast tick. [default: 60, or config.toml]
    #[arg(
        long,
        value_name = "SECS",
        env = "PG_LENS_SCHEMA_INTERVAL",
        global = true
    )]
    schema_interval: Option<u64>,

    /// Use built-in mock data instead of a real database (dev/demo mode).
    #[arg(long, global = true)]
    mock: bool,

    /// Refuse every mutating/admin action (`c` cancel, `K` terminate in the
    /// TUI; `POST /api/admin/*` in `serve`) regardless of the connected
    /// role's actual privileges — for shared/audited deployments and
    /// least-privilege roles (pairs with `docs/connection-user.md`). Also
    /// settable via `PG_LENS_READ_ONLY` (any value other than empty/`0`/
    /// `false`/`no`/`off`, case-insensitive) or `read_only = true` in
    /// config.toml. Enforced in `update()` and the web admin handlers — not
    /// just hidden in the UI. [default: false, or config.toml]
    #[arg(long, global = true)]
    read_only: bool,

    /// Load the services file (see `--services-file`) from a remote,
    /// read-only source instead of (or merged with) the local file — for
    /// teams that want to share one curated connection list. Two schemes:
    /// `github:OWNER/REPO/PATH[@REF]` (GitHub's Contents API — works for
    /// private repos given a token; `REF` optional, defaults to the repo's
    /// default branch) or a plain `https://`/`http://` URL to raw file
    /// bytes (self-hosted git, GitLab raw, ...). Auth token, in order:
    /// `PG_LENS_CONFIG_TOKEN`, `GITHUB_TOKEN`, then a `remote_config_token_cmd`
    /// in config.toml (mirrors `password_cmd` — an external command, trimmed
    /// stdout; never a literal token in any file). Sent as `Authorization:
    /// Bearer <token>` and refused over plain `http://`. Fetched once at
    /// startup (10s timeout) and cached at
    /// `$XDG_CACHE_HOME/pg_lens/remote-services.toml`; a failed fetch falls
    /// back to that cache (or the local file alone) with a warning — it
    /// never blocks startup on a flaky network. When both a local file and
    /// this remote source define a service, the remote entry wins. Also
    /// settable via `PG_LENS_CONFIG_URL` or `remote_config` in config.toml.
    #[arg(long, value_name = "URL", env = "PG_LENS_CONFIG_URL", global = true)]
    config_url: Option<String>,
}

/// Loose boolean parse for `PG_LENS_READ_ONLY` — a plain presence-style env
/// var (unlike the value-carrying `PG*`/`PG_LENS_*` vars elsewhere in this
/// file), so it accepts common truthy/falsy spellings rather than requiring
/// exactly `"true"`. Consistent with `settings.rs`'s "empty = unset".
fn is_truthy(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

#[cfg(feature = "web")]
#[derive(Debug, Args)]
struct ServeArgs {
    /// Address to bind. Non-loopback addresses are refused unless
    /// PG_LENS_AUTH_TOKEN is set (all /api routes then require
    /// `Authorization: Bearer <token>`). [default: 127.0.0.1:8080, or config.toml]
    #[arg(long, value_name = "ADDR", env = "PG_LENS_LISTEN")]
    listen: Option<std::net::SocketAddr>,
}

impl ConnArgs {
    /// Everything `settings::resolve` needs; the process environment is
    /// captured exactly once, here, and injected.
    fn spec(&self) -> ConnSpec {
        ConnSpec {
            dsn: self.dsn.clone(),
            service: self.service.clone(),
            services_file: self.services_file.clone(),
            env: std::env::vars().collect::<HashMap<_, _>>(),
            services_override: None,
        }
    }

    /// [`Self::spec`] plus a pre-resolved `--config-url` overlay (see
    /// [`Self::resolve_remote_overlay`]) — the merged local+remote services
    /// set every connection-resolving call site (`--list-services`, the
    /// picker, `resolve`) must share, so they all agree on what
    /// `--config-url` actually resolved to.
    fn spec_with(&self, overlay: Option<&ServicesFile>) -> ConnSpec {
        let mut spec = self.spec();
        spec.services_override = overlay.cloned();
        spec
    }

    /// --list-services: plain stdout, no TUI/server. Names + host/user only
    /// — a password or password_cmd never reaches this output. `overlay` is
    /// `--config-url`'s merged result, if configured (see
    /// [`Self::resolve_remote_overlay`]) — when set, this lists exactly what
    /// will be resolved, remote entries included.
    fn list_services(&self, overlay: Option<&ServicesFile>) -> color_eyre::Result<()> {
        let (services, warnings) = settings::list_services(&self.spec_with(overlay))?;
        for warning in &warnings {
            eprintln!("warning: {warning}");
        }
        for service in services {
            println!(
                "{name}\thost={host}\tuser={user}",
                name = service.name,
                host = service.host.as_deref().unwrap_or("-"),
                user = service.user.as_deref().unwrap_or("-"),
            );
        }
        Ok(())
    }

    /// Resolves the connection (unless `--mock`), printing warnings to
    /// stderr. Shared verbatim by `tui` and `serve` — same resolution, same
    /// poller, per the "one core pipeline" invariant. `overlay` is
    /// `--config-url`'s merged result, if configured.
    fn resolve(&self, overlay: Option<&ServicesFile>) -> color_eyre::Result<Option<Resolved>> {
        if self.mock {
            return Ok(None);
        }
        let resolved = settings::resolve(&self.spec_with(overlay))?;
        for warning in &resolved.warnings {
            eprintln!("warning: {warning}");
        }
        Ok(Some(resolved))
    }

    /// Loads `config.toml` (best-effort), printing any non-fatal warnings.
    /// The file supplies defaults for values not given by a flag or env var.
    fn app_config(&self) -> settings::AppConfig {
        let (config, warnings) = settings::load_app_config(&self.spec());
        for warning in &warnings {
            eprintln!("warning: {warning}");
        }
        config
    }

    /// `--interval`, then `PG_LENS_INTERVAL` (both via clap), then
    /// `config.toml`, then the 2s default — floored at 0.5s.
    fn interval(&self, config: &settings::AppConfig) -> Duration {
        let secs = self.interval.or(config.interval).unwrap_or(2.0);
        Duration::from_secs_f64(secs.max(0.5))
    }

    /// `--schema-interval`, then env, then `config.toml`, then the 60s
    /// default — floored at the core's sanity minimum (5s).
    fn schema_interval(&self, config: &settings::AppConfig) -> Duration {
        let secs = self.schema_interval.or(config.schema_interval).unwrap_or(60);
        Duration::from_secs(secs).max(pg_lens_core::poller::SCHEMA_INTERVAL_MIN)
    }

    /// `--read-only`, then `PG_LENS_READ_ONLY`, then `config.toml`'s
    /// `read_only`, then `false` — same flag → env → config → default
    /// precedence as `interval`/`schema_interval` above. Delegates to the
    /// pure [`resolve_read_only`] so the precedence itself is testable
    /// without touching real process env (same "environment is injected"
    /// discipline `settings.rs` uses).
    fn read_only(&self, config: &settings::AppConfig) -> bool {
        resolve_read_only(self.read_only, &self.spec().env, config.read_only)
    }

    /// `--config-url`, then `PG_LENS_CONFIG_URL` (both already merged into
    /// `self.config_url` by clap's `env = ...`, same as `--dsn`), then
    /// `config.toml`'s `remote_config`. `None`/empty means "no remote
    /// config configured" — every other call site keeps behaving exactly as
    /// it did before this feature existed.
    fn config_url(&self, config: &settings::AppConfig) -> Option<String> {
        self.config_url
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| config.remote_config.clone().filter(|s| !s.is_empty()))
    }

    /// The full `--config-url` startup flow: parse → resolve the token →
    /// fetch (10s timeout, off the async runtime via `spawn_blocking`) →
    /// cache on success / fall back to the cache or the local file on
    /// failure → merge over the local `services.toml` (remote wins by
    /// name). Warnings go to stderr as they happen (same convention as
    /// every other `warning: ...` line this binary prints); `Ok(None)`
    /// means `--config-url` is not configured at all — the classic,
    /// disk-only path. Only genuinely fatal cases return `Err`: a bad URL,
    /// a token over `http://`, a failed `token_cmd`, or a fetch failure
    /// with neither a cache nor a local file to fall back on.
    ///
    /// This is deliberately thin I/O glue: the actual precedence/merge
    /// logic is [`pg_lens_core::remote_config::resolve_effective_services`],
    /// which is pure and unit-tested in `pg_lens_core` without any network
    /// or filesystem access.
    async fn resolve_remote_overlay(
        &self,
        config: &settings::AppConfig,
    ) -> color_eyre::Result<Option<ServicesFile>> {
        use color_eyre::eyre::eyre;

        let Some(raw_url) = self.config_url(config) else {
            return Ok(None);
        };
        let spec = self.spec();
        let url = pg_lens_core::remote_config::parse_config_url(&raw_url)?;
        let token =
            resolve_config_token(&spec.env, config.remote_config_token_cmd.as_deref()).await?;

        let cache_path = remote_config_cache_path();
        let cached_bytes = cache_path.as_deref().and_then(|p| std::fs::read(p).ok());

        // ureq is blocking; this is a one-shot startup call, so it runs off
        // the async runtime rather than pulling in an async HTTP stack.
        let fetch_timeout = pg_lens_core::remote_config::FETCH_TIMEOUT;
        let fetch_result: Result<Vec<u8>, String> = tokio::task::spawn_blocking(move || {
            pg_lens_core::remote_config::fetch_remote_bytes(&url, token.as_deref(), fetch_timeout)
        })
        .await
        .map_err(|e| eyre!("--config-url fetch task panicked: {e}"))?
        .map_err(|e| e.to_string());

        if let (Ok(bytes), Some(path)) = (&fetch_result, &cache_path)
            && let Err(e) = write_remote_cache(path, bytes)
        {
            eprintln!(
                "warning: could not cache --config-url fetch at {}: {e}",
                path.display()
            );
        }

        let local_file = pg_lens_core::settings::services_file_path(&spec)
            .filter(|(path, _)| path.exists())
            .map(|(path, _)| pg_lens_core::services::ServicesFile::load(&path))
            .transpose()?
            .map(|(file, warnings)| {
                for warning in warnings {
                    eprintln!("warning: {warning}");
                }
                file
            });

        let (merged, warnings) = pg_lens_core::remote_config::resolve_effective_services(
            fetch_result,
            cached_bytes,
            local_file,
        )?;
        for warning in warnings {
            eprintln!("warning: {warning}");
        }
        Ok(Some(merged))
    }
}

/// Bearer token for `--config-url`: `PG_LENS_CONFIG_TOKEN`, then
/// `GITHUB_TOKEN`, then `remote_config_token_cmd` in config.toml (mirrors
/// `password_cmd` — an external command, trimmed stdout via
/// [`pg_lens_core::services::resolve_password_cmd`]; never a literal token in
/// any file). `Ok(None)` means the remote source is unauthenticated (a
/// public repo, or a plain URL that needs none).
async fn resolve_config_token(
    env: &HashMap<String, String>,
    token_cmd: Option<&str>,
) -> color_eyre::Result<Option<String>> {
    if let Some(token) = env.get("PG_LENS_CONFIG_TOKEN").filter(|v| !v.is_empty()) {
        return Ok(Some(token.clone()));
    }
    if let Some(token) = env.get("GITHUB_TOKEN").filter(|v| !v.is_empty()) {
        return Ok(Some(token.clone()));
    }
    if let Some(cmd) = token_cmd.filter(|c| !c.is_empty()) {
        let bytes = pg_lens_core::services::resolve_password_cmd(cmd)
            .await
            .map_err(|e| color_eyre::eyre::eyre!("remote_config_token_cmd: {e}"))?;
        return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
    }
    Ok(None)
}

/// Where a successful `--config-url` fetch is cached:
/// `$XDG_CACHE_HOME/pg_lens/remote-services.toml`, else
/// `$HOME/.cache/pg_lens/remote-services.toml`. `None` when neither can be
/// derived — caching (and the offline fallback it enables) is then simply
/// off; a fresh fetch is still attempted every startup.
fn remote_config_cache_path() -> Option<PathBuf> {
    let cache_dir = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(cache_dir.join("pg_lens").join("remote-services.toml"))
}

/// Writes a successful `--config-url` fetch to the cache at `0600` (Unix) —
/// defense in depth: even though the cache only ever holds a services file
/// (never a token; see the module doc), a services file can carry
/// `password_cmd`, so it deserves the same restrictive mode `services.rs`
/// expects of the local file.
fn write_remote_cache(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Pure precedence resolution for read-only mode: the flag and the env var
/// are OR'd in (either alone is enough to arm the mode; neither can turn it
/// back off once the other says yes), then `config.toml`, then `false`.
fn resolve_read_only(flag: bool, env: &HashMap<String, String>, config_value: Option<bool>) -> bool {
    flag
        || env.get("PG_LENS_READ_ONLY").is_some_and(|v| is_truthy(v))
        || config_value.unwrap_or(false)
}

/// The web `serve` bind address: `--listen`, then `PG_LENS_LISTEN` (both via
/// clap), then `config.toml`, then `127.0.0.1:8080`. A config value that
/// fails to parse is ignored with a warning.
#[cfg(feature = "web")]
fn resolve_listen(
    flag: Option<std::net::SocketAddr>,
    config: &settings::AppConfig,
) -> std::net::SocketAddr {
    if let Some(addr) = flag {
        return addr;
    }
    if let Some(raw) = config.listen.as_deref() {
        match raw.parse() {
            Ok(addr) => return addr,
            Err(e) => eprintln!("warning: ignoring config listen \"{raw}\": {e}"),
        }
    }
    std::net::SocketAddr::from(([127, 0, 0, 1], 8080))
}

/// How `run` starts: with a connection already resolved (the classic path),
/// or in picker mode — no poller yet; the user chooses first.
enum Startup {
    // Boxed: `Resolved` is large next to the picker variant (clippy
    // large_enum_variant), and this value exists exactly once at startup.
    Connect(Box<Option<Resolved>>),
    Picker(Vec<PickerEntry>),
}

/// Env vars whose presence (non-empty — empty values count as unset,
/// consistent with `settings.rs`) expresses a connection intent that the
/// picker must not second-guess.
const PICKER_SUPPRESSING_ENV: [&str; 4] =
    ["PGHOST", "PGSERVICE", "PG_LENS_SERVICE", "PG_LENS_DSN"];

/// Interactive service picker trigger rule (TUI mode only — never `serve`,
/// never `--mock`; see README "Interactive service picker"). The picker
/// shows iff ALL of:
/// - no `--dsn` and no `--service` (empty strings count as unset), and
/// - none of PGHOST / PGSERVICE / PG_LENS_SERVICE / PG_LENS_DSN is set to a
///   non-empty value in the captured environment, and
/// - the services file (the default XDG path, or the `--services-file` /
///   `PG_LENS_SERVICES_FILE` override) exists, parses, passes the existing
///   permission checks, and defines at least one service.
///
/// Any failure of that chain returns `None`, which means EXACTLY the
/// pre-picker behavior: plain `settings::resolve` (localhost default) or
/// its pre-TUI error. Returns display-safe summaries only (names +
/// host/user — never a `password`/`password_cmd`) plus the loader's
/// non-fatal warnings.
fn picker_services(spec: &ConnSpec) -> Option<(Vec<ServiceSummary>, Vec<String>)> {
    if spec.dsn.as_deref().is_some_and(|d| !d.is_empty())
        || spec.service.as_deref().is_some_and(|s| !s.is_empty())
    {
        return None;
    }
    if PICKER_SUPPRESSING_ENV
        .iter()
        .any(|var| spec.env.get(*var).is_some_and(|v| !v.is_empty()))
    {
        return None;
    }
    // `list_services` runs the whole chain (locate → read → parse →
    // permission checks); any error degrades to the default behavior.
    match settings::list_services(spec) {
        Ok((services, warnings)) if !services.is_empty() => Some((services, warnings)),
        _ => None,
    }
}

/// Builds the picker rows: one per service, rendered exactly as the file
/// says (`?` for fields the entry leaves out — env/default fallbacks are
/// NOT applied here), plus the trailing `localhost — (default)` entry that
/// maps to the plain no-service resolution.
fn picker_entries(services: Vec<ServiceSummary>) -> Vec<PickerEntry> {
    let mut entries: Vec<PickerEntry> = services
        .into_iter()
        .map(|s| {
            let detail = format!(
                "{user}@{host}",
                user = s.user.as_deref().unwrap_or("?"),
                host = s.host.as_deref().unwrap_or("?"),
            );
            PickerEntry {
                name: s.name.clone(),
                detail,
                service: Some(s.name),
            }
        })
        .collect();
    entries.push(PickerEntry {
        name: "localhost".to_string(),
        detail: "(default)".to_string(),
        service: None,
    });
    entries
}

/// Spawns the poller (mock or real) and returns its snapshot channel plus a
/// display label. The interval sender rides along: the TUI feeds `+`/`-`
/// changes into it, `serve` just keeps it alive. `schema_refresh_rx` is the
/// force-recollection signal (a bumped counter): Fase S3 wires the TUI's `R`
/// key to its sender — until then callers just keep the sender alive.
/// `db_switch_rx` is U2's database-picker channel (`d` in the TUI); `serve`
/// and `--mock` have no picker, so they simply drop it (the poller never
/// resolves that branch of its select, per `poller::wait_db_switch`'s
/// dropped-sender contract) — real switching is TUI-only for now.
fn spawn_poller(
    conn: Option<Resolved>,
    interval_rx: watch::Receiver<Duration>,
    schema_interval: Duration,
    schema_refresh_rx: watch::Receiver<u64>,
    admin_rx: mpsc::Receiver<AdminCommand>,
    shutdown_rx: watch::Receiver<bool>,
    db_switch_rx: mpsc::Receiver<String>,
) -> (watch::Receiver<Arc<DbSnapshot>>, String, JoinHandle<()>) {
    match conn {
        // The mock has no DB queries to cancel on shutdown — a already-done
        // handle keeps the caller's await uniform. `db_switch_rx` is simply
        // dropped here: mock mode never simulates a real reconnect (see
        // `app::handle_db_picker_key`'s "mock mode" toast).
        None => (
            pg_lens_core::poller::spawn_mock(interval_rx, schema_refresh_rx, admin_rx),
            "mock".to_string(),
            tokio::spawn(async {}),
        ),
        Some(resolved) => {
            // The label is the only connection info any frontend sees —
            // host and user, never the password. When the resolution came
            // with a password_cmd, the poller re-runs it per (re)connection.
            let label = resolved.label.to_string();
            // `pg_lens_core` never reads env/XDG itself, so the per-database
            // history path is computed HERE and injected as a closure the
            // poller can re-call on a database switch (U2) — `None` uses the
            // base config's own dbname (the classic, pre-U2 path), `Some(db)`
            // overrides it with the newly picked database.
            let base_config = resolved.config.clone();
            let history_path_fn: pg_lens_core::poller::HistoryPathFn =
                Arc::new(move |db: Option<&str>| history_file_path(&base_config, db));
            let (snapshots, handle) = pg_lens_core::poller::spawn(
                resolved.config,
                resolved.password_source,
                interval_rx,
                schema_interval,
                schema_refresh_rx,
                admin_rx,
                Some(history_path_fn),
                shutdown_rx,
                db_switch_rx,
            );
            (snapshots, label, handle)
        }
    }
}

/// The connection `!` (v0.11) hands to `psql` — captured right before the
/// resolved `Config`/`PasswordSource` are moved into `spawn_poller`, so a
/// later `psql` launch dials the EXACT same target the poller connects to.
/// `None` while no real connection exists yet (`--mock`, or still on the
/// startup picker) — see the `!`-handling block in [`run`].
struct PsqlTarget {
    config: pg_lens_core::tokio_postgres::Config,
    password_source: Option<pg_lens_core::PasswordSource>,
}

impl PsqlTarget {
    fn from_resolved(resolved: &Resolved) -> Self {
        Self {
            config: resolved.config.clone(),
            password_source: resolved.password_source.clone(),
        }
    }
}

/// Suspends the TUI, runs an interactive `psql` shell on `target` (the SAME
/// connection the poller uses), and restores the TUI on every exit path —
/// a clean `\q`, a non-zero exit, or a failed spawn (e.g. `psql` missing
/// from `PATH`). There is no early return between the suspend and the
/// restore below, so the restore always runs; this is the "restore
/// terminal state even if psql crashes" guarantee the feature requires.
///
/// The input task is aborted for the duration and respawned afterwards:
/// left running, crossterm's `EventStream` would keep reading raw stdin
/// concurrently with the child psql, and the two would race for the
/// operator's keystrokes.
///
/// The password (if any) is resolved here, as late as possible — right
/// before spawn — and travels only in the child's environment
/// (`PGPASSWORD`), never on argv, never logged. Returns statusbar feedback
/// text; the caller reports it through `Action::PsqlResult` (`update()` is
/// still the only place `App` itself mutates).
async fn launch_psql(
    terminal: &mut DefaultTerminal,
    input_task: &mut JoinHandle<()>,
    tx: &mpsc::Sender<Action>,
    target: &PsqlTarget,
    read_only: bool,
) -> (String, bool) {
    let password = psql::resolve_password(&target.config, target.password_source.as_ref()).await;
    let conn_info = psql::conn_info_from_config(&target.config, password);
    let invocation = psql::build_psql_invocation(&conn_info, read_only);

    // Mirrors `run_tui`'s own init/restore teardown exactly (same two
    // free functions), just mid-run instead of at process exit.
    input_task.abort();
    ratatui::restore();
    if read_only {
        println!(
            "pg_lens: read-only mode \u{2014} psql opens with a read-only default transaction \
             (PGOPTIONS=\"{}\"); this cannot stop psql itself from writing if you override it.",
            psql::READ_ONLY_PGOPTIONS
        );
    }
    let outcome = std::process::Command::new("psql")
        .args(&invocation.args)
        .envs(invocation.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .status();

    *terminal = ratatui::init();
    let _ = terminal.clear();
    *input_task = event::spawn_input(tx.clone());

    match outcome {
        Ok(status) if status.success() => ("psql session ended".to_string(), false),
        Ok(status) => (format!("psql exited with {status}"), false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ("psql not found on PATH".to_string(), true)
        }
        Err(e) => (format!("failed to launch psql: {e}"), true),
    }
}

/// Grace period for the poller to cancel its in-flight query and stop on
/// shutdown. Cancelling opens a brief new connection to the server; if that
/// stalls we still exit rather than hang the whole program.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Signals the poller to shut down and waits (briefly) for it to cancel any
/// running query and exit. Keeping the runtime alive for this window is what
/// lets the CancelRequest reach PostgreSQL — otherwise the process would exit
/// and leave a heavy query (e.g. bloat estimation) running server-side.
async fn shutdown_poller(shutdown_tx: &watch::Sender<bool>, handle: JoinHandle<()>) {
    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, handle).await;
}

/// Where to persist this connection's history ring: `<state>/pg_lens/`
/// (`$XDG_STATE_HOME`, else `$HOME/.local/state`) with a per-target filename
/// so distinct servers (and, per U2, distinct DATABASES on the same server)
/// keep distinct series. `None` when no state directory can be derived —
/// persistence is then simply off (in-memory only). `db_override` is U2's
/// hook: `None` uses `config`'s own dbname (the classic path, computed once
/// at the first connection); `Some(db)` names the database explicitly — used
/// after a database-picker switch, when the connection is about to reconnect
/// to `db` but `config` itself has not been updated yet.
fn history_file_path(
    config: &pg_lens_core::tokio_postgres::Config,
    db_override: Option<&str>,
) -> Option<PathBuf> {
    let state_dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".local").join("state"))
        })?;
    Some(
        state_dir
            .join("pg_lens")
            .join(format!("history-{}.jsonl", history_key(config, db_override))),
    )
}

/// A stable, filesystem-safe key for one connection target (host_port_db),
/// so `pg_lens` against different servers/databases doesn't cross-contaminate
/// history. See [`history_file_path`] for `db_override`.
fn history_key(config: &pg_lens_core::tokio_postgres::Config, db_override: Option<&str>) -> String {
    use pg_lens_core::tokio_postgres::config::Host;
    let host = config
        .get_hosts()
        .first()
        .map(|h| match h {
            Host::Tcp(s) => s.clone(),
            Host::Unix(p) => p.to_string_lossy().into_owned(),
        })
        .unwrap_or_else(|| "localhost".to_string());
    let port = config.get_ports().first().copied().unwrap_or(5432);
    let db = db_override.unwrap_or_else(|| {
        config
            .get_dbname()
            .or_else(|| config.get_user())
            .unwrap_or("postgres")
    });
    format!("{host}_{port}_{db}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Forwards admin commands queued by `update()` (the `y` of the confirm
/// modal) into the poller's channel. `try_send` on purpose: the UI loop
/// must never block — on a momentarily full channel the remaining commands
/// simply go out on the next pass through the loop.
fn drain_admin(app: &mut App, admin_tx: &mpsc::Sender<AdminCommand>) {
    while let Some(&cmd) = app.pending_admin.first() {
        match admin_tx.try_send(cmd) {
            Ok(()) => {
                app.pending_admin.remove(0);
            }
            // Full: retry next pass. Closed: the poller is gone — the loop
            // is about to end anyway; dropping the queue is harmless.
            Err(_) => break,
        }
    }
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = Cli::parse();
    color_eyre::install()?;

    match cli.command {
        // Bare `pg_lens [flags]` = the historical flat CLI = `tui`. The
        // connection flags are global, so they always live on `cli.conn`
        // regardless of whether a subcommand was given or where the flag sat.
        None | Some(Command::Tui) => run_tui(cli.conn).await,
        #[cfg(feature = "web")]
        Some(Command::Serve(args)) => run_serve(cli.conn, args).await,
    }
}

async fn run_tui(conn_args: ConnArgs) -> color_eyre::Result<()> {
    // `config.toml` (for `remote_config`) and the `--config-url` fetch both
    // have to happen before `--list-services`/the picker so they see the
    // fully merged (local + remote) service set.
    let config = conn_args.app_config();
    let overlay = conn_args.resolve_remote_overlay(&config).await?;

    if conn_args.list_services {
        return conn_args.list_services(overlay.as_ref());
    }
    // Startup mode decision. Picker mode (see `picker_services` for the
    // full trigger rule) enters the TUI with no poller; otherwise resolve
    // the connection *before* entering the alternate screen so a bad DSN /
    // env var / services file prints as a normal error (and permission
    // warnings land on stderr), not inside a raw terminal.
    let startup = match picker_services(&conn_args.spec_with(overlay.as_ref())) {
        Some((services, warnings)) if !conn_args.mock => {
            for warning in &warnings {
                eprintln!("warning: {warning}");
            }
            Startup::Picker(picker_entries(services))
        }
        _ => Startup::Connect(Box::new(conn_args.resolve(overlay.as_ref())?)),
    };

    let terminal = ratatui::init();
    let result = run(terminal, &conn_args, &config, startup, overlay).await;
    ratatui::restore();
    result
}

/// `pg_lens serve`: same resolution and poller as the TUI, but the watch
/// channel feeds pg_lens_web's router instead of ratatui. Runs until Ctrl+C.
#[cfg(feature = "web")]
async fn run_serve(conn: ConnArgs, args: ServeArgs) -> color_eyre::Result<()> {
    use color_eyre::eyre::eyre;

    let config = conn.app_config();
    let overlay = conn.resolve_remote_overlay(&config).await?;

    if conn.list_services {
        return conn.list_services(overlay.as_ref());
    }

    let listen = resolve_listen(args.listen, &config);

    // Empty tokens count as unset: `PG_LENS_AUTH_TOKEN= pg_lens serve`
    // must not silently create a server "protected" by the empty string.
    let token = std::env::var("PG_LENS_AUTH_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    // Security gate: non-loopback bind without a token refuses to start.
    pg_lens_web::ensure_listen_allowed(&listen, token.is_some()).map_err(|e| eyre!(e))?;
    let auth_enabled = token.is_some();

    let resolved = conn.resolve(overlay.as_ref())?;
    // `serve` has no `+`/`-` keys; the sender only needs to outlive the
    // server so the poller keeps its cadence.
    let (_interval_tx, interval_rx) = watch::channel(conn.interval(&config));
    // Web parity (Fase #24): `POST /api/schema/refresh` bumps this counter…
    let (schema_refresh_tx, schema_refresh_rx) = watch::channel(0u64);
    // …and `POST /api/admin/*` (token-gated) sends here. Both feed the same
    // poller channels the TUI uses.
    let (admin_tx, admin_rx) = mpsc::channel::<AdminCommand>(8);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // U2's database picker is TUI-only for now (see ROADMAP): `serve` has no
    // sender for this channel, so it is dropped right here — the poller's
    // `wait_db_switch` branch of its select then never resolves again,
    // exactly like an admin/interval sender no frontend wired up.
    let (_db_switch_tx, db_switch_rx) = mpsc::channel::<String>(1);
    let (snapshots, label, poller_handle) = spawn_poller(
        resolved,
        interval_rx,
        conn.schema_interval(&config),
        schema_refresh_rx,
        admin_rx,
        shutdown_rx,
        db_switch_rx,
    );

    let read_only = conn.read_only(&config);
    let router = pg_lens_web::router(snapshots, schema_refresh_tx, admin_tx, token, read_only);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let addr = listener.local_addr()?;
    // Operator info on stderr: bound address + auth mode + safe label
    // (user@host — never the DSN or password).
    eprintln!("pg_lens: serving {label} on http://{addr}/");
    eprintln!(
        "pg_lens: auth {}",
        if auth_enabled {
            "enabled — /api requires `Authorization: Bearer <PG_LENS_AUTH_TOKEN>`"
        } else {
            "disabled (loopback bind without PG_LENS_AUTH_TOKEN)"
        }
    );
    if read_only {
        eprintln!(
            "pg_lens: read-only mode — /api/admin/* refuses every request, even with a valid token"
        );
    }
    // The poller must be stopped INSIDE the graceful-shutdown window (see
    // pg_lens_web::serve): open SSE streams only end when the poller drops
    // the snapshot channel, and axum's graceful shutdown waits for exactly
    // those connections — stopping the poller afterwards would deadlock
    // Ctrl+C whenever a browser tab is attached. Stopping it here also
    // cancels the in-flight query (same reason as the TUI path).
    pg_lens_web::serve(listener, router, move || async move {
        shutdown_poller(&shutdown_tx, poller_handle).await;
    })
    .await?;
    Ok(())
}

async fn run(
    mut terminal: DefaultTerminal,
    conn_args: &ConnArgs,
    config: &settings::AppConfig,
    startup: Startup,
    // `--config-url`'s merged local+remote services (if configured) — the
    // picker's lazy re-resolve below must reuse this exact set, not
    // silently fall back to a disk-only read.
    remote_services: Option<ServicesFile>,
) -> color_eyre::Result<()> {
    let interval = conn_args.interval(config);
    let schema_interval = conn_args.schema_interval(config);
    let mut app = App::new();
    app.refresh_interval = interval;
    app.read_only = conn_args.read_only(config);

    let (tx, mut actions) = mpsc::channel::<Action>(64);
    // `mut`/kept (not detached like the bridge tasks below): `!` (v0.11)
    // aborts and respawns this task around a `psql` session — see
    // `launch_psql` — so crossterm's `EventStream` never races the child
    // for stdin.
    let mut input_task = event::spawn_input(tx.clone());
    // The SAME connection target `!` hands to `psql` — populated the moment
    // a real connection is resolved (classic startup, or the picker's lazy
    // resolve below); stays `None` for `--mock` and while still on the
    // startup picker, in which case `!` reports "not connected yet"
    // (see the handling block near the end of the loop below).
    let mut psql_target: Option<PsqlTarget> = None;
    // The poller reads its cadence live from this watch channel; the loop
    // below mirrors `app.refresh_interval` into it whenever `+`/`-` change
    // it. Message-passing only — no shared Mutex.
    let (interval_tx, interval_rx) = watch::channel(interval);
    // Schema force-refresh (Fase S3): the `R` key bumps
    // `app.schema_refresh_requests`; the loop below mirrors that counter
    // into this watch channel, and the poller recollects on the next tick.
    let (schema_refresh_tx, schema_refresh_rx) = watch::channel(0u64);
    // Admin actions (`c`/`K` + confirm): update() queues commands in
    // `app.pending_admin`; the loop below drains them into this channel and
    // the poller (sole owner of the DB client) executes them. Message
    // passing only, like every other TUI↔poller conversation.
    let (admin_tx, admin_rx) = mpsc::channel::<AdminCommand>(8);
    // U2's database picker (`d`): update() queues at most one pending switch
    // in `app.pending_db_switch`; the loop below forwards it to the poller,
    // which reconnects with a different `dbname` (PostgreSQL cannot switch
    // databases in-session). `--mock` never sends here (see
    // `app::handle_db_picker_key`'s toast).
    let (db_switch_tx, db_switch_rx) = mpsc::channel::<String>(4);
    // Shutdown signal: on quit we set this, and wait for the poller to cancel
    // its in-flight query (so a heavy bloat estimate does not keep running
    // server-side) before the runtime tears down.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    app.is_mock = conn_args.mock;
    let mut poller_handle: Option<JoinHandle<()>> = None;
    // Picker mode enters the loop with NO poller: the channels above exist
    // as usual, but their receivers are parked here until the user picks an
    // entry — the loop then spawns the poller (and its snapshot bridge)
    // lazily, reusing the exact same wiring as the classic path below.
    #[allow(clippy::type_complexity)]
    let mut parked_rx: Option<(
        watch::Receiver<Duration>,
        watch::Receiver<u64>,
        mpsc::Receiver<AdminCommand>,
        watch::Receiver<bool>,
        mpsc::Receiver<String>,
    )> = None;
    match startup {
        Startup::Connect(conn) => {
            // Captured before `*conn` moves into `spawn_poller` below — `!`
            // (v0.11) needs the same target the poller connects to.
            if let Some(resolved) = conn.as_ref() {
                psql_target = Some(PsqlTarget::from_resolved(resolved));
            }
            let (snapshots, label, handle) = spawn_poller(
                *conn,
                interval_rx,
                schema_interval,
                schema_refresh_rx,
                admin_rx,
                shutdown_rx,
                db_switch_rx,
            );
            poller_handle = Some(handle);
            app.host = label;
            // Seed the app with the poller's initial snapshot before the
            // first frame (in real mode this avoids one frame of
            // placeholder mock data).
            update(&mut app, Action::Snapshot(snapshots.borrow().clone()));
            // Dropping the JoinHandle detaches the task; it keeps running.
            let _bridge_task = event::spawn_snapshot_bridge(snapshots, tx.clone());
        }
        Startup::Picker(entries) => {
            app.picker = Some(PickerState::new(entries));
            parked_rx = Some((interval_rx, schema_refresh_rx, admin_rx, shutdown_rx, db_switch_rx));
        }
    }

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
        // Same pattern for `R`: mirror the bumped request counter so the
        // poller force-recollects the schema stats on its next tick.
        schema_refresh_tx.send_if_modified(|current| {
            if *current == app.schema_refresh_requests {
                false
            } else {
                *current = app.schema_refresh_requests;
                true
            }
        });
        // Confirmed admin commands (the modal's `y`) go to the poller task,
        // which executes them and reports back inside the next snapshot.
        drain_admin(&mut app, &admin_tx);
        // U2: a database picked in `d`'s overlay goes to the poller, which
        // reconnects with that dbname. `try_send` (never blocking the UI
        // loop) is safe here even at capacity 4 — the channel is only ever
        // fed by deliberate, infrequent user selections.
        if let Some(name) = app.pending_db_switch.take()
            && db_switch_tx.try_send(name.clone()).is_err()
        {
            // Full or closed channel: put it back for the next pass (closed
            // just means the poller is gone — the loop is about to end).
            app.pending_db_switch = Some(name);
        }

        // Lazy poller spawn (picker mode): the first pass after Enter sees
        // `app.picked` plus the parked receivers and starts the exact same
        // pipeline `Startup::Connect` uses — the resolved label and the
        // initial Connecting snapshot go through update() (the sole
        // mutation point), so the very next draw is the connection splash
        // ("connecting to user@host…"), then the dashboard on first data.
        // Connection failures are NOT errors here: they surface as
        // `PollerStatus::Error` on the splash, retrying with backoff.
        if let Some(entry) = app.picked.clone()
            && let Some((interval_rx, schema_refresh_rx, admin_rx, shutdown_rx, db_switch_rx)) =
                parked_rx.take()
        {
            // Re-resolve with the chosen service (or none, for the default
            // entry). The file was readable moments ago at trigger time; if
            // it changed underneath us, exit the TUI with the plain error —
            // the same text the pre-TUI path would have printed. Non-fatal
            // warnings were already printed before the TUI took over.
            let mut spec = conn_args.spec_with(remote_services.as_ref());
            spec.dsn = None;
            spec.service = entry.service.clone();
            let resolved = settings::resolve(&spec)?;
            // Same capture as `Startup::Connect`, just on the delayed path —
            // computed before `resolved` moves into `spawn_poller` below.
            psql_target = Some(PsqlTarget::from_resolved(&resolved));
            let (snapshots, label, handle) = spawn_poller(
                Some(resolved),
                interval_rx,
                schema_interval,
                schema_refresh_rx,
                admin_rx,
                shutdown_rx,
                db_switch_rx,
            );
            poller_handle = Some(handle);
            update(&mut app, Action::HostLabel(label));
            update(&mut app, Action::Snapshot(snapshots.borrow().clone()));
            // Detaches on drop; the bridge lives as long as its channels.
            let _bridge_task = event::spawn_snapshot_bridge(snapshots, tx.clone());
        }

        // `!` (v0.11): suspend the TUI, run psql, restore, report back. Mock
        // mode is already filtered out in `handle_key` (its own toast, no
        // flag ever set) — reaching here with no `psql_target` means a real
        // connection has not resolved yet (still on the startup picker),
        // which gets its own short, clear feedback instead of silently
        // doing nothing.
        if std::mem::take(&mut app.launch_psql_requested) {
            let (text, error) = match psql_target.as_ref() {
                Some(target) => {
                    launch_psql(&mut terminal, &mut input_task, &tx, target, app.read_only).await
                }
                None => ("not connected yet".to_string(), true),
            };
            update(&mut app, Action::PsqlResult { text, error });
        }
    }

    // Graceful stop: cancel any in-flight query BEFORE the runtime tears down,
    // so a heavy bloat estimate does not keep running on the server after the
    // user quits. (No-op when picker mode never spawned a poller.)
    if let Some(handle) = poller_handle {
        shutdown_poller(&shutdown_tx, handle).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A 0600 services file on disk, kept alive for the test's duration.
    fn services_file(contents: &str) -> tempfile::TempPath {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(contents.as_bytes()).expect("write");
        f.flush().expect("flush");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600))
                .expect("chmod");
        }
        f.into_temp_path()
    }

    // --- U2: per-database history key recomputation --------------------------

    /// Without an override, the key follows the config's own dbname (the
    /// classic pre-U2 behavior) — this is the `None` call `run()` makes once
    /// at the first session.
    #[test]
    fn history_key_uses_the_configs_own_dbname_with_no_override() {
        let config: pg_lens_core::tokio_postgres::Config =
            "host=db.internal port=5433 dbname=shop user=ro".parse().expect("dsn");
        assert_eq!(history_key(&config, None), "db_internal_5433_shop");
    }

    /// A database switch (U2's `d` picker) recomputes the SAME host/port key
    /// but with the newly picked database — `config` itself has not been
    /// mutated with the new dbname yet at the point `run()` calls this, which
    /// is exactly why the override exists instead of just re-reading `config`.
    #[test]
    fn history_key_override_replaces_only_the_database_component() {
        let config: pg_lens_core::tokio_postgres::Config =
            "host=db.internal port=5433 dbname=shop user=ro".parse().expect("dsn");
        assert_eq!(
            history_key(&config, Some("warehouse")),
            "db_internal_5433_warehouse"
        );
        // Switching back is symmetric — no state leaks between the two keys.
        assert_ne!(
            history_key(&config, Some("warehouse")),
            history_key(&config, None)
        );
    }

    /// No dbname in the config and no override: falls back to the user, then
    /// "postgres" — unchanged from the pre-U2 behavior.
    #[test]
    fn history_key_falls_back_to_user_then_postgres() {
        let with_user: pg_lens_core::tokio_postgres::Config =
            "host=localhost user=monitor".parse().expect("dsn");
        assert_eq!(history_key(&with_user, None), "localhost_5432_monitor");

        let bare: pg_lens_core::tokio_postgres::Config = "host=localhost".parse().expect("dsn");
        assert_eq!(history_key(&bare, None), "localhost_5432_postgres");
    }

    const TWO_SERVICES: &str = r#"
        [services.prod]
        host = "db.prod.internal"
        user = "svc_ro"
        password_cmd = "echo hidden-secret"

        [services.dead]
        host = "dead.invalid"
    "#;

    fn spec_with_file(file: &tempfile::TempPath, env_pairs: &[(&str, &str)]) -> ConnSpec {
        ConnSpec {
            dsn: None,
            service: None,
            services_file: Some(file.to_path_buf()),
            env: env(env_pairs),
            services_override: None,
        }
    }

    // --- read-only mode: flag → env → config → default -----------------------
    //
    // `resolve_read_only` is pure (no real process env), matching this
    // codebase's "environment is injected" testability discipline — the
    // same reason `settings::resolve` takes a `ConnSpec` instead of reading
    // `std::env` itself.

    #[test]
    fn read_only_defaults_to_false() {
        assert!(!resolve_read_only(false, &HashMap::new(), None));
    }

    #[test]
    fn read_only_flag_wins_regardless_of_env_or_config() {
        assert!(resolve_read_only(true, &HashMap::new(), Some(false)));
    }

    #[test]
    fn read_only_env_var_is_loosely_truthy() {
        for value in ["1", "true", "TRUE", "yes", "on", "anything"] {
            let e = env(&[("PG_LENS_READ_ONLY", value)]);
            assert!(resolve_read_only(false, &e, None), "{value:?} must be truthy");
        }
        for value in ["0", "false", "FALSE", "no", "off", ""] {
            let e = env(&[("PG_LENS_READ_ONLY", value)]);
            assert!(!resolve_read_only(false, &e, None), "{value:?} must be falsy");
        }
    }

    #[test]
    fn read_only_config_toml_applies_when_flag_and_env_are_absent() {
        assert!(resolve_read_only(false, &HashMap::new(), Some(true)));
    }

    #[test]
    fn read_only_env_beats_a_false_config() {
        let e = env(&[("PG_LENS_READ_ONLY", "1")]);
        assert!(
            resolve_read_only(false, &e, Some(false)),
            "env must still arm read-only"
        );
    }

    #[test]
    fn read_only_flag_parses_from_the_cli() {
        let cli = Cli::try_parse_from(["pg_lens", "--read-only"]).expect("parse --read-only");
        assert!(cli.conn.read_only);
        let cli = Cli::try_parse_from(["pg_lens"]).expect("parse bare");
        assert!(!cli.conn.read_only);
    }

    // --- --config-url: flag → env → config precedence -------------------------

    #[test]
    fn config_url_flag_parses_from_the_cli() {
        let cli = Cli::try_parse_from([
            "pg_lens",
            "--config-url",
            "github:acme/infra/services.toml",
        ])
        .expect("parse --config-url");
        assert_eq!(
            cli.conn.config_url.as_deref(),
            Some("github:acme/infra/services.toml")
        );
        let cli = Cli::try_parse_from(["pg_lens"]).expect("parse bare");
        assert_eq!(cli.conn.config_url, None);
    }

    #[test]
    fn config_url_flag_beats_config_toml() {
        let conn = ConnArgs {
            dsn: None,
            service: None,
            services_file: None,
            list_services: false,
            interval: None,
            schema_interval: None,
            mock: false,
            read_only: false,
            config_url: Some("https://example.com/from-flag.toml".to_string()),
        };
        let config = settings::AppConfig {
            remote_config: Some("https://example.com/from-config-toml".to_string()),
            ..settings::AppConfig::default()
        };
        assert_eq!(
            conn.config_url(&config).as_deref(),
            Some("https://example.com/from-flag.toml")
        );
    }

    #[test]
    fn config_url_falls_back_to_config_toml_when_the_flag_is_unset() {
        let conn = ConnArgs {
            dsn: None,
            service: None,
            services_file: None,
            list_services: false,
            interval: None,
            schema_interval: None,
            mock: false,
            read_only: false,
            config_url: None,
        };
        let config = settings::AppConfig {
            remote_config: Some("https://example.com/from-config-toml".to_string()),
            ..settings::AppConfig::default()
        };
        assert_eq!(
            conn.config_url(&config).as_deref(),
            Some("https://example.com/from-config-toml")
        );
        assert_eq!(conn.config_url(&settings::AppConfig::default()), None);
    }

    #[tokio::test]
    async fn resolve_remote_overlay_is_a_noop_when_config_url_is_unset() {
        let conn = ConnArgs {
            dsn: None,
            service: None,
            services_file: None,
            list_services: false,
            interval: None,
            schema_interval: None,
            mock: false,
            read_only: false,
            config_url: None,
        };
        let overlay = conn
            .resolve_remote_overlay(&settings::AppConfig::default())
            .await
            .expect("no --config-url must never fail");
        assert!(overlay.is_none(), "classic disk-only path must be untouched");
    }

    // --- CLI parsing: global connection flags ---------------------------------

    /// The connection flags are global, so `--service` resolves to the same
    /// place whether it sits before or after the `serve` subcommand. This is
    /// the regression guard for the bug where `--service X serve` silently
    /// ignored the service (it bound to a back-compat top-level copy that the
    /// subcommand never read).
    #[test]
    fn service_flag_works_before_and_after_the_serve_subcommand() {
        let before = Cli::try_parse_from(["pg_lens", "--service", "demo", "serve"])
            .expect("parse --service before serve");
        assert_eq!(before.conn.service.as_deref(), Some("demo"));
        assert!(matches!(before.command, Some(Command::Serve(_))));

        let after = Cli::try_parse_from(["pg_lens", "serve", "--service", "demo"])
            .expect("parse serve --service");
        assert_eq!(after.conn.service.as_deref(), Some("demo"));
        assert!(matches!(after.command, Some(Command::Serve(_))));
    }

    /// Bare `pg_lens --dsn ...` (no subcommand) still parses as the historical
    /// flat CLI, and `--dsn`/`--service` stay mutually exclusive everywhere.
    #[test]
    fn flat_backcompat_and_dsn_service_conflict_hold() {
        let flat = Cli::try_parse_from(["pg_lens", "--dsn", "host=x"]).expect("flat --dsn");
        assert!(flat.command.is_none());
        assert_eq!(flat.conn.dsn.as_deref(), Some("host=x"));

        // Conflict is enforced regardless of position relative to `serve`.
        assert!(Cli::try_parse_from(["pg_lens", "--dsn", "host=x", "--service", "y", "serve"]).is_err());
    }

    // --- admin command drain ---------------------------------------------------

    /// update() queues on `y`; the loop's drain forwards the command into
    /// the poller channel — a test receiver observes exactly what a poller
    /// would (the "command surfaces on a test mpsc receiver" proof).
    #[tokio::test]
    async fn drain_admin_forwards_queued_commands_to_the_channel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = App::new();
        // Micro Lens, selected row, open the cancel modal, confirm with y.
        update(&mut app, Action::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        let pid = app.selected_row().expect("selection").pid;
        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)),
        );
        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
        );

        let (admin_tx, mut admin_rx) = mpsc::channel::<AdminCommand>(8);
        drain_admin(&mut app, &admin_tx);
        assert!(app.pending_admin.is_empty(), "queue drained");
        assert_eq!(
            admin_rx.try_recv().expect("command forwarded"),
            AdminCommand::CancelBackend(pid)
        );
        assert!(admin_rx.try_recv().is_err(), "exactly one command");
    }

    #[tokio::test]
    async fn drain_admin_keeps_commands_when_the_channel_is_full() {
        let mut app = App::new();
        app.pending_admin = vec![
            AdminCommand::CancelBackend(1),
            AdminCommand::TerminateBackend(2),
        ];
        // Capacity 1: the second command must survive for the next pass.
        let (admin_tx, mut admin_rx) = mpsc::channel::<AdminCommand>(1);
        drain_admin(&mut app, &admin_tx);
        assert_eq!(app.pending_admin, vec![AdminCommand::TerminateBackend(2)]);
        assert_eq!(
            admin_rx.try_recv().expect("first sent"),
            AdminCommand::CancelBackend(1)
        );
        // Next pass (channel drained): the rest goes out.
        drain_admin(&mut app, &admin_tx);
        assert!(app.pending_admin.is_empty());
        assert_eq!(
            admin_rx.try_recv().expect("second sent"),
            AdminCommand::TerminateBackend(2)
        );
    }

    // --- picker trigger rule -------------------------------------------------

    #[test]
    fn picker_triggers_with_no_hints_and_a_valid_services_file() {
        let file = services_file(TWO_SERVICES);
        let (services, _) =
            picker_services(&spec_with_file(&file, &[])).expect("picker must trigger");
        let names: Vec<_> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["dead", "prod"], "BTreeMap order");
    }

    #[test]
    fn picker_suppressed_by_dsn_or_service_flags() {
        let file = services_file(TWO_SERVICES);
        let mut spec = spec_with_file(&file, &[]);
        spec.dsn = Some("host=x".to_string());
        assert!(picker_services(&spec).is_none(), "--dsn suppresses");

        let mut spec = spec_with_file(&file, &[]);
        spec.service = Some("prod".to_string());
        assert!(picker_services(&spec).is_none(), "--service suppresses");
    }

    #[test]
    fn picker_suppressed_by_each_connection_env_var() {
        let file = services_file(TWO_SERVICES);
        for var in PICKER_SUPPRESSING_ENV {
            let spec = spec_with_file(&file, &[(var, "something")]);
            assert!(
                picker_services(&spec).is_none(),
                "{var} set must suppress the picker"
            );
        }
        // Unrelated env vars (even PGUSER/PGPORT) do NOT suppress it.
        let spec = spec_with_file(&file, &[("PGUSER", "monitor"), ("PGPORT", "5433")]);
        assert!(picker_services(&spec).is_some());
    }

    #[test]
    fn empty_string_values_count_as_unset_everywhere() {
        let file = services_file(TWO_SERVICES);
        // Empty env values: consistent with settings.rs, still a picker.
        let spec = spec_with_file(
            &file,
            &[
                ("PGHOST", ""),
                ("PGSERVICE", ""),
                ("PG_LENS_SERVICE", ""),
                ("PG_LENS_DSN", ""),
            ],
        );
        assert!(picker_services(&spec).is_some(), "empty env = unset");

        // Empty flag values too (clap env-fill can produce Some("")).
        let mut spec = spec_with_file(&file, &[]);
        spec.dsn = Some(String::new());
        spec.service = Some(String::new());
        assert!(picker_services(&spec).is_some(), "empty flags = unset");
    }

    #[test]
    fn picker_suppressed_when_no_services_file_exists() {
        // Default XDG location pointing into nowhere: no file, no picker.
        let spec = ConnSpec {
            env: env(&[("HOME", "/nonexistent-home-for-test")]),
            ..ConnSpec::default()
        };
        assert!(picker_services(&spec).is_none());

        // No derivable location at all (HOME/XDG unset): same.
        assert!(picker_services(&ConnSpec::default()).is_none());

        // Explicit override pointing at a missing file: same (the error
        // then surfaces through the normal resolve path, unchanged).
        let spec = ConnSpec {
            services_file: Some(PathBuf::from("/nonexistent/services.toml")),
            ..ConnSpec::default()
        };
        assert!(picker_services(&spec).is_none());
    }

    #[test]
    fn picker_suppressed_by_parse_error_or_zero_services() {
        let broken = services_file("this is [not really { toml");
        assert!(picker_services(&spec_with_file(&broken, &[])).is_none());

        let empty = services_file("");
        assert!(picker_services(&spec_with_file(&empty, &[])).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn picker_suppressed_by_insecure_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let file = services_file(TWO_SERVICES);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666))
            .expect("chmod 666");
        assert!(
            picker_services(&spec_with_file(&file, &[])).is_none(),
            "group/other-writable file must fail the permission check"
        );
    }

    // --- picker entries ------------------------------------------------------

    #[test]
    fn entries_show_what_the_file_says_plus_default_last_and_no_secrets() {
        let file = services_file(TWO_SERVICES);
        let (services, _) =
            picker_services(&spec_with_file(&file, &[])).expect("picker must trigger");
        let entries = picker_entries(services);

        assert_eq!(entries.len(), 3, "two services + the default");
        // Verbatim file fields; `?` where the entry is silent (no env or
        // localhost/postgres fallbacks applied here).
        assert_eq!(entries[0].name, "dead");
        assert_eq!(entries[0].detail, "?@dead.invalid");
        assert_eq!(entries[0].service.as_deref(), Some("dead"));
        assert_eq!(entries[1].name, "prod");
        assert_eq!(entries[1].detail, "svc_ro@db.prod.internal");
        // The default entry is LAST and maps to no-service resolution.
        assert_eq!(entries[2].name, "localhost");
        assert_eq!(entries[2].detail, "(default)");
        assert_eq!(entries[2].service, None);

        // Secrets canary: nothing password-shaped reaches the picker rows.
        let rendered = format!("{entries:?}");
        assert!(!rendered.contains("hidden-secret"), "got: {rendered}");
        assert!(!rendered.contains("password"), "got: {rendered}");
    }
}
