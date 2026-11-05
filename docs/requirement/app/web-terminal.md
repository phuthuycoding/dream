---
feature: "web-terminal"
context: "app"
created: "20261105_1828"
kind: feature
status: confirmed
---

# Spec Requirement

## Feature
web-terminal

## Objective
A single-binary personal web terminal ("cooked in-house") that serves the owner's Mac as a browser-accessible **AI-agent workbench**: a dashboard of terminal sessions where AI agent CLIs (claude, codex, aider and the like) run, that the owner can drive and monitor from any browser, where sessions survive browser disconnects and — via a per-session replay log — keep their screen state across server restarts.

## Problem Statement
The owner runs AI coding agents in terminals on his Mac mini and wants to work with them from anywhere (laptop, phone) through a browser — attach to a running agent, answer its prompts, watch it work — with session state managed server-side so closing the browser loses nothing. Agent CLIs are fullscreen TUI programs, so the web terminal must render interactive TUIs faithfully, not just line-mode shells. Prior art splits badly: ttyd/gotty/wetty are dumb transports with no session registry or reattach (they punt persistence to tmux); VibeTunnel has the right shape (PTY registry + asciinema stream files, agent-monitoring UX) but the owner does not want to depend on it. The product is a small self-owned service combining a real session registry, own-PTY management and a replay log, in one binary, deployed inside his Tailscale tailnet.

## Scope
### In Scope
- One self-contained Rust binary embedding the web frontend (no external runtime, no separate assets)
- Session registry persisted as files: per-session `session.json` metadata + replay-log stream file, durable across restarts (no database)
- Web dashboard listing sessions; full session CRUD (create / rename-edit / kill / delete) from the browser
- Per-session setup script executed when its shell spawns (typical use: launch an agent CLI, cd into a project, source env)
- Faithful rendering of fullscreen TUI programs (alt-screen, 256-colour, wide glyphs) — the workload is interactive agent CLIs
- Log cleanup: bounded replay logs with an explicit clean mechanism (manual + policy-driven)
- Full interactive terminal in browser (xterm.js-class renderer): input, output, resize over WebSocket
- Reattach semantics: new client receives buffered scrollback from the replay log, then the live stream
- Server-restart resurrection: sessions respawn a fresh shell in the recorded cwd and restore the screen from the replay log, visibly marked as resurrected (running processes are lost — explicit owner decision over tmux-backed survival)
- Single-user authentication (password or token) on every HTTP and WebSocket endpoint
- Tailscale-first deployment: loopback bind by default, printed `tailscale serve` guidance, install hints when tailscale is absent

### Out of Scope
- Multi-user accounts, per-user session isolation, ACLs (single owner only)
- TLS termination inside the binary (Tailscale Serve handles HTTPS)
- Public-internet exposure modes (no Funnel-style sharing; non-loopback bind is opt-in with a warning)
- tmux/screen/zellij integration — deliberately rejected by the owner in favour of own-PTY + replay log
- SSH client/proxy modes, file transfer, remote command execution API
- Session sharing with other people, collaborative cursors, read-only viewers
- Agent-aware integrations beyond the terminal itself — token-usage dashboards, parsing specific agent CLIs' status, agent hooks/plugins
- Native mobile apps; asciinema export UI; auto-update

## Affected surfaces
- `README.md` — install, run, auth setup, `tailscale serve` recipe
- CLI `--help` and first-run guidance printed by the binary (install-tailscale hint, bind warning)
- Packaged binary distribution (build/release docs, checksums)
- Default config/data-dir documentation

## Actors
- Owner — the single human user, on any browser device inside (or reaching) his tailnet
- The daemon — the single binary owning all PTYs and session state
- Browser client — dashboard + terminal views served by the daemon
- Tailscale Serve — external HTTPS ingress into the loopback service

## Functional Requirements
### FR-001
- Requirement: The server serves an embedded web dashboard listing every live session with id, name, shell command, cwd, status and creation time.
- Priority: must
- Notes: The session registry is the differentiator vs ttyd (see reviews/research-terminal-internals.md — VibeTunnel's per-session dir is the prior art).

### FR-002
- Requirement: The owner can create a new terminal session from the dashboard (default login shell, optional name, cwd and setup script); each session owns exactly one PTY, and its setup script runs when the shell spawns — including on resurrect and respawn.
- Priority: must
- Notes: Setup script = owner-defined commands run at shell start (e.g. cd, source env, launch a command). Owner requirement: "có setup script khi start session".

### FR-003
- Requirement: Attaching to a session opens a full interactive terminal in the browser; keyboard input, PTY output and window resize propagate both ways over a WebSocket.
- Priority: must
- Notes: Binary frame protocol with short opcodes, the shape all four surveyed tools converge on.

### FR-004
- Requirement: Closing, reloading or switching browser devices never kills a session: the PTY lives in the daemon and output is appended to the session's replay log (subject to the log's documented cap); a reattaching client first receives the retained scrollback, then the live stream.
- Priority: must
- Notes: This is the owner's core requirement ("tắt browser đi cũng không bị làm sao"). "Scrollback" means the retained window under the cap, not an unbounded history.

