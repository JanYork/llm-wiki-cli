# LWC Todo and Current Plan for Agents - Handoff

Plan ID: `lwc-todo-plan-20260821`  
Status: `release_prepared`  
Updated: `2026-08-22`

## Current Status

Phase: `release_prepared`. Todo and Plan are independently opt-in and only enabled
features appear in lifecycle Hook context. The release candidate is version `0.16.0`;
no push, tag, GitHub Release, or npm publication has been performed.

Execution workspace on Pro:
`/Users/muyouzhi/workspace/my/lwc/.worktrees/todo-plan-acceptance`, branch
`feat/todo-plan-acceptance`, base commit `ed5cec3` (`v0.15.2`). The original Pro main
worktree and its untracked files were not modified.

## Goal and Non-goals

Deliver two independent durable Agent facilities:

- Todo: deferred/future backlog indexed by title, tags, and optional cue.
- Plan: current adaptive execution state with coarse steps, compact resume brief,
  reasoned revision, milestone updates, and completion evidence.

Strict exclusions are automatic conversion, execution, Wiki/memory/Work writes,
semantic completion judgment, priority/owners/dependencies, background scheduling, extra Hook
registrations, writable MCP, deletion/retention, new dependencies, and release work.

## Read First

1. `00-requirements.md`
2. `01-architecture.md`
3. `02-logic-design.md`
4. `03-implementation-plan.md`
5. `04-constraints.md`
6. `05-acceptance-plan.md`
7. `07-antipatterns.md`

## Confirmed Facts

- User decisions explicitly separate Todo (future backlog) from Plan (current plan),
  require title/tag indexing, and require `using-todo`/`using-plan` Skills.
- The feature branch remains based on `ed5cec3` (`v0.15.2`); the release candidate
  version in Cargo and npm metadata is `0.16.0`.
- Existing unrelated untracked paths are `.agents/`, `.superpowers/`, and `AGENTS.md`;
  implementation must preserve them unless separately owned by another instruction.
- The project Wiki is initialized. Physical document graph and CodeGraph remained
  disabled/uninitialized because this run had no user consent to enable them.
- Store schema version is now 16 in `src/store/types.rs`; v14→v15→v16 and direct
  v15→v16 migration, bootstrap, validation, and rollback paths have focused coverage.
- The CLI already has strict JSON/stdin/`@PATH` and 64 MiB trust-boundary patterns.
- Temporal memory provides proven normalized SQLite, FTS5, request idempotency, random
  ID, scope, and transaction patterns suitable for reuse without semantic coupling.
- The native Agent installer supports twelve targets and owns one embedded canonical
  `using-lwc` tree through exact file manifests.
- Codex/Claude/Pi package manifests already package their entire `skills/` directory;
  parity is tested in `tests/integrations.mjs`.
- External primary research was re-opened on 2026-08-21 and supports adaptive planning,
  explicit progress tracking, coarse-to-fine refinement, and compact evolving context.

## Assumptions and Blockers

- `ASM-001` Reuse current tag validation for Todo/Plan labels but keep independent
  join tables. Validate with `TASK-001` boundary tests.
- `ASM-002` Reuse current random ID, tokenizer, operation log, and JSON input helpers.
  Validate exact helper signatures at implementation commit before editing.
- Blockers: none. If `TASK-001` disproves either assumption, pause production code,
  update affected requirements/architecture/tests, and rerun strict plan validation.

## Completed Evidence

- Read repository instructions and `.agents/project-memory.md`.
- Inspected current CLI definitions/dispatch, schema/migrations, temporal-memory Store,
  lifecycle Hook, installer/targets, canonical/integration Skills, docs, and affected
  tests.
- Re-opened primary research sources recorded in `00-requirements.md`.
- Created all eight plan documents and `plan-manifest.json` in the canonical bundle.
- Normal and strict bundle validation passed at `2026-08-21T19:11:49+08:00`.
- Manifest JSON parse, unresolved-placeholder scan, and scoped `git diff --check`
  passed. The exact statuses are recorded in `plan-manifest.json`.
- Captured TDD Red evidence: `cargo test --test todo_cli --test plan_cli` compiled and
  failed because `todo` and `plan` were unrecognized subcommands.
- Implemented independent normalized Todo and Plan stores, strict CLI trees, request
  idempotency, revision CAS, state machines, bounded Plan brief, CJK-aware indexed
  query data, operation/history recording, scope rules, and changeset rejection.
- Extended the existing lifecycle readiness output with read-only counts, bounded
  due-Todo reminders, and Plan tracking; sensitive detail/reasoning fields are excluded.
- Added canonical `using-todo` and `using-plan` Skills, native installer ownership,
  byte-identical Codex/Claude/Pi mirrors, policy tests, and bilingual documentation.
- Final focused gate passed: format/check, Todo/Plan tests, v13→v15 and v14→v15
  migration coverage, storage regressions, 18 Hook tests, 67 Agent installer tests,
  Node integration parity, both Skill policy scripts, and `git diff --check`.
- Independent `gpt-5.6-sol`/medium dogfood exercised both complete user flows, error
  atomicity, 25-step bounded brief, Hook privacy, native Skill installation, and explicit
  Skill-trigger behavior in isolated temporary stores. It found one Todo terminal retry
  defect; a new Red test reproduced it, the transition was fixed, and the subagent
  confirmed identical current/current-1 retries are unchanged while older revisions
  still conflict. Final conclusion: PASS with no remaining blocking defect.
