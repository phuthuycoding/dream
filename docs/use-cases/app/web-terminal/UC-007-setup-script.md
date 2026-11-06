---
feature: web-terminal
context: app
status: archived
---
# Use Case

## Overview

| Field | Value |
|---|---|
| ID | UC-007 |
| Name | Setup script on spawn, resurrect, respawn |
| Requirement reference | FR-002, FR-005 |
| Goal | Every shell start runs the session's owner-defined setup |
| Primary actor | Daemon |

## Supporting Actors
- Owner (writes the setup script at create/edit time)

## Preconditions
- Session with a setup script configured

## Trigger
Session shell spawns — at create, daemon resurrect, or dead-session respawn.

## Main Flow

| Step | Actor / system | Action | Outcome |
|---|---|---|---|
| 1 | Daemon | Spawns shell in recorded cwd | PTY created |
| 2 | Daemon | Injects setup script into the shell (prelude) | Commands run (cd, env, launch agent CLI) |
| 3 | Owner | Attaches | Sees script output/result; interactive shell or agent ready |

## Alternative Flows
### A1
- Trigger: no setup script

| Step | Action | Outcome / return to main flow |
|---|---|---|
| A1.1 | Plain shell at cwd | Standard session |

## Exception Flows
### E1

| Trigger | Handling | Resulting state / message |
|---|---|---|
| Script exits shell (e.g. `exec cmd` then cmd exits) | Session → dead, respawnable | Dead state |
| Script fails mid-way | Output shows failure; shell remains | Owner sees error in scrollback |
| Script hangs | Shell occupied; kill/respawn available | Recoverable |

## Postconditions
- Session environment prepared per script

## Business Rules
- Script stored in session.json; editable via UC-005; re-runs on every respawn/resurrect

## Data
- setup_script field in session.json

## Acceptance Criteria
- [ ] Create with script → script output visible on attach
- [ ] Resurrect and respawn re-run the script
- [ ] Failing script leaves a usable shell with visible error
