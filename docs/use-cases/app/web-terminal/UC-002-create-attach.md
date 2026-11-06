---
feature: web-terminal
context: app
status: archived
---
# Use Case

## Overview

| Field | Value |
|---|---|
| ID | UC-002 |
| Name | Create session and attach |
| Requirement reference | FR-002, FR-003, FR-012, FR-013 |
| Goal | Owner creates a terminal session and drives it interactively |
| Primary actor | Owner |

## Supporting Actors
- Daemon (PTY spawn, WS attach), headless VT emulator

## Preconditions
- Authenticated on dashboard (UC-001)

## Trigger
Owner clicks "new session" and confirms (optionally name, cwd, setup script).

## Main Flow

| Step | Actor / system | Action | Outcome |
|---|---|---|---|
| 1 | Owner | POST /api/sessions {name?, cwd?, setup_script?} | Session created; PTY spawned in cwd; setup script runs |
| 2 | Owner | Click session → attach | WS opens; server sends snapshot then live stream |
| 3 | Owner | Types input | Keystrokes forwarded to PTY; output renders |
| 4 | Owner | Resizes browser | RESIZE frame → PTY winsize updated; TUI reflows |

## Alternative Flows
### A1
- Trigger: session created without a setup script

| Step | Action | Outcome / return to main flow |
|---|---|---|
| A1.1 | Shell spawns at cwd directly | Normal interactive shell |

## Exception Flows
### E1

| Trigger | Handling | Resulting state / message |
|---|---|---|
| Setup script fails or hangs | Shell still usable; failure visible in output | Session live, flagged output |
| cwd does not exist | Reject create with clear error, or fall back to $HOME (documented) | No session or session in $HOME |
| WS drops mid-session | PTY unaffected; client auto-reconnects | Session lives server-side |

## Postconditions
- Session live, attached, persisted in registry

## Business Rules
- One session = one PTY at a time
- TUI apps must render faithfully (alt-screen, 256 colour, wide glyphs)

## Data
- session.json metadata; replay log stream file

## Acceptance Criteria
- [ ] Created session appears in list and attaches to a working shell
- [ ] `echo hi` produces output; `stty size` tracks browser resize
- [ ] An alt-screen TUI (vim/htop) renders and accepts input correctly
- [ ] Setup script output is visible in the session on attach
