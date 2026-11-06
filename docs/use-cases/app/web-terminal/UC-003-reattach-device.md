---
feature: web-terminal
context: app
status: archived
---
# Use Case

## Overview

| Field | Value |
|---|---|
| ID | UC-003 |
| Name | Close browser, reattach from another device |
| Requirement reference | FR-003, FR-004, FR-010, FR-013 |
| Goal | The session outlives the browser; another device resumes it exactly |
| Primary actor | Owner |

## Supporting Actors
- Daemon (PTY ownership, VT snapshot)

## Preconditions
- Live session running (e.g. an agent CLI or `sleep`/build)

## Trigger
Owner closes the tab/browser, then opens the dashboard on another device and attaches.

## Main Flow

| Step | Actor / system | Action | Outcome |
|---|---|---|---|
| 1 | Owner | Closes browser | WS disconnects; PTY keeps running; output keeps logging |
| 2 | Owner | Opens dashboard on device 2, attaches | Server sends VT snapshot → screen restored incl. alt-screen state |
| 3 | Owner | Continues typing | Live stream resumes seamlessly |

## Alternative Flows
### A1
- Trigger: two clients attach simultaneously

| Step | Action | Outcome / return to main flow |
|---|---|---|
| A1.1 | Both receive snapshot + live stream | Both render; input is last-writer |

## Exception Flows
### E1

| Trigger | Handling | Resulting state / message |
|---|---|---|
| Attach to dead session | Replay retained screen, offer respawn | Dead session view |
| Session deleted between visits | List shows it gone | Clear empty state |

## Postconditions
- Owner resumes work with full screen state, including TUI scrollback

## Business Rules
- Detach never kills; only explicit kill/delete ends a session
- Snapshot must restore alt-screen TUIs, not just line output

## Data
- Live VT emulator state; replay log retained window

## Acceptance Criteria
- [ ] With `sleep 600` running, reattach on a second device shows the same session still running
- [ ] Reattached screen matches what was visible (snapshot, incl. TUI apps)
- [ ] Second simultaneous client sees identical output
