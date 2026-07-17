//! `--config-url`: loading a team-shared `services.toml` (see
//! [`crate::services`]) from a remote source — a private GitHub repo, or any
//! plain HTTPS URL to raw file bytes — instead of everyone copying the file
//! around by hand.
//!
//! ## URL scheme
//!
//! One shorthand, one escape hatch — see [`parse_config_url`]:
//! - `github:OWNER/REPO/PATH[@REF]` — GitHub's Contents API
//!   (`https://api.github.com/repos/OWNER/REPO/contents/PATH?ref=REF`; `REF`
//!   optional, defaults to the repo's default branch). Works for private
//!   repos given a token with `contents: read`. The request sends
//!   `Accept: application/vnd.github.raw+json` so the response body is the
//!   raw `services.toml` bytes directly — no base64/JSON envelope to peel.
//! - anything else must be a plain `https://`/`http://` URL to raw file
//!   bytes (self-hosted git server, GitLab raw, a signed URL, ...).
//!
//! Example: `--config-url github:my-org/pg-lens-config/services.toml@main`
//!
//! ## Auth
//!
//! Same secret discipline as `password_cmd` ([`crate::services`]): the token
//! NEVER lives in `config.toml` or this URL. `pg_lens_tui::main` resolves it
//! from `PG_LENS_CONFIG_TOKEN`, then `GITHUB_TOKEN`, then a
//! `remote_config_token_cmd` in `config.toml` (run via
//! [`crate::services::resolve_password_cmd`] — the exact same "external
//! command, trimmed stdout" mechanism `password_cmd` uses). It is sent as
//! `Authorization: Bearer <token>` — never in the URL/query string, and
//! [`fetch_remote_bytes`] refuses to send it over plain `http://`.
//!
//! ## Fetch, cache, fallback
//!
//! [`fetch_remote_bytes`] performs one blocking GET with a short timeout —
//! this runs once at startup, before the async poller, so a blocking client
//! (`ureq`) is simpler than pulling in an async HTTP stack. This module
//! never touches disk itself (same "paths are injected" discipline as the
//! rest of `pg_lens_core` — see `settings::services_file_path` /
//! `history_file_path` precedent): [`resolve_effective_services`] is the
//! pure precedence/merge logic (fully unit-testable without any network or
//! filesystem access), and `pg_lens_tui::main::resolve_remote_overlay` is
//! the disk/network glue that calls both.

use std::time::Duration;

use crate::services::ServicesFile;
use crate::settings::SettingsError;

/// A parsed `--config-url` value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigUrl {
    /// `github:OWNER/REPO/PATH[@REF]`.
    Github {
        owner: String,
        repo: String,
        path: String,
        git_ref: Option<String>,
    },
    /// A plain `https://`/`http://` URL to raw file bytes.
    Http(String),
}

impl ConfigUrl {
    /// The URL actually dialed for the GET.
    pub fn fetch_url(&self) -> String {
        match self {
            ConfigUrl::Github {
                owner,
                repo,
                path,
                git_ref,
            } => {
                let mut url =
                    format!("https://api.github.com/repos/{owner}/{repo}/contents/{path}");
                if let Some(git_ref) = git_ref {
                    url.push_str("?ref=");
                    url.push_str(git_ref);
                }
                url
            }
            ConfigUrl::Http(url) => url.clone(),
        }
    }
}

