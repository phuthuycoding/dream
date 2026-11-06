---
feature: "ui-v2"
context: "app"
created: "20261105_2322"
kind: feature
status: confirmed
---

# Spec Requirement

## Feature
ui-v2

## Objective
Restyle the web frontend as a flat, professional, master-detail UI (VS Code/Linear-class polish) — a persistent left sidebar carrying the full session list (status dot, name, mono cmd·cwd, attention dot, dim row actions) always visible when authed, with the main area swapping between a home panel and the attached terminal; clicking a session switches in place. Sharp corners throughout (no border radius), zero shadows — surfaces separate by tone and hairline borders only. Vanilla ES modules, no build step, no backend changes, every existing e2e selector stays resolvable.

## Problem Statement
The dashboard is a bare card grid under a utilitarian toolbar: no brand presence, no guidance on first run, and cramped hard-bordered cards. The terminal view is strictly single-session — switching between attached sessions means going back to the dashboard and losing visual track of what is open; there is no glanceable "which sessions am I attached to" state. The overall look is generic dark-admin rather than the flat, professional, list-first design the owner wants (Linear/GitHub-class polish: full-width rows, hairline borders, no shadows).

## Scope
### In Scope
- Design-token system in `web/style.css`: calm dark palette plus spacing/radius/typography tokens; every view restyled from tokens (hairline borders, soft rounded surfaces, zero shadows)
- Persistent sessions sidebar (replaces the separate dashboard view): every session listed with status dot, name, mono cmd·cwd, attention marker and dim row actions (identical labels); the active route's item is highlighted; clicking an item attaches/switches in place; on small screens it collapses to a drawer (`#btn-side` toggle + backdrop)
- Multi-attach client state: a lazily created xterm + WebSocket kept alive per open tab (at most 6 live instances — LRU-evicted tabs stay in the strip and re-attach on next activation) so switching preserves screen state without a replay; a session poll refreshes tab status dots and labels
- Home: `#/` shows a welcome panel in the main area ("pick a session") while the sidebar carries the real list — page head (`Sessions` + count, quick actions, primary `#btn-new`), compact items (name, badge, `cmd · cwd`, created), guided `#dashboard-empty` state for first-time users
- Persisted open-tabs list (localStorage) restored across reloads; attention indicators surfaced on tabs as well as rows
- Accessibility basics: `prefers-reduced-motion` honoured, visible focus styles, tablist semantics with keyboard navigation, aria-labels on icon-only controls
- New playwright spec for tabs + home; all 14 existing e2e tests keep passing unchanged

### Out of Scope
- Framework adoption (no React/Vue/Svelte — stays vanilla JS, no build step, no new runtime deps)
- Backend or API changes — REST and WS wire contracts are untouched
- Tab reordering/drag-and-drop, per-session themes or colours, split panes
- Multi-user anything, session sharing
- Redesigning the gate or modal flows (restyled only — same ids, same behaviour)

## Affected surfaces
- `web/index.html` — page head + session-row list markup, terminal tab-strip container, restyled gate/modal wrappers
- `web/style.css` — flat design-token system (colour, spacing, radius, typography) restyling all views
- `web/dashboard.js` — home view render: session rows with last-active, guided empty state, quick actions
- `web/terminal.js`, `web/app.js` — session tab strip, per-tab multi-attach lifecycle, persisted open tabs
- `e2e/` — new ui-v2 playwright spec; the existing 14 tests must pass unchanged
- `README.md`, `docs/requirement/app/web-terminal.md` — UI description updated for the home view and session tabs

## Actors
- Owner (single user driving the daemon from desktop and mobile browsers)

## Functional Requirements
### FR-001
- Requirement: Flat design system — CSS custom-property tokens for colour, spacing, radius and typography in `web/style.css`; a calm dark palette with generous spacing, soft rounded surfaces and hairline borders applied consistently across topbar, gate, home, terminal, modal, softkeys and the new tab strip; zero shadows or elevation, no gradients or heavy strokes; the home view lays out full-width, not in a centred column.
- Priority: must

### FR-002
- Requirement: Sessions sidebar — a persistent left sidebar (the dashboard itself, always visible when authed) listing every session with a live status dot, name, attention marker and dim row actions; clicking an item attaches or switches the terminal in place without leaving the view and without losing screen state; the active route's item is highlighted; on small viewports the sidebar collapses into a drawer (`#btn-side` + backdrop); the terminal view slides in on attach.
- Priority: must

### FR-003
- Requirement: Sidebar session list + home panel — `#view-dashboard` is the persistent sidebar carrying `#session-list` (compact items: `.session-name`, `.attention-dot`, `.badge`, `.session-meta` mono `cmd · cwd · id · created`, `.session-actions` with identical labels attach/edit/kill/respawn/clean/delete, dim until hover/focus), a `Sessions` head with `#session-count`, primary `#btn-new` and quick actions; `#/` shows a welcome panel in the main area; a guided `#dashboard-empty` state steers first-time users toward creating a session.
- Priority: must