### FR-005
- Requirement: After a daemon restart, sessions are resurrected from persisted session state (JSON metadata + replay logs on disk): a fresh shell spawns in the recorded cwd re-running the setup script, the client's view is restored from the replay log, and the session is visibly marked as resurrected (previously running processes are gone).
- Priority: must
- Notes: Owner decisions — "Own PTY + replay log", not tmux-backed; session state as JSON files, no database (owner revised the earlier SQLite idea). Processes do not survive restart; screen state and session identity do.

### FR-006
- Requirement: The owner can manage each session's lifecycle from the dashboard: kill the running shell (dead state preserved), rename/edit its metadata (name, cwd, setup script), and delete the session entirely (removing its persisted state and replay log).
- Priority: must
- Notes: Owner requirement — "quản lý session cũng như là thêm sửa xoá session". Edit applies to metadata; a live shell keeps running until killed/respawned.

### FR-007
- Requirement: Every HTTP and WebSocket endpoint requires single-owner authentication (configured password or token); unauthenticated requests are rejected. When no credential is configured, the daemon generates a random token at first run, stores it with owner-only permissions in the data dir and prints the authenticated URL once at startup.
- Priority: must
- Notes: A web terminal is unauthenticated RCE without this layer (reviews/research-remote-exposure.md). Generated-first-run behaviour is the defined answer to "no credential configured".

### FR-008
- Requirement: The product distributes as one self-contained executable embedding the frontend; `--help` documents run options (port, auth, data dir).
- Priority: must
- Notes: Owner's words — "1 binary up phát là xong".

### FR-009
- Requirement: The binary is Tailscale-first: it binds to loopback by default and prints concise `tailscale serve` setup guidance; when no tailscale installation is detected it tells the user how to install it; binding a non-loopback interface requires an explicit flag and prints a security warning.
- Priority: should
- Notes: Owner's words — "chưa có tailscale thì hướng dẫn user cài".

### FR-010
- Requirement: More than one browser client may attach to the same session simultaneously; all clients render output and may send input (single owner, no locking).
- Priority: could
- Notes: VibeTunnel semantics — live clients tail the session stream.

### FR-011
- Requirement: When a session's shell/PTY exits, the session transitions to a dead state that stays listed on the dashboard (status visible per FR-001); attaching to a dead session replays its retained screen and offers a respawn action.
- Priority: must
- Notes: Covers the lifecycle gap — exit detection and dead-session attach behaviour.

### FR-012
- Requirement: The web UI is usable on a phone-sized browser: readable text, functioning touch keyboard input and terminal fit on small screens.
- Priority: could
- Notes: Problem statement targets phone access; AC only gates that the dashboard loads.

### FR-013
- Requirement: Fullscreen TUI programs — AI agent CLIs, vim, htop, fzf-style pickers — render correctly and accept input in the browser terminal, including alt-screen transitions, 256-colour and box/wide glyphs, across attach and reattach.
- Priority: must
- Notes: The stated mission is working with AI agents; agent CLIs are alt-screen TUIs, so this is a hard requirement, not fidelity polish.

### FR-014
- Requirement: The dashboard surfaces an attention signal per session when its process rings the terminal bell or stops outputting while a prompt awaits (idle-awaiting heuristic); attaching clears the signal.
- Priority: could
- Notes: Matches the agent-monitoring use case (agent asks → owner answers from phone). Heuristic details are a planning decision.

### FR-015
- Requirement: The owner can clean replay logs: a per-session and a global clean action, plus a configurable retention policy (max log size and/or age) the daemon enforces automatically; cleaning a live session's log does not disrupt the PTY.
- Priority: must
- Notes: Owner requirement — "cơ chế clean log". Policy details (defaults, size vs age) are planning decisions.

## Non-Functional Requirements
- Latency: keystroke-to-echo p50 < 100 ms on a tailnet link (non-gating target, measured not enforced)
- Footprint: binary ≤ ~25 MB; idle RSS target < 150 MB with ~10 live sessions (non-gating target)
- Replay log bounded per session (ring or capped size, drop policy documented) and tolerant of torn writes
- Security: auth checked before any session data flows; no unauthenticated public bind possible by default
- Platforms: macOS arm64 primary; Linux amd64 secondary
- Terminal fidelity: fullscreen alt-screen TUI apps render correctly (256-colour, wide/box glyphs, synchronized updates); correct `TERM` (xterm-256color) and `stty` size propagation

