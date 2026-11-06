---
feature: web-terminal
context: app
created: 20261105_1911
status: archived
---
# Test Plan

## Feature Test Summary

| Field | Value |
|---|---|
| Feature | web-terminal |
| Context | app |
| Test level | full |
| UI scope | critical |
| Tools / commands | cargo test; cargo test --test integration; npx playwright test |
| Coverage target | 80% |

## Overall Case Counts

| Test type | Planned | Must pass | Notes |
|---|---:|---:|---|
| Unit | 8 | 8 | pure logic: state, replay, pty, auth, frames, vt |
| Integration | 9 | 9 | HTTP API, WS, respawn, resurrect, attention, dead-attach |
| UI / E2E | 8 | 8 | Playwright critical flows |
| **Total** | **25** | **25** | |

## Use Case Coverage Matrix

| Use case | Requirement(s) | Test cases | Planned | Pass criteria |
|---|---|---|---:|---|
| UC-001 | FR-001, FR-007, FR-014 | TC-001, TC-011, TC-015, TC-017 | 4 | authed list + attention |
| UC-002 | FR-002, FR-003, FR-012, FR-013 | TC-003, TC-004, TC-009, TC-018, TC-020 | 5 | spawn + attach + TUI |
| UC-003 | FR-003, FR-004, FR-010, FR-013 | TC-008, TC-012, TC-016, TC-019, TC-024 | 5 | scrollback+snapshot reattach, multi-client, dead-attach |
| UC-004 | FR-005 | TC-013, TC-021 | 2 | resurrect + marked + screen |
| UC-005 | FR-006, FR-011 | TC-002, TC-014, TC-022, TC-024 | 4 | kill/rename/delete/respawn/dead-attach |
| UC-006 | FR-007, FR-008, FR-009 | TC-010, TC-017, TC-023 | 3 | token, bind, guidance |
| UC-007 | FR-002, FR-005 | TC-004, TC-021 | 2 | script on spawn/resurrect/respawn |
| UC-008 | FR-004, FR-015 | TC-005, TC-006, TC-007, TC-025 | 4 | auto-trim, retention, clean (unit+UI) |

## Requirement Coverage Matrix

| Requirement | Use case(s) | Test case(s) | Covered? | Gap / note |
|---|---|---|---|---|
| FR-001 | UC-001 | TC-001, TC-017 | yes | |
| FR-002 | UC-002, UC-007 | TC-003, TC-004, TC-018 | yes | |
| FR-003 | UC-002, UC-003 | TC-008, TC-012, TC-018, TC-019 | yes | |
| FR-004 | UC-003, UC-008 | TC-005, TC-012, TC-019, TC-024 | yes | |
| FR-005 | UC-004, UC-007 | TC-013, TC-021 | yes | |
| FR-006 | UC-005 | TC-002, TC-014, TC-022 | yes | |
| FR-007 | UC-001, UC-006 | TC-010, TC-011, TC-017 | yes | |
| FR-008 | UC-006 | TC-010, TC-023 | yes | |
| FR-009 | UC-006 | TC-023 | yes | |
| FR-010 | UC-003 | TC-016, TC-019 | yes | |
| FR-011 | UC-005, UC-003 | TC-002, TC-014, TC-022, TC-024 | yes | |
| FR-012 | UC-002 | TC-020 | yes | phone viewport smoke |
| FR-013 | UC-002, UC-003 | TC-009, TC-020 | yes | |
| FR-014 | UC-001 | TC-015, TC-022 | yes | |
| FR-015 | UC-008 | TC-005, TC-006, TC-007, TC-025 | yes | |

## TC-001

| Field | Detail |
|---|---|
| Test case ID | TC-001 |
| Requirement reference | FR-001, FR-006 |
| Use case reference | UC-001, UC-005 |
| Test type | Unit |
| Priority | High |
| Preconditions | temp state dir |
| Input | registry with 3 sessions (1 corrupt session.json) |
| Steps | See steps table below |
| Expected outcome | registry CRUD + startup load + quarantine correct |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Create/save/load sessions | Round-trip preserves all fields |
| 2 | Write corrupt session.json, reload | Entry quarantined, warning logged, others load |

## TC-002

| Field | Detail |
|---|---|
| Test case ID | TC-002 |
| Requirement reference | FR-006, FR-011 |
| Use case reference | UC-005 |
| Test type | Unit |
| Priority | High |
| Preconditions | live session spawned with short-lived command |
| Input | exit; kill; rename; edit script; delete |
| Steps | See steps table below |
| Expected outcome | dead-mark on exit, kill→dead, edit persists, delete removes dir |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Let shell exit | status dead, session still listed |
| 2 | Rename + edit script → save → reload | fields persisted |
| 3 | Delete | state dir removed |

