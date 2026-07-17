//! Services file: named connection presets with external password
//! resolution (Fase C2).
//!
//! `~/.config/pg_lens/services.toml` (XDG) holds `[services.<name>]` tables
//! inspired by libpq's `pg_service.conf`, plus one thing libpq doesn't have:
//! `password_cmd`, an external command (`sh -c`) whose stdout becomes the
//! password — so the file never needs to contain a secret. The sugar
//! `password = "$(...)"` is converted to `password_cmd` at parse time.
//!
//! Because this file can *execute commands*, it is held to a stricter
//! standard than `pg_service.conf` (Unix only):
//! - group/world-**writable** → refused outright;
//! - plaintext `password` present and group/world-**readable** → refused
//!   (the `.pgpass` spirit);
//! - anything other than `0600` → warning.
//!
//! The command referenced by [`PasswordSource::Command`] is re-executed on
//! **every** (re)connection attempt — inside the poller's connect path, not
//! once at startup — so short-lived tokens (vault leases, OIDC) keep working
//! across reconnects.
//!
//! Like the rest of `settings`, nothing here reads `std::env`: callers pass
//! resolved paths in, and errors never echo passwords or command stdout.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;

use crate::settings::SettingsError;

/// One `[services.<name>]` table. Optional fields only: anything absent
/// falls through to the next precedence level (env vars, then defaults).
///
/// No `Debug` impl on purpose — a plaintext `password` must never end up in
/// a log via `{:?}`.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEntry {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub dbname: Option<String>,
    pub application_name: Option<String>,
    /// Connect timeout in whole seconds; `0` means "wait indefinitely"
    /// (libpq semantics) and also pins the field against `PGCONNECT_TIMEOUT`.
    pub connect_timeout_secs: Option<u64>,
    /// Plaintext password — accepted but discouraged (forces stricter file
    /// permissions). The `"$(...)"` form is moved into `password_cmd` during
    /// validation, so after [`ServicesFile::load`] this is plaintext-only.
    pub(crate) password: Option<String>,
    /// Command run as `sh -c <cmd>`; its trimmed stdout is the password.
    pub(crate) password_cmd: Option<String>,
}

/// Manual `Debug`: connection coordinates only — `password` (redacted) and
/// `password_cmd` never appear, so a stray `{:?}` cannot leak a secret.
impl std::fmt::Debug for ServiceEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceEntry")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("dbname", &self.dbname)
            .field("application_name", &self.application_name)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .field("has_password", &self.password.is_some())
            .field("has_password_cmd", &self.password_cmd.is_some())
            .finish()
    }
}

impl ServiceEntry {
    /// Plaintext password, if the entry (discouraged) carries one.
    pub(crate) fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    /// External password command, if configured (or via `"$(...)"` sugar).
    pub(crate) fn password_cmd(&self) -> Option<&str> {
        self.password_cmd.as_deref()
    }
}

/// The parsed, validated services file. `BTreeMap` keeps service names
/// sorted, so listings and "unknown service" errors are deterministic.
///
/// `Clone` exists so a [`crate::settings::ConnSpec`] can carry an
/// already-resolved file (`services_override`, populated by
/// `--config-url`'s local+remote merge) without re-reading disk on every
/// `resolve`/`list_services` call.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServicesFile {
    #[serde(default)]
    services: BTreeMap<String, ServiceEntry>,
}

