use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::bail;

use crate::cli::Cli;

#[derive(Clone)]
#[allow(dead_code)] // fields are consumed by later tasks (auth, http, state)
pub struct Config {
    pub port: u16,
    pub bind: IpAddr,
    pub data_dir: PathBuf,
    pub auth_token: Option<String>,
    pub github_oauth: Option<GithubOauth>,
    pub public: bool,
    pub log_cap: Option<u64>,
    pub log_max_age: Option<Duration>,
}

// Manual Debug so a `{:?}` on Config cannot leak the bearer token —
// the same redaction GithubOauth applies to the client secret below.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("port", &self.port)
            .field("bind", &self.bind)
            .field("data_dir", &self.data_dir)
            .field("auth_token", &self.auth_token.as_ref().map(|_| "<redacted>"))
            .field("github_oauth", &self.github_oauth)
            .field("public", &self.public)
            .field("log_cap", &self.log_cap)
            .field("log_max_age", &self.log_max_age)
            .finish()
    }
}

/// GitHub OAuth app credentials plus the allowlist of who may log in.
/// Present only when the operator configures oauth — without it the bearer
/// token stays the only gate.
#[derive(Clone)]
#[allow(dead_code)] // consumed by the auth layer (oauth flow, session minting)
pub struct GithubOauth {
    pub client_id: String,
    pub client_secret: String,
    /// Entries are bare logins or `org:name`, split from the
    /// comma-separated flag/env value.
    pub allowlist: Vec<String>,
}

// Manual Debug so a `{:?}` on Config cannot leak the client secret —
// mirrors the redaction in auth::oauth::GithubOauth.
impl fmt::Debug for GithubOauth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GithubOauth")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("allowlist", &self.allowlist)
            .finish()
    }
}

impl Config {
    pub fn load(cli: &Cli) -> anyhow::Result<Self> {
        Self::load_inner(cli, true)
    }

    /// `load` for the local admin subcommands (status/restart/url/ps/
    /// doctor): identical except the GitHub oauth config is not resolved.
    /// Those commands only need port/bind/data-dir — resolving the oauth
    /// settings would let a half-set DREAM_GITHUB_* in the operator's
    /// shell break a read-only status check, and none of them consume
    /// `github_oauth` anyway. Chosen over warn-and-continue because the
    /// commands never look at the field, so skipping resolution is
    /// strictly simpler than softening its errors.
    pub fn load_admin(cli: &Cli) -> anyhow::Result<Self> {
        Self::load_inner(cli, false)
    }

    fn load_inner(cli: &Cli, resolve_oauth: bool) -> anyhow::Result<Self> {
        let bind: IpAddr = cli
            .bind
            .as_deref()
            .unwrap_or("127.0.0.1")
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid --bind address: {}", cli.bind.as_deref().unwrap_or("")))?;
        let public = cli.public || !bind.is_loopback();
        if !bind.is_loopback() && !cli.public {
            bail!(
                "refusing to bind non-loopback address {bind} without --public; \
                 exposing a terminal on a public interface is dangerous"
            );
        }
        if public && !bind.is_loopback() {
            eprintln!(
                "WARNING: binding {bind} exposes dream on a non-loopback interface. \
                 Traffic is cleartext HTTP — the token and all terminal I/O \
                 are sniffable. Anyone reaching this address with the token \
                 gets a shell."
            );
        }
        let data_dir = cli
            .data_dir
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);
        std::fs::create_dir_all(&data_dir)?;
        let github_oauth = if resolve_oauth {
            resolve_github_oauth(cli)?
        } else {
            None
        };
        Ok(Self {
            port: cli.port,
            bind,
            data_dir,
            auth_token: cli.auth_token.clone(),
            github_oauth,
            public,
            // A tiny cap degenerates hysteresis (cap/4 headroom → 0) into a
            // per-append rewrite; floor it at 4 KiB.
            log_cap: cli.log_cap.map(|cap| cap.max(4096)),
            log_max_age: cli.log_max_age.map(Duration::from_secs),
        })
    }

    pub fn listen_addr(&self) -> String {
        if self.bind.is_ipv6() {
            format!("[{}]:{}", self.bind, self.port)
        } else {
            format!("{}:{}", self.bind, self.port)
        }
    }
}

pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dream")
}