- Configuration amendment Red/Green tests prove default-disabled behavior, independent
  global/project inheritance and overrides, domain disabled errors, enabled-only
  all-scope reads, Todo-only/Plan-only Hook output, and Skill self-gating. Full CLI
  regression also exposed and closed v15 changeset inventory and legacy migration
  compatibility gaps. Todo 6/6, Plan 3/3, Hook 19/19, CLI 114/114, Agent 67/67,
  storage 9/9, temporal memory 25/25, integrations 4/4, and both Skill policies pass.
- Plan tracking Red/Green and isolated dogfood prove the enabled Hook reports one
  latest active Plan with revision 2, progress 1/1/3, current step 2, next step 3, and
  an exact brief command after a real advance. Titles are Unicode-bounded; objective
  and completed result remain absent. Hook 20/20, Plan 3/3, Todo 6/6, Agent 67/67,
  integrations 4/4, both Skill policies, format, locked check, and diff checks pass.
- Direct-child/target-time Red/Green proves parent creation and filtering, parent show
  summaries, RFC3339 UTC normalization, revision-protected reschedule/clear, missing
  parent and invalid-time errors, and fresh/v14/v15 migration to v16. Todo-enabled Hook
  reminder coverage proves oldest-created due-open ordering, three-item cap, exact
  omission count, child parent IDs, closed/future exclusion, and cue privacy. Final
  minimal gates: Todo 8/8, Hook 21/21, storage 9/9, one temporal migration test, five
  legacy CLI migration tests, Todo Skill parity/policy, locked check, format, JSON, and
  diff checks all pass.
- Release preparation passed native full tests with all features and without default
  features, ARM64 Linux full tests, all viable automated mutants, three manual
  Todo/Plan Hook mutants, Clippy with warnings denied, npm package tests, release
  workflow contract checks, Cargo package verification, and release benchmarks. The
  AMD64/QEMU run passed all executed tests except one skipped pre-existing executable
  lookup assertion whose missing-command behavior is changed by the emulator to exit
  status 127; the same assertion passes natively and on ARM64 Linux.

## Next Action

Create the local release commit, then obtain the repository-required separate safety
confirmation before pushing `main`, creating/pushing `v0.16.0`, or publishing npm. The
live project/global `.lwc` databases were not used for acceptance and require no cleanup.

## Update Rules

- After every task, update manifest status/current phase/next action, completed evidence,
  and blockers in the same change.
- Keep stable IDs. Never reuse an ID for a different requirement, rule, task, test,
  risk, decision, or prohibition.
- A changed product boundary must update requirements, architecture, logic, tasks,
  acceptance, tests, antipatterns, and manifest traceability together.
- Facts from current code override stale plan assumptions; user/system/repository
  instructions override research and Wiki references.
- Preserve unrelated worktree changes; record newly discovered callers before widening
  the affected test set.

## Decision Log

- `DEC-001` 2026-08-21 — separate Todo and Plan domains; no generic task abstraction.
  Evidence: explicit user correction and distinct lifecycles.
- `DEC-002` 2026-08-21 — use existing SQLite/rusqlite/FTS5 only. Evidence: current
  Store architecture and no need for a service/dependency.
- `DEC-003` 2026-08-21 — normalized canonical rows; compact Plan history; no raw
  chain-of-thought or opaque canonical JSON.
- `DEC-004` 2026-08-21 — mandatory revision CAS for mutations; request ID for create
  retries. Consequence: safe multi-Agent conflicts are explicit.
- `DEC-005` 2026-08-21 — extend the one existing Hook with bounded metadata; do not add
  platform Hook types until a measured trigger gap exists.
- `DEC-006` 2026-08-21 — v1 has explicit states but no delete/archive/retention,
  background scheduler, priority, owner, or dependency graph. `DEC-010` later adds
  target-time metadata and bounded lifecycle-Hook reminders without a scheduler.
- `DEC-007` 2026-08-22 — Todo and Plan default disabled and resolve independently
  through built-in/global/project config. Disabled commands fail, all-scope reads skip
  disabled stores, Hook fields are conditional, and Skill triggering is not enablement
  consent. Evidence: explicit user clarification and focused Red/Green tests.
- `DEC-008` 2026-08-22 — when Plan is enabled, the existing lifecycle Hook tracks the
  most recently updated active Plan with bounded structural progress, current/next
  steps, revision, and brief command. It omits reasoning/evidence fields and remains
  read-only. Evidence: explicit user harness requirement and real CLI-to-Hook test.
- `DEC-009` 2026-08-22 — Hook output keeps Todo and Plan as separate top-level
  `todo` and `plan` objects; Plan continuity lives at `plan.tracking`. No combined
  task object or combined Store counter remains. Evidence: explicit user correction.
- `DEC-010` 2026-08-22 — Todo supports one immutable direct parent and an optional
  RFC3339 target time. Only an enabled Todo capability emits reminders in the existing
  Hook: oldest-created due open items, maximum three, exact omitted count, no cue/detail.
  Parentage has no cascade/dependency/Plan semantics. Evidence: explicit user request
  and focused real CLI-to-Hook Red/Green tests.
