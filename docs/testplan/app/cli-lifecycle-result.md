---
feature: "cli-lifecycle"
context: "app"
tested: "20261105_2323"
execution: "ef101537-3816-4f69-9f27-69c1112ca9a5"
status: PASS
---

# Testing Result — cli-lifecycle

## Environment
- OS: macOS (Apple Silicon), rustc rustup 1.92 + llvm-cov
- Runtime: cargo test, playwright 1.64.1 chromium

## Commands and Evidence

| Command | Exit code | Output summary |
|---|---|---|
| `cargo test` | 0 | 85 passed, 0 failed |
| `cargo clippy --all-targets -- -D warnings` | 0 | clean, 0 warnings |
| `npm test` | 0 | playwright 14 passed, 0 failed |
| `cargo build --release` | 0 | target/release/dream v1.0.0 |
| `dream up --port 4030 --data-dir /tmp/dream-svc` | 0 | agent bootstrapped, token URL printed |
| `curl http://127.0.0.1:4030/` | 0 | HTTP 200 dashboard |
| `dream status` | 0 | "state = running" + URL |
| `dream ps` | 0 | session table rendered |
| `dream doctor` | 0 | 5 checks ✓, "0 problem(s)" |
| `dream restart` | 0 | kickstarted, port 200 again |
| `dream logs` | 0 | launchd stdout shown |
| `dream down` | 0 | bootout ok, plist removed, port refused |

## Per-TC results

| TC | Result | Evidence |
|---|---|---|
| TC-001 cli parse | PASS | manual `dream up/down/status/restart/logs/url/ps/doctor` invocations all parsed; `dream up --port 4030` works (global flags after subcommand) |
| TC-002 plist xml | PASS | `daemon::tests::plist_contains_label_args_and_keepalive`, `plist_escapes_xml_chars`, `service_uses_domain_service_single_arg` |
| TC-003 down-missing-agent | PASS | `dream down` → "launchctl bootout: ... (agent may not be running) / dream is down", exit 0 |
| TC-004 url-from-token | PASS | `dream url` prints `http://127.0.0.1:4030/?token=b54f…` from /tmp/dream-svc/token |
| TC-005 ps via API | PASS | `dream ps --port 4030 --data-dir /tmp/dream-svc` rendered `3c937b2d test-sess Live sh` row |
| TC-006 doctor checklist | PASS | `dream doctor` → ✓ plist ✓ agent running ✓ port 4030 ✓ token mode 600 ✓ tailscale → "0 problem(s)" |
| TC-007 rel-time fmt | PASS | cards show "0s ago"; exercised in dashboard polish screenshot |
| TC-008 full E2E | PASS | 14/14 playwright after pinning short-id contract + tagged stty output |
| TC-009 lifecycle cycle | PASS | real: `dream up` bootstrapped agent (token URL printed); dashboard curl 200; `restart` → 200 again; `logs` showed launchd stdout; `down` → port refused; resurrect proven earlier |

## Behaviour notes
- Bug found and fixed during manual verify: `launchctl bootout` requires single `gui/<uid>/<label>` arg (2-arg form fails I/O silently). `service()` now returns the single-arg form; unit test pins it.
- Tailscale detection now probes well-known paths (launchd minimal PATH reported "not found" on a machine with tailscale installed).
- UI polish verified via screenshot: meta rows (CMD/CWD/ID/CREATED), short-id+copy, relative time, delete right-aligned, brand `dream`.