impl ServicesFile {
    /// Reads, permission-checks, parses and validates `path`.
    ///
    /// Returns the file plus non-fatal warnings (e.g. mode != 0600) for the
    /// frontend to surface *before* taking over the terminal.
    pub fn load(path: &Path) -> Result<(Self, Vec<String>), SettingsError> {
        let contents =
            std::fs::read_to_string(path).map_err(|error| SettingsError::ServicesFileIo {
                path: path.to_path_buf(),
                error,
            })?;

        #[cfg(unix)]
        let mode = file_mode(path)?;
        // A file writable by others can inject arbitrary commands into
        // password_cmd — refuse before even parsing it.
        #[cfg(unix)]
        if mode & 0o022 != 0 {
            return Err(SettingsError::InsecureServicesFile {
                path: path.to_path_buf(),
                mode,
                reason: "writable by group/others; this file can execute commands, \
                         so treat it like code (fix: chmod 0600)"
                    .to_string(),
            });
        }

        let mut file: ServicesFile =
            toml::from_str(&contents).map_err(|e| SettingsError::ServicesFileParse {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;
        file.validate()?;

        #[allow(unused_mut)] // pushed to under cfg(unix) only
        let mut warnings = Vec::new();
        #[cfg(unix)]
        {
            let has_plaintext = file.services.values().any(|s| s.password.is_some());
            if has_plaintext && mode & 0o044 != 0 {
                return Err(SettingsError::InsecureServicesFile {
                    path: path.to_path_buf(),
                    mode,
                    reason: "contains a plaintext `password` and is readable by \
                             group/others (fix: chmod 0600, or switch to password_cmd)"
                        .to_string(),
                });
            }
            if mode != 0o600 {
                warnings.push(format!(
                    "services file {} has mode {mode:04o}; 0600 is recommended",
                    path.display()
                ));
            }
        }

        Ok((file, warnings))
    }

    /// Per-entry validation: the `"$(...)"` sugar becomes `password_cmd`,
    /// and `password` + `password_cmd` together is a hard error.
    fn validate(&mut self) -> Result<(), SettingsError> {
        for (name, entry) in &mut self.services {
            if let Some(pw) = &entry.password {
                let sugar = command_sugar(pw);
                if entry.password_cmd.is_some() {
                    return Err(SettingsError::ServiceConfig {
                        service: name.clone(),
                        message: "`password` and `password_cmd` are mutually exclusive \
                                  (note: password = \"$(...)\" is shorthand for password_cmd)"
                            .to_string(),
                    });
                }
                if let Some(cmd) = sugar {
                    entry.password_cmd = Some(cmd);
                    entry.password = None;
                }
            }
        }
        Ok(())
    }

    /// Looks a service up by name; the error carries the available names so
    /// a typo is a one-glance fix.
    pub fn get(&self, name: &str) -> Result<&ServiceEntry, SettingsError> {
        self.services
            .get(name)
            .ok_or_else(|| SettingsError::UnknownService {
                name: name.to_string(),
                available: self.services.keys().cloned().collect(),
            })
    }

    /// Sorted `(name, entry)` pairs — the basis for `--list-services`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ServiceEntry)> {
        self.services.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// An empty services file (no entries) — the base case when
    /// `--config-url`'s local+remote merge finds neither.
    pub fn empty() -> Self {
        ServicesFile {
            services: BTreeMap::new(),
        }
    }

    /// Parses and validates bytes fetched from `--config-url` — the exact
    /// same `[services.<name>]` TOML shape as [`ServicesFile::load`], minus
    /// the on-disk permission checks (there is no local file to check; the
    /// fetch's own transport — https, plus a token scoped to a private
    /// repo — is the trust boundary here, not a Unix file mode). The bytes
    /// this parses are themselves cached to a file `pg_lens` creates at
    /// `0600` (see the TUI's `remote_config_cache_path`), so a later reload
    /// from that cache still goes through the normal `load` permission path.
    pub fn from_remote_bytes(bytes: &[u8]) -> Result<Self, SettingsError> {
        let remote_path = PathBuf::from("<--config-url>");
        let contents = String::from_utf8(bytes.to_vec()).map_err(|_| {
            SettingsError::ServicesFileParse {
                path: remote_path.clone(),
                message: "remote services file is not valid UTF-8".to_string(),
            }
        })?;
        let mut file: ServicesFile =
            toml::from_str(&contents).map_err(|e| SettingsError::ServicesFileParse {
                path: remote_path,
                message: e.to_string(),
            })?;
        file.validate()?;
        Ok(file)
    }

    /// Merges `remote` over `self` (the local file): entries in `remote`
    /// replace any same-named local entry; local-only entries survive
    /// untouched. This is `--config-url`'s precedence rule in full —
    /// "remote wins per service name, local fills in the rest".
    pub fn merge_remote_over(mut self, remote: ServicesFile) -> Self {
        for (name, entry) in remote.services {
            self.services.insert(name, entry);
        }
        self
    }
}

/// `"$(cmd)"` → `Some("cmd")`; anything else is a literal password.
fn command_sugar(password: &str) -> Option<String> {
    password
        .strip_prefix("$(")
        .and_then(|rest| rest.strip_suffix(')'))
        .map(str::to_string)
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Result<u32, SettingsError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path).map_err(|error| SettingsError::ServicesFileIo {
        path: path.to_path_buf(),
        error,
    })?;
    Ok(metadata.permissions().mode() & 0o7777)
}

/// Where the password comes from when it is not a static value already
/// baked into the `Config`.
///
/// Carried *next to* the `Config` (never inside it) so the poller can
/// re-resolve it before **each** connection attempt — see
/// [`resolve_password_cmd`].
#[derive(Clone)]
pub enum PasswordSource {
    /// Run `sh -c <cmd>`; trimmed stdout is the password.
    Command(String),
}

const PASSWORD_CMD_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs `sh -c <cmd>` and returns its stdout — trailing `\n`/`\r\n`
/// stripped — as the password bytes.
///
/// Failure modes (all [`SettingsError::PasswordCmd`], all safe to display):
/// - non-zero exit → the message carries an excerpt of **stderr** (stdout is
///   never included: it may be a partial secret);
/// - timeout (10s) → the child is killed and reported;
/// - `sh` cannot be spawned → the OS error.
pub async fn resolve_password_cmd(cmd: &str) -> Result<Vec<u8>, SettingsError> {
    resolve_password_cmd_with_timeout(cmd, PASSWORD_CMD_TIMEOUT).await
}

async fn resolve_password_cmd_with_timeout(
    cmd: &str,
    timeout: Duration,
) -> Result<Vec<u8>, SettingsError> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // On timeout the future is dropped — take the child down with it.
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(timeout, output)
        .await
        .map_err(|_| SettingsError::PasswordCmd {
            message: format!("timed out after {}s", timeout.as_secs()),
        })?
        .map_err(|e| SettingsError::PasswordCmd {
            message: format!("could not run `sh -c`: {e}"),
        })?;

