# Test cases — github-oauth

## TC-001 cli/env parse
| Field | Value |
|---|---|
| Requirement | FR-001, FR-002 |
| Use case | UC-001, UC-002 |
| Test type | Unit |
| Purpose | `--github-client-id/secret/allowlist` flags + `DREAM_GITHUB_*` env fallbacks parse; oauth-with-empty-allowlist errors |

## TC-002 session cookie mint/verify
| Field | Value |
|---|---|
| Requirement | FR-001 |
| Use case | UC-001, UC-003 |
| Test type | Unit |
| Purpose | mint→verify round-trip; tampered/expired/wrong-key rejected; signing key persisted 0600 |

## TC-003 authorize url + state
| Field | Value |
|---|---|
| Requirement | FR-001 |
| Use case | UC-001 |
| Test type | Unit |
| Purpose | `/auth/github` redirect URL carries client_id, redirect_uri, scope, random state; state cookie set |

## TC-004 callback exchange + session
| Field | Value |
|---|---|
| Requirement | FR-001, FR-002 |
| Use case | UC-001 |
| Test type | Integration |
| Purpose | callback against stub GitHub (token + /user + /user/orgs) → Set-Cookie `dream_session` + 302 `/` |

## TC-005 allowlist enforcement
| Field | Value |
|---|---|
| Requirement | FR-002 |
| Use case | UC-002 |
| Test type | Integration |
| Purpose | allowlisted login admitted; `org:` entry admits org member; others → 403, no cookie |

## TC-006 state CSRF rejection
| Field | Value |
|---|---|
| Requirement | FR-001 |
| Use case | UC-001 |
| Test type | Integration |
| Purpose | callback with missing/mismatched state → 400, no cookie; state cookie cleared |

## TC-007 middleware cookie-or-bearer
| Field | Value |
|---|---|
| Requirement | FR-001, FR-004 |
| Use case | UC-001, UC-004 |
| Test type | Integration |
| Purpose | `/api/sessions` 200 with valid cookie; 200 with Bearer; 401 with neither; WS `?token=` unchanged |

## TC-008 logout clears cookie
| Field | Value |
|---|---|
| Requirement | FR-003 |
| Use case | UC-003 |
| Test type | Integration |
| Purpose | `POST /auth/logout` returns expired `dream_session`; subsequent `/api` call → 401 |

## TC-009 gate button E2E
| Field | Value |
|---|---|
| Requirement | FR-003 |
| Use case | UC-001, UC-003 |
| Test type | E2E |
| Purpose | gate shows "Sign in with GitHub" when oauth configured; token field still works; logout button returns to gate |

## TC-010 bearer regression
| Field | Value |
|---|---|
| Requirement | FR-004 |
| Use case | UC-004 |
| Test type | E2E |
| Purpose | full existing playwright suite green on token auth with oauth configured or absent |
