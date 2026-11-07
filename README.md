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
  fetches). Every **API** request must carry a credential: `Authorization: Bearer <token>`,
  a signed `dream_session` cookie (see GitHub OAuth below), or
  `?token=` on the WebSocket upgrade — a pasted share-link URL
  (`/?token=<token>`) boots the public shell, which picks the token up
  for its API calls.

## GitHub OAuth sign-in (optional)

The bearer token always stays valid — GitHub login is an optional
second way in, enabled by registering an OAuth App:

1. GitHub → Settings → Developer settings → OAuth Apps → **New OAuth
   App**. Set the **Authorization callback URL** to
   `<base>/auth/callback` — the scheme + host the browser opens the
   dashboard on, e.g. `http://127.0.0.1:4020/auth/callback` locally or
   `https://<machine>.<tailnet>.ts.net/auth/callback` behind
   `tailscale serve`. (dream rebuilds the callback per request from the
   `Host` header and `X-Forwarded-Proto`; the registered URL just has
   to match what the browser uses.)
2. Start dream with the three values:

   ```sh
   dream --github-client-id <ID> \
         --github-client-secret <SECRET> \
         --github-allowlist <LOGIN[,LOGIN|org:ORG...]>
   ```

   `DREAM_GITHUB_CLIENT_ID`, `DREAM_GITHUB_CLIENT_SECRET` and
   `DREAM_GITHUB_ALLOWLIST` fill in when a flag is absent — prefer the
   env var for the secret (a flag value shows in `ps`). A flag beats
   its env var; an empty value counts as unset.

- **Allowlist grammar**: comma-separated entries. A bare `login`
  admits that GitHub user (case-insensitive); `org:name` admits any
  member of that org (membership checked against the user's
  first 100 orgs from /user/orgs). `org:` entries are why the authorize request
  carries the `read:org` scope — the callback lists `/user/orgs` to
  check membership. A user who matches no entry gets `403`.
  Configuring any oauth value without a usable allowlist — or only
  half the credentials — is a startup error, so oauth can never come
  up half-wired.
- The callback mints a signed `dream_session` cookie (`HttpOnly`,
  `SameSite=Lax`, 30-day Max-Age) whose HMAC key persists at
  `<data-dir>/session-key` (mode `0600`) — logins survive a daemon
  restart. `POST /auth/logout` expires it.
- **Secure cookies**: dream speaks cleartext HTTP itself, so
  `dream_session` is marked `Secure` only when the request arrives
  behind an HTTPS front that sets `X-Forwarded-Proto: https`
  (`tailscale serve` does). Minting `Secure` on plain http would make
  browsers drop the cookie — a login loop.
- **`dream up`**: the resolved config (flags or env — same merge)
  travels to the service as unit environment
  (`Environment=`/`EnvironmentVariables`, never args — that would
  expose the secret via `ps`). Pass the flags or set the
  `DREAM_GITHUB_*` vars on the `dream up` invocation and the installed
  service picks them up.
- **Token fallback**: nothing about the bearer path changes —
  `Authorization: Bearer` on API calls and `?token=` on the WebSocket
  attach still work, and the gate shows the token form next to the
  "Sign in with GitHub" button.

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
  30 s of output silence while detached; attaching clears it. `GET
  /api/sessions` and `GET /api/sessions/<id>` report it as `attention` —
  runtime state only, never written to `session.json`.
- **Setup script**: `setup_script` on create is queued as the PTY's first
  input line — use it for env setup, `cd`, or launching an agent CLI.
- **Log retention**: `--log-cap` / `--log-max-age` bound replay logs; the
  enforcer trims or resets them every 60 s.

## Web UI

Flat, professional list UI (no framework, no build step — vanilla ES
modules embedded in the binary).

- **Home**: full-width session list — one row per session (icon, name,
  `cmd · cwd`, created-time, status pill, row actions), guided empty
  state, page head with create/refresh/clean-all.
- **Sessions sidebar**: a persistent left sidebar lists every session —
  status dot, name, `cmd · cwd`, attention dot, row actions; clicking
  one attaches or switches the terminal in place. On mobile it becomes
  a drawer (☰ in the topbar).
- **Keep-alive terminals**: each attached session keeps its own
  xterm + WebSocket in a hidden pane, so switching preserves screen
  state with no replay. At most 6 terminals stay live — the
  least-recently-used background instance is evicted and re-attaches
  (with a replay) on next selection.
- **Attention**: the bell/idle flag shows on the tab and the home row.

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

## Mobile app

A native iOS companion (`ios/`) that pairs with a running daemon and
offers the session browser plus a full terminal attach on iPhone.
Requires iOS 17+.

- **Pairing**: run `dream url` on the daemon host and paste the printed
  `…/?token=…` URL into the app's pairing form. The token is stored in
  the iOS Keychain; several daemons can be saved as named profiles.
  The printed URL reflects the bind — for `0.0.0.0`/`::` binds it
  resolves the machine's LAN IPv4; use that IP if a hostname fails.
