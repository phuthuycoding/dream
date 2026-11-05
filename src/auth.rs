//! First-run auth token (TC-010 / UC-006 / FR-007, FR-008).
//!
//! Test-writer seat for TASK-006: this file currently carries ONLY the
//! `#[cfg(test)]` suite below — no production code. The `AuthToken`
//! implementation lands in this module; the tests already pin the contract
//! fixed by `task-006-brief.md`:
//!
//! ```text
//! AuthToken::load_or_generate(data_dir) -> token + first-run flag
//! ```
//!
//! Assumed shape (the dev pass reconciles — see task-006-tests-report.md):
//!
//! ```text
//! pub struct AuthToken { pub token: String, pub first_run: bool }
//! pub fn load_or_generate(data_dir: &Path) -> anyhow::Result<AuthToken>
//! ```
//!
//! "Printed exactly once" is asserted through the `first_run` flag: printing
//! the URL is the caller's job (main.rs), so the observable unit-test contract
//! is `first_run == true` on generation and `false` on every later load.

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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("cannot set 0600 on token file {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

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
}
