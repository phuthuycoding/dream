# AGENTS.md

Conventions for AI coding agents working in this repository. Seeded by `kf init`; edit freely.

## Commands

| Task | Command |
|---|---|
| Build | `TODO: fill in` |
| Test | `TODO: fill in` |
| Lint | `TODO: fill in` |

Detect anything else by reading the repository, never by guessing.

## Workflow (kanban-flow)

- Start a feature with one command: `/kf:flow <what you want>` (a bug goes through the same command and is triaged as a bug).
- In Claude Code the workflow is the kf plugin: `/kf:flow` drives an item, and `/kf:brainstorm`, `/kf:plan`, `/kf:implement`, `/kf:test`, `/kf:review`, `/kf:archive`, `/kf:bug`, `/kf:spike` and `/kf:travel` run one phase or the day. Other runtimes use the `kf-<name>` skill copies that `kf install` writes.
- Human gates: confirm the requirement (Phase 1), approve the plan (Phase 2: `kf approve` starts implementation, `--backlog` parks it), and merge the PR after a PASS review (`kf status --change <feature>` closes the item). Do not ask "continue?" between other phases.
- Work item state lives in `.works/`; move items only with `kf stage` / `kf archive`, never by moving folders.
- Never use `--force` or `--skip-hooks` without explicit user approval; every bypass is recorded in the work item's metadata.
- Run `kf autoconfig` for the setup checklist and the review rules to apply as coding conventions.
