//! Connection resolution: DSN + services file + libpq environment variables
//! → `tokio_postgres::Config` (Fases C1/C2).
//!
//! `tokio-postgres` parses connection strings but deliberately ignores the
//! libpq environment (`PGHOST`, `PGUSER`, ...) and service files; this
//! module fills that gap with the libpq precedence: **explicit DSN field >
//! services-file entry > env var > default** (`host=localhost
//! user=postgres`). The service is selected by `--service`, then
//! `PG_LENS_SERVICE`, then `PGSERVICE`.
//!
//! The environment is *injected* through [`ConnSpec::env`] — [`resolve`]
//! never touches `std::env` — so the whole precedence matrix is testable
//! with plain maps, no `set_var` flakiness. Frontends capture
//! `std::env::vars()` once at startup and hand it over. The services file
//! location is likewise derived from the injected env (`PG_LENS_SERVICES_FILE`,
//! `XDG_CONFIG_HOME`, `HOME`) or an explicit [`ConnSpec::services_file`].
//!
//! Security: the resolved [`ConnLabel`] is the only thing meant for
//! display/logs, and it never carries the password. Nothing here (including
//! errors) prints the `Config` via `Debug`, a password, or `password_cmd`
//! stdout.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use tokio_postgres::Config;
use tokio_postgres::config::Host;

use crate::services::{PasswordSource, ServiceEntry, ServicesFile};

/// What a frontend knows about the desired connection: an optional DSN
/// (`key=value` or `postgres://` URL), an optional service selection, plus a
/// snapshot of the process environment, captured by the caller.
#[derive(Clone, Debug, Default)]
pub struct ConnSpec {
    /// Explicit connection string (e.g. from `--dsn`). Fields present here
    /// always win over the services file and environment variables.
    pub dsn: Option<String>,
    /// Explicit service name (e.g. from `--service`). When `None`, the
    /// `PG_LENS_SERVICE` / `PGSERVICE` env vars are consulted.
    pub service: Option<String>,
    /// Explicit services-file path (e.g. from `--services-file`). When
    /// `None`, `PG_LENS_SERVICES_FILE`, then
    /// `$XDG_CONFIG_HOME/pg_lens/services.toml`, then
    /// `~/.config/pg_lens/services.toml` (via the injected env) are used.
    pub services_file: Option<PathBuf>,
    /// The process environment (typically `std::env::vars().collect()`),
    /// injected so resolution stays a pure function.
    pub env: HashMap<String, String>,
    /// A pre-resolved services file — `--config-url`'s local+remote merge,
    /// built once by the caller (see `pg_lens_tui::main::resolve_remote_overlay`).
    /// When `Some`, [`resolve`] and [`list_services`] use these entries
    /// directly and never touch `services_file`/disk. `None` is the classic
    /// path (load `services_file_path` from disk), completely unaffected by
    /// `--config-url` when it is not configured.
    pub services_override: Option<ServicesFile>,
}

/// Why connection resolution failed. Rendering these is safe: the DSN
/// (which may carry a password), plaintext service passwords and
/// `password_cmd` stdout are never echoed back.
#[derive(Debug)]
pub enum SettingsError {
    /// The DSN string did not parse as a `key=value` list or URL.
    DsnParse(tokio_postgres::Error),
    /// An environment variable held a value we could not interpret.
    InvalidEnvVar {
        name: &'static str,
        value: String,
        expected: &'static str,
    },
    /// The services file could not be read (missing, unreadable, ...).
    ServicesFileIo {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The services file is not valid TOML / has unknown keys.
    ServicesFileParse { path: PathBuf, message: String },
    /// The services file permissions are too loose to trust (it can execute
    /// commands and may hold plaintext passwords).
    InsecureServicesFile {
        path: PathBuf,
        /// Unix permission bits (e.g. `0o644`).
        mode: u32,
        reason: String,
    },
    /// A `[services.<name>]` entry is self-contradictory (e.g. `password`
    /// and `password_cmd` both set) or the file cannot be located at all.
    ServiceConfig { service: String, message: String },
    /// The requested service is not defined; `available` lists what is.
    UnknownService {
        name: String,
        available: Vec<String>,
    },
    /// `password_cmd` failed (non-zero exit, timeout, spawn failure). The
    /// message may include a stderr excerpt — never stdout.
    PasswordCmd { message: String },
    /// `--config-url` / `PG_LENS_CONFIG_URL` / `remote_config` is not a
    /// recognized scheme, or a token is configured against a plain `http://`
    /// URL (refused — a token must only ever travel over https).
    RemoteConfigUrl { message: String },
    /// The remote services file (`--config-url`) could not be fetched, and
    /// neither a cached copy nor a local `services.toml` could cover for
    /// it. Always safe to display — never includes the token.
    RemoteConfigFetch { message: String },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::DsnParse(e) => write!(f, "invalid --dsn: {e}"),
            SettingsError::InvalidEnvVar {
                name,
                value,
                expected,
            } => write!(f, "invalid {name}={value:?}: expected {expected}"),
            SettingsError::ServicesFileIo { path, error } => {
                write!(f, "cannot read services file {}: {error}", path.display())
            }
            SettingsError::ServicesFileParse { path, message } => {
                write!(f, "invalid services file {}: {message}", path.display())
            }
            SettingsError::InsecureServicesFile { path, mode, reason } => write!(
                f,
                "refusing services file {} (mode {mode:04o}): {reason}",
                path.display()
            ),
            SettingsError::ServiceConfig { service, message } => {
                write!(f, "service {service:?}: {message}")
            }
            SettingsError::UnknownService { name, available } => {
                if available.is_empty() {
                    write!(f, "unknown service {name:?}: the services file defines none")
                } else {
                    write!(
                        f,
                        "unknown service {name:?}; available services: {}",
                        available.join(", ")
                    )
                }
            }
            SettingsError::PasswordCmd { message } => {
                write!(f, "password_cmd failed: {message}")
            }
            SettingsError::RemoteConfigUrl { message } => {
                write!(f, "invalid --config-url: {message}")
            }
            SettingsError::RemoteConfigFetch { message } => {
                write!(f, "remote config: {message}")
            }
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SettingsError::DsnParse(e) => Some(e),
            SettingsError::ServicesFileIo { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// A display-safe description of the resolved connection: host and user,
/// **never** the password. This is what headers and logs should show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnLabel {
    host: String,
    user: Option<String>,
}

impl ConnLabel {
    /// The resolved host (or Unix-socket directory) as shown to the user.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Built from the *resolved* config, so the label reflects what will
    /// actually be dialed (post env-var merge), not the raw DSN text.
    fn from_config(config: &Config) -> Self {
        let host = match config.get_hosts().first() {
            Some(Host::Tcp(h)) => h.clone(),
            #[cfg(unix)]
            Some(Host::Unix(path)) => path.display().to_string(),
            None => config
                .get_hostaddrs()
                .first()
                .map(ToString::to_string)
                .unwrap_or_else(|| "localhost".to_string()),
        };
        ConnLabel {
            host,
            user: config.get_user().map(str::to_string),
        }
    }
}

impl fmt::Display for ConnLabel {
    /// `user@host` when the user is known, plain `host` otherwise.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.user {
            Some(user) => write!(f, "{user}@{host}", host = self.host),
            None => write!(f, "{host}", host = self.host),
        }
    }
}

