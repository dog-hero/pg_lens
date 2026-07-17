//! `!`: suspend the TUI and drop the operator into an interactive `psql`
//! shell on the SAME connection as the poller (v0.11).
//!
//! This module holds the PURE, unit-testable pieces — extracting
//! host/port/user/dbname from a resolved `tokio_postgres::Config` and
//! building the argv/env for `std::process::Command` — plus the one bit of
//! async I/O that cannot be pure (re-resolving a `password_cmd` late, the
//! same discipline `poller.rs::session` already uses). The actual
//! suspend-terminal / spawn / restore-terminal dance stays in `main.rs`:
//! that's the only place that owns the `DefaultTerminal` and the input
//! task, and no `.await`/process I/O is allowed under `ui/`.
//!
//! Secret handling: a password NEVER reaches `PsqlInvocation::args` (only
//! `--host`/`--port`/`--username`/`--dbname`, matching the PRD's explicit
//! "never on argv" instruction) — it travels solely in `PsqlInvocation::env`
//! as `PGPASSWORD`, resolved as late as possible (right before spawn) and
//! never written to disk, the services cache, or any log line. Both structs
//! below implement `Debug` by hand so an incidental `{:?}` (test failure
//! output, a stray `dbg!()`) cannot leak it either — the same defense in
//! depth `settings::Resolved` already applies to the DSN/password.

use pg_lens_core::PasswordSource;
use pg_lens_core::tokio_postgres::Config;
use pg_lens_core::tokio_postgres::config::Host;

/// Read-only mode's `PGOPTIONS`: a read-only default transaction. This
/// cannot stop `psql` from running an explicit `BEGIN; ... ; COMMIT;` or
/// `SET default_transaction_read_only = off` — a full shell is definitionally
/// unrestricted — but it honors read-only mode's spirit (pg_lens itself
/// never mutates) for the common case of an operator just looking around,
/// and the caller always prints a notice alongside it so nobody is misled
/// into thinking this is a hard sandbox.
pub const READ_ONLY_PGOPTIONS: &str = "-c default_transaction_read_only=on";

/// Connection parameters resolved late, held only long enough to build one
/// [`PsqlInvocation`] and then dropped — never persisted, never logged.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PsqlConnInfo {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub dbname: Option<String>,
    pub password: Option<String>,
}

/// Manual `Debug`: everything except whether a password is present.
impl std::fmt::Debug for PsqlConnInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PsqlConnInfo")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("dbname", &self.dbname)
            .field("has_password", &self.password.is_some())
            .finish()
    }
}

