---
feature: web-terminal
context: app
status: archived
---
# Use Case

## Overview

| Field | Value |
|---|---|
| ID | UC-001 |
| Name | Open dashboard and list sessions |
| Requirement reference | FR-001, FR-007, FR-014 |
| Goal | The owner sees every session and its status from any browser |
| Primary actor | Owner |

## Supporting Actors
- Daemon (serves the UI and session data), Tailscale Serve (HTTPS ingress)

## Preconditions
- Daemon running and bound (default loopback, or tailscale serve URL)
- Owner holds the auth token

## Trigger
Owner opens the dashboard URL in a browser.

## Main Flow

| Step | Actor / system | Action | Outcome |
|---|---|---|---|
| 1 | Owner | Opens dashboard URL | Auth challenge shown if no session cookie |
| 2 | Owner | Enters token/password | Authenticated session cookie issued |
| 3 | Daemon | GET /api/sessions | JSON list of sessions returned |
| 4 | Owner | Sees session cards: name, command, cwd, status, created, attention badge | Dashboard rendered |

## Alternative Flows
### A1
- Trigger: no sessions exist yet

| Step | Action | Outcome / return to main flow |
|---|---|---|
| A1.1 | Dashboard shows empty state with "new session" action | Owner proceeds to UC-002 |

## Exception Flows
### E1

| Trigger | Handling | Resulting state / message |
|---|---|---|
| Invalid/expired token | Reject with 401, show auth form again | No session data disclosed |
| Daemon unreachable | Browser shows connection error | No partial UI |

## Postconditions
- Owner has an authenticated view of the session list

## Business Rules
- Every API response requires auth; nothing is public
- Dead sessions stay listed until deleted (FR-011)

## Data
- session.json: id, name, cmd, cwd, setup_script, status, created_at, log path
- attention flag: bell/idle-derived per session

## Acceptance Criteria
- [ ] Unauthenticated GET / or /api/sessions is rejected
- [ ] After auth, dashboard lists sessions with correct name/cwd/status
- [ ] Attention badge appears on a session that rang bell / went idle-awaiting and clears on attach