/// One (env var, apply-if-absent) rule of the merge below.
///
/// `is_set` looks at the parsed config to detect whether the DSN already
/// provided the field (libpq semantics: env vars are only *defaults*).
struct EnvRule {
    name: &'static str,
    is_set: fn(&Config) -> bool,
    apply: fn(&mut Config, &str) -> Result<(), SettingsError>,
}

const ENV_RULES: &[EnvRule] = &[
    EnvRule {
        name: "PGHOST",
        is_set: has_host,
        apply: |c, v| {
            c.host(v);
            Ok(())
        },
    },
    EnvRule {
        name: "PGPORT",
        is_set: |c| !c.get_ports().is_empty(),
        apply: |c, v| {
            let port: u16 = v.parse().map_err(|_| SettingsError::InvalidEnvVar {
                name: "PGPORT",
                value: v.to_string(),
                expected: "a TCP port number (1-65535)",
            })?;
            c.port(port);
            Ok(())
        },
    },
    EnvRule {
        name: "PGDATABASE",
        is_set: |c| c.get_dbname().is_some(),
        apply: |c, v| {
            c.dbname(v);
            Ok(())
        },
    },
    EnvRule {
        name: "PGUSER",
        is_set: |c| c.get_user().is_some(),
        apply: |c, v| {
            c.user(v);
            Ok(())
        },
    },
    EnvRule {
        name: "PGPASSWORD",
        is_set: |c| c.get_password().is_some(),
        apply: |c, v| {
            c.password(v);
            Ok(())
        },
    },
    EnvRule {
        name: "PGAPPNAME",
        is_set: |c| c.get_application_name().is_some(),
        apply: |c, v| {
            c.application_name(v);
            Ok(())
        },
    },
    EnvRule {
        name: "PGCONNECT_TIMEOUT",
        is_set: |c| c.get_connect_timeout().is_some(),
        apply: |c, v| {
            let secs: u64 = v.parse().map_err(|_| SettingsError::InvalidEnvVar {
                name: "PGCONNECT_TIMEOUT",
                value: v.to_string(),
                expected: "a whole number of seconds",
            })?;
            // libpq: zero means "wait indefinitely" — leave the timeout off.
            if secs > 0 {
                c.connect_timeout(Duration::from_secs(secs));
            }
            Ok(())
        },
    },
];

/// A host counts as "specified" whether it came as a name/socket-dir
/// (`host=`) or a literal address (`hostaddr=`).
fn has_host(config: &Config) -> bool {
    !config.get_hosts().is_empty() || !config.get_hostaddrs().is_empty()
}

/// The output of [`resolve`]: everything a frontend needs to connect.
pub struct Resolved {
    /// Ready-to-dial config. May already carry a static password (DSN,
    /// service `password`, or `PGPASSWORD`).
    pub config: Config,
    /// Display-safe `user@host` label — the only part meant for the UI.
    pub label: ConnLabel,
    /// When set, the poller must resolve the password through this source
    /// before **every** connection attempt (rotating tokens stay fresh
    /// across reconnects). `None` means the config is complete as-is.
    pub password_source: Option<PasswordSource>,
    /// Non-fatal notes (e.g. services file mode != 0600, or a service
    /// selected via env being skipped because no file exists). Frontends
    /// should print these before taking over the terminal.
    pub warnings: Vec<String>,
}

/// Manual `Debug`: the `Config` (which may hold a password) and the
/// `password_cmd` text are deliberately left out — only display-safe parts.
impl fmt::Debug for Resolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resolved")
            .field("label", &self.label)
            .field("has_password_source", &self.password_source.is_some())
            .field("warnings", &self.warnings)
            .finish_non_exhaustive()
    }
}