/// What to hand `std::process::Command`: positional flags (never the
/// password) plus the env pairs that carry the secret and/or the
/// read-only-mode `PGOPTIONS`.
#[derive(Clone, PartialEq, Eq)]
pub struct PsqlInvocation {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Manual `Debug`: env VALUES are never printed, only the key names (so a
/// stray `{:?}` still shows "PGPASSWORD was set" without the secret itself).
impl std::fmt::Debug for PsqlInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PsqlInvocation")
            .field("args", &self.args)
            .field(
                "env_keys",
                &self.env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Builds the args + env for launching `psql` on `conn`. Fields left `None`
/// (unresolved, or not present on the resolved `Config`) are simply
/// omitted — psql then falls back to its own libpq defaults instead of
/// getting an empty `--host=`. Read-only mode adds `PGOPTIONS` (see
/// [`READ_ONLY_PGOPTIONS`]); a present, non-empty password adds
/// `PGPASSWORD` — and ONLY there, never in `args`.
pub fn build_psql_invocation(conn: &PsqlConnInfo, read_only: bool) -> PsqlInvocation {
    fn push(args: &mut Vec<String>, flag: &str, value: &Option<String>) {
        if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
            args.push(flag.to_string());
            args.push(v.to_string());
        }
    }

    let mut args = Vec::new();
    push(&mut args, "--host", &conn.host);
    if let Some(port) = conn.port {
        args.push("--port".to_string());
        args.push(port.to_string());
    }
    push(&mut args, "--username", &conn.user);
    push(&mut args, "--dbname", &conn.dbname);

    let mut env = Vec::new();
    if let Some(password) = conn.password.as_deref().filter(|p| !p.is_empty()) {
        env.push(("PGPASSWORD".to_string(), password.to_string()));
    }
    if read_only {
        env.push(("PGOPTIONS".to_string(), READ_ONLY_PGOPTIONS.to_string()));
    }
    PsqlInvocation { args, env }
}

/// Extracts host/port/user/dbname from a resolved `tokio_postgres::Config`
/// — the exact target the poller itself connects to. `password` is supplied
/// separately (see [`resolve_password`]) because `Config::get_password`
/// only ever holds a STATIC secret (DSN / service `password` / `PGPASSWORD`
/// env) — a `password_cmd` source needs a fresh, late re-resolution instead.
pub fn conn_info_from_config(config: &Config, password: Option<String>) -> PsqlConnInfo {
    let host = config.get_hosts().first().map(|h| match h {
        Host::Tcp(s) => s.clone(),
        Host::Unix(p) => p.to_string_lossy().into_owned(),
    });
    let port = config.get_ports().first().copied();
    let user = config.get_user().map(str::to_string);
    let dbname = config.get_dbname().map(str::to_string);
    PsqlConnInfo {
        host,
        port,
        user,
        dbname,
        password,
    }
}

/// Resolves the password to hand `psql`, as LATE as possible (call this
/// immediately before spawning, never earlier): a `password_cmd` source is
/// re-run fresh — the same "resolve per attempt, never cache" discipline
/// `poller.rs::session` already applies to reconnects, so a rotating token
/// stays valid — otherwise falls back to the `Config`'s own static
/// password, if any. Best-effort: a failed `password_cmd` returns `None`
/// rather than propagating an error — `psql` then simply prompts
/// interactively instead of silently getting a stale/empty secret.
pub async fn resolve_password(config: &Config, source: Option<&PasswordSource>) -> Option<String> {
    if let Some(PasswordSource::Command(cmd)) = source {
        return pg_lens_core::services::resolve_password_cmd(cmd)
            .await
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    }
    config
        .get_password()
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(
        host: Option<&str>,
        port: Option<u16>,
        user: Option<&str>,
        dbname: Option<&str>,
        password: Option<&str>,
    ) -> PsqlConnInfo {
        PsqlConnInfo {
            host: host.map(str::to_string),
            port,
            user: user.map(str::to_string),
            dbname: dbname.map(str::to_string),
            password: password.map(str::to_string),
        }
    }

    #[test]
    fn full_connection_builds_every_flag_and_pgpassword() {
        let c = conn(
            Some("db.internal"),
            Some(5433),
            Some("ro"),
            Some("shop"),
            Some("s3cr3t"),
        );
        let inv = build_psql_invocation(&c, false);
        assert_eq!(
            inv.args,
            vec![
                "--host", "db.internal", "--port", "5433", "--username", "ro", "--dbname", "shop",
            ]
        );
        assert_eq!(inv.env, vec![("PGPASSWORD".to_string(), "s3cr3t".to_string())]);
    }

    #[test]
    fn missing_fields_are_simply_omitted_not_empty_flags() {
        let c = conn(None, None, None, None, None);
        let inv = build_psql_invocation(&c, false);
        assert!(inv.args.is_empty(), "no libpq defaults should be overridden: {inv:?}");
        assert!(inv.env.is_empty());
    }

    #[test]
    fn no_password_means_no_pgpassword_env() {
        let c = conn(Some("localhost"), Some(5432), Some("postgres"), None, None);
        let inv = build_psql_invocation(&c, false);
        assert!(!inv.env.iter().any(|(k, _)| k == "PGPASSWORD"));
    }

    #[test]
    fn empty_password_string_is_treated_as_absent() {
        let c = conn(Some("localhost"), None, None, None, Some(""));
        let inv = build_psql_invocation(&c, false);
        assert!(inv.env.is_empty());
    }

    #[test]
    fn read_only_sets_pgoptions_default_read_only_transaction() {
        let c = conn(Some("localhost"), None, None, None, None);
        let inv = build_psql_invocation(&c, true);
        assert_eq!(
            inv.env,
            vec![("PGOPTIONS".to_string(), READ_ONLY_PGOPTIONS.to_string())]
        );
    }

    #[test]
    fn read_only_and_password_both_present_together() {
        let c = conn(Some("localhost"), None, Some("ro"), None, Some("pw"));
        let inv = build_psql_invocation(&c, true);
        assert_eq!(inv.env.len(), 2, "{inv:?}");
        assert!(inv.env.contains(&("PGPASSWORD".to_string(), "pw".to_string())));
        assert!(inv.env.contains(&("PGOPTIONS".to_string(), READ_ONLY_PGOPTIONS.to_string())));
    }

    #[test]
    fn args_never_contain_the_password() {
        let c = conn(Some("h"), Some(1), Some("u"), Some("d"), Some("super-secret"));
        let inv = build_psql_invocation(&c, false);
        assert!(!inv.args.iter().any(|a| a.contains("super-secret")));
    }

    #[test]
    fn debug_output_never_prints_the_password() {
        let c = conn(Some("h"), Some(1), Some("u"), Some("d"), Some("super-secret"));
        let inv = build_psql_invocation(&c, false);
        let rendered = format!("{c:?} {inv:?}");
        assert!(!rendered.contains("super-secret"), "leaked: {rendered}");
    }

    #[tokio::test]
    async fn resolve_password_falls_back_to_the_configs_static_password() {
        let config: Config = "host=localhost user=ro password=static-pw".parse().expect("dsn");
        let resolved = resolve_password(&config, None).await;
        assert_eq!(resolved.as_deref(), Some("static-pw"));
    }

    #[tokio::test]
    async fn resolve_password_prefers_a_fresh_password_cmd_over_the_static_one() {
        let config: Config = "host=localhost user=ro password=stale".parse().expect("dsn");
        let source = PasswordSource::Command("echo fresh-pw".to_string());
        let resolved = resolve_password(&config, Some(&source)).await;
        assert_eq!(resolved.as_deref(), Some("fresh-pw"));
    }

    #[tokio::test]
    async fn resolve_password_is_none_when_nothing_is_set() {
        let config: Config = "host=localhost user=ro".parse().expect("dsn");
        assert!(resolve_password(&config, None).await.is_none());
    }

    #[test]
    fn conn_info_from_config_reads_host_port_user_dbname() {
        let config: Config = "host=db.internal port=5433 dbname=shop user=ro"
            .parse()
            .expect("dsn");
        let info = conn_info_from_config(&config, Some("pw".to_string()));
        assert_eq!(info.host.as_deref(), Some("db.internal"));
        assert_eq!(info.port, Some(5433));
        assert_eq!(info.user.as_deref(), Some("ro"));
        assert_eq!(info.dbname.as_deref(), Some("shop"));
        assert_eq!(info.password.as_deref(), Some("pw"));
    }
}
