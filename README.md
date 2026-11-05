# webterm

Single-binary web terminal: real PTY sessions served over HTTP + WebSocket,
built as a workbench for AI-agent CLIs. Sessions keep running when the
browser tab closes, reattach with full scrollback, and survive a daemon
restart.

## Install

```sh
cargo build --release
# binary lands at target/release/webterm — copy it anywhere on PATH, e.g.
cp target/release/webterm ~/.local/bin/
```

No runtime dependencies; the dashboard frontend is embedded in the binary.

## Run

```sh
webterm                # listens on http://127.0.0.1:4020
webterm --port 8080
```

webterm binds loopback only by default. Binding anything else requires
`--public`, which prints a security warning — anyone who can reach the
address *and* holds the token gets a shell on your machine.

## Auth flow

- On **first run** webterm generates a random 64-char token, stores it at
  `<data-dir>/token` (mode `0600`), and prints the authenticated URL exactly
  once:

  ```
  webterm ready — open this URL once to authenticate:
    http://127.0.0.1:4020/?token=<token>
  ```

- On later runs the same token is reloaded and the URL is not reprinted;
  the token file path is printed instead. Rotate by overwriting the file.
- `--auth-token <T>` lets you supply your own token instead of the
  generated/persisted one.
- Every request must carry the token: `Authorization: Bearer <token>` for
  API calls; `?token=` is also accepted on the WebSocket upgrade and on
  non-API GETs so a pasted share-link URL bootstraps index.html.

## Data-dir layout

Default `<data-dir>` is the platform data dir (`--data-dir` overrides):

- macOS: `~/Library/Application Support/webterm`
- Linux: `~/.local/share/webterm`

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

webterm is loopback-only by default; reach it from other devices over your
tailnet without exposing it publicly:

```sh
tailscale serve --bg 4020
```

`tailscale serve` terminates TLS on the tailnet and proxies to
`http://127.0.0.1:4020` — webterm is then reachable at
`https://<machine>.<tailnet>.ts.net/?token=<token>` from tailnet devices
only. `tailscale` must be installed (`brew install tailscale` on macOS, or
see https://tailscale.com/download) and logged in, and the tailnet's HTTPS
certificates feature must be enabled (admin console → DNS → HTTPS
Certificates).

## Auto-start on macOS (launchd)

Save as `~/Library/LaunchAgents/dev.webterm.plist`, adjusting the binary
and log paths (launchd does not expand `~` — use absolute paths):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.webterm</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/webterm</string>
    <string>--port</string>
    <string>4020</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>/Users/<you>/Library/Logs/webterm.out.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/<you>/Library/Logs/webterm.err.log</string>
</dict>
</plist>
```

```sh
launchctl load ~/Library/LaunchAgents/dev.webterm.plist
launchctl start dev.webterm
```

webterm prints the token URL once on first run to stdout, so keep these
logs owner-private — `~/Library/Logs` is, a world-readable `/tmp` file is
not.

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
(cd target/release && shasum -a 256 webterm) > webterm-<version>-<os>-<arch>.sha256
```

Publish the binary and the `.sha256` side-by-side; consumers verify with
`shasum -a 256 -c webterm-<version>-<os>-<arch>.sha256` run in the
directory containing the `webterm` binary. The release profile
strips symbols, enables LTO and `codegen-units = 1`, and optimizes for size
(`opt-level = "z"`).