## TC-003

| Field | Detail |
|---|---|
| Test case ID | TC-003 |
| Requirement reference | FR-002, FR-003, FR-011 |
| Use case reference | UC-002 |
| Test type | Unit |
| Priority | High |
| Preconditions | Pty handle over portable-pty |
| Input | spawn, write, resize, exit, kill |
| Steps | See steps table below |
| Expected outcome | spawn in cwd, bidirectional I/O, resize propagated, exit status, kill works |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Spawn `sh -c pwd` in tmp dir | output reports the tmp cwd |
| 2 | Write `echo hi\n` | `hi` in output |
| 3 | Resize to 120x40 | stty reports new size |
| 4 | Exit / kill | status observable |

## TC-004

| Field | Detail |
|---|---|
| Test case ID | TC-004 |
| Requirement reference | FR-002, FR-005 |
| Use case reference | UC-002, UC-007 |
| Test type | Integration |
| Priority | High |
| Preconditions | session with setup_script `export X=1; echo SETUP-DONE` |
| Input | spawn, respawn, resurrect path |
| Steps | See steps table below |
| Expected outcome | script runs at every spawn mode; env applied; failure leaves usable shell |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Spawn session | output contains SETUP-DONE |
| 2 | Simulate resurrect respawn | script runs again |
| 3 | Failing script (`false; echo AFTER`) | shell usable, failure visible |

## TC-005

| Field | Detail |
|---|---|
| Test case ID | TC-005 |
| Requirement reference | FR-004, FR-015 |
| Use case reference | UC-008 |
| Test type | Unit |
| Priority | High |
| Preconditions | replay writer with small cap |
| Input | append past cap; torn tail; clean; retention |
| Steps | See steps table below |
| Expected outcome | auto-trim drops oldest, reader tolerates torn tail, clean/retention work |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Append 2x cap bytes | file stays ≤ cap, newest events kept |
| 2 | Truncate mid-frame, read | tolerant reader stops at torn tail |
| 3 | Clean live log | truncated cleanly, writer continues |

## TC-006

| Field | Detail |
|---|---|
| Test case ID | TC-006 |
| Requirement reference | FR-015 |
| Use case reference | UC-008 |
| Test type | Unit |
| Priority | Medium |
| Preconditions | retention policy size+age configured |
| Input | aged logs, global clean |
| Steps | See steps table below |
| Expected outcome | enforcer trims aged/oversized logs; global clean hits all sessions |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Seed old + oversized logs, run enforcer | trimmed per policy |
| 2 | Global clean | all logs reset |

## TC-007

| Field | Detail |
|---|---|
| Test case ID | TC-007 |
| Requirement reference | FR-004 |
| Use case reference | UC-008 |
| Test type | Integration |
| Priority | Medium |
| Preconditions | live session mid-attach |
| Input | clean log during attach |
| Steps | See steps table below |
| Expected outcome | attach consistent, no torn escape frames, live stream continues |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Attach, then clean log | replay boundary consistent |
| 2 | Reattach | replays only post-clean data |

## TC-008

| Field | Detail |
|---|---|
| Test case ID | TC-008 |
| Requirement reference | FR-003, FR-004, FR-010 |
| Use case reference | UC-003 |
| Test type | Unit |
| Priority | Medium |
| Preconditions | ws frame codec |
| Input | encode/decode i/r/o/s/e/a frames |
| Steps | See steps table below |
| Expected outcome | binary frames round-trip; unknown opcode rejected |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Encode+decode each frame type | round-trip equal |
| 2 | Feed invalid opcode | clean error |

## TC-009

| Field | Detail |
|---|---|
| Test case ID | TC-009 |
| Requirement reference | FR-013 |
| Use case reference | UC-002, UC-003 |
| Test type | Unit |
| Priority | High |
| Preconditions | vt emulator + snapshot generator |
| Input | recorded TUI byte stream (alt-screen enter/exit, colors, wide glyphs) |
| Steps | See steps table below |
| Expected outcome | snapshot reproduces screen state incl alt-screen + cursor |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Feed TUI stream | emulator state matches golden |
| 2 | Generate snapshot → feed to fresh emulator | identical screen |

## TC-010