/// Where the service name came from — decides how hard a missing services
/// file fails (an explicit `--service` must error; an ambient `PGSERVICE`
/// pointing at nothing degrades to a warning).
enum ServiceOrigin {
    Flag,
    Env(&'static str),
}

/// `--service` > `PG_LENS_SERVICE` > `PGSERVICE` (empty values = unset).
fn selected_service(spec: &ConnSpec) -> Option<(String, ServiceOrigin)> {
    if let Some(name) = spec.service.as_ref().filter(|s| !s.is_empty()) {
        return Some((name.clone(), ServiceOrigin::Flag));
    }
    for var in ["PG_LENS_SERVICE", "PGSERVICE"] {
        if let Some(name) = spec.env.get(var).filter(|v| !v.is_empty()) {
            return Some((name.clone(), ServiceOrigin::Env(var)));
        }
    }
    None
}

/// The base config directory: `$XDG_CONFIG_HOME`, else `$HOME/.config`
/// (from the injected env). `None` when neither is set.
fn xdg_config_dir(spec: &ConnSpec) -> Option<PathBuf> {
    spec.env
        .get("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            spec.env
                .get("HOME")
                .filter(|v| !v.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
}

/// The services-file path plus whether it was named explicitly (flag/env —
/// must exist) or is just the XDG default (may quietly not exist). `pub`
/// (not `pub(crate)`) so `--config-url`'s local+remote merge (in
/// `pg_lens_tui::main`) can locate the same local file `resolve` would have
/// used, without duplicating this precedence.
pub fn services_file_path(spec: &ConnSpec) -> Option<(PathBuf, bool)> {
    if let Some(path) = &spec.services_file {
        return Some((path.clone(), true));
    }
    if let Some(path) = spec.env.get("PG_LENS_SERVICES_FILE").filter(|v| !v.is_empty()) {
        return Some((PathBuf::from(path), true));
    }
    let config_dir = xdg_config_dir(spec)?;
    Some((config_dir.join("pg_lens").join("services.toml"), false))
}

/// User defaults from `config.toml` (alongside `services.toml`). Every field
/// is optional and overridden by the matching flag or env var; its own
/// built-in default applies when all are unset. Unlike the services file it
/// holds no secrets, so no permission checks — a missing file is simply the
/// empty config.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    /// Poll interval in seconds (`--interval` / `PG_LENS_INTERVAL`).
    pub interval: Option<f64>,
    /// Schema Lens collection interval in seconds
    /// (`--schema-interval` / `PG_LENS_SCHEMA_INTERVAL`).
    pub schema_interval: Option<u64>,
    /// Web `serve` bind address (`--listen` / `PG_LENS_LISTEN`).
    pub listen: Option<String>,
    /// Read-only mode: hard-disables every mutating/admin action (`c`/`K`
    /// in the TUI, `/api/admin/*` in `serve`) regardless of the connected
    /// role's actual privileges (`--read-only` / `PG_LENS_READ_ONLY`).
    /// Default `false` (actions enabled) when unset.
    pub read_only: Option<bool>,
    /// Remote services-file source (`--config-url` / `PG_LENS_CONFIG_URL`) —
    /// `github:OWNER/REPO/PATH[@REF]` or an `https://` URL. See
    /// `pg_lens_core::remote_config`.
    pub remote_config: Option<String>,
    /// External command whose trimmed stdout is the bearer token for
    /// `remote_config` (mirrors `password_cmd` — never a literal token in
    /// this file). Only consulted when neither `PG_LENS_CONFIG_TOKEN` nor
    /// `GITHUB_TOKEN` is set.
    pub remote_config_token_cmd: Option<String>,
}

/// Locates `config.toml`: `PG_LENS_CONFIG_FILE`, else
/// `<xdg-config>/pg_lens/config.toml`. `None` when no config dir can be
/// derived and no override is set.
fn config_file_path(spec: &ConnSpec) -> Option<PathBuf> {
    if let Some(path) = spec.env.get("PG_LENS_CONFIG_FILE").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(path));
    }
    Some(xdg_config_dir(spec)?.join("pg_lens").join("config.toml"))
}

/// Loads `config.toml` best-effort: a missing file is the empty config; an
/// unreadable or unparsable one is ALSO the empty config plus a warning — a
/// broken config must never stop pg_lens from starting. Returns the parsed
/// defaults and any non-fatal warnings (for the caller to print to stderr).
pub fn load_app_config(spec: &ConnSpec) -> (AppConfig, Vec<String>) {
    let Some(path) = config_file_path(spec) else {
        return (AppConfig::default(), Vec::new());
    };
    match std::fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<AppConfig>(&contents) {
            Ok(config) => (config, Vec::new()),
            Err(e) => (
                AppConfig::default(),
                vec![format!("ignoring {}: {e}", path.display())],
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (AppConfig::default(), Vec::new())
        }
        Err(e) => (
            AppConfig::default(),
            vec![format!("cannot read {}: {e}", path.display())],
        ),
    }
}

