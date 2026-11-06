# Test cases — ui-v2

## TC-001 token stylesheet applied
| Field | Value |
|---|---|
| Requirement | FR-001 |
| Use case | UC-001 |
| Test type | E2E |
| Purpose | home, gate and terminal render under the token-driven stylesheet; no unstyled/broken views; radius/border tokens present on surfaces, no shadows |

## TC-002 home list rows + empty state
| Field | Value |
|---|---|
| Requirement | FR-003, FR-004 |
| Use case | UC-001 |
| Test type | E2E |
| Purpose | empty home shows the guided `#dashboard-empty` CTA; rows render as `.session-card[data-session-id]` with `.session-name`, `.badge`, `.session-meta` incl. last-active |

## TC-003 create + attach from home
| Field | Value |
|---|---|
| Requirement | FR-003, FR-004 |
| Use case | UC-001 |
| Test type | E2E |
| Purpose | `#btn-new` page-head action opens `#modal`; create attaches the new session (route `#/s/<id>`, `#view-terminal` visible) |

## TC-004 tab switch preserves state
| Field | Value |
|---|---|
| Requirement | FR-002 |
| Use case | UC-002 |
| Test type | E2E |
| Purpose | two attached sessions → two tabs; click switches without leaving `#view-terminal`; returning to tab A shows its screen intact (xterm/`__ws` content, no replay) |

## TC-005 tab detach never kills
| Field | Value |
|---|---|
| Requirement | FR-002 |
| Use case | UC-003 |
| Test type | E2E |
| Purpose | close control and middle-click remove the tab; `/api/sessions` still lists the session live; closing the active tab activates a neighbour; last tab → home |

## TC-006 "+" tab and persistence
| Field | Value |
|---|---|
| Requirement | FR-002 |
| Use case | UC-002, UC-003 |
| Test type | E2E |
| Purpose | "+" navigates home; open-tab list survives `page.reload()` with the active tab attached and the rest restored lazily |

## TC-007 attention on background tab
| Field | Value |
|---|---|
| Requirement | FR-002, FR-004 |
| Use case | UC-004 |
| Test type | E2E |
| Purpose | bell/`a` frame on a background tab shows its attention marker; activating the tab clears it; home row `.attention-dot` unchanged |

## TC-008 existing selectors regression
| Field | Value |
|---|---|
| Requirement | FR-004 |
| Use case | UC-001, UC-004 |
| Test type | E2E |
| Purpose | all 14 existing playwright specs pass unmodified — gate, dashboard lifecycle, attach, reattach, resurrect, multiclient, mobile softkeys, `#term-overlay`, modal |

## TC-009 reduced motion + tablist a11y
| Field | Value |
|---|---|
| Requirement | FR-005 |
| Use case | UC-005 |
| Test type | E2E |
| Purpose | `reducedMotion: 'reduce'` emulation removes transitions/animations; strip exposes `tablist`/`tab`/`aria-selected`; arrow keys move focus; icon-only controls labelled |

## TC-010 mobile softkeys with tabs
| Field | Value |
|---|---|
| Requirement | FR-004, FR-005 |
| Use case | UC-004 |
| Test type | E2E |
| Purpose | small-viewport attach keeps `#softkeys` visible/usable and the tab strip scrollable; softkey taps still emit the right input frames |

## TC-012 live-instance LRU cap
| Field | Value |
|---|---|
| Requirement | FR-002 |
| Use case | UC-002 |
| Test type | E2E |
| Purpose | 7 open tabs evict the least-recently-used background terminal; the evicted tab stays in the strip and re-attaches (replays) on next activation; the active tab is never evicted |

## TC-011 backend contract unchanged
| Field | Value |
|---|---|
| Requirement | FR-004 |
| Use case | UC-002 |
| Test type | Integration |
| Purpose | `cargo test` green; REST responses and WS frame protocol identical — proves the change is frontend-only |