| Field | Detail |
|---|---|
| Test case ID | TC-010 |
| Requirement reference | FR-007, FR-008 |
| Use case reference | UC-006 |
| Test type | Unit |
| Priority | High |
| Preconditions | fresh data dir |
| Input | first-run token generation |
| Steps | See steps table below |
| Expected outcome | token generated 0600, printed once, reused later |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Start with no token | generated, file mode 0600, URL printed once |
| 2 | Restart | same token loaded, not reprinted |

## TC-011

| Field | Detail |
|---|---|
| Test case ID | TC-011 |
| Requirement reference | FR-007, FR-001 |
| Use case reference | UC-001 |
| Test type | Integration |
| Priority | High |
| Preconditions | daemon running with token |
| Input | unauth/auth HTTP + WS requests |
| Steps | See steps table below |
| Expected outcome | unauth → 401 everywhere; authed API works |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | GET /, /api/sessions, WS upgrade without token | 401 |
| 2 | Same with token | 200/101 + data |

## TC-012

| Field | Detail |
|---|---|
| Test case ID | TC-012 |
| Requirement reference | FR-003, FR-004 |
| Use case reference | UC-003 |
| Test type | Integration |
| Priority | High |
| Preconditions | live session with screen content |
| Input | attach → disconnect → reattach |
| Steps | See steps table below |
| Expected outcome | attach = retained log-tail scrollback → snapshot → live; reattach restores scrollback + exact screen |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Attach, run commands, disconnect | PTY still running |
| 2 | Reattach | scrollback tail replays, then snapshot matches pre-disconnect screen |

## TC-013

| Field | Detail |
|---|---|
| Test case ID | TC-013 |
| Requirement reference | FR-005 |
| Use case reference | UC-004 |
| Test type | Integration |
| Priority | High |
| Preconditions | daemon + sessions; kill daemon |
| Input | restart daemon |
| Steps | See steps table below |
| Expected outcome | sessions resurrected, marked, screen restored, interactive |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Create sessions with output; stop daemon | state persisted |
| 2 | Start daemon | sessions live, marked resurrected, snapshot restores screen |

## TC-014

| Field | Detail |
|---|---|
| Test case ID | TC-014 |
| Requirement reference | FR-006 |
| Use case reference | UC-005 |
| Test type | Integration |
| Priority | Medium |
| Preconditions | authed client + session |
| Input | kill / rename / delete / respawn API calls |
| Steps | See steps table below |
| Expected outcome | each mutation reflected in API + on disk |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | POST kill | dead status |
| 2 | PATCH name/cwd/script | persisted |
| 3 | POST respawn | live again |
| 4 | DELETE | session + files gone |

## TC-015

| Field | Detail |
|---|---|
| Test case ID | TC-015 |
| Requirement reference | FR-014 |
| Use case reference | UC-001 |
| Test type | Integration |
| Priority | Medium |
| Preconditions | live session |
| Input | BEL char output; idle period ≥ 30 s with no output while process alive |
| Steps | See steps table below |
| Expected outcome | attention flag set on bell or ≥30 s output-idle; cleared on attach |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Emit `\a` in session | attention flag true via API |
| 2 | Leave session output-idle ≥ 30 s | attention flag true via API |
| 3 | Attach | flag cleared |

## TC-016

| Field | Detail |
|---|---|
| Test case ID | TC-016 |
| Requirement reference | FR-010 |
| Use case reference | UC-003 |
| Test type | Integration |
| Priority | Low |
| Preconditions | live session |
| Input | two concurrent WS attaches |
| Steps | See steps table below |
| Expected outcome | both get snapshot+live; both can input |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Attach client A and B | identical streams |

## TC-017

| Field | Detail |
|---|---|
| Test case ID | TC-017 |
| Requirement reference | FR-001, FR-007 |
| Use case reference | UC-001, UC-006 |
| Test type | E2E |
| Priority | High |
| Preconditions | built binary, Playwright |
| Input | browser flows |
| Steps | See steps table below |
| Expected outcome | auth gate + dashboard list works in real browser |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Open URL without auth | auth gate shown, no data |
| 2 | Authenticate | dashboard lists sessions |

## TC-018

| Field | Detail |
|---|---|
| Test case ID | TC-018 |
| Requirement reference | FR-002, FR-003 |
| Use case reference | UC-002 |
| Test type | E2E |
| Priority | High |
| Preconditions | authed browser |
| Input | create session, type `echo hi`, resize |
| Steps | See steps table below |
| Expected outcome | terminal interactive in browser; resize propagates |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Click new session | session created + attached |
| 2 | Type `echo hi` | `hi` visible in terminal |
| 3 | Resize viewport | `stty size` reflects new cols |

