---
feature: "ui-v2"
execution: "26082b1b-4a98-433e-b5cc-98533a682e9d"
status: PASS
---
# Testing result — ui-v2 (execution: current)

**PASS.** Run on the implemented sidebar build, 2026-11-06.

## E2E (playwright) — 20/20 green

| Spec | Tests | Result |
|---|---|---|
| existing suite (attach, dashboard, firstrun, mobile, multiclient, reattach, resurrect) | 14 | pass, unmodified — FR-004 selector contract held |
| ui-v2.spec.ts (new) | 6 | pass |

ui-v2 spec covers: sidebar list + guided empty state; click-to-attach + in-place switch with zero snapshot replay (`s`-frame count unchanged across switch); back → `#/` with sessions live; bell → sidebar `.attention-dot`; reduced-motion collapses transitions; LRU cap — 7th attach evicts oldest instance (6 panes), re-attach on reselect.

helpers.ts: 2-line change only (`.xterm:visible`) — required for strict-mode clicks with multiple keep-alive panes.

## Backend

- `cargo test`: 85 passed, 0 failed
- `cargo clippy --all-targets`: clean
- `cargo build`: clean — frontend is embedded via rust-embed

## Manual verify

Sidebar mock + real daemon at :4173 — list renders, drawer on mobile, switch instant, icon.svg served 200.

## Commands and Evidence

| Command | Exit code | Result |
|---|---|---|
| `npx playwright test` | 0 | 20/20 passed (14 existing unmodified + 6 ui-v2) |
| `cargo test` | 0 | 85 passed, 0 failed |
| `cargo clippy --all-targets` | 0 | clean |
| `cargo build` | 0 | clean (web assets embedded) |
