---
feature: web-terminal
context: app
status: archived
---
# Use Case

## Overview

| Field | Value |
|---|---|
| ID | UC-008 |
| Name | Clean replay logs manually and via policy |
| Requirement reference | FR-004, FR-015 |
| Goal | Logs stay bounded without breaking live sessions |
| Primary actor | Owner |

## Supporting Actors
- Daemon (auto-trim, retention enforcer)

## Preconditions
- Sessions with accumulated replay logs

## Trigger
Owner clicks clean (per-session or global), or the daemon's retention policy fires.

## Main Flow

| Step | Actor / system | Action | Outcome |
|---|---|---|---|
| 1 | Daemon | On append, log exceeds size cap | Oldest events dropped continuously; newest kept |
| 2 | Daemon | Retention check (size and/or age) | Over-limit logs trimmed in background |
| 3 | Owner | Clean a session's log | Log truncated to fresh start; PTY unaffected |
| 4 | Owner | Global clean | All session logs cleaned |

## Alternative Flows
### A1
- Trigger: clean while a client is attached

| Step | Action | Outcome / return to main flow |
|---|---|---|
| A1.1 | Live stream continues; next attach replays only post-clean data | No torn frames |

## Exception Flows
### E1

| Trigger | Handling | Resulting state / message |
|---|---|---|
| Clean during heavy output | Atomic rotate/trim; PTY keeps writing | No lost live stream |
| Log file missing on clean | No-op with notice | Consistent state |
| Cap smaller than a single frame | Documented minimum floor enforced | No zero-byte loop |

## Postconditions
- Logs bounded; sessions still functional and attachable

## Business Rules
- Auto-trim keeps newest output — scrollback is the retained window, not full history
- Clean never kills or pauses the PTY

## Data
- replay log files; retention config

## Acceptance Criteria
- [ ] A spammy session's log stays under the cap without intervention
- [ ] Manual clean shrinks the log while the terminal keeps working
- [ ] Reattach after clean replays consistently (no torn escape frames)