## TC-019

| Field | Detail |
|---|---|
| Test case ID | TC-019 |
| Requirement reference | FR-004, FR-010 |
| Use case reference | UC-003 |
| Test type | E2E |
| Priority | High |
| Preconditions | session running `sleep` + visible output |
| Input | reload page; second context attach |
| Steps | See steps table below |
| Expected outcome | reattach shows retained scrollback + identical screen; second client mirrors |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Run commands, reload tab | scrollback + screen restored |
| 2 | Second browser context attach | identical render |

## TC-020

| Field | Detail |
|---|---|
| Test case ID | TC-020 |
| Requirement reference | FR-012, FR-013 |
| Use case reference | UC-002 |
| Test type | E2E |
| Priority | High |
| Preconditions | session attached |
| Input | `vim` (or htop) fullscreen; phone viewport |
| Steps | See steps table below |
| Expected outcome | TUI renders fullscreen correctly, survives reattach; usable on phone viewport |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Launch vim, type, check render | alt-screen correct |
| 2 | Reload → reattach | TUI screen restored |
| 3 | iPhone viewport smoke | terminal fits, input works |

## TC-021

| Field | Detail |
|---|---|
| Test case ID | TC-021 |
| Requirement reference | FR-005 |
| Use case reference | UC-004, UC-007 |
| Test type | E2E |
| Priority | High |
| Preconditions | session with setup script + screen content |
| Input | restart daemon process |
| Steps | See steps table below |
| Expected outcome | session resurrected marked, screen restored, script re-ran |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Stop daemon | — |
| 2 | Start daemon, open dashboard | session marked resurrected |
| 3 | Attach | restored screen + fresh shell works |

## TC-022

| Field | Detail |
|---|---|
| Test case ID | TC-022 |
| Requirement reference | FR-006, FR-011, FR-014 |
| Use case reference | UC-005, UC-001 |
| Test type | E2E |
| Priority | Medium |
| Preconditions | authed dashboard + session |
| Input | kill, rename, delete, attention badge |
| Steps | See steps table below |
| Expected outcome | lifecycle actions work via UI; badge shows |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Kill via UI | dead state shown, stays listed |
| 2 | Rename + delete via UI | reflected immediately; session gone |
| 3 | Trigger bell → dashboard | attention badge visible |

## TC-023

| Field | Detail |
|---|---|
| Test case ID | TC-023 |
| Requirement reference | FR-008, FR-009 |
| Use case reference | UC-006 |
| Test type | E2E |
| Priority | Medium |
| Preconditions | fresh machine state |
| Input | run binary first time; tailscale absent scenario |
| Steps | See steps table below |
| Expected outcome | printed guidance; single self-contained binary |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Run `webterm` fresh | token URL + tailscale serve recipe printed |
| 2 | Simulate no tailscale | install guidance printed |
| 3 | Run `webterm --public` | loud security warning printed before bind |
| 4 | `otool -L` + sibling-dir check | only system libs, no asset dir needed |
| 5 | `tailscale serve <port>` on a tailnet device (manual check, note result) | dashboard loads over HTTPS on second device |

## TC-024

| Field | Detail |
|---|---|
| Test case ID | TC-024 |
| Requirement reference | FR-011, FR-004 |
| Use case reference | UC-003, UC-005 |
| Test type | Integration |
| Priority | High |
| Preconditions | session whose shell has exited (dead) with retained log |
| Input | WS attach to dead session |
| Steps | See steps table below |
| Expected outcome | client receives retained screen replay + dead notice + respawn offer |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Kill session shell, attach via WS | retained screen replayed, session-ended/dead notice delivered |
| 2 | Send respawn request | fresh shell spawns, setup script re-runs |

## TC-025

| Field | Detail |
|---|---|
| Test case ID | TC-025 |
| Requirement reference | FR-015 |
| Use case reference | UC-008 |
| Test type | E2E |
| Priority | Medium |
| Preconditions | authed dashboard + session with bloated log |
| Input | clean action via dashboard UI |
| Steps | See steps table below |
| Expected outcome | log shrinks; live terminal keeps working; reattach consistent |
| Status | PENDING |

### Steps

| Step | Action | Expected result |
|---|---|---|
| 1 | Attach session, click clean in dashboard | log truncated; terminal still interactive |
| 2 | Reload → reattach | consistent screen, no torn output |
