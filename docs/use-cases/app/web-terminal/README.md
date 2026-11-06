---
feature: web-terminal
context: app
created: 20261105_1911
status: archived
---
# Use Case Index

Every use case is its own file under `use-cases/`, named `UC-###-<slug>.md`. Do not write a combined narrative here.

## Use Case Files

| ID | Name | File | Primary Actor | Status |
|---|---|---|---|---|
| UC-001 | Open dashboard and list sessions | [UC-001](UC-001-open-dashboard.md) | Owner | implemented |
| UC-002 | Create session and attach | [UC-002](UC-002-create-attach.md) | Owner | implemented |
| UC-003 | Close browser, reattach from another device | [UC-003](UC-003-reattach-device.md) | Owner | implemented |
| UC-004 | Restart daemon, resurrect sessions | [UC-004](UC-004-resurrect.md) | Daemon | implemented |
| UC-005 | Kill, rename/edit and delete sessions | [UC-005](UC-005-manage-sessions.md) | Owner | implemented |
| UC-006 | First run: bind, auth and Tailscale guidance | [UC-006](UC-006-first-run.md) | Owner | implemented |
| UC-007 | Setup script on spawn, resurrect, respawn | [UC-007](UC-007-setup-script.md) | Daemon | implemented |
| UC-008 | Clean replay logs manually and via policy | [UC-008](UC-008-clean-logs.md) | Owner | implemented |

## Use Case Coverage

| UC ID | FR references | TC references | Acceptance coverage |
|---|---|---|---|
| UC-001 | FR-001, FR-007, FR-014 | TC-001, TC-011, TC-015, TC-017 | dashboard lists sessions authed; attention flag shows |
| UC-002 | FR-002, FR-003, FR-012, FR-013 | TC-003, TC-004, TC-009, TC-018, TC-020 | session spawns, attach interactive, TUI renders |
| UC-003 | FR-003, FR-004, FR-010, FR-013 | TC-008, TC-012, TC-016, TC-019, TC-024 | reattach restores scrollback+screen; second device mirrors; dead-attach replays |
| UC-004 | FR-005 | TC-013, TC-021 | restart resurrects marked sessions with screen restored |
| UC-005 | FR-006, FR-011 | TC-002, TC-014, TC-022, TC-024 | kill→dead, rename/edit persists, delete removes state |
| UC-006 | FR-007, FR-008, FR-009 | TC-010, TC-017, TC-023 | first-run token, loopback bind, guidance printed |
| UC-007 | FR-002, FR-005 | TC-004, TC-021 | setup script runs on spawn, resurrect, respawn |
| UC-008 | FR-004, FR-015 | TC-005, TC-006, TC-007, TC-025 | auto-trim on append, retention, manual clean |

## Totals

| Metric | Total |
|---|---:|
| Use cases | 8 |
| Actors | 2 (Owner, Daemon) |
| Functional requirements covered | 15 |
| Test cases linked | 25 |