### FR-004
- Requirement: Existing functionality and e2e selectors keep working — `#btn-new`, `.session-card`, `.session-meta`, `.session-name`, `.badge`, `.attention-dot`, `data-session-id`, `#session-list`, `#dashboard-empty`, `#term-overlay`, `#overlay-text`, `#term-title`, `#term-status`, `#btn-back`, `#btn-term-respawn`, `#btn-clean-all`, `#softkeys`, the gate view ids, `#modal` and `#f-*` fields, and row action buttons labelled attach/edit/kill/respawn/clean/delete all remain resolvable and behave as today; new wrappers are allowed but the listed ids/classes must survive. Mobile softkeys and the attention flow work unchanged.
- Priority: must

### FR-005
- Requirement: Reduced motion and a11y basics — `prefers-reduced-motion` disables animations/transitions (incl. the terminal slide-in); interactive elements show visible `:focus-visible` focus; icon-only controls (drawer toggle, softkeys) carry aria-labels/titles.
- Priority: should

## Non-Functional Requirements
- Vanilla ES modules only; no framework, no bundler, no new dependencies
- No backend change: REST routes, the WS frame protocol and auth are untouched
- Text and status colours keep ~WCAG AA contrast on the new palette
- Tab switching must not re-play scrollback — background tabs keep their own xterm and socket (max 6 live instances, LRU eviction); detached or evicted tabs release both and re-attach lazily on activation
- Selector compatibility is a contract: the FR-004 list is enforced by the existing playwright suite

## Main Use Cases
- UC-001 Browse the home view and create/attach a session
- UC-002 Switch between attached sessions via the tab strip
- UC-003 Detach a session tab without killing it
- UC-004 Use the terminal on mobile; see attention indicators on tabs and rows
- UC-005 Navigate with reduced motion / keyboard and assistive tech

## Constraints
- Existing selectors (FR-004 list) must stay resolvable; row action buttons keep their current labels because e2e locates them by text
- `#/s/<id>` remains the route for the active terminal; the router stays the source of truth for which tab is active
- localStorage is the only persistence added on the client (same mechanism already used for token + attention)
- At most 6 terminal instances (xterm + WebSocket + WebGL canvas) stay alive; least-recently-used background tabs are evicted and re-attach lazily — bounds memory and the browser's WebGL context limit
- The 14 existing playwright specs are modified only if strictly necessary — target zero changes

## Assumptions
- The daemon already supports multiple concurrent attaches and per-session WS connections (proven by the multiclient e2e)
- Up to 6 live xterm instances (one per open tab) is affordable for the expected session counts
- localStorage is available in target browsers (already required by the token gate)

## Acceptance Criteria
- [ ] Sidebar shows `#session-list` items as `.session-card[data-session-id]` containing `.session-name`, `.badge`, `.session-meta` and dim row actions, plus `#session-count`, `#btn-new`, quick actions and a guided `#dashboard-empty` state; `#/` shows the welcome panel
- [ ] Clicking a sidebar item attaches/switches the terminal in place without leaving the view and preserves screen state (no replay); the switched-to item is highlighted `.active`
- [ ] `← back` returns to `#/` (welcome panel); the sidebar stays visible on both routes; sessions keep running regardless of view
- [ ] Reloading on `#/s/<id>` re-attaches that session; previously attached terminals (LRU live set, max 6) keep screen state on switch with zero replay
- [ ] Sidebar status dots and `.attention-dot` reflect session status and attention; `#term-title`/`#term-status` track the attached session
- [ ] `prefers-reduced-motion` removes animations/transitions; aria-labels on icon-only controls present
- [ ] All 14 existing playwright tests pass unchanged; a new ui-v2 spec covers tabs and home; `#softkeys`, `#term-overlay`, gate and modal behave as before

## Edge Cases
- Sidebar item for a deleted session disappears on the next poll (dashboard 3s refresh)
- Attach to a session that disappears mid-switch — router fallback returns to `#/`
- Session renamed from another client — the poll updates the sidebar item name
- Bell on a backgrounded terminal instance — `.attention-dot` shows on the sidebar item (and `●` title when document hidden), cleared when that session is attached
- Many sessions — the sidebar scrolls vertically; beyond 6 live terminals the least-recently-used background instance is evicted (re-attaches + replays on next selection)
- Terminal instances are per-session panes inside `#term-container` — only the routed session attaches; others attach on first selection
- Small screens — strip scrolls, `#softkeys` stays reachable, row actions remain tappable (always visible on touch — no hover affordance)

## Open Questions
- "+": brief says "opens session picker/dashboard" — implemented as navigating home (which is the picker); an inline picker is a possible follow-up

## Test Strategy
- Level: full
- UI Tests: critical — the feature is UI-only; the 14 existing specs must stay green and a new spec covers tabs + home
- Tools: cargo test (backend regression), playwright
- Coverage Target: 80%
