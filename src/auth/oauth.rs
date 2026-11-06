//! GitHub OAuth web-flow client (TC-003, TC-005, TC-006 / UC-002).
//!
//! `build_authorize_url` + `generate_state` start the login (the state is
//! a CSRF token that must round-trip through GitHub verbatim), the
//! callback exchanges the `code` for an access token, `/user` and
//! `/user/orgs` answer who logged in, and `is_allowlisted` decides
//! admission from the configured allowlist.
//!
//! `GithubOauth` here is the client struct: credentials + allowlist +
//! the three endpoints, injectable so tests point them at a loopback
//! stub. `crate::config::GithubOauth` is the operator-facing config
//! (credentials + allowlist only); `From` fills in the github.com
//! defaults.

use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

/// CSRF cookie set by `/auth/github` and matched on the callback.
pub const STATE_COOKIE: &str = "dream_oauth_state";

/// GitHub OAuth app credentials, the allowlist of who may log in, and
/// the three GitHub endpoints the flow talks to.
#[derive(Clone)]
pub struct GithubOauth {
    pub client_id: String,
    pub client_secret: String,
    /// Bare logins or `org:name` entries.
    pub allowlist: Vec<String>,
    pub authorize_url: String,
    pub token_url: String,
    /// API base — `/user` and `/user/orgs` are appended to it.
    pub api_url: String,
}

// Manual Debug so a stray `{:?}` in logs cannot leak the client secret.
impl fmt::Debug for GithubOauth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GithubOauth")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("allowlist", &self.allowlist)
            .field("authorize_url", &self.authorize_url)
            .field("token_url", &self.token_url)
            .field("api_url", &self.api_url)
            .finish()
    }
}

impl From<crate::config::GithubOauth> for GithubOauth {
    /// Operator config → client with the real github.com endpoints.
    fn from(config: crate::config::GithubOauth) -> Self {
        GithubOauth {
            client_id: config.client_id,
            client_secret: config.client_secret,
            allowlist: config.allowlist,
            authorize_url: "https://github.com/login/oauth/authorize".to_string(),
            token_url: "https://github.com/login/oauth/access_token".to_string(),
            api_url: "https://api.github.com".to_string(),
        }
    }
}

impl GithubOauth {
    /// The URL `/auth/github` 302s to. `read:org` scope is requested only
    /// when the allowlist holds an `org:` entry — it exists solely to
    /// serve the `/user/orgs` call, so login-only setups ask for nothing.
    pub fn build_authorize_url(&self, redirect_uri: &str, state: &str) -> String {
        let scope = if self.needs_orgs() {
            "&scope=read:org"
        } else {
            ""
        };
        format!(
            "{}?client_id={}&redirect_uri={}&state={}{scope}",
            self.authorize_url,
            urlencoded(&self.client_id),
            urlencoded(redirect_uri),
            urlencoded(state),
        )
    }

    /// True when the allowlist carries an `org:` entry — the only
    /// consumer of `/user/orgs`, so it decides both the `read:org`
    /// scope request and whether the callback fetches orgs at all.
    pub fn needs_orgs(&self) -> bool {
        self.allowlist
            .iter()
            .any(|entry| entry.starts_with("org:"))
    }

    /// Exchange the callback `code` for an access token.
    pub async fn exchange_code(&self, code: &str, redirect_uri: &str) -> anyhow::Result<String> {
        let response = Self::client()?
            .post(&self.token_url)
            .header("accept", "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .with_context(|| format!("token request to {} failed", self.token_url))?;
        let response = response
            .error_for_status()
            .with_context(|| format!("{} rejected the code exchange", self.token_url))?;
        let body: TokenResponse = response
            .json()
            .await
            .context("token endpoint did not return json")?;
        // GitHub answers a refused exchange with HTTP 200 and an error
        // payload, not a status — the absent access_token is the error
        // path and error_description is what an operator needs to see.
        match body.access_token {
            Some(token) => Ok(token),
            None => Err(anyhow::anyhow!(
                "{} refused the code exchange: {}",
                self.token_url,
                body.error_description
                    .or(body.error)
                    .unwrap_or_else(|| "missing access_token".to_string())
            )),
        }
    }

    /// The login of the user `access_token` authenticates (`GET /user`).
    pub async fn fetch_login(&self, access_token: &str) -> anyhow::Result<String> {
        let user: GithubUser = self
            .get_json(&format!("{}/user", self.api_url), access_token)
            .await?;
        Ok(user.login)
    }

    /// The org logins `access_token`'s user belongs to (`GET /user/orgs`,
    /// first 100 — the page a human's memberships realistically fit in).
    pub async fn fetch_orgs(&self, access_token: &str) -> anyhow::Result<Vec<String>> {
        let orgs: Vec<GithubOrg> = self
            .get_json(
                &format!("{}/user/orgs?per_page=100", self.api_url),
                access_token,
            )
            .await?;
        Ok(orgs.into_iter().map(|org| org.login).collect())
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        access_token: &str,
    ) -> anyhow::Result<T> {
        let response = Self::client()?
            .get(url)
            .header("authorization", format!("Bearer {access_token}"))
            .header("accept", "application/vnd.github+json")
            .send()
            .await
            .with_context(|| format!("GET {url} failed"))?;
        let response = response
            .error_for_status()
            .with_context(|| format!("GET {url} rejected the access token"))?;
        response
            .json()
            .await
            .with_context(|| format!("GET {url} did not return json"))
    }

    /// One pooled client for the process — `GithubOauth`'s pub fields are
    /// a test seam (tests build it by struct literal), so the client
    /// cannot live on the struct, and a per-call builder would throw the
    /// connection pool away each request. GitHub rejects API calls
    /// without a User-Agent; the 10 s total timeout keeps a wedged
    /// endpoint from hanging the callback, and a build failure surfaces
    /// as a call error, not a panic.
    fn client() -> anyhow::Result<&'static reqwest::Client> {
        static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
        if let Some(client) = CLIENT.get() {
            return Ok(client);
        }
        let client = reqwest::Client::builder()
            .user_agent("dream")
            .timeout(Duration::from_secs(10))
            .build()
            .context("cannot build the github http client")?;
        Ok(CLIENT.get_or_init(|| client))
    }
}

/// The token endpoint's payload — GitHub reports a refused exchange as
/// HTTP 200 with `error`/`error_description` instead of a status, so
/// `access_token` stays optional and the error text survives to the log.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    login: String,
}

#[derive(Deserialize)]
struct GithubOrg {
    login: String,
}

/// 16 random bytes, hex-encoded — the CSRF token riding the authorize
/// redirect and the `dream_oauth_state` cookie.
pub fn generate_state() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Admission check: a bare entry matches the login (GitHub logins are
/// case-insensitive) and an `org:name` entry matches any org the user
/// belongs to. `orgs` is the `/user/orgs` result, a parameter so the
/// check stays pure.
pub fn is_allowlisted(allowlist: &[String], login: &str, orgs: &[String]) -> bool {
    allowlist.iter().any(|entry| {
        if let Some(org) = entry.strip_prefix("org:") {
            orgs.iter().any(|member_of| member_of.eq_ignore_ascii_case(org))
        } else {
            entry.eq_ignore_ascii_case(login)
        }
    })
}

/// Percent-encode one query value — every byte outside RFC 3986
/// unreserved becomes `%XX`.
fn urlencoded(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
