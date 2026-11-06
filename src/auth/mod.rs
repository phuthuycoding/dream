//! First-run auth token (TC-010 / UC-006 / FR-007, FR-008).
//!
//! `AuthToken::load_or_generate` returns the bearer token persisted in the
//! data dir, generating a fresh random one (written owner-only, mode 0600)
//! on first run. `first_run` is `true` only on the generating call, so the
//! caller (main.rs) can print the authed URL exactly once.

pub mod oauth;
pub mod session;

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::Context;

/// Bearer token persisted in the data dir.
pub struct AuthToken {
    /// The secret itself; compared verbatim against `Bearer` / `?token=`.
    pub token: String,
    /// `true` only on the call that generated the token — the caller prints
    /// the authed URL exactly once.
    pub first_run: bool,
}

const TOKEN_FILE: &str = "token";

impl AuthToken {
    /// Load the persisted token, or generate one on first run.
    ///
    /// An existing non-empty token file is reused verbatim (operator rotation
    /// is honoured); a missing or empty file is replaced by a fresh random
    /// token written owner-only (mode 0600).
    pub fn load_or_generate(data_dir: &Path) -> anyhow::Result<Self> {
        let path = data_dir.join(TOKEN_FILE);
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let token = contents.trim();
                if !token.is_empty() {
                    ensure_owner_only(&path)?;
                    return Ok(AuthToken {
                        token: token.to_string(),
                        first_run: false,
                    });
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("cannot read token file {}", path.display()));
            }
        }
        fs::create_dir_all(data_dir)
            .with_context(|| format!("cannot create data dir {}", data_dir.display()))?;
        let token = generate_token();
        write_token_file(&path, &token)?;
        Ok(AuthToken {
            token,
            first_run: true,
        })
    }
}

