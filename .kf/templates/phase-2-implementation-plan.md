---
feature: "{feature_name}"
context: "{context}"
created: "{timestamp}"
status: planning
---

# Implementation Plan

## Feature
{feature_name}

## Objective
{objective}

## Implementation Scope
### Implement Now
- {implement_now_item}

### Supporting Features
- {supporting_feature}

### Future Features
- {future_feature}

### Explicitly Not Implemented
- {not_implemented_item}

## Task Breakdown

| Task | Description | Area | Dependencies | Expected output | FR / UC references | Files |
|---|---|---|---|---|---|---|
| TASK-001 | {task_description} | {area} | {dependencies} | {expected_output} | {requirement_reference} | {file_paths_dirs_or_globs} |

Add one row per task. Execution progress lives outside the approved plan: in the hand-maintained `tasks.md` and the loop ledger `loop/progress.md`. **Files** is a comma-separated list of paths, directories or globs: the allowlist `kf verify --task` enforces for everything the task may touch; an empty Files cell means the files are unknown and nothing is refused. Dependencies are `TASK-###` tokens or `-`. Every surface declared under `## Affected surfaces` in the spec must appear here — as a task row or a `## Documentation Impact` entry — or `kf validate` warns it is uncovered.

## Complexity
- Level: {complexity_level}
- Reason: {complexity_reason}

## Impact Analysis

| Area | Impact |
|---|---|
| Backend | {backend_impact} |
| Frontend | {frontend_impact} |
| Database | {database_impact} |
| API | {api_impact} |
| Infrastructure | {infrastructure_impact} |
| Security | {security_impact} |
| Performance | {performance_impact} |
| Regression | {regression_impact} |
| Dependencies | {dependencies_impact} |

## Risks and Mitigation

| Risk | Impact | Mitigation / rollback |
|---|---|---|
| {risk} | {risk_impact} | {risk_mitigation} |

## Architecture Constraints
- {architecture_constraint}

## Implementation Constraints
- {implementation_constraint}

## Global Constraints
<!-- The spec's project-wide requirements that bind every task — version floors, dependency limits, naming and copy rules, exact values — one line each, verbatim. The task reviewer receives this section word for word, so keep guidance out of it. `None` when the spec states none. -->
- {global_constraint}

## Review Focus
<!-- Up to five inputs or failure modes the spec implies but no task's tests exercise, likeliest first: the condition and the behaviour expected. Each line's pinning test belongs to the task that owns the behaviour. `None` only after checking. -->
- {review_focus}

## Testing Strategy
- Unit: {unit_strategy}
- Integration: {integration_strategy}
- E2E: {e2e_strategy}
- Coverage target: >= {coverage_target}%

## Documentation Impact
- {documentation_impact}

## Definition of Done
- [ ] Implementation complete
- [ ] Unit tests
- [ ] Integration tests where applicable
- [ ] Coverage meets the agreed target
- [ ] E2E tests when required by the agreed Test Level
- [ ] All test cases pass
- [ ] Review passes
- [ ] Documentation updated
- [ ] Feature report generated
