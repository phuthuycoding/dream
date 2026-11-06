---
feature: web-terminal
context: app
status: archived
---
# Use Case

## Overview

| Field | Value |
|---|---|
| ID | UC-006 |
| Name | First run: bind, auth and Tailscale guidance |
| Requirement reference | FR-007, FR-008, FR-009 |
| Goal | One binary, one command, secure-by-default reachable URL |
| Primary actor | Owner |

## Supporting Actors
- Tailscale (detection + serve guidance)

## Preconditions
- The `webterm` binary on a machine; no prior data dir

## Trigger
Owner runs `webterm` (or `webterm --install`) for the first time.

## Main Flow

| Step | Actor / system | Action | Outcome |
|---|---|---|---|
| 1 | Owner | Runs `webterm` | Config defaults: loopback bind, data dir created |
| 2 | Daemon | No credential configured → generate random token | Token stored mode-0600, URL printed once |
| 3 | Daemon | Detect tailscale | If present: print `tailscale serve <port>` recipe |
| 4 | Owner | Opens printed URL | Dashboard with auth |

## Alternative Flows
### A1
- Trigger: tailscale not installed

| Step | Action | Outcome / return to main flow |
|---|---|---|
| A1.1 | Print install hint + serve recipe | Owner installs tailscale, runs serve |

### A2
- Trigger: `--public` passed

| Step | Action | Outcome / return to main flow |
|---|---|---|
| A2.1 | Bind non-loopback + loud warning | Explicit opt-in public exposure |

## Exception Flows
### E1

| Trigger | Handling | Resulting state / message |
|---|---|---|
| Port in use | Clear startup error naming the port | No silent fallback |
| Data dir unwritable | Startup error | No half-initialized state |

## Postconditions
- Daemon running, authenticated URL known, tailscale recipe in hand

## Business Rules
- Never starts without a credential (generated if none given)
- Token printed exactly once at first run

## Data
- auth token file (0600), config, data dir

## Acceptance Criteria
- [ ] First run prints an authenticated URL and the `tailscale serve` recipe
- [ ] Machine without tailscale gets install guidance
- [ ] Token file has 0600 perms; subsequent runs don't reprint the token
- [ ] `--public` bind prints a security warning