/// 32 random bytes, hex-encoded (64 chars).
fn generate_token() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Write the token with owner-only permissions. `mode()` only applies when
/// the file is created, so permissions are forced afterwards to cover a
/// pre-existing file whose mode drifted.
fn write_token_file(path: &Path, token: &str) -> anyhow::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("cannot write token file {}", path.display()))?;
    file.write_all(token.as_bytes())
        .with_context(|| format!("cannot write token file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("cannot fsync token file {}", path.display()))?;
    ensure_owner_only(path)
}

/// Force owner-only permissions on an existing token file — the mode can
/// drift when an operator rotates the token by hand. No-op off unix.
#[cfg(unix)]
fn ensure_owner_only(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot set 0600 on token file {}", path.display()))
}

#[cfg(not(unix))]
fn ensure_owner_only(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::oauth::{generate_state, is_allowlisted, GithubOauth};
    use super::session::SessionSigner;
    use super::AuthToken;

    /// Locate the file inside `data_dir` whose contents are the token.
    /// The TC fixes the behaviour (token persisted inside the data dir,
    /// mode 0600) but not the file name, so the test finds it by content.
    fn token_file(data_dir: &Path, token: &str) -> PathBuf {
        let mut found = Vec::new();
        for entry in fs::read_dir(data_dir).expect("data dir must be readable") {
            let path = entry.expect("dir entry").path();
            if !path.is_file() {
                continue;
            }
            if let Ok(contents) = fs::read_to_string(&path) {
                if contents.trim() == token {
                    found.push(path);
                }
            }
        }
        assert_eq!(
            found.len(),
            1,
            "expected exactly one file in the data dir holding the token, found {found:?}"
        );
        found.pop().unwrap()
    }

    #[cfg(unix)]
    fn assert_owner_only(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .expect("stat token file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "token file must be mode 0600 (owner read/write only), got {mode:o}"
        );
    }

    #[cfg(not(unix))]
    fn assert_owner_only(_path: &Path) {}

    #[test]
    fn first_run_generates_token_stored_owner_only_and_reports_first_run() {
        let data = TempDir::new().expect("tempdir");

        let auth = AuthToken::load_or_generate(data.path())
            .expect("load_or_generate on a fresh data dir must succeed");

        assert!(
            !auth.token.is_empty(),
            "a generated token must be a non-empty secret"
        );
        assert!(
            auth.first_run,
            "first run must be reported so the authed URL is printed once"
        );

        let file = token_file(data.path(), &auth.token);
        assert_owner_only(&file);
    }

    #[test]
    fn restart_loads_the_same_token_and_reports_not_first_run() {
        let data = TempDir::new().expect("tempdir");

        let first = AuthToken::load_or_generate(data.path()).expect("first run");
        let second =
            AuthToken::load_or_generate(data.path()).expect("restarting must reload the token");

        assert_eq!(
            first.token, second.token,
            "the persisted token must be reused verbatim, not regenerated"
        );
        assert!(
            !second.first_run,
            "an existing token file means this is not first run — the URL is never reprinted"
        );
    }

    #[test]
    fn the_persisted_token_file_is_what_gets_reloaded() {
        let data = TempDir::new().expect("tempdir");

        let generated = AuthToken::load_or_generate(data.path()).expect("first run");
        let file = token_file(data.path(), &generated.token);
        let on_disk = fs::read_to_string(&file).expect("token file readable");
        assert_eq!(
            on_disk.trim(),
            generated.token,
            "the file must hold the token verbatim so a restart reuses it"
        );

        // Overwrite the file as an operator would when rotating the token:
        // the next load must return the file contents, not regenerate.
        fs::write(&file, "rotated-token-value").expect("rotate token");
        let reloaded = AuthToken::load_or_generate(data.path()).expect("reload");
        assert_eq!(reloaded.token, "rotated-token-value");
        assert!(!reloaded.first_run);
    }

    // ----------------------------------------------------------------
    // Session-cookie signing + GitHub OAuth client unit tests.
    //
    // Assumed surface (the dev pass reconciles; same pattern as
    // crate::http::tests):
    //   - `session::SessionSigner::load_or_generate(data_dir)` — HMAC
    //     key persisted at `<data-dir>/session-key` (0600), mirroring
    //     `AuthToken::load_or_generate`
    //   - `SessionSigner::mint(login, expires_at_unix) -> String` —
    //     cookie value `<login>|<unix expiry>|<hex hmac-sha256>`
    //   - `SessionSigner::verify(cookie_value, now_unix)`
    //     -> `Option<String>` login
    //   - `oauth::GithubOauth { client_id, client_secret, allowlist,
    //     authorize_url, token_url, api_url }` — endpoint fields
    //     injectable so tests can point at any host
    //   - `GithubOauth::build_authorize_url(redirect_uri, state)`
    //     -> String
    //   - `oauth::generate_state() -> String` — 16 random bytes, hex
    //   - `oauth::is_allowlisted(&[String], login, &[String]) -> bool`
    // ----------------------------------------------------------------

    /// A `GithubOauth` with every endpoint pointed at a fake host, so the
    /// tests prove the fields — not hardcoded github.com — drive the URLs.
    fn test_oauth() -> GithubOauth {
        GithubOauth {
            client_id: "client-id-123".to_string(),
            client_secret: "client-secret-xyz".to_string(),
            allowlist: vec!["alice".to_string()],
            authorize_url: "https://github.example.com/login/oauth/authorize".to_string(),
            token_url: "https://github.example.com/login/oauth/access_token".to_string(),
            api_url: "https://api.github.example.com".to_string(),
        }
    }

    /// Decode an `application/x-www-form-urlencoded` query string into
    /// `(key, value)` pairs.
    fn query_params(url: &str) -> Vec<(String, String)> {
        let (_, query) = url
            .split_once('?')
            .unwrap_or_else(|| panic!("url {url:?} must carry a query string"));
        query
            .split('&')
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                (percent_decode(key), percent_decode(value))
            })
            .collect()
    }

    fn percent_decode(input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Ok(byte) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
            out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
            i += 1;
        }
        String::from_utf8(out).expect("a percent-decoded param must be utf-8")
    }

    #[test]
    fn a_minted_session_cookie_round_trips_back_to_the_login() {
        let data = TempDir::new().expect("tempdir");
        let signer = SessionSigner::load_or_generate(data.path()).expect("signer");

        let cookie = signer.mint("alice", 1_000_000);

        // Sessions are signed, not stored: the value must be
        // self-contained as `<login>|<unix expiry>|<hex hmac-sha256>`.
        let parts: Vec<&str> = cookie.split('|').collect();
        assert_eq!(
            parts.len(),
            3,
            "cookie value must be login|expiry|signature, got {cookie:?}"
        );
        assert_eq!(parts[0], "alice");
        assert_eq!(parts[1], "1000000");
        assert_eq!(
            parts[2].len(),
            64,
            "hmac-sha256 hex-encodes to 64 chars, got {:?}",
            parts[2]
        );
        assert!(
            parts[2].chars().all(|c| c.is_ascii_hexdigit()),
            "signature must be hex, got {:?}",
            parts[2]
        );

        assert_eq!(
            signer.verify(&cookie, 999_999),
            Some("alice".to_string()),
            "a freshly minted cookie must verify before its expiry"
        );
    }

    #[test]
    fn an_expired_session_cookie_is_rejected() {
        let data = TempDir::new().expect("tempdir");
        let signer = SessionSigner::load_or_generate(data.path()).expect("signer");

        let cookie = signer.mint("alice", 1_000);

        assert_eq!(
            signer.verify(&cookie, 999),
            Some("alice".to_string()),
            "a cookie inside its validity window must verify"
        );
        assert_eq!(
            signer.verify(&cookie, 1_001),
            None,
            "a cookie past its unix expiry must not verify"
        );
    }

    #[test]
    fn a_session_cookie_with_a_forged_signature_is_rejected() {
        let data = TempDir::new().expect("tempdir");
        let signer = SessionSigner::load_or_generate(data.path()).expect("signer");
        let cookie = signer.mint("alice", 1_000_000);

        // Flip the last signature character; the rest stays well-formed.
        let mut forged = cookie.clone();
        let last = forged.pop().expect("cookie non-empty");
        forged.push(if last == '0' { '1' } else { '0' });
        assert_ne!(forged, cookie);

        assert_eq!(
            signer.verify(&forged, 500_000),
            None,
            "a modified signature must not verify"
        );
    }

    #[test]
    fn a_session_cookie_with_an_altered_login_is_rejected() {
        let data = TempDir::new().expect("tempdir");
        let signer = SessionSigner::load_or_generate(data.path()).expect("signer");
        let cookie = signer.mint("alice", 1_000_000);

        // Keep the original signature but swap the signed login — the
        // signature no longer covers the payload.
        let forged = cookie.replacen("alice|", "mallory|", 1);
        assert_ne!(forged, cookie);

        assert_eq!(
            signer.verify(&forged, 500_000),
            None,
            "the signature must cover the login, not just the expiry"
        );
    }

    #[test]
    fn a_malformed_session_cookie_is_rejected() {
        let data = TempDir::new().expect("tempdir");
        let signer = SessionSigner::load_or_generate(data.path()).expect("signer");

        // Verify must reject — never panic on — input that does not match
        // the login|expiry|signature shape.
        for bad in [
            "",
            "alice",
            "alice|1000",
            "alice|not-a-number|deadbeef",
            "alice|1000|zz",
            "a|b|c|d",
        ] {
            assert_eq!(
                signer.verify(bad, 500),
                None,
                "malformed cookie {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn a_session_cookie_with_a_multibyte_signature_is_rejected() {
        let data = TempDir::new().expect("tempdir");
        let signer = SessionSigner::load_or_generate(data.path()).expect("signer");

        // The signature is attacker input: multi-byte UTF-8 at an even
        // byte length must be rejected, not panic the hex decoder.
        for bad in ["alice|1000|éé", "alice|1000|xéy"] {
            assert_eq!(
                signer.verify(bad, 500),
                None,
                "multibyte signature {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn a_cookie_signed_under_a_different_key_is_rejected() {
        let dir_a = TempDir::new().expect("tempdir");
        let dir_b = TempDir::new().expect("tempdir");
        let signer_a = SessionSigner::load_or_generate(dir_a.path()).expect("signer a");
        let signer_b = SessionSigner::load_or_generate(dir_b.path()).expect("signer b");

        let cookie = signer_a.mint("alice", 1_000_000);

        assert_eq!(
            signer_b.verify(&cookie, 500_000),
            None,
            "each data dir gets its own key — foreign cookies must fail"
        );
    }

    #[test]
    fn the_signing_key_is_persisted_owner_only() {
        let data = TempDir::new().expect("tempdir");
        let _signer = SessionSigner::load_or_generate(data.path()).expect("signer");

        // The signing key is as sensitive as the bearer token, so it gets
        // the same on-disk treatment: a named file, mode 0600.
        let key_path = data.path().join("session-key");
        let metadata = fs::metadata(&key_path).unwrap_or_else(|err| {
            panic!(
                "signing key must be persisted at {}: {err}",
                key_path.display()
            )
        });
        assert!(metadata.is_file());
        assert!(
            !fs::read_to_string(&key_path)
                .expect("session-key readable")
                .trim()
                .is_empty(),
            "the persisted key must be non-empty"
        );
        assert_owner_only(&key_path);
    }

    #[test]
    fn a_reloaded_signer_verifies_cookies_minted_before_restart() {
        let data = TempDir::new().expect("tempdir");
        let cookie = {
            let signer = SessionSigner::load_or_generate(data.path()).expect("first run");
            signer.mint("alice", 1_000_000)
        };

        let reloaded = SessionSigner::load_or_generate(data.path())
            .expect("a restart must reload the persisted key");

        assert_eq!(
            reloaded.verify(&cookie, 500_000),
            Some("alice".to_string()),
            "sessions must survive a daemon restart — the key reloads, it is not regenerated"
        );
    }

    #[test]
    fn the_authorize_url_carries_client_id_redirect_uri_state_and_scope() {
        let oauth = test_oauth();
        let url =
            oauth.build_authorize_url("http://localhost:9000/auth/callback", "state-token-abc");

        assert!(
            url.starts_with("https://github.example.com/login/oauth/authorize"),
            "the redirect must target the configured authorize endpoint, got {url:?}"
        );
        let params = query_params(&url);
        let get = |key: &str| {
            params
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(
            get("client_id"),
            Some("client-id-123"),
            "github identifies the app by client_id"
        );
        assert_eq!(
            get("redirect_uri"),
            Some("http://localhost:9000/auth/callback"),
            "github sends the code back to the daemon's own callback"
        );
        assert_eq!(
            get("state"),
            Some("state-token-abc"),
            "the CSRF token must round-trip to github verbatim"
        );
        let scope = get("scope").unwrap_or_else(|| {
            panic!("authorize url {url:?} must carry a scope — org entries need read:org")
        });
        assert!(
            scope.split([' ', ',']).any(|s| s == "read:org"),
            "scope must include read:org so org memberships can be listed, got {scope:?}"
        );
    }

    #[test]
    fn a_bare_login_entry_matches_case_insensitively() {
        let allowlist = vec!["Alice".to_string(), "org:dream-team".to_string()];

        assert!(
            is_allowlisted(&allowlist, "alice", &[]),
            "github logins compare case-insensitively"
        );
        assert!(is_allowlisted(&allowlist, "ALICE", &[]));
        assert!(
            !is_allowlisted(&allowlist, "alicia", &[]),
            "a shared prefix must not admit a different login"
        );
    }

    #[test]
    fn an_org_entry_matches_a_member_of_that_org() {
        let allowlist = vec!["org:dream-team".to_string()];

        assert!(
            is_allowlisted(&allowlist, "bob", &["dream-team".to_string()]),
            "org:name admits any member of that org"
        );
        assert!(
            !is_allowlisted(&allowlist, "bob", &["other-org".to_string()]),
            "membership in a different org must not admit"
        );
        assert!(
            !is_allowlisted(&allowlist, "bob", &[]),
            "a user with no org memberships cannot match an org entry"
        );
    }

    #[test]
    fn a_login_matching_no_allowlist_entry_is_denied() {
        let allowlist = vec!["alice".to_string(), "org:dream-team".to_string()];

        assert!(
            !is_allowlisted(&allowlist, "mallory", &["other-org".to_string()]),
            "neither login nor org match — deny"
        );
        assert!(!is_allowlisted(&allowlist, "mallory", &[]));
    }

    #[test]
    fn the_state_token_is_sixteen_random_bytes_hex_encoded() {
        let state = generate_state();

        assert_eq!(
            state.len(),
            32,
            "16 bytes hex-encode to 32 chars, got {state:?}"
        );
        assert!(
            state.chars().all(|c| c.is_ascii_hexdigit()),
            "state must be hex, got {state:?}"
        );
        assert_ne!(
            state,
            generate_state(),
            "state is a CSRF token — it must be random per request"
        );
    }
}
