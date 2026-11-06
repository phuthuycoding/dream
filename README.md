# dream

Single-binary web terminal: real PTY sessions served over HTTP + WebSocket,
built as a workbench for AI-agent CLIs. Sessions keep running when the
browser tab closes, reattach with full scrollback, and survive a daemon
restart.

## Install

```sh
cargo build --release
# binary lands at target/release/dream — copy it anywhere on PATH, e.g.
cp target/release/dream ~/.local/bin/
```

No runtime dependencies; the dashboard frontend is embedded in the binary.

## Run

```sh
dream                # listens on http://127.0.0.1:4020
dream --port 8080
```

## Service commands (macOS + Linux)

```
dream up      # install + start the service (launchd agent / systemd --user)
dream status  # show service state + token URL
dream restart # restart the service
dream logs    # show daemon logs (-f to follow)
dream url     # reprint the token URL
dream ps      # list sessions
dream doctor  # diagnose: service, port, token, tailscale
dream down    # stop + remove the service
```

Upgrading from `webterm`? The data dir moved to `~/Library/Application Support/dream` (`~/.local/share/dream` on Linux) — copy the old `webterm` dir over, or just re-open the printed URL once. `DREAM_AUTH_TOKEN` replaces `WEBTERM_AUTH_TOKEN`; the browser re-prompts the token.

dream binds loopback only by default. Binding anything else requires
`--public`, which prints a security warning — anyone who can reach the
address *and* holds the token gets a shell on your machine.

## Auth flow

- On **first run** dream generates a random 64-char token, stores it at
  `<data-dir>/token` (mode `0600`), and prints the authenticated URL exactly
  once:

  ```
  dream ready — open this URL once to authenticate:
    http://127.0.0.1:4020/?token=<token>
  ```

- On later runs the same token is reloaded and the URL is not reprinted;
  the token file path is printed instead. Rotate by overwriting the file.
- `--auth-token <T>` lets you supply your own token instead of the
  generated/persisted one.
- The static dashboard shell is served without auth (it contains no
  secrets — browsers cannot attach `Bearer` to `<script src>`/`<link>`
  fetches). Every **API** request must carry the token: `Authorization: Bearer <token>` for
  API calls; `?token=` is also accepted on the WebSocket upgrade and on
  non-API GETs so a pasted share-link URL bootstraps index.html.

## Data-dir layout

Default `<data-dir>` is the platform data dir (`--data-dir` overrides):

- macOS: `~/Library/Application Support/dream`
- Linux: `~/.local/share/dream`

```
<data-dir>/
  token                     # bearer token, mode 0600
  sessions/
    <session-uuid>/
      session.json          # name, cmd, cwd, status, timestamps
      replay.cast           # asciinema-style output replay log
```

`session.json` writes are atomic; a corrupt file is quarantined to
`session.json.corrupt-<uuid>` instead of crashing startup.

## Sessions

Create, list, patch, delete via the REST API (`/api/sessions`,
`/api/sessions/<id>` plus `kill`, `respawn`, `clean`), or drive everything
from the built-in dashboard.

- **Reattach**: `GET /api/sessions/<id>/ws` replays `replay.cast` scrollback
  then sends a live vt100 screen snapshot, so a fresh browser sees exactly
  what a detached one left.
- **Resurrect**: sessions alive when the daemon stopped come back on the
  next start (status `resurrected`).
- **Attention**: a session flags attention on a bell in its output or after
  30 s of output silence while detached; attaching clears it.
- **Setup script**: `setup_script` on create is queued as the PTY's first
  input line — use it for env setup, `cd`, or launching an agent CLI.
- **Log retention**: `--log-cap` / `--log-max-age` bound replay logs; the
  enforcer trims or resets them every 60 s.

## Remote access via Tailscale

dream is loopback-only by default; reach it from other devices over your
tailnet without exposing it publicly:

```sh
tailscale serve --bg 4020
```

`tailscale serve` terminates TLS on the tailnet and proxies to
`http://127.0.0.1:4020` — dream is then reachable at
`https://<machine>.<tailnet>.ts.net/?token=<token>` from tailnet devices
only. `tailscale` must be installed (`brew install tailscale` on macOS, or
see https://tailscale.com/download) and logged in, and the tailnet's HTTPS
certificates feature must be enabled (admin console → DNS → HTTPS
Certificates).

## Auto-start on macOS (launchd)

`dream up` does this automatically — recommended. Manual recipe for custom setups: the agent must be label `com.dream.daemon` for `dream down`/`status` to manage it; use `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.dream.daemon.plist` (modern verbs — not `load`), with logs at `~/Library/Logs/dream/{out,err}.log`. Under launchd the token is never printed to the log — it lives in the 0600 token file (`<data-dir>/token`), printed once on a TTY first run or reprinted with `dream url`.

## CLI reference

| Flag | Default | Purpose |
|---|---|---|
| `--port <N>` | `4020` | Port to listen on |
| `--bind <ADDR>` | `127.0.0.1` | Bind address; non-loopback requires `--public` |
| `--data-dir <DIR>` | platform data dir | Session state + token location |
| `--auth-token <T>` | generated | Override the bearer token |
| `--public` | off | Allow non-loopback bind (prints a security warning) |
| `--log-cap <BYTES>` | unset | Per-session replay log size cap |
| `--log-max-age <SECONDS>` | unset | Max replay log age; older logs are reset |

## Release build + checksum

```sh
cargo build --release
(cd target/release && shasum -a 256 dream) > dream-<version>-<os>-<arch>.sha256
```

Publish the binary and the `.sha256` side-by-side; consumers verify with
`shasum -a 256 -c dream-<version>-<os>-<arch>.sha256` run in the
directory containing the `dream` binary. The release profile
strips symbols, enables LTO and `codegen-units = 1`, and optimizes for size
(`opt-level = "z"`).
