---
feature: web-terminal
context: app
status: archived
---
# Use Case

## Overview

| Field | Value |
|---|---|
| ID | UC-004 |
| Name | Restart daemon, resurrect sessions |
| Requirement reference | FR-005 |
| Goal | Daemon restart restores session identities and screens |
| Primary actor | Daemon |

## Supporting Actors
- Owner (observes the resurrected dashboard)

## Preconditions
- Sessions persisted on disk (session.json + replay log) from before restart

## Trigger
Daemon process restarts (crash, update, reboot).

## Main Flow

| Step | Actor / system | Action | Outcome |
|---|---|---|---|
| 1 | Daemon | Startup: scan state dir, load session.json entries | Registry rebuilt |
| 2 | Daemon | For each session: spawn fresh shell in recorded cwd, run setup script | New PTY bound |
| 3 | Daemon | Replay log into fresh VT emulator | Screen state restored |
| 4 | Owner | Opens dashboard | Sessions listed, marked "resurrected" |
| 5 | Owner | Attaches | Snapshot shows restored screen; new shell interactive |

## Alternative Flows
### A1
- Trigger: resurrect deferred until first attach

| Step | Action | Outcome / return to main flow |
|---|---|---|
| A1.1 | Session listed as preserved; PTY spawned on attach | Same end state, lazier |

## Exception Flows
### E1

| Trigger | Handling | Resulting state / message |
|---|---|---|
| Corrupt/missing session.json | Quarantine entry, log warning | Other sessions still resurrect |
| Missing/truncated log | Resurrect with empty/partial screen | Session still usable |
| Old processes | Gone by design | Marked resurrected so owner knows |

## Postconditions
- All intact sessions live again, clearly marked

## Business Rules
- Processes do not survive restart — screen state and identity do
- Setup script re-runs on resurrect (documented)

## Data
- session.json, replay log, resurrected flag

## Acceptance Criteria
- [ ] After restart, prior sessions reappear marked resurrected
- [ ] Attached session shows restored screen and accepts input in fresh shell
- [ ] A corrupt session.json does not block other resurrections