## Main Use Cases
- UC-001 Open dashboard and list live sessions
- UC-002 Create a session and attach
- UC-003 Close browser, reattach from another device
- UC-004 Restart daemon, resurrect sessions
- UC-005 Kill, rename/edit and delete sessions
- UC-006 First run: bind, auth and Tailscale guidance
- UC-007 Session setup script runs on spawn, resurrect and respawn
- UC-008 Clean replay logs manually and via retention policy

## Constraints
- Backend in Rust (owner: "backend có thể là rust hoặc gì đó em" — Rust chosen; crates are a planning decision)
- One self-contained binary, frontend embedded at build time, no runtime asset directory
- Own PTY management — no tmux, screen or zellij dependency (owner decision)
- Single owner; never multi-tenant

## Assumptions
- Likely stack (planning may adjust): Rust + axum/tokio + tokio-tungstenite + portable-pty; xterm.js frontend vendored and embedded via include_bytes; replay log in an asciinema-v2-shaped stream file per session — grounded in reviews/research-terminal-internals.md
- Per-session state dir holding `session.json` (metadata: name, cwd, setup script, status, log path) + the replay-log stream file — the VibeTunnel control-dir pattern (owner decision: JSON files, no SQLite)
- Data dir defaults to a platform user-data path (e.g. `~/.local/share/webterm` / `~/Library/Application Support/webterm`)
- Tailscale Serve terminates TLS; the app serves plain HTTP on loopback (reviews/research-remote-exposure.md)
- Test level defaulted to `full`, UI coverage `critical` — the owner answered the packaging constraint instead of the test question; flagged in the confirmation ask

## Acceptance Criteria
- [ ] Running the binary serves the dashboard on loopback; an unauthenticated request to any page or the WS endpoint is rejected
- [ ] After signing in, the dashboard lists a created session with the correct name, command, cwd and status
- [ ] Clicking a session opens a terminal; typing `echo hi` produces visible output in the browser
- [ ] An alt-screen TUI (e.g. `vim` or `htop`) renders fullscreen, accepts input, and survives reattach looking correct
- [ ] Resizing the browser window makes `stty size` inside the session report the new columns/rows
- [ ] With a `sleep 600` running in a session, closing the tab and reopening it on another device reattaches to the same session with the retained scrollback window intact
- [ ] Restarting the daemon while a session shows content resurrects the session: the screen is restored from the log, the session is marked resurrected, and typing works in the fresh shell
- [ ] Renaming a session and editing its cwd/setup script persists across daemon restart
- [ ] A session created with a setup script runs that script when its shell spawns, again on resurrect after restart, and on respawn of a dead session
- [ ] Killing a session marks it dead in the list; deleting it removes the session and its replay state entirely
- [ ] The clean action shrinks a bloated session log without breaking the running terminal, and the retention policy trims logs automatically
- [ ] The distributed binary runs with no sibling asset directory and `otool -L` lists only system libraries; running it where tailscale is absent prints the install-and-serve guidance
- [ ] Via `tailscale serve`, the dashboard loads over HTTPS on a second tailnet device (e.g. a phone)

## Edge Cases
- Torn replay-log write on daemon crash — reader must tolerate a truncated tail frame
- Two attached clients send input simultaneously — last-writer wins, documented behaviour
- Resize races during heavy output bursts
- Session name collision — auto-suffix or explicit rejection
- Log growth under spammy output — cap + documented drop policy
- Killing a session while clients are attached — clients receive a session-ended notice
- Attaching to a dead-but-preserved session — replay then offer respawn
- Port already in use — clear startup error
- Corrupt or missing `session.json` on startup — skip/quarantine the session with a logged warning rather than crash the daemon
- Deleting a session while clients are attached — clients get session-ended notice
- Setup script failing or hanging on spawn — session still usable, failure visible
- Cleaning a log while a client is mid-reattach — consistent replay, no torn frames

## Open Questions
- Binary name and default port (planning detail)
- Default retention policy values for log cleanup (size cap, age, on-attach trim?)
- Does a resurrected session respawn its shell immediately or on first attach?
- Always-on across Mac reboots: document a launchd recipe only, or ship a `--install`/service flag?
- Setup script semantics: run as shell prelude (same tty) vs a spawned command — planning decision

## Test Strategy
- Level: full
- UI Tests: critical
- Tools: cargo test (unit + integration over PTY/WS/replay-log layers) + Playwright (chromium) driving the built binary
- Coverage Target: 80%