    if !output.status.success() {
        // stderr only — stdout could hold (part of) a secret.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let excerpt: String = stderr.trim().chars().take(200).collect();
        let message = if excerpt.is_empty() {
            format!("{}", output.status)
        } else {
            format!("{}: {excerpt}", output.status)
        };
        return Err(SettingsError::PasswordCmd { message });
    }

    let mut password = output.stdout;
    while matches!(password.last(), Some(b'\n' | b'\r')) {
        password.pop();
    }
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::io::Write;

    fn parse(toml_src: &str) -> Result<ServicesFile, SettingsError> {
        let mut file: ServicesFile = toml::from_str(toml_src).map_err(|e| {
            SettingsError::ServicesFileParse {
                path: std::path::PathBuf::from("<test>"),
                message: e.to_string(),
            }
        })?;
        file.validate()?;
        Ok(file)
    }

    // --- TOML parsing + validation ---------------------------------------

    #[test]
    fn parses_a_full_service_entry() {
        let file = parse(
            r#"
            [services.prod]
            host = "db.prod.internal"
            port = 6432
            user = "pg_monitor_ro"
            dbname = "app"
            application_name = "pg_lens"
            connect_timeout_secs = 5
            password_cmd = "vault kv get -field=password secret/pg/prod"
            "#,
        )
        .expect("valid file must parse");
        let entry = file.get("prod").expect("prod must exist");
        assert_eq!(entry.host.as_deref(), Some("db.prod.internal"));
        assert_eq!(entry.port, Some(6432));
        assert_eq!(entry.user.as_deref(), Some("pg_monitor_ro"));
        assert_eq!(entry.dbname.as_deref(), Some("app"));
        assert_eq!(entry.application_name.as_deref(), Some("pg_lens"));
        assert_eq!(entry.connect_timeout_secs, Some(5));
        assert_eq!(
            entry.password_cmd(),
            Some("vault kv get -field=password secret/pg/prod")
        );
        assert_eq!(entry.password(), None);
    }

    #[test]
    fn dollar_paren_sugar_becomes_password_cmd() {
        let file = parse(
            r#"
            [services.staging]
            host = "db.staging.internal"
            password = "$(op read op://infra/pg-staging/password)"
            "#,
        )
        .expect("sugar must be accepted");
        let entry = file.get("staging").expect("staging must exist");
        assert_eq!(
            entry.password_cmd(),
            Some("op read op://infra/pg-staging/password")
        );
        assert_eq!(entry.password(), None, "sugar must not stay as plaintext");
    }

