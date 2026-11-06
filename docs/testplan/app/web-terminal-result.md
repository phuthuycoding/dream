---
status: PASS
execution: ecdd079e-ffaa-4b77-9a77-a7fba0ae2244
---

# Phase 4 — Testing result: web-terminal (re-run after repair round 2)

Follows Phase-5 re-review FAIL (execution 86d62359) and repair commits `57cd317` (trim hysteresis + windowed cut), `8a77e90` (pump-write gate, pre-insert runtime, env strip, ws-only `?token=`), plus docs commit (FR-007/AC + README static-shell exemption — contract re-approved by human after amendment).

## Commands and Evidence

| Command | Exit code | Result |
|---|---|---|
| `cargo test` | 0 | 76 passed / 0 failed |
| `cargo clippy --all-targets -- -D warnings` | 0 | clean |
| `npx playwright test` | 0 | 14 passed / 0 failed in 6.5s |
| `cargo build --release` | 0 | release binary up to date post-repair |

## Test-case results

25/25 TCs green at HEAD (per-TC table identical to prior executions — all still pass; +1 new replay unit test from the hysteresis fix, 76 total).

Post-repair deltas verified by suite + diff:
- Append-time trim amortized: trigger at cap, trim to `cap − cap/4`, windowed cut via `read_tail_bytes` — no more O(cap) rewrite per chunk.
- Superseded runtime's pump can no longer write file output/`x` into the fresh log (`appends_enabled` gate).
- Runtimes inserted into the shared map before pump spawn (clean/enforcer see spawn-in-flight sessions).
- `WEBTERM_AUTH_TOKEN` stripped from pty children; `?token=` honored only on `/api/sessions/{id}/ws`.
- FR-007/AC + README now describe the public static shell / gated API+WS split; contract re-approved.

## Behaviour run

Same verified behaviour as prior executions (static shell public, API Bearer-gated, setup-script marker, `--bind` refusal + cleartext warning, tailscale guidance, first-run token URL) — unchanged by repair-2.

## Regression

None — full suite green post-repair; Playwright attach/dead/reattach/resurrect paths unaffected by the two deltas.

## Conclusion

PASS — 76 unit/integration + 14 E2E green at HEAD, coverage ≈80% (llvm-cov measured 80.09% lines earlier this cycle). Ready for scoped re-review on the repair-2 diff.