/// Resolves the GitHub OAuth flags and their DREAM_GITHUB_* env fallbacks —
/// a flag beats the env var and an empty value counts as unset on both
/// sides, the same convention as DREAM_AUTH_TOKEN. Any oauth value turns
/// the feature on, so credentials without a usable allowlist (which would
/// lock everyone out or let everyone in) and half-given credentials are
/// config errors.
fn resolve_github_oauth(cli: &Cli) -> anyhow::Result<Option<GithubOauth>> {
    fn flag_or_env(flag: &Option<String>, env: &str) -> Option<String> {
        flag.clone()
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var(env).ok().filter(|v| !v.is_empty()))
    }
    let client_id = flag_or_env(&cli.github_client_id, "DREAM_GITHUB_CLIENT_ID");
    let client_secret = flag_or_env(&cli.github_client_secret, "DREAM_GITHUB_CLIENT_SECRET");
    let allowlist: Vec<String> = flag_or_env(&cli.github_allowlist, "DREAM_GITHUB_ALLOWLIST")
        .map(|raw| {
            raw.split(',')
                .map(|entry| entry.trim().to_string())
                .filter(|entry| !entry.is_empty())
                .collect()
        })
        .unwrap_or_default();

    if client_id.is_none() && client_secret.is_none() && allowlist.is_empty() {
        return Ok(None);
    }
    if allowlist.is_empty() {
        bail!(
            "github oauth is configured but the allowlist is empty — \
             set --github-allowlist or DREAM_GITHUB_ALLOWLIST"
        );
    }
    // `org:` is the only qualifier the matcher understands — and it
    // strips exactly the lowercase prefix, so `ORG:acme` must be stored
    // as `org:acme` or it never matches. The name is trimmed too
    // (`org: acme` would otherwise compare against a leading-space name
    // forever). An org entry with no name and a colon entry with any
    // other prefix both match nobody — reject them loudly, naming the
    // offending entry.
    let mut normalized = Vec::with_capacity(allowlist.len());
    for entry in &allowlist {
        match entry.split_once(':') {
            Some((prefix, name)) if prefix.eq_ignore_ascii_case("org") => {
                let name = name.trim();
                if name.is_empty() {
                    bail!("github oauth allowlist entry {entry:?} has an empty org name");
                }
                normalized.push(format!("org:{name}"));
            }
            Some(_) => {
                bail!("github oauth allowlist entry {entry:?} is not a login or org:name");
            }
            None => normalized.push(entry.clone()),
        }
    }
    let allowlist = normalized;
    match (client_id, client_secret) {
        (Some(client_id), Some(client_secret)) => Ok(Some(GithubOauth {
            client_id,
            client_secret,
            allowlist,
        })),
        _ => bail!(
            "github oauth requires both --github-client-id and \
             --github-client-secret (or DREAM_GITHUB_CLIENT_ID / \
             DREAM_GITHUB_CLIENT_SECRET)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard};

    use clap::Parser;
    use tempfile::TempDir;

    use super::Config;
    use crate::cli::Cli;

    /// Env vars are process-global and shared by every test thread in this
    /// binary — serialise any test that touches them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const GITHUB_ENV_VARS: [&str; 3] = [
        "DREAM_GITHUB_CLIENT_ID",
        "DREAM_GITHUB_CLIENT_SECRET",
        "DREAM_GITHUB_ALLOWLIST",
    ];

    /// Restores the GitHub oauth env vars to their previous values on drop.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    /// Clear every DREAM_GITHUB_* var and return the lock guard plus the
    /// restore-on-drop guard. Both must be held for the test's duration.
    fn clean_github_env() -> (MutexGuard<'static, ()>, EnvGuard) {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = GITHUB_ENV_VARS
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        for key in GITHUB_ENV_VARS {
            std::env::remove_var(key);
        }
        (lock, EnvGuard { saved })
    }

    /// Parse args (always with a temp --data-dir so load never touches the
    /// real data dir) and run the config load.
    fn load_with(args: &[&str], data_dir: &Path) -> anyhow::Result<Config> {
        let mut argv = vec![
            "dream".to_string(),
            "--data-dir".to_string(),
            data_dir.display().to_string(),
        ];
        argv.extend(args.iter().map(|s| s.to_string()));
        let cli = Cli::try_parse_from(&argv).expect("args must parse");
        Config::load(&cli)
    }

    #[test]
    fn github_oauth_without_an_allowlist_refuses_to_start() {
        let (_lock, _env) = clean_github_env();
        let data = TempDir::new().expect("tempdir");

        // OAuth credentials configured but no allowlist would either lock
        // everyone out or let everyone in — it must be a config error.
        let result = load_with(
            &[
                "--github-client-id",
                "client-123",
                "--github-client-secret",
                "secret-xyz",
            ],
            data.path(),
        );

        let err = result.expect_err("oauth without an allowlist must be a config error");
        assert!(
            err.to_string().contains("allowlist"),
            "the error must point the operator at the allowlist, got: {err}"
        );
    }

    #[test]
    fn an_empty_allowlist_value_refuses_to_start() {
        let (_lock, _env) = clean_github_env();
        let data = TempDir::new().expect("tempdir");

        // A present-but-empty allowlist resolves to zero entries — same lock
        // everyone out / let everyone in trap as a missing one.
        let result = load_with(
            &[
                "--github-client-id",
                "client-123",
                "--github-client-secret",
                "secret-xyz",
                "--github-allowlist",
                "",
            ],
            data.path(),
        );

        assert!(
            result.is_err(),
            "an allowlist resolving to zero entries must refuse to start"
        );
    }

    #[test]
    fn env_configured_oauth_without_allowlist_refuses_to_start() {
        let (_lock, _env) = clean_github_env();
        std::env::set_var("DREAM_GITHUB_CLIENT_ID", "client-123");
        std::env::set_var("DREAM_GITHUB_CLIENT_SECRET", "secret-xyz");
        let data = TempDir::new().expect("tempdir");

        let result = load_with(&[], data.path());

        assert!(
            result.is_err(),
            "env-configured oauth without DREAM_GITHUB_ALLOWLIST must refuse to start"
        );
    }

    #[test]
    fn daemon_stays_token_only_without_github_oauth() {
        let (_lock, _env) = clean_github_env();
        let data = TempDir::new().expect("tempdir");

        let cfg = load_with(&[], data.path()).expect("plain config must still load");

        // No oauth flags or env vars → unchanged behaviour: bearer token is
        // the only gate.
        assert!(
            cfg.github_oauth.is_none(),
            "oauth must stay unconfigured when no flags or env vars are set"
        );
    }

    #[test]
    fn github_oauth_flags_populate_the_config() {
        let (_lock, _env) = clean_github_env();
        let data = TempDir::new().expect("tempdir");

        let cfg = load_with(
            &[
                "--github-client-id",
                "client-123",
                "--github-client-secret",
                "secret-xyz",
                "--github-allowlist",
                "alice,org:acme",
            ],
            data.path(),
        )
        .expect("a fully configured oauth must load");

        let oauth = cfg
            .github_oauth
            .expect("oauth flags must produce a configured GithubOauth");
        assert_eq!(oauth.client_id, "client-123");
        assert_eq!(oauth.client_secret, "secret-xyz");
        // Comma-separated allowlist: bare logins and org: entries.
        assert_eq!(oauth.allowlist, ["alice", "org:acme"]);
    }

    #[test]
    fn github_oauth_env_vars_fill_in_when_flags_are_absent() {
        let (_lock, _env) = clean_github_env();
        std::env::set_var("DREAM_GITHUB_CLIENT_ID", "env-id");
        std::env::set_var("DREAM_GITHUB_CLIENT_SECRET", "env-secret");
        std::env::set_var("DREAM_GITHUB_ALLOWLIST", "bob,org:envorg");
        let data = TempDir::new().expect("tempdir");

        let cfg = load_with(&[], data.path())
            .expect("oauth configured purely via env must load");

        let oauth = cfg
            .github_oauth
            .expect("DREAM_GITHUB_* env vars must configure oauth");
        assert_eq!(oauth.client_id, "env-id");
        assert_eq!(oauth.client_secret, "env-secret");
        assert_eq!(oauth.allowlist, ["bob", "org:envorg"]);
    }

    #[test]
    fn github_oauth_flags_beat_env_vars() {
        let (_lock, _env) = clean_github_env();
        std::env::set_var("DREAM_GITHUB_CLIENT_ID", "env-id");
        std::env::set_var("DREAM_GITHUB_CLIENT_SECRET", "env-secret");
        std::env::set_var("DREAM_GITHUB_ALLOWLIST", "env-login");
        let data = TempDir::new().expect("tempdir");

        // Same precedence as DREAM_AUTH_TOKEN: an explicit flag wins, the env
        // var only fills in when the flag is absent.
        let cfg = load_with(
            &[
                "--github-client-id",
                "flag-id",
                "--github-client-secret",
                "flag-secret",
                "--github-allowlist",
                "flag-login",
            ],
            data.path(),
        )
        .expect("flag-configured oauth must load");

        let oauth = cfg.github_oauth.expect("oauth must be configured");
        assert_eq!(oauth.client_id, "flag-id");
        assert_eq!(oauth.client_secret, "flag-secret");
        assert_eq!(oauth.allowlist, ["flag-login"]);
    }

    #[test]
    fn empty_github_env_vars_count_as_unset() {
        let (_lock, _env) = clean_github_env();
        // Mirrors the DREAM_AUTH_TOKEN convention: an env var set to "" is
        // treated as absent, not as an empty value.
        std::env::set_var("DREAM_GITHUB_CLIENT_ID", "");
        std::env::set_var("DREAM_GITHUB_CLIENT_SECRET", "");
        std::env::set_var("DREAM_GITHUB_ALLOWLIST", "");
        let data = TempDir::new().expect("tempdir");

        let cfg = load_with(&[], data.path()).expect("plain config must still load");

        assert!(
            cfg.github_oauth.is_none(),
            "empty-string env vars must not count as configured oauth"
        );
    }

    #[test]
    fn github_oauth_client_id_without_secret_refuses_to_start() {
        let (_lock, _env) = clean_github_env();
        let data = TempDir::new().expect("tempdir");

        // Half a credential pair must fail loudly — otherwise the daemon
        // would start with oauth half-wired.
        let result = load_with(
            &[
                "--github-client-id",
                "client-123",
                "--github-allowlist",
                "alice",
            ],
            data.path(),
        );

        let err = result.expect_err("client id without a secret must be a config error");
        assert!(
            err.to_string().contains("client-secret"),
            "the error must name the missing secret, got: {err}"
        );
    }

    #[test]
    fn github_oauth_org_entry_with_empty_name_refuses_to_start() {
        let (_lock, _env) = clean_github_env();
        let data = TempDir::new().expect("tempdir");

        // `org:` with no name silently matches nobody — fail loudly and
        // name the offending entry. The check is case-insensitive.
        for (allowlist, entry) in [("org:", "org:"), ("alice,ORG:", "ORG:")] {
            let result = load_with(
                &[
                    "--github-client-id",
                    "client-123",
                    "--github-client-secret",
                    "secret-xyz",
                    "--github-allowlist",
                    allowlist,
                ],
                data.path(),
            );

            let err = result.expect_err("an empty org name must be a config error");
            assert!(
                err.to_string().contains(entry),
                "the error must name the offending entry {entry:?}, got: {err}"
            );
        }
    }

    #[test]
    fn org_prefixed_entries_are_stored_lowercase_and_trimmed() {
        let (_lock, _env) = clean_github_env();
        let data = TempDir::new().expect("tempdir");

        // `is_allowlisted` strips exactly `org:` — an uppercase prefix
        // accepted at validation would match nobody, and a space between
        // prefix and name would compare against a leading-space name.
        let cfg = load_with(
            &[
                "--github-client-id",
                "client-123",
                "--github-client-secret",
                "secret-xyz",
                "--github-allowlist",
                "alice,ORG:Acme,org: spaced",
            ],
            data.path(),
        )
        .expect("mixed-case and spaced org entries must load");

        let oauth = cfg
            .github_oauth
            .expect("oauth flags must produce a configured GithubOauth");
        assert_eq!(oauth.allowlist, ["alice", "org:Acme", "org:spaced"]);
    }

    #[test]
    fn colon_entries_with_a_non_org_prefix_refuse_to_start() {
        let (_lock, _env) = clean_github_env();
        let data = TempDir::new().expect("tempdir");

        // `foo:bar` is neither a bare login nor an org entry — stored
        // verbatim it would silently match nobody, so it must fail loudly.
        let result = load_with(
            &[
                "--github-client-id",
                "client-123",
                "--github-client-secret",
                "secret-xyz",
                "--github-allowlist",
                "alice,foo:bar",
            ],
            data.path(),
        );

        let err = result.expect_err("a non-org colon entry must be a config error");
        assert!(
            err.to_string().contains("foo:bar"),
            "the error must name the offending entry, got: {err}"
        );
    }

    #[test]
    fn config_debug_redacts_the_auth_token() {
        let (_lock, _env) = clean_github_env();
        let data = TempDir::new().expect("tempdir");

        let cfg = load_with(&["--auth-token", "tok-secret-123"], data.path())
            .expect("config must load");

        let debug = format!("{cfg:?}");
        assert!(
            debug.contains("<redacted>"),
            "auth_token must be redacted in Debug output: {debug}"
        );
        assert!(
            !debug.contains("tok-secret-123"),
            "the token must not appear in Debug output: {debug}"
        );
    }

    #[test]
    fn admin_load_ignores_partial_github_env_vars() {
        let (_lock, _env) = clean_github_env();
        // A half-set DREAM_GITHUB_* in the operator's shell must not break
        // the read-only admin subcommands — they never consume the oauth
        // config, so it is not resolved for them.
        std::env::set_var("DREAM_GITHUB_CLIENT_ID", "env-id");
        let data = TempDir::new().expect("tempdir");

        let cli = Cli::try_parse_from([
            "dream".to_string(),
            "--data-dir".to_string(),
            data.path().display().to_string(),
        ])
        .expect("args must parse");
        let cfg = Config::load_admin(&cli).expect("admin load must ignore partial oauth env");

        assert!(
            cfg.github_oauth.is_none(),
            "admin load must leave the oauth config unresolved"
        );
        // The daemon path still validates the same environment.
        assert!(
            Config::load(&cli).is_err(),
            "a client id without a secret must stay a config error for the daemon"
        );
    }
}
