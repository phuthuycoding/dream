---
feature: web-terminal
context: app
status: archived
---
# Use Case

## Overview

| Field | Value |
|---|---|
| ID | UC-005 |
| Name | Kill, rename/edit and delete sessions |
| Requirement reference | FR-006, FR-011 |
| Goal | Full session lifecycle control from the dashboard |
| Primary actor | Owner |

## Supporting Actors
- Daemon (PTY kill, state mutation)

## Preconditions
- At least one session exists

## Trigger
Owner uses the session's controls on the dashboard.

## Main Flow

| Step | Actor / system | Action | Outcome |
|---|---|---|---|
| 1 | Owner | Kill on a live session | PTY terminated; status → dead; session stays listed |
| 2 | Owner | Edit name/cwd/setup_script | session.json updated; live shell unaffected until respawn |
| 3 | Owner | Delete on any session | PTY killed if live; state dir removed; session gone from list |
| 4 | Owner | Respawn on a dead session | Fresh shell in cwd re-runs setup script; status → live |

## Alternative Flows
### A1
- Trigger: shell exits on its own

| Step | Action | Outcome / return to main flow |
|---|---|---|
| A1.1 | Daemon detects exit, marks dead | Same dead-state behaviour as kill |

## Exception Flows
### E1

| Trigger | Handling | Resulting state / message |
|---|---|---|
| Kill/delete while clients attached | Clients get session-ended notice | Clean disconnect |
| Name collision | Auto-suffix or reject (documented) | No duplicate display names |
| Delete while log being written | Orderly teardown, no partial dir | Consistent state |

## Postconditions
- Registry and disk state match the dashboard

## Business Rules
- Kill ≠ delete: kill preserves session + screen; delete removes everything
- Edit applies to metadata; running shell unaffected until respawn

## Data
- session.json mutations; state dir lifecycle

## Acceptance Criteria
- [ ] Kill marks session dead and it stays listed
- [ ] Rename + edited cwd/setup script persist across daemon restart
- [ ] Delete removes the session and its files entirely
- [ ] Respawn on a dead session produces a working shell
