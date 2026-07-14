//! Connection resolution: DSN + libpq environment variables →
//! `tokio_postgres::Config` (Fase C1).
//!
//! `tokio-postgres` parses connection strings but deliberately ignores the
//! libpq environment (`PGHOST`, `PGUSER`, ...); this module fills that gap
//! with the libpq precedence: **explicit DSN field > env var > default**
//! (`host=localhost user=postgres`).
//!
//! The environment is *injected* through [`ConnSpec::env`] — [`resolve`]
//! never touches `std::env` — so the whole precedence matrix is testable
//! with plain maps, no `set_var` flakiness. Frontends capture
//! `std::env::vars()` once at startup and hand it over.
//!
//! Security: the resolved [`ConnLabel`] is the only thing meant for
//! display/logs, and it never carries the password. Nothing here (including
//! errors) prints the `Config` via `Debug`.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use tokio_postgres::Config;
use tokio_postgres::config::Host;

/// What a frontend knows about the desired connection: an optional DSN
/// (`key=value` or `postgres://` URL) plus a snapshot of the process
/// environment, captured by the caller.
#[derive(Clone, Debug, Default)]
pub struct ConnSpec {
    /// Explicit connection string (e.g. from `--dsn`). Fields present here
    /// always win over environment variables.
    pub dsn: Option<String>,
    /// The process environment (typically `std::env::vars().collect()`),
    /// injected so resolution stays a pure function.
    pub env: HashMap<String, String>,
}

/// Why connection resolution failed. Rendering these is safe: the DSN
/// itself (which may carry a password) is never echoed back.
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
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SettingsError::DsnParse(e) => Some(e),
            SettingsError::InvalidEnvVar { .. } => None,
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

/// Resolves a [`ConnSpec`] into a ready-to-dial `tokio_postgres::Config`
/// plus its display-safe [`ConnLabel`].
///
/// Precedence per field (libpq semantics): explicit DSN field, then the
/// matching `PG*` env var, then the defaults `host=localhost` /
/// `user=postgres`. Empty env values are treated as unset.
pub fn resolve(spec: &ConnSpec) -> Result<(Config, ConnLabel), SettingsError> {
    let mut config = match &spec.dsn {
        Some(dsn) => Config::from_str(dsn).map_err(SettingsError::DsnParse)?,
        None => Config::new(),
    };

    for rule in ENV_RULES {
        if (rule.is_set)(&config) {
            continue; // the DSN already said so — env is only a default
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
    Ok((config, label))
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

    fn resolve_ok(dsn: Option<&str>, env: HashMap<String, String>) -> (Config, ConnLabel) {
        resolve(&ConnSpec {
            dsn: dsn.map(str::to_string),
            env,
        })
        .expect("resolution must succeed")
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
        })
        .expect_err("bad PGCONNECT_TIMEOUT must fail");
        assert!(err.to_string().contains("PGCONNECT_TIMEOUT"));
    }

    #[test]
    fn invalid_dsn_is_a_clear_error() {
        let err = resolve(&ConnSpec {
            dsn: Some("port=notaport".to_string()),
            env: HashMap::new(),
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
}
