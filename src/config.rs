use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::bail;

use crate::cli::Cli;

#[derive(Debug, Clone)]
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

/// GitHub OAuth app credentials plus the allowlist of who may log in.
/// Present only when the operator configures oauth — without it the bearer
/// token stays the only gate.
#[derive(Debug, Clone)]
#[allow(dead_code)] // consumed by the auth layer (oauth flow, session minting)
pub struct GithubOauth {
    pub client_id: String,
    pub client_secret: String,
    /// Entries are bare logins or `org:name`, split from the
    /// comma-separated flag/env value.
    pub allowlist: Vec<String>,
}

impl Config {
    pub fn load(cli: &Cli) -> anyhow::Result<Self> {
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
        let github_oauth = resolve_github_oauth(cli)?;
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
/// a flag beats the env var and an empty env value counts as unset, the same
/// convention as DREAM_AUTH_TOKEN. Any oauth value turns the feature on, so
/// credentials without a usable allowlist (which would lock everyone out or
/// let everyone in) and half-given credentials are config errors.
fn resolve_github_oauth(cli: &Cli) -> anyhow::Result<Option<GithubOauth>> {
    fn flag_or_env(flag: &Option<String>, env: &str) -> Option<String> {
        flag.clone()
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
}
