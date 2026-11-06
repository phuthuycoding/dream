---
status: archived
---
# Phase 1 — Requirement: cli-lifecycle

## Problem
The shipped binary is still named `webterm` while the product is `dream`, and there is no one-command way to install/start/stop the daemon — the launchd recipe is documentation-only. The owner wants `dream up` the way `tailscale up` works: one command, daemon is running now and on every login. Dashboard UX also has rough edges (UUID text-wraps, raw ISO timestamps, flat actions).

## Objective
A polished v1.0.0: binary/package named `dream`, a `tailscale`-style lifecycle CLI (`up|down|status|restart|logs|url|ps|doctor`) managing the launchd agent and local admin, and a dashboard that reads cleanly at a glance.

## In Scope
- Rename package/binary `webterm` → `dream` (crate, CLI name, env var DREAM_AUTH_TOKEN, localStorage keys, title/brand, docs, e2e binary path)
- `dream up` — install + start the service: `~/Library/LaunchAgents/com.dream.daemon.plist` (RunAtLoad + KeepAlive) on macOS, `~/.config/systemd/user/dream.service` (Restart=always) on Linux; idempotent, prints token URL + logs path + tailscale hint
- `dream down` — stop the service (`launchctl bootout gui/<uid>/<label>` on macOS — KNOWN BUG: single-arg form required; `systemctl --user disable --now dream` on Linux) + remove the unit file
- `dream status` — service state + URL from token file (`launchctl print` / `systemctl --user is-active`)
- `dream restart` — `launchctl kickstart -k gui/<uid>/<label>`
- `dream logs [-f]` — tail `~/Library/Logs/dream/{out,err}.log`
- `dream url` — print the token URL from the token file
- `dream ps` — list sessions via the local API (token from data dir): id, name, cmd, status
- `dream doctor` — checks: launchd agent state, port reachable, token file exists+0600, tailscale present/connected, prints a checklist
- UI polish: session cards (short id + copy, relative time, structured meta rows), nicer buttons/badges/hover, keep all e2e selectors
- Version bump to 1.0.0

## Out of Scope
- Windows service support (launchd + systemd --user this round)
- `dream attach <id>` (local-terminal attach — v2), `kill/respawn/clean` via CLI (needs ps; v2), `dream update`, plist customization UI, brew formula
- Behaviour changes to daemon runtime/attach/auth

## Affected surfaces
- Cargo.toml name/version, src/cli.rs, src/main.rs, src/lib.rs, new src/daemon.rs
- src/config.rs default data dir (`~/Library/Application Support/dream`), warnings
- web/index.html brand/title, web/api.js storage keys, web/{style,dashboard,terminal,app}.js
- e2e/helpers.ts binary path + specs, README.md (commands, plist recipe, launchd section)

## Actors
- Owner (single user, machine operator)

## Functional Requirements

### FR-001
- Requirement: Package, binary and user-facing name is `dream` — `cargo build` produces `target/release/dream`; `dream --help` shows `dream`; env var `DREAM_AUTH_TOKEN`; browser title/brand `dream`; README commands use `dream`.
- Priority: must

### FR-002
- Requirement: `dream up` writes `~/Library/LaunchAgents/com.dream.daemon.plist` (current binary path + resolved port/bind/data-dir/log flags), bootstraps `gui/<uid>`, and is idempotent (re-`up` replaces the agent). On success it prints the token URL, log dir, stop hint, and the tailscale hint on loopback.
- Priority: must

### FR-003
- Requirement: `dream down` runs `launchctl bootout gui/<uid>/<label>` (single `domain/service` argument — manual test proved the two-arg form fails with I/O error) and removes the plist; `dream status` reports running/stopped + prints the token URL when running.
- Priority: must

### FR-004
- Requirement: Sessions and daemon logs keep working under launchd — KeepAlive restarts the daemon, session resurrect behaves as in foreground runs, stderr/stdout go to `~/Library/Logs/dream/`.
- Priority: must

### FR-005
- Requirement: `dream restart` (kickstart -k / systemctl restart), `dream logs [-f]` (tail launchd log files / journalctl --user -u dream), `dream url` (reprint the token URL), `dream ps` (session table via local API using the token file), `dream doctor` (agent state, port reachability, token file presence+perms, tailscale presence/connection — one checklist output).
- Priority: must

### FR-006
- Requirement: Dashboard polish — session cards show name + status badge, cmd/cwd in mono, short uuid + click-to-copy, relative created time, attention dot; no mid-word wraps; all existing e2e selectors (.session-card, .session-meta, .session-name, .badge, .attention-dot, data-session-id, #btn-*) remain working.
- Priority: should

### FR-007
- Requirement: `Cargo.toml` version is 1.0.0; README documents the full CLI surface (`up|down|status|restart|logs|url|ps|doctor`).
- Priority: should

## Non-functional
- service ops support macOS (launchd) and Linux (systemd --user); Windows prints a clear unsupported message
- no new dependencies; plist written as XML string
- UI polish is CSS/DOM-structure only where possible

## Acceptance criteria
- [ ] `cargo build` → `target/release/dream`; `dream up` creates+bootstraps the agent; curl to the port returns the dashboard
- [ ] `dream status` prints running + URL; `dream down` actually kills the daemon (verified: port refuses) and removes the plist
- [ ] `dream restart` restarts the agent; `dream logs` shows daemon output; `dream url` prints a working token URL; `dream ps` lists sessions; `dream doctor` prints a correct checklist
- [ ] re-`dream up` with different flags replaces the agent args
- [ ] cargo test + playwright suites green incl. renamed binary paths
- [ ] dashboard screenshot shows the polished cards

## Test Strategy
- Level: full (unit for plist/cli parse, integration for lifecycle helpers, E2E rerun for renamed binary + a `dream up/down` smoke where feasible headlessly via launchd)
- UI Tests: critical (dashboard polish must not break selectors)
- Tools: cargo test, playwright
- Coverage: ≥80% lines (inherited gate)

## Edge cases
- `dream up` while a foreground `dream` holds the port → daemon exits on bind error; err.log shows it (documented behavior)
- stale plist from an older binary path → `up` rewrites it
- token file absent at `up` → wait ~15s then print logs hint
- `down` when agent not installed → friendly message, exit 0
- non-macOS/Linux → clear "unsupported" error; missing systemctl on Linux → friendly fallback hint
- uuid/session names longer than card width → ellipsis, never mid-word wrap