/// Copies every service field the DSN did not already pin into `config`
/// (service beats env: this runs *before* the env rules, and a field set
/// here reads as "set" to them). Returns the password source when the entry
/// uses `password_cmd`, plus whether `connect_timeout_secs = 0` pinned the
/// timeout to "indefinite" (which must also mask `PGCONNECT_TIMEOUT`).
fn apply_service(config: &mut Config, entry: &ServiceEntry) -> (Option<PasswordSource>, bool) {
    if !has_host(config)
        && let Some(host) = &entry.host
    {
        config.host(host);
    }
    if config.get_ports().is_empty()
        && let Some(port) = entry.port
    {
        config.port(port);
    }
    if config.get_user().is_none()
        && let Some(user) = &entry.user
    {
        config.user(user);
    }
    if config.get_dbname().is_none()
        && let Some(dbname) = &entry.dbname
    {
        config.dbname(dbname);
    }
    if config.get_application_name().is_none()
        && let Some(app) = &entry.application_name
    {
        config.application_name(app);
    }
    let mut timeout_pinned = false;
    if config.get_connect_timeout().is_none()
        && let Some(secs) = entry.connect_timeout_secs
    {
        // libpq semantics: 0 = wait indefinitely (leave the timeout unset,
        // but still shadow PGCONNECT_TIMEOUT — service beats env).
        if secs > 0 {
            config.connect_timeout(Duration::from_secs(secs));
        } else {
            timeout_pinned = true;
        }
    }
    let mut source = None;
    if config.get_password().is_none() {
        if let Some(password) = entry.password() {
            config.password(password);
        } else if let Some(cmd) = entry.password_cmd() {
            source = Some(PasswordSource::Command(cmd.to_string()));
        }
    }
    (source, timeout_pinned)
}

/// Resolves a [`ConnSpec`] into a ready-to-dial `tokio_postgres::Config`,
/// its display-safe [`ConnLabel`], and (when the selected service uses
/// `password_cmd`) the [`PasswordSource`] the poller re-runs per attempt.
///
/// Precedence per field (libpq semantics): explicit DSN field, then the
/// selected services-file entry, then the matching `PG*` env var, then the
/// defaults `host=localhost` / `user=postgres`. Empty env values are
/// treated as unset.
pub fn resolve(spec: &ConnSpec) -> Result<Resolved, SettingsError> {
    let mut config = match &spec.dsn {
        Some(dsn) => Config::from_str(dsn).map_err(SettingsError::DsnParse)?,
        None => Config::new(),
    };

    let mut warnings = Vec::new();
    let mut password_source = None;
    let mut timeout_pinned = false;

    if let Some((name, origin)) = selected_service(spec) {
        // `--config-url` already resolved (and merged with any local file)
        // into `services_override` — use it directly and skip disk I/O.
        // Unlike the ambient-PGSERVICE-degrades-to-a-warning rule below, an
        // unknown name here is always a hard error: the override already
        // represents the full merged set the operator configured, so a
        // silent "ignoring it" would hide a typo'd/removed remote entry.
        if let Some(file) = &spec.services_override {
            let entry = file.get(&name)?;
            (password_source, timeout_pinned) = apply_service(&mut config, entry);
        } else {
            match services_file_path(spec) {
                // Explicit path (flag/env), or the XDG default when it exists:
                // load it (a missing explicit file is a hard error) and merge.
                Some((path, explicit)) if explicit || path.exists() => {
                    let (file, mut file_warnings) = ServicesFile::load(&path)?;
                    warnings.append(&mut file_warnings);
                    let entry = file.get(&name)?;
                    (password_source, timeout_pinned) = apply_service(&mut config, entry);
                }
                // No usable file. An ambient PGSERVICE/PG_LENS_SERVICE
                // pointing at nothing degrades to a warning (the user may
                // have it set for psql); an explicit --service must fail
                // loudly.
                missing => match origin {
                    ServiceOrigin::Env(var) => warnings.push(match missing {
                        Some((path, _)) => format!(
                            "{var}={name} is set but there is no services file at {}; ignoring it",
                            path.display()
                        ),
                        None => format!(
                            "{var}={name} is set but no services file location could be \
                             derived (HOME/XDG_CONFIG_HOME unset); ignoring it"
                        ),
                    }),
                    ServiceOrigin::Flag => {
                        return Err(match missing {
                            Some((path, _)) => SettingsError::ServicesFileIo {
                                error: std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    "no such file",
                                ),
                                path,
                            },
                            None => SettingsError::ServiceConfig {
                                service: name,
                                message: "cannot locate a services file: pass --services-file, \
                                          set PG_LENS_SERVICES_FILE, or set HOME/XDG_CONFIG_HOME"
                                    .to_string(),
                            },
                        });
                    }
                },
            }
        }
    }

    for rule in ENV_RULES {
        if (rule.is_set)(&config) {
            continue; // DSN or service already said so — env is only a default
        }
        // A service-level password_cmd / connect_timeout_secs=0 counts as
        // "set" even though the Config field is still empty.
        if rule.name == "PGPASSWORD" && password_source.is_some() {
            continue;
        }
        if rule.name == "PGCONNECT_TIMEOUT" && timeout_pinned {
            continue;
        }
        if let Some(value) = spec.env.get(rule.name).filter(|v| !v.is_empty()) {
            (rule.apply)(&mut config, value)?;
        }
    }

    // Last resort defaults (previously clap's default DSN in the TUI).
    if !has_host(&config) {
        config.host("localhost");
    }
    if config.get_user().is_none() {
        config.user("postgres");
    }

    let label = ConnLabel::from_config(&config);
    Ok(Resolved {
        config,
        label,
        password_source,
        warnings,
    })
}

/// One row of `--list-services` output: never a password or `password_cmd`.
#[derive(Clone)]
pub struct ServiceSummary {
    pub name: String,
    pub host: Option<String>,
    pub user: Option<String>,
}