    #[test]
    fn literal_password_stays_a_password() {
        let file = parse(
            r#"
            [services.legacy]
            password = "hunter2"
            "#,
        )
        .expect("plaintext password is discouraged but accepted");
        let entry = file.get("legacy").expect("legacy must exist");
        assert_eq!(entry.password(), Some("hunter2"));
        assert_eq!(entry.password_cmd(), None);
    }

    #[test]
    fn password_and_password_cmd_together_is_an_error() {
        let err = parse(
            r#"
            [services.bad]
            password = "x"
            password_cmd = "echo y"
            "#,
        )
        .expect_err("both set must fail validation");
        let msg = err.to_string();
        assert!(msg.contains("bad"), "got: {msg}");
        assert!(msg.contains("mutually exclusive"), "got: {msg}");
    }

    #[test]
    fn sugar_plus_password_cmd_is_also_an_error() {
        let err = parse(
            r#"
            [services.bad]
            password = "$(echo x)"
            password_cmd = "echo y"
            "#,
        )
        .expect_err("sugar + password_cmd must fail validation");
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn unknown_service_error_lists_the_available_names() {
        let file = parse(
            r#"
            [services.prod]
            host = "a"
            [services.staging]
            host = "b"
            "#,
        )
        .expect("valid file");
        let err = file.get("prdo").expect_err("typo must fail");
        let msg = err.to_string();
        assert!(msg.contains("prdo"), "got: {msg}");
        assert!(msg.contains("prod"), "got: {msg}");
        assert!(msg.contains("staging"), "got: {msg}");
    }

    #[test]
    fn unknown_field_is_a_parse_error() {
        let err = parse(
            r#"
            [services.prod]
            host = "a"
            passwordcmd = "typo"
            "#,
        )
        .expect_err("unknown key must be rejected (deny_unknown_fields)");
        assert!(matches!(err, SettingsError::ServicesFileParse { .. }));
    }

    // --- permission matrix (Unix) -----------------------------------------

    #[cfg(unix)]
    fn write_services_file(contents: &str, mode: u32) -> tempfile::TempPath {
        use std::os::unix::fs::PermissionsExt;
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(contents.as_bytes()).expect("write");
        f.flush().expect("flush");
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(mode))
            .expect("chmod");
        f.into_temp_path()
    }

    #[cfg(unix)]
    const PLAIN_PASSWORD: &str = "[services.a]\nhost = \"h\"\npassword = \"pw\"\n";
    #[cfg(unix)]
    const CMD_ONLY: &str = "[services.a]\nhost = \"h\"\npassword_cmd = \"echo pw\"\n";