/// Parses `--config-url` / `PG_LENS_CONFIG_URL` / `remote_config`.
///
/// `github:OWNER/REPO/PATH[@REF]` → [`ConfigUrl::Github`]; a value starting
/// with `https://` or `http://` → [`ConfigUrl::Http`] verbatim; anything else
/// is a [`SettingsError::RemoteConfigUrl`].
pub fn parse_config_url(raw: &str) -> Result<ConfigUrl, SettingsError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(SettingsError::RemoteConfigUrl {
            message: "empty value".to_string(),
        });
    }
    if let Some(rest) = raw.strip_prefix("github:") {
        let (path_part, git_ref) = match rest.split_once('@') {
            Some((p, r)) if !r.is_empty() => (p, Some(r.to_string())),
            _ => (rest, None),
        };
        let mut parts = path_part.splitn(3, '/');
        let owner = parts.next().filter(|s| !s.is_empty());
        let repo = parts.next().filter(|s| !s.is_empty());
        let path = parts.next().filter(|s| !s.is_empty());
        return match (owner, repo, path) {
            (Some(owner), Some(repo), Some(path)) => Ok(ConfigUrl::Github {
                owner: owner.to_string(),
                repo: repo.to_string(),
                path: path.to_string(),
                git_ref,
            }),
            _ => Err(SettingsError::RemoteConfigUrl {
                message: format!(
                    "invalid github: shorthand {raw:?}: expected github:OWNER/REPO/PATH[@REF]"
                ),
            }),
        };
    }
    if raw.starts_with("https://") || raw.starts_with("http://") {
        return Ok(ConfigUrl::Http(raw.to_string()));
    }
    Err(SettingsError::RemoteConfigUrl {
        message: format!(
            "unrecognized --config-url {raw:?}: expected github:OWNER/REPO/PATH[@REF] \
             or an https:// URL"
        ),
    })
}

/// Refuses to send a token over a non-https URL — a token must never
/// travel in the clear. Pure and network-free, so this check is
/// unit-testable on its own.
pub fn ensure_https_if_token(fetch_url: &str, token: Option<&str>) -> Result<(), SettingsError> {
    if token.is_some() && !fetch_url.starts_with("https://") {
        return Err(SettingsError::RemoteConfigUrl {
            message: "a token is configured but the URL is not https:// — refusing to send \
                      it in the clear"
                .to_string(),
        });
    }
    Ok(())
}

/// Connect+read timeout for [`fetch_remote_bytes`] — this must never hang
/// startup indefinitely (offline networks, a stalled proxy, ...).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Performs the one blocking, read-only GET. Never writes anything — GitHub
/// Contents API reads only ever use `GET`; there is no write path here.
///
/// Errors are always safe to display: the token is sent as a header value
/// ureq's error rendering does not echo back, and HTTP status errors are
/// reduced to just the status code.
pub fn fetch_remote_bytes(
    url: &ConfigUrl,
    token: Option<&str>,
    timeout: Duration,
) -> Result<Vec<u8>, SettingsError> {
    let fetch_url = url.fetch_url();
    ensure_https_if_token(&fetch_url, token)?;

    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build();
    let agent: ureq::Agent = config.into();

    let mut request = agent
        .get(&fetch_url)
        .header("User-Agent", "pg_lens")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if matches!(url, ConfigUrl::Github { .. }) {
        // Raw bytes back, not the base64+JSON envelope the default
        // `application/vnd.github+json` media type would return.
        request = request.header("Accept", "application/vnd.github.raw+json");
    }
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }

    let mut response = request
        .call()
        .map_err(|e| SettingsError::RemoteConfigFetch {
            message: fetch_error_message(&e),
        })?;
    response
        .body_mut()
        .read_to_vec()
        .map_err(|e| SettingsError::RemoteConfigFetch {
            message: format!("reading response body: {e}"),
        })
}

/// A safe-to-display summary of a `ureq` error — status codes render as
/// plain `HTTP <code>`, everything else falls back to `ureq`'s own
/// `Display` (transport/DNS/TLS diagnostics only; it never echoes request
/// headers, so the token cannot leak through it).
fn fetch_error_message(err: &ureq::Error) -> String {
    match err {
        ureq::Error::StatusCode(code) => format!("HTTP {code}"),
        other => other.to_string(),
    }
}