- **Reachability**: the phone must be able to reach the daemon — a
  non-loopback bind on the same LAN (`--bind`/`--public`, see the flag
  table), or anywhere over your tailnet (see above). Note: iOS (ATS)
  blocks plain `http` to named hosts — pair by IP literal or serve the
  daemon over `https` (e.g. `tailscale serve`).
- **Build**: open `ios/Dream.xcodeproj` in Xcode, or from the repo root:

  ```sh
  xcodebuild -project ios/Dream.xcodeproj -scheme Dream \
      -destination 'platform=iOS Simulator,name=iPhone 15' build
  ```

- **Attention**: a session flagging attention shows an orange dot on its
  row and posts a local notification; tapping it opens that session's
  terminal. Denying notification permission only mutes the banners —
  the in-app dot keeps working.
- **Limitations**: notifications are local and fire only while the app
  is alive — foreground or the short iOS background grace window — so
  they are driven by the app's ~15 s session poll, not a push channel.
  A killed app cannot notify.

### Running the iOS UI tests

The `DreamUITests` suite drives the app against real daemons on
loopback (see `ios/DreamUITests/DreamUITestCase.swift` for the fixture
contract):

- `127.0.0.1:4161` — "primary" daemon, token `e2e-ui-token-alpha`
- `127.0.0.1:4162` — "secondary" daemon, token `e2e-ui-token-beta`
- `127.0.0.1:4171` — a transparent TCP proxy in front of the primary.
  While `/tmp/dream-e2e/deny.flag` exists it answers `401` to new
  connections instead of forwarding — that is how a stored token going
  stale is exercised without restarting anything.

Launch the fixture daemons from the repo root:

```sh
mkdir -p /tmp/dream-e2e
target/debug/dream --port 4161 --auth-token e2e-ui-token-alpha \
    --data-dir /tmp/dream-e2e/alpha &
target/debug/dream --port 4162 --auth-token e2e-ui-token-beta \
    --data-dir /tmp/dream-e2e/beta &
```

For the gate, any TCP forwarder that honors the flag works; this
one-file Python proxy is enough (save as `/tmp/dream-e2e/gate.py`,
run `python3 /tmp/dream-e2e/gate.py &`):

```python
import os, socket, threading

FLAG = "/tmp/dream-e2e/deny.flag"
DENIED = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"

def pump(src, dst):
    try:
        while chunk := src.recv(65536):
            dst.sendall(chunk)
    except OSError:
        pass
    for s in (src, dst):
        try: s.close()
        except OSError: pass

def handle(client):
    if os.path.exists(FLAG):
        client.recv(65536)
        client.sendall(DENIED)
        client.close()
        return
    upstream = socket.create_connection(("127.0.0.1", 4161))
    threading.Thread(target=pump, args=(client, upstream), daemon=True).start()
    pump(upstream, client)

listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", 4171))
listener.listen()
while True:
    client, _ = listener.accept()
    threading.Thread(target=handle, args=(client,), daemon=True).start()
```

Then run the suite:

```sh
xcodebuild -project ios/Dream.xcodeproj -scheme DreamUITests \
    -destination 'platform=iOS Simulator,name=iPhone 15' test
```

Unit tests (`DreamTests`) need no daemons:
`xcodebuild -project ios/Dream.xcodeproj -scheme Dream -destination 'platform=iOS Simulator,name=iPhone 15' test`.

## Auto-start on macOS (launchd)

`dream up` does this automatically — recommended. Manual recipe for custom setups: the agent must be label `com.dream.daemon` for `dream down`/`status` to manage it; use `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.dream.daemon.plist` (modern verbs — not `load`), with logs at `~/Library/Logs/dream/{out,err}.log`. Under launchd the token is never printed to the log — it lives in the 0600 token file (`<data-dir>/token`), printed once on a TTY first run or reprinted with `dream url`.

## CLI reference

| Flag | Default | Purpose |
|---|---|---|
| `--port <N>` | `4020` | Port to listen on |
| `--bind <ADDR>` | `127.0.0.1` | Bind address; non-loopback requires `--public` |
| `--data-dir <DIR>` | platform data dir | Session state + token location |
| `--auth-token <T>` | generated | Override the bearer token |
| `--github-client-id <ID>` | unset | GitHub OAuth App client id — enables GitHub login (`DREAM_GITHUB_CLIENT_ID`) |
| `--github-client-secret <S>` | unset | GitHub OAuth App client secret (`DREAM_GITHUB_CLIENT_SECRET`) |
| `--github-allowlist <LIST>` | unset | Comma-separated logins / `org:name` allowed in (`DREAM_GITHUB_ALLOWLIST`) |
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
