---
status: archived
---
# Phase 1 — Requirement: github-oauth

## Problem
The daemon gates `/api/*` and the WS attach on a single static bearer token pasted into the gate view (or `?token=` URL). That is unfriendly for daily browser use — the owner must re-paste a 64-char token on every new browser/device — and a token leak means indefinite access with no identity attached. GitHub OAuth gives the owner a one-click login tied to their GitHub identity.

## Objective
"Sign in with GitHub" on the gate view: server-side OAuth App flow (`/auth/github` → github authorize → `/auth/callback` → session cookie) that authenticates `/api/*` and the WS upgrade for ~30 days, restricted to configured GitHub login(s)/org, with the existing bearer token kept as a working fallback for curl/CLI.

## In Scope
- Config: `--github-client-id`, `--github-client-secret`, `--github-allowlist` flags with `DREAM_GITHUB_CLIENT_ID` / `DREAM_GITHUB_CLIENT_SECRET` / `DREAM_GITHUB_ALLOWLIST` env fallbacks
- `GET /auth/github` → 302 to github.com/login/oauth/authorize with client_id, redirect_uri, CSRF `state`
- `GET /auth/callback` → verify `state`, exchange `code` for access token, fetch user identity + orgs, check allowlist, set session cookie, redirect to `/`
- Session cookie `dream_session`: HMAC-signed `login|expiry`, HttpOnly, Secure, SameSite=Lax, Max-Age ~30d; signing key generated once, persisted mode 0600 in the data dir
- Allowlist: comma-separated GitHub logins and `org:NAME` entries; non-allowlisted identities get 403
- Gate view: "Sign in with GitHub" button when OAuth is configured; logout button clears the cookie via `POST /auth/logout`
- Auth middleware accepts session cookie OR bearer token OR `?token=` on the WS attach route (unchanged)

## Out of Scope
- Multi-user sessions, per-user ACLs, or identity-scoped terminal sessions
- Other OAuth providers (Google, GitLab), SSO/SAML, OAuth device flow
- Refresh-token rotation or server-side session revocation list
- Replacing or removing the bearer-token flow

## Affected surfaces
- src/cli.rs, src/config.rs github oauth + allowlist flags/env (DREAM_GITHUB_CLIENT_ID/SECRET/ALLOWLIST)
- new src/auth/session.rs + src/auth/oauth.rs; src/auth.rs becomes a module dir
- src/http/routes.rs /auth/* routes + AppState; src/http/middleware.rs cookie OR bearer; src/main.rs wiring
- web/index.html gate view + logout button, web/app.js gate/auth logic, web/api.js logout call, web/style.css login button
- Cargo.toml deps (reqwest promoted to dep, hmac+sha2), e2e oauth spec + helpers, README.md oauth setup, docs/requirement/app/web-terminal.md

## Actors
- Owner (single user, machine operator, allowlisted GitHub login)
- GitHub OAuth service (external dependency)

## Functional Requirements

### FR-001
- Requirement: GitHub OAuth login flow — the OAuth App's client_id/secret are configured server-side via env/flags; `GET /auth/github` redirects to `https://github.com/login/oauth/authorize` with a crypto-random `state`; `GET /auth/callback` verifies `state`, exchanges the `code` for an access token, fetches the user's identity, and sets a `dream_session` cookie (HttpOnly, Secure, SameSite=Lax, ~30d) that the middleware accepts on `/api/*` and the WS upgrade.
- Priority: must

### FR-002
- Requirement: Allowlist — only configured GitHub login(s) or members of configured `org:` entries may authenticate; any other authenticated GitHub identity gets a 403 response and no session cookie.
- Priority: must

### FR-003
- Requirement: Login page on the static shell — the gate view shows a "Sign in with GitHub" button when no valid session cookie is present and OAuth is configured; the cookie lives ~30 days so repeat visits skip the gate; a logout action POSTs `/auth/logout` which clears the cookie and returns the user to the gate.
- Priority: must

### FR-004
- Requirement: Bearer token remains a valid fallback auth — `Authorization: Bearer <token>` on `/api/*` and `?token=` on `/api/sessions/{id}/ws` keep working unchanged, so curl, `dream ps`/`doctor`, and existing e2e specs are unaffected.
- Priority: must

## Non-functional
- client secret is never shipped to the frontend or logged; it is used only in the server-side code exchange
- CSRF protection: crypto-random `state` param on the authorize URL, stored in a short-lived HttpOnly cookie and verified on the callback
- session cookie is crypto-random/HMAC-signed (signing key persisted 0600 in the data dir), not a JWT — no JWT dependency unless trivial
- token exchange and user-info calls go through `reqwest` (promoted from dev-dep); GitHub base URLs are injectable for tests
- OAuth routes are public (pre-auth); every other existing gate behaviour is unchanged

## Acceptance criteria
- [ ] `dream --github-client-id X --github-client-secret Y --github-allowlist mylogin` starts; `GET /auth/github` 302s to github authorize with a `state` param
- [ ] full flow against a stub GitHub server: callback mints a cookie; `/api/sessions` returns 200 with the cookie and 401 without it
- [ ] a non-allowlisted GitHub identity gets 403 and no cookie; `org:` entries admit org members
- [ ] gate view renders "Sign in with GitHub"; logout clears the cookie and returns to the gate
- [ ] `curl -H "Authorization: Bearer $TOKEN"` and WS `?token=` attach still pass; existing cargo + playwright suites green
- [ ] tampered/expired cookie → 401; `state` mismatch on callback → 400/403

## Test Strategy
- Level: full (unit for cookie mint/verify, allowlist match, authorize URL, cli/env parse; integration for callback + middleware with a stub GitHub server; E2E for the gate button + bearer regression)
- UI Tests: critical (gate view gains the GitHub button; token-paste fallback must keep working — existing selectors preserved)
- Tools: cargo test, playwright
- Coverage: ≥80% lines (inherited gate)

## Edge cases
- OAuth not configured (no client id/secret) → `/auth/*` 404s and the gate shows only the token field; daemon behaves exactly as today
- `--github-allowlist` empty while OAuth configured → refuse to start (would lock everyone out or let everyone in — both wrong)
- GitHub unreachable or code exchange fails → callback 502/friendly gate error, no cookie
- `state` mismatch / missing / reused → 400; the short-lived state cookie is cleared on the callback regardless of outcome
- expired or tampered `dream_session` → treated as absent → gate view
- WS upgrade carries the cookie automatically (same-origin); browser clients need no `?token=` once logged in
- cookie set over plain loopback HTTP with `Secure` — localhost is a trustworthy origin in modern browsers; Safari on a bare LAN IP may drop it (documented; bearer fallback covers it)
- daemon restart must keep sessions valid → signing key is persisted in the data dir, not regenerated per boot