    #[cfg(unix)]
    #[test]
    fn mode_0600_loads_without_warnings() {
        let path = write_services_file(CMD_ONLY, 0o600);
        let (file, warnings) = ServicesFile::load(&path).expect("0600 must load");
        assert!(warnings.is_empty(), "got warnings: {warnings:?}");
        assert!(file.get("a").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn group_or_world_writable_is_refused() {
        for mode in [0o620, 0o602, 0o666] {
            let path = write_services_file(CMD_ONLY, mode);
            let err = ServicesFile::load(&path)
                .map(|_| ())
                .expect_err("writable by others must be refused");
            let msg = err.to_string();
            assert!(msg.contains("writable"), "mode {mode:o}: got {msg}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn readable_with_plaintext_password_is_refused() {
        let path = write_services_file(PLAIN_PASSWORD, 0o644);
        let err = ServicesFile::load(&path)
            .map(|_| ())
            .expect_err("0644 + plaintext password must be refused");
        let msg = err.to_string();
        assert!(msg.contains("plaintext"), "got: {msg}");
        assert!(!msg.contains("pw\""), "error must not echo the password: {msg}");
    }

    #[cfg(unix)]
    #[test]
    fn readable_without_plaintext_password_is_only_a_warning() {
        let path = write_services_file(CMD_ONLY, 0o644);
        let (_, warnings) = ServicesFile::load(&path).expect("0644 without password loads");
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(warnings[0].contains("0644"), "got: {warnings:?}");
        assert!(warnings[0].contains("0600"), "got: {warnings:?}");
    }

    #[cfg(unix)]
    #[test]
    fn sugar_password_does_not_count_as_plaintext() {
        // password = "$(...)" is converted to password_cmd, so 0644 is only
        // a warning, not a refusal.
        let sugar = "[services.a]\nhost = \"h\"\npassword = \"$(echo pw)\"\n";
        let path = write_services_file(sugar, 0o644);
        assert!(ServicesFile::load(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn mode_0666_is_always_refused() {
        let path = write_services_file(PLAIN_PASSWORD, 0o666);
        assert!(ServicesFile::load(&path).is_err());
    }

    // --- resolve_password_cmd ---------------------------------------------

    #[tokio::test]
    async fn echo_stdout_becomes_the_password() {
        let pw = resolve_password_cmd("echo secret")
            .await
            .expect("echo must succeed");
        assert_eq!(pw, b"secret");
    }

    #[tokio::test]
    async fn trailing_newlines_are_trimmed_but_inner_content_kept() {
        let pw = resolve_password_cmd("printf 'x\\n'")
            .await
            .expect("printf must succeed");
        assert_eq!(pw, b"x");

        let pw = resolve_password_cmd("printf 'a b\\r\\n'")
            .await
            .expect("printf must succeed");
        assert_eq!(pw, b"a b");
    }

    #[tokio::test]
    async fn nonzero_exit_is_an_error_with_stderr_but_never_stdout() {
        let err = resolve_password_cmd("echo topsecret; echo broken >&2; exit 3")
            .await
            .expect_err("exit 3 must fail");
        let msg = err.to_string();
        assert!(msg.contains("broken"), "stderr must be included: {msg}");
        assert!(
            !msg.contains("topsecret"),
            "stdout must NEVER leak into the error: {msg}"
        );
    }

    #[tokio::test]
    async fn false_is_a_clear_error() {
        let err = resolve_password_cmd("false")
            .await
            .expect_err("false must fail");
        assert!(matches!(err, SettingsError::PasswordCmd { .. }));
    }

    // --- remote (--config-url) helpers -------------------------------------

    #[test]
    fn from_remote_bytes_parses_like_a_local_file() {
        let file = ServicesFile::from_remote_bytes(PROD_SERVICE.as_bytes())
            .expect("valid remote bytes must parse");
        let entry = file.get("prod").expect("prod must exist");
        assert_eq!(entry.host.as_deref(), Some("db.service.internal"));
    }

    #[test]
    fn from_remote_bytes_rejects_invalid_utf8() {
        let err = ServicesFile::from_remote_bytes(&[0xff, 0xfe, 0x00])
            .expect_err("invalid utf-8 must fail");
        assert!(matches!(err, SettingsError::ServicesFileParse { .. }));
    }

    #[test]
    fn merge_remote_over_prefers_remote_by_name_and_keeps_local_only_entries() {
        let local = parse(
            r#"
            [services.prod]
            host = "local-prod"
            [services.staging]
            host = "local-staging"
            "#,
        )
        .expect("local file");
        let remote = parse(
            r#"
            [services.prod]
            host = "remote-prod"
            [services.ci]
            host = "remote-ci"
            "#,
        )
        .expect("remote file");
        let merged = local.merge_remote_over(remote);
        assert_eq!(
            merged.get("prod").unwrap().host.as_deref(),
            Some("remote-prod"),
            "remote wins on a name collision"
        );
        assert_eq!(
            merged.get("staging").unwrap().host.as_deref(),
            Some("local-staging"),
            "local-only entries survive"
        );
        assert_eq!(
            merged.get("ci").unwrap().host.as_deref(),
            Some("remote-ci"),
            "remote-only entries are added"
        );
    }

    #[test]
    fn empty_has_no_services() {
        let file = ServicesFile::empty();
        assert!(file.iter().next().is_none());
    }

    const PROD_SERVICE: &str = r#"
        [services.prod]
        host = "db.service.internal"
        port = 6432
        user = "svc_user"
    "#;

    #[tokio::test]
    async fn slow_commands_time_out() {
        let err = resolve_password_cmd_with_timeout("sleep 30", Duration::from_millis(100))
            .await
            .expect_err("sleep must hit the timeout");
        assert!(err.to_string().contains("timed out"), "got: {err}");
    }
}