/// Combines a (possibly failed) fetch attempt with the last cached bytes and
/// the local `services.toml`, per `--config-url`'s fallback/precedence rule:
///
/// - a successful fetch is used (the caller is responsible for writing it to
///   the cache — this function is disk-free);
/// - a failed fetch falls back to the cache, with a warning;
/// - with neither a successful fetch nor a cache, the local file alone is
///   used (also a warning);
/// - with NONE of a successful fetch, a cache, or a local file, this is a
///   hard error — there is nothing to connect with.
///
/// Remote entries win over local ones with the same name
/// ([`ServicesFile::merge_remote_over`]).
///
/// Pure and side-effect-free (no disk, no network) on purpose, so the whole
/// precedence/fallback matrix is unit-testable by injecting fake fetch
/// results and byte blobs — see `pg_lens_tui::main::resolve_remote_overlay`
/// for the disk/network glue that supplies these arguments for real.
pub fn resolve_effective_services(
    fetch_result: Result<Vec<u8>, String>,
    cached_bytes: Option<Vec<u8>>,
    local: Option<ServicesFile>,
) -> Result<(ServicesFile, Vec<String>), SettingsError> {
    let mut warnings = Vec::new();
    let remote_bytes = match fetch_result {
        Ok(bytes) => Some(bytes),
        Err(fetch_err) => match cached_bytes {
            Some(cached) => {
                warnings.push(format!(
                    "--config-url fetch failed ({fetch_err}); using the last cached copy"
                ));
                Some(cached)
            }
            None if local.is_some() => {
                warnings.push(format!(
                    "--config-url fetch failed and no cache exists yet ({fetch_err}); \
                     using the local services file only"
                ));
                None
            }
            None => {
                return Err(SettingsError::RemoteConfigFetch {
                    message: format!(
                        "{fetch_err} (no cached copy and no local services file to fall back on)"
                    ),
                });
            }
        },
    };

    let remote = remote_bytes
        .map(|bytes| ServicesFile::from_remote_bytes(&bytes))
        .transpose()?;

    let merged = match (local, remote) {
        (Some(local), Some(remote)) => local.merge_remote_over(remote),
        (Some(local), None) => local,
        (None, Some(remote)) => remote,
        (None, None) => ServicesFile::empty(),
    };
    Ok((merged, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_config_url ---------------------------------------------------

    #[test]
    fn github_shorthand_with_ref() {
        let url = parse_config_url("github:my-org/pg-lens-config/services.toml@main")
            .expect("valid shorthand");
        assert_eq!(
            url,
            ConfigUrl::Github {
                owner: "my-org".to_string(),
                repo: "pg-lens-config".to_string(),
                path: "services.toml".to_string(),
                git_ref: Some("main".to_string()),
            }
        );
        assert_eq!(
            url.fetch_url(),
            "https://api.github.com/repos/my-org/pg-lens-config/contents/services.toml?ref=main"
        );
    }

    #[test]
    fn github_shorthand_without_ref() {
        let url =
            parse_config_url("github:acme/infra/pg_lens/services.toml").expect("valid shorthand");
        assert_eq!(
            url,
            ConfigUrl::Github {
                owner: "acme".to_string(),
                repo: "infra".to_string(),
                path: "pg_lens/services.toml".to_string(),
                git_ref: None,
            }
        );
        assert_eq!(
            url.fetch_url(),
            "https://api.github.com/repos/acme/infra/contents/pg_lens/services.toml"
        );
    }

    #[test]
    fn github_shorthand_missing_parts_is_an_error() {
        for bad in ["github:", "github:owner", "github:owner/repo", "github:/repo/path"] {
            let err = parse_config_url(bad).expect_err(&format!("{bad:?} must be rejected"));
            assert!(matches!(err, SettingsError::RemoteConfigUrl { .. }), "{bad}");
        }
    }

    #[test]
    fn plain_https_and_http_urls_pass_through() {
        let url = parse_config_url("https://example.com/services.toml").expect("https url");
        assert_eq!(url, ConfigUrl::Http("https://example.com/services.toml".to_string()));
        assert_eq!(url.fetch_url(), "https://example.com/services.toml");

        let url = parse_config_url("http://internal.example/services.toml").expect("http url");
        assert_eq!(
            url,
            ConfigUrl::Http("http://internal.example/services.toml".to_string())
        );
    }

    #[test]
    fn unrecognized_scheme_is_an_error() {
        let err = parse_config_url("s3://bucket/services.toml").expect_err("must be rejected");
        assert!(matches!(err, SettingsError::RemoteConfigUrl { .. }));
    }

    #[test]
    fn empty_url_is_an_error() {
        let err = parse_config_url("   ").expect_err("must be rejected");
        assert!(matches!(err, SettingsError::RemoteConfigUrl { .. }));
    }

    // --- https-required-with-token ------------------------------------------

    #[test]
    fn http_with_a_token_is_refused() {
        let err = ensure_https_if_token("http://internal.example/services.toml", Some("secret"))
            .expect_err("http + token must be refused");
        let msg = err.to_string();
        assert!(msg.contains("https"), "got: {msg}");
        assert!(!msg.contains("secret"), "token must never appear in the error: {msg}");
    }

    #[test]
    fn http_without_a_token_is_fine() {
        assert!(ensure_https_if_token("http://internal.example/services.toml", None).is_ok());
    }

    #[test]
    fn https_with_a_token_is_fine() {
        assert!(
            ensure_https_if_token("https://api.github.com/repos/a/b/contents/c", Some("tok"))
                .is_ok()
        );
    }

    // --- resolve_effective_services (cache/fallback precedence) -------------

    const LOCAL_TOML: &str = "[services.local_only]\nhost = \"local-host\"\n";
    const REMOTE_TOML: &str = "[services.remote_only]\nhost = \"remote-host\"\n";
    const CACHED_TOML: &str = "[services.cached_only]\nhost = \"cached-host\"\n";

    fn local() -> ServicesFile {
        ServicesFile::from_remote_bytes(LOCAL_TOML.as_bytes()).expect("local parses")
    }

    #[test]
    fn successful_fetch_wins_and_produces_no_warnings() {
        let (merged, warnings) = resolve_effective_services(
            Ok(REMOTE_TOML.as_bytes().to_vec()),
            Some(CACHED_TOML.as_bytes().to_vec()),
            Some(local()),
        )
        .expect("must succeed");
        assert!(warnings.is_empty(), "got: {warnings:?}");
        assert!(merged.get("remote_only").is_ok());
        assert!(merged.get("local_only").is_ok(), "local entries survive the merge");
        assert!(merged.get("cached_only").is_err(), "stale cache must not apply on a fresh fetch");
    }

    #[test]
    fn failed_fetch_falls_back_to_cache_with_a_warning() {
        let (merged, warnings) = resolve_effective_services(
            Err("connection refused".to_string()),
            Some(CACHED_TOML.as_bytes().to_vec()),
            Some(local()),
        )
        .expect("must succeed via cache");
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(warnings[0].contains("connection refused"), "got: {warnings:?}");
        assert!(merged.get("cached_only").is_ok());
        assert!(merged.get("local_only").is_ok());
    }

    #[test]
    fn failed_fetch_no_cache_falls_back_to_local_alone_with_a_warning() {
        let (merged, warnings) =
            resolve_effective_services(Err("timed out".to_string()), None, Some(local()))
                .expect("must succeed via local");
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(merged.get("local_only").is_ok());
    }

    #[test]
    fn failed_fetch_no_cache_no_local_is_a_hard_error() {
        let err = resolve_effective_services(Err("dns failure".to_string()), None, None)
            .expect_err("nothing to fall back on must fail");
        let msg = err.to_string();
        assert!(msg.contains("dns failure"), "got: {msg}");
        assert!(matches!(err, SettingsError::RemoteConfigFetch { .. }));
    }

    #[test]
    fn no_local_file_but_a_successful_fetch_is_fine() {
        let (merged, warnings) =
            resolve_effective_services(Ok(REMOTE_TOML.as_bytes().to_vec()), None, None)
                .expect("remote alone must be enough");
        assert!(warnings.is_empty());
        assert!(merged.get("remote_only").is_ok());
    }

    #[test]
    fn remote_wins_a_name_collision_with_local() {
        let local_toml = "[services.shared]\nhost = \"local-host\"\n";
        let remote_toml = "[services.shared]\nhost = \"remote-host\"\n";
        let (merged, _) = resolve_effective_services(
            Ok(remote_toml.as_bytes().to_vec()),
            None,
            Some(ServicesFile::from_remote_bytes(local_toml.as_bytes()).unwrap()),
        )
        .expect("must succeed");
        assert_eq!(merged.get("shared").unwrap().host.as_deref(), Some("remote-host"));
    }
}
