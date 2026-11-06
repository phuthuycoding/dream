# Test cases — cli-lifecycle

## TC-001 cli parse
| Field | Value |
|---|---|
| Requirement | FR-001, FR-002, FR-003, FR-005 |
| Use case | UC-001, UC-002, UC-003, UC-005 |
| Test type | Unit |
| Purpose | all subcommands + global flags parse in any position |

## TC-002 plist xml
| Field | Value |
|---|---|
| Requirement | FR-002 |
| Use case | UC-001 |
| Test type | Unit |
| Purpose | label, ProgramArguments, RunAtLoad, KeepAlive, log paths, escaping |

## TC-003 down-missing-agent
| Field | Value |
|---|---|
| Requirement | FR-003 |
| Use case | UC-002 |
| Test type | Integration |
| Purpose | `dream down` without agent exits 0 with friendly message |

## TC-004 url-from-token
| Field | Value |
|---|---|
| Requirement | FR-005 |
| Use case | UC-003 |
| Test type | Integration |
| Purpose | `dream url` prints URL from token file; missing → helpful error |

## TC-005 ps via API
| Field | Value |
|---|---|
| Requirement | FR-005 |
| Use case | UC-003 |
| Test type | Integration |
| Purpose | `dream ps` renders table against stub /api/sessions; unreachable → friendly |

## TC-006 doctor checklist
| Field | Value |
|---|---|
| Requirement | FR-005 |
| Use case | UC-003 |
| Test type | Unit |
| Purpose | check aggregation renders a correct pass/fail list |

## TC-007 rel-time fmt
| Field | Value |
|---|---|
| Requirement | FR-006 |
| Use case | UC-004 |
| Test type | Unit |
| Purpose | seconds/minutes/hours/days buckets |

## TC-008 full E2E on renamed binary
| Field | Value |
|---|---|
| Requirement | FR-001, FR-006, FR-007 |
| Use case | UC-001, UC-004, UC-005 |
| Test type | E2E |
| Purpose | 14 playwright tests green; firstrun text `dream ready`, `dream.token` |

## TC-009 real lifecycle cycle
| Field | Value |
|---|---|
| Requirement | FR-002, FR-003, FR-004, FR-005 |
| Use case | UC-001, UC-002, UC-003 |
| Test type | Behaviour |
| Purpose | dream up → status → restart → logs → ps → down; port refuses after down |