/// Loads the services file selected by `spec` and returns display-safe
/// summaries (plus permission warnings). Used by `--list-services`. When
/// `spec.services_override` is set (`--config-url`), it lists exactly that
/// merged set instead of reading disk — so `--list-services` always shows
/// what will actually be resolved.
pub fn list_services(spec: &ConnSpec) -> Result<(Vec<ServiceSummary>, Vec<String>), SettingsError> {
    if let Some(file) = &spec.services_override {
        let summaries = file
            .iter()
            .map(|(name, entry)| ServiceSummary {
                name: name.to_string(),
                host: entry.host.clone(),
                user: entry.user.clone(),
            })
            .collect();
        return Ok((summaries, Vec::new()));
    }
    let Some((path, _)) = services_file_path(spec) else {
        return Err(SettingsError::ServiceConfig {
            service: "<list>".to_string(),
            message: "cannot locate a services file: pass --services-file, set \
                      PG_LENS_SERVICES_FILE, or set HOME/XDG_CONFIG_HOME"
                .to_string(),
        });
    };
    let (file, warnings) = ServicesFile::load(&path)?;
    let summaries = file
        .iter()
        .map(|(name, entry)| ServiceSummary {
            name: name.to_string(),
            host: entry.host.clone(),
            user: entry.user.clone(),
        })
        .collect();
    Ok((summaries, warnings))
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

    fn config_spec(file: &std::path::Path) -> ConnSpec {
        ConnSpec {
            env: env(&[("PG_LENS_CONFIG_FILE", file.to_str().unwrap())]),
            ..ConnSpec::default()
        }
    }

    #[test]
    fn app_config_parses_all_fields() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            "interval = 5.0\nschema_interval = 120\nlisten = \"0.0.0.0:9000\"\nread_only = true"
        )
        .unwrap();
        let (cfg, warnings) = load_app_config(&config_spec(f.path()));
        assert!(warnings.is_empty());
        assert_eq!(cfg.interval, Some(5.0));
        assert_eq!(cfg.schema_interval, Some(120));
        assert_eq!(cfg.listen.as_deref(), Some("0.0.0.0:9000"));
        assert_eq!(cfg.read_only, Some(true));
    }

    #[test]
    fn app_config_parses_remote_config_fields() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            "remote_config = \"github:acme/infra/services.toml@main\"\n\
             remote_config_token_cmd = \"vault kv get -field=token secret/gh\""
        )
        .unwrap();
        let (cfg, warnings) = load_app_config(&config_spec(f.path()));
        assert!(warnings.is_empty());
        assert_eq!(
            cfg.remote_config.as_deref(),
            Some("github:acme/infra/services.toml@main")
        );
        assert_eq!(
            cfg.remote_config_token_cmd.as_deref(),
            Some("vault kv get -field=token secret/gh")
        );
    }

    #[test]
    fn app_config_missing_file_is_empty_and_silent() {
        let spec = ConnSpec {
            env: env(&[("PG_LENS_CONFIG_FILE", "/no/such/pg_lens_config.toml")]),
            ..ConnSpec::default()
        };
        let (cfg, warnings) = load_app_config(&spec);
        assert!(cfg.interval.is_none() && cfg.schema_interval.is_none() && cfg.listen.is_none());
        assert!(warnings.is_empty(), "missing file must be silent: {warnings:?}");
    }

    #[test]
    fn app_config_unknown_key_or_bad_toml_warns_but_defaults() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "bogus_key = 1").unwrap();
        let (cfg, warnings) = load_app_config(&config_spec(f.path()));
        assert!(cfg.interval.is_none());
        assert_eq!(warnings.len(), 1, "unknown key must warn");
        assert!(warnings[0].contains("ignoring"));
    }

    fn resolve_ok(dsn: Option<&str>, env: HashMap<String, String>) -> (Config, ConnLabel) {
        let resolved = resolve(&ConnSpec {
            dsn: dsn.map(str::to_string),
            env,
            ..ConnSpec::default()
        })
        .expect("resolution must succeed");
        (resolved.config, resolved.label)
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

    // --- precedence matrix -------------------------------------------------

    #[test]
    fn dsn_field_beats_env_var() {
        let (config, label) = resolve_ok(
            Some("host=db.prod.internal user=app password=fromdsn port=6432"),
            env(&[
                ("PGHOST", "nonexistent.invalid"),
                ("PGUSER", "envuser"),
                ("PGPASSWORD", "fromenv"),
                ("PGPORT", "9999"),
            ]),
        );
        assert_eq!(
            config.get_hosts(),
            &[Host::Tcp("db.prod.internal".to_string())]
        );
        assert_eq!(config.get_user(), Some("app"));
        assert_eq!(config.get_password(), Some(b"fromdsn".as_slice()));
        assert_eq!(config.get_ports(), &[6432]);
        assert_eq!(label.to_string(), "app@db.prod.internal");
    }

    #[test]
    fn env_var_beats_default() {
        let (config, label) = resolve_ok(
            None,
            env(&[
                ("PGHOST", "db.env.internal"),
                ("PGPORT", "54316"),
                ("PGUSER", "envuser"),
                ("PGDATABASE", "appdb"),
                ("PGAPPNAME", "pg_lens_test"),
            ]),
        );
        assert_eq!(
            config.get_hosts(),
            &[Host::Tcp("db.env.internal".to_string())]
        );
        assert_eq!(config.get_ports(), &[54316]);
        assert_eq!(config.get_user(), Some("envuser"));
        assert_eq!(config.get_dbname(), Some("appdb"));
        assert_eq!(config.get_application_name(), Some("pg_lens_test"));
        assert_eq!(label.to_string(), "envuser@db.env.internal");
    }

    #[test]
    fn defaults_apply_when_nothing_else_does() {
        let (config, label) = resolve_ok(None, HashMap::new());
        assert_eq!(config.get_hosts(), &[Host::Tcp("localhost".to_string())]);
        assert_eq!(config.get_user(), Some("postgres"));
        assert_eq!(config.get_password(), None);
        assert_eq!(label.to_string(), "postgres@localhost");
    }

    #[test]
    fn env_fills_only_the_fields_the_dsn_left_out() {
        // DSN pins the host; env supplies port/user/password.
        let (config, _) = resolve_ok(
            Some("host=localhost"),
            env(&[
                ("PGPORT", "54316"),
                ("PGUSER", "postgres"),
                ("PGPASSWORD", "pg"),
            ]),
        );
        assert_eq!(config.get_hosts(), &[Host::Tcp("localhost".to_string())]);
        assert_eq!(config.get_ports(), &[54316]);
        assert_eq!(config.get_user(), Some("postgres"));
        assert_eq!(config.get_password(), Some(b"pg".as_slice()));
    }

    #[test]
    fn pgpassword_is_applied() {
        let (config, label) = resolve_ok(None, env(&[("PGPASSWORD", "sekret")]));
        assert_eq!(config.get_password(), Some(b"sekret".as_slice()));
        // ... and never leaks into the display label.
        assert!(!label.to_string().contains("sekret"));
    }

    #[test]
    fn pguser_is_the_official_name_pgusername_is_ignored() {
        let (config, _) = resolve_ok(None, env(&[("PGUSERNAME", "impostor")]));
        assert_eq!(config.get_user(), Some("postgres"));
    }

    #[test]
    fn pgconnect_timeout_maps_to_connect_timeout() {
        let (config, _) = resolve_ok(None, env(&[("PGCONNECT_TIMEOUT", "5")]));
        assert_eq!(config.get_connect_timeout(), Some(&Duration::from_secs(5)));

        // DSN connect_timeout wins over the env var.
        let (config, _) = resolve_ok(
            Some("connect_timeout=2"),
            env(&[("PGCONNECT_TIMEOUT", "30")]),
        );
        assert_eq!(config.get_connect_timeout(), Some(&Duration::from_secs(2)));

        // libpq: 0 disables the timeout.
        let (config, _) = resolve_ok(None, env(&[("PGCONNECT_TIMEOUT", "0")]));
        assert_eq!(config.get_connect_timeout(), None);
    }

    #[test]
    fn empty_env_values_are_treated_as_unset() {
        let (config, _) = resolve_ok(None, env(&[("PGHOST", ""), ("PGPORT", "")]));
        assert_eq!(config.get_hosts(), &[Host::Tcp("localhost".to_string())]);
        assert!(config.get_ports().is_empty());
    }

    // --- env parse failures are errors, not panics --------------------------

    #[test]
    fn invalid_pgport_is_a_clear_error() {
        let err = resolve(&ConnSpec {
            dsn: None,
            env: env(&[("PGPORT", "fivethousand")]),
            ..ConnSpec::default()
        })
        .expect_err("bad PGPORT must fail");
        let msg = err.to_string();
        assert!(msg.contains("PGPORT"), "got: {msg}");
        assert!(msg.contains("fivethousand"), "got: {msg}");
    }

    #[test]
    fn invalid_pgconnect_timeout_is_a_clear_error() {
        let err = resolve(&ConnSpec {
            dsn: None,
            env: env(&[("PGCONNECT_TIMEOUT", "soon")]),
            ..ConnSpec::default()
        })
        .expect_err("bad PGCONNECT_TIMEOUT must fail");
        assert!(err.to_string().contains("PGCONNECT_TIMEOUT"));
    }

    #[test]
    fn invalid_dsn_is_a_clear_error() {
        let err = resolve(&ConnSpec {
            dsn: Some("port=notaport".to_string()),
            env: HashMap::new(),
            ..ConnSpec::default()
        })
        .expect_err("bad DSN must fail");
        assert!(err.to_string().contains("invalid --dsn"));
    }

    // --- ConnLabel: migrated from the TUI's old `dsn_host` tests ------------

    #[test]
    fn host_from_key_value_dsn() {
        let (_, label) = resolve_ok(Some("host=db.prod.internal user=app"), HashMap::new());
        assert_eq!(label.host(), "db.prod.internal");
        let (_, label) = resolve_ok(Some("user=app host='10.0.0.7' port=6432"), HashMap::new());
        assert_eq!(label.host(), "10.0.0.7");
    }

    #[test]
    fn host_from_url_dsn_without_leaking_credentials() {
        let (_, label) = resolve_ok(
            Some("postgres://alice:s3cret@db.example.com:5432/app"),
            HashMap::new(),
        );
        assert_eq!(label.host(), "db.example.com");
        assert_eq!(label.to_string(), "alice@db.example.com");
        assert!(!label.to_string().contains("s3cret"));

        let (_, label) = resolve_ok(
            Some("postgresql://db.example.com/app?sslmode=require"),
            HashMap::new(),
        );
        assert_eq!(label.host(), "db.example.com");
    }

    #[test]
    fn host_defaults_to_localhost() {
        let (_, label) = resolve_ok(Some("user=postgres"), HashMap::new());
        assert_eq!(label.host(), "localhost");
        let (_, label) = resolve_ok(Some("postgres:///app"), HashMap::new());
        assert_eq!(label.host(), "localhost");
    }

    #[test]
    fn label_reflects_env_resolved_values() {
        // The header must show what will actually be dialed.
        let (_, label) = resolve_ok(
            None,
            env(&[("PGHOST", "db.env.internal"), ("PGUSER", "monitor")]),
        );
        assert_eq!(label.to_string(), "monitor@db.env.internal");
    }

    // --- Fase C2: services file in the precedence chain ---------------------

    const PROD_SERVICE: &str = r#"
        [services.prod]
        host = "db.service.internal"
        port = 6432
        user = "svc_user"
        dbname = "svc_db"
        connect_timeout_secs = 7
        password_cmd = "echo svc-pw"
    "#;

    #[test]
    fn full_precedence_dsn_beats_service_beats_env_beats_default() {
        let file = services_file(PROD_SERVICE);
        // DSN pins host; service pins port/user/dbname; env only gets to
        // fill what neither set (application_name); defaults fill nothing.
        let resolved = resolve(&ConnSpec {
            dsn: Some("host=db.dsn.internal".to_string()),
            service: Some("prod".to_string()),
            services_file: Some(file.to_path_buf()),
            env: env(&[
                ("PGHOST", "db.env.internal"),
                ("PGPORT", "9999"),
                ("PGUSER", "envuser"),
                ("PGDATABASE", "envdb"),
                ("PGAPPNAME", "from_env"),
            ]),
            services_override: None,
        })
        .expect("resolution must succeed");
        let config = &resolved.config;
        assert_eq!(
            config.get_hosts(),
            &[Host::Tcp("db.dsn.internal".to_string())],
            "dsn beats service and env"
        );
        assert_eq!(config.get_ports(), &[6432], "service beats env");
        assert_eq!(config.get_user(), Some("svc_user"), "service beats env");
        assert_eq!(config.get_dbname(), Some("svc_db"), "service beats env");
        assert_eq!(
            config.get_application_name(),
            Some("from_env"),
            "env fills fields the service left out"
        );
        assert_eq!(config.get_connect_timeout(), Some(&Duration::from_secs(7)));
        assert_eq!(resolved.label.to_string(), "svc_user@db.dsn.internal");
    }

    #[test]
    fn service_password_cmd_becomes_a_password_source_and_masks_pgpassword() {
        let file = services_file(PROD_SERVICE);
        let resolved = resolve(&ConnSpec {
            dsn: None,
            service: Some("prod".to_string()),
            services_file: Some(file.to_path_buf()),
            env: env(&[("PGPASSWORD", "env-pw")]),
       
            services_override: None,
        })
        .expect("resolution must succeed");
        assert!(
            matches!(&resolved.password_source, Some(PasswordSource::Command(c)) if c == "echo svc-pw"),
            "password_cmd must travel as a PasswordSource"
        );
        assert_eq!(
            resolved.config.get_password(),
            None,
            "service password_cmd beats PGPASSWORD (service > env)"
        );
    }

    #[test]
    fn dsn_password_beats_service_password_cmd() {
        let file = services_file(PROD_SERVICE);
        let resolved = resolve(&ConnSpec {
            dsn: Some("password=from-dsn".to_string()),
            service: Some("prod".to_string()),
            services_file: Some(file.to_path_buf()),
            env: HashMap::new(),
       
            services_override: None,
        })
        .expect("resolution must succeed");
        assert_eq!(
            resolved.config.get_password(),
            Some(b"from-dsn".as_slice())
        );
        assert!(
            resolved.password_source.is_none(),
            "no command should run when the DSN already provides the password"
        );
    }

    #[test]
    fn service_plaintext_password_lands_in_the_config() {
        let file = services_file(
            r#"
            [services.legacy]
            host = "h"
            password = "plain-pw"
            "#,
        );
        let resolved = resolve(&ConnSpec {
            dsn: None,
            service: Some("legacy".to_string()),
            services_file: Some(file.to_path_buf()),
            env: HashMap::new(),
       
            services_override: None,
        })
        .expect("resolution must succeed");
        assert_eq!(
            resolved.config.get_password(),
            Some(b"plain-pw".as_slice())
        );
        assert!(resolved.password_source.is_none());
        assert!(!resolved.label.to_string().contains("plain-pw"));
    }

    #[test]
    fn connect_timeout_zero_in_service_masks_pgconnect_timeout() {
        let file = services_file(
            r#"
            [services.slow]
            host = "h"
            connect_timeout_secs = 0
            "#,
        );
        let resolved = resolve(&ConnSpec {
            dsn: None,
            service: Some("slow".to_string()),
            services_file: Some(file.to_path_buf()),
            env: env(&[("PGCONNECT_TIMEOUT", "5")]),
       
            services_override: None,
        })
        .expect("resolution must succeed");
        assert_eq!(
            resolved.config.get_connect_timeout(),
            None,
            "service's 0 (= indefinitely) must shadow PGCONNECT_TIMEOUT"
        );
    }

    #[test]
    fn service_can_come_from_pgservice_env_and_pg_lens_service_wins() {
        let file = services_file(PROD_SERVICE);
        let path = file.to_path_buf().display().to_string();
        // PGSERVICE selects it...
        let resolved = resolve(&ConnSpec {
            dsn: None,
            service: None,
            services_file: None,
            env: env(&[("PGSERVICE", "prod"), ("PG_LENS_SERVICES_FILE", &path)]),
       
            services_override: None,
        })
        .expect("resolution must succeed");
        assert_eq!(resolved.config.get_user(), Some("svc_user"));

        // ...but PG_LENS_SERVICE outranks it (unknown name → error proves
        // which one was consulted).
        let err = resolve(&ConnSpec {
            dsn: None,
            service: None,
            services_file: None,
            env: env(&[
                ("PG_LENS_SERVICE", "missing"),
                ("PGSERVICE", "prod"),
                ("PG_LENS_SERVICES_FILE", &path),
            ]),
            services_override: None,
        })
        .expect_err("PG_LENS_SERVICE=missing must win and fail");
        assert!(matches!(err, SettingsError::UnknownService { .. }));
    }

    #[test]
    fn unknown_service_via_flag_lists_available_names() {
        let file = services_file(PROD_SERVICE);
        let err = resolve(&ConnSpec {
            dsn: None,
            service: Some("prdo".to_string()),
            services_file: Some(file.to_path_buf()),
            env: HashMap::new(),
       
            services_override: None,
        })
        .expect_err("typo'd service must fail");
        let msg = err.to_string();
        assert!(msg.contains("prdo"), "got: {msg}");
        assert!(msg.contains("prod"), "got: {msg}");
    }

    #[test]
    fn explicit_flag_with_missing_file_is_a_hard_error() {
        let err = resolve(&ConnSpec {
            dsn: None,
            service: Some("prod".to_string()),
            services_file: Some(PathBuf::from("/nonexistent/services.toml")),
            env: HashMap::new(),
       
            services_override: None,
        })
        .expect_err("--service with a missing file must fail");
        assert!(matches!(err, SettingsError::ServicesFileIo { .. }));
    }

    #[test]
    fn env_selected_service_with_no_file_degrades_to_a_warning() {
        // PGSERVICE may be set globally for psql; without a services.toml we
        // must keep working (env vars/defaults) and just say why we ignored it.
        let resolved = resolve(&ConnSpec {
            dsn: None,
            service: None,
            services_file: None,
            env: env(&[
                ("PGSERVICE", "prod"),
                ("HOME", "/nonexistent-home-for-test"),
                ("PGHOST", "db.env.internal"),
            ]),
            services_override: None,
        })
        .expect("ambient PGSERVICE without a file must not break resolution");
        assert_eq!(
            resolved.config.get_hosts(),
            &[Host::Tcp("db.env.internal".to_string())]
        );
        assert_eq!(resolved.warnings.len(), 1, "got: {:?}", resolved.warnings);
        assert!(resolved.warnings[0].contains("PGSERVICE"));
    }

    #[test]
    fn services_file_location_prefers_xdg_over_home() {
        let spec = ConnSpec {
            env: env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")]),
            ..ConnSpec::default()
        };
        let (path, explicit) = services_file_path(&spec).expect("path must resolve");
        assert_eq!(path, PathBuf::from("/xdg/pg_lens/services.toml"));
        assert!(!explicit);

        let spec = ConnSpec {
            env: env(&[("HOME", "/home/u")]),
            ..ConnSpec::default()
        };
        let (path, _) = services_file_path(&spec).expect("path must resolve");
        assert_eq!(path, PathBuf::from("/home/u/.config/pg_lens/services.toml"));
    }

    // --- services_override (--config-url) ------------------------------------

    #[test]
    fn services_override_is_used_instead_of_disk_and_skips_missing_file_errors() {
        let override_file = ServicesFile::from_remote_bytes(
            b"[services.prod]\nhost = \"remote-prod\"\nuser = \"svc\"\n",
        )
        .expect("override parses");
        let resolved = resolve(&ConnSpec {
            dsn: None,
            service: Some("prod".to_string()),
            // A path that does not exist: proves resolve() never touches it
            // when services_override is set.
            services_file: Some(PathBuf::from("/nonexistent/services.toml")),
            env: HashMap::new(),
            services_override: Some(override_file),
        })
        .expect("override must resolve without touching disk");
        assert_eq!(resolved.config.get_user(), Some("svc"));
        assert_eq!(resolved.label.to_string(), "svc@remote-prod");
    }

    #[test]
    fn services_override_unknown_service_is_a_hard_error_even_via_env_origin() {
        let override_file = ServicesFile::from_remote_bytes(b"[services.prod]\nhost = \"h\"\n")
            .expect("override parses");
        let err = resolve(&ConnSpec {
            dsn: None,
            service: None,
            services_file: None,
            env: env(&[("PGSERVICE", "typo")]),
            services_override: Some(override_file),
        })
        .expect_err("unknown name in the override must fail loudly, not degrade to a warning");
        assert!(matches!(err, SettingsError::UnknownService { .. }));
    }

    #[test]
    fn list_services_reflects_the_override_not_disk() {
        let override_file = ServicesFile::from_remote_bytes(
            b"[services.remote_a]\nhost = \"ha\"\nuser = \"ua\"\n",
        )
        .expect("override parses");
        let (summaries, warnings) = list_services(&ConnSpec {
            services_file: Some(PathBuf::from("/nonexistent/services.toml")),
            services_override: Some(override_file),
            ..ConnSpec::default()
        })
        .expect("must list the override");
        assert!(warnings.is_empty());
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "remote_a");
        assert_eq!(summaries[0].host.as_deref(), Some("ha"));
    }

    #[test]
    fn list_services_exposes_names_host_user_and_nothing_secret() {
        let file = services_file(
            r#"
            [services.b]
            host = "hb"
            password_cmd = "echo hidden-cmd"
            [services.a]
            host = "ha"
            user = "ua"
            "#,
        );
        let (summaries, _) = list_services(&ConnSpec {
            services_file: Some(file.to_path_buf()),
            ..ConnSpec::default()
        })
        .expect("listing must succeed");
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "a"); // BTreeMap: sorted
        assert_eq!(summaries[0].host.as_deref(), Some("ha"));
        assert_eq!(summaries[0].user.as_deref(), Some("ua"));
        assert_eq!(summaries[1].name, "b");
        // ServiceSummary simply has no secret-bearing fields — nothing to
        // assert beyond the type, but keep a canary on the rendered form.
        assert!(!format!("{}:{:?}:{:?}", summaries[1].name, summaries[1].host, summaries[1].user)
            .contains("hidden-cmd"));
    }
}
