//! `dream_session` cookie signing (TC-002 / UC-002).
//!
//! Sessions are signed, not stored: `SessionSigner::mint` emits a
//! self-contained `login|expiry|signature` value and `verify` checks the
//! HMAC-SHA256 signature plus the expiry. The 32-byte key is persisted at
//! `<data-dir>/session-key` (mode 0600) so sessions survive daemon
//! restarts — the same load-or-generate discipline as `AuthToken`.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::Context;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Cookie name the auth middleware accepts next to the bearer token.
pub const SESSION_COOKIE: &str = "dream_session";

const KEY_FILE: &str = "session-key";

/// HMAC-SHA256 signer for `dream_session` cookie values.
pub struct SessionSigner {
    key: [u8; 32],
}

impl SessionSigner {
    /// Load the persisted signing key, or generate one on first run —
    /// mirrors `AuthToken::load_or_generate`: an existing non-empty
    /// `session-key` file is reused (operator rotation honoured), a
    /// missing or empty file is replaced by a fresh random key written
    /// owner-only (mode 0600).
    pub fn load_or_generate(data_dir: &Path) -> anyhow::Result<Self> {
        let path = data_dir.join(KEY_FILE);
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let key_hex = contents.trim();
                if !key_hex.is_empty() {
                    let key = decode_key(key_hex).with_context(|| {
                        format!("invalid session key in {}", path.display())
                    })?;
                    ensure_owner_only(&path)?;
                    return Ok(SessionSigner { key });
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("cannot read key file {}", path.display()));
            }
        }
        fs::create_dir_all(data_dir)
            .with_context(|| format!("cannot create data dir {}", data_dir.display()))?;
        let key: [u8; 32] = rand::random();
        write_key_file(&path, &key)?;
        Ok(SessionSigner { key })
    }

    /// Cookie value `<login>|<unix expiry>|<hex hmac-sha256(login|expiry)>`.
    pub fn mint(&self, login: &str, expires_at_unix: u64) -> String {
        let payload = format!("{login}|{expires_at_unix}");
        format!("{payload}|{}", self.sign(&payload))
    }

    /// `Some(login)` when the value is well-formed, signed by this key and
    /// `now_unix` is still before its expiry — `None` otherwise. `now` is
    /// a parameter so callers inject the clock at the boundary.
    pub fn verify(&self, cookie_value: &str, now_unix: u64) -> Option<String> {
        let parts: Vec<&str> = cookie_value.split('|').collect();
        if parts.len() != 3 {
            return None;
        }
        let (login, expiry_raw, signature) = (parts[0], parts[1], parts[2]);
        let expiry: u64 = expiry_raw.parse().ok()?;
        if now_unix >= expiry {
            return None;
        }
        let mut mac = self.mac();
        mac.update(format!("{login}|{expiry_raw}").as_bytes());
        mac.verify_slice(&hex_decode(signature)?).ok()?;
        Some(login.to_string())
    }

    fn mac(&self) -> Hmac<Sha256> {
        // HMAC accepts keys of any length; a 32-byte slice cannot fail.
        Hmac::new_from_slice(&self.key).expect("hmac key of 32 bytes")
    }

    fn sign(&self, payload: &str) -> String {
        let mut mac = self.mac();
        mac.update(payload.as_bytes());
        hex_encode(&mac.finalize().into_bytes())
    }
}

/// Decode a 64-char hex key file body into the 32 key bytes.
fn decode_key(key_hex: &str) -> anyhow::Result<[u8; 32]> {
    let bytes =
        hex_decode(key_hex).ok_or_else(|| anyhow::anyhow!("session key is not hex"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("session key must be 32 bytes"))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Write the hex-encoded key with owner-only permissions — same on-disk
/// treatment as the bearer token file.
fn write_key_file(path: &Path, key: &[u8; 32]) -> anyhow::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("cannot write key file {}", path.display()))?;
    file.write_all(hex_encode(key).as_bytes())
        .with_context(|| format!("cannot write key file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("cannot fsync key file {}", path.display()))?;
    ensure_owner_only(path)
}

/// Force owner-only permissions on an existing key file — the mode can
/// drift when an operator rotates the key by hand. No-op off unix.
#[cfg(unix)]
fn ensure_owner_only(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot set 0600 on key file {}", path.display()))
}

#[cfg(not(unix))]
fn ensure_owner_only(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}
