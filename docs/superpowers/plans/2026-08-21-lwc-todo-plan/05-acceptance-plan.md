# LWC Todo and Current Plan for Agents - Acceptance Plan

Plan ID: `lwc-todo-plan-20260821`  
Status: `executable`  
Updated: `2026-08-21T19:00:49+08:00`

## Acceptance Criteria

- `AC-TODO-01` Given valid flags or structured JSON, Todo add returns one durable open
  entity with normalized title/tags/cue/detail, revision 1, stable ID, scope, and
  searchable index; an identical request replay returns the same ID.
- `AC-TODO-02` List/search/show return exact bounded, deterministic, origin-tagged
  results for state/title/tag/cue, including CJK input, without reading unrelated
  domain records.
- `AC-TODO-03` Update/done/cancel/reopen enforce the exact state table, CAS revision,
  required result/reason, idempotent unchanged behavior, and atomic FTS/operation
  updates.
- `AC-PLAN-01` Plan create returns one active Plan with normalized objective,
  `done_when`, constraints/tags, 1..=100 stable-ID steps, first step in progress, later
  steps pending, and revision-1 history.
- `AC-PLAN-02` Current/list/search/show/brief return deterministic bounded state; brief
  contains the approved resume fields and omits raw reasoning and excess terminal rows.
- `AC-PLAN-03` Advance/block atomically change only the focal/selected next step,
  increment revision once, append one history/operation entry, and reject invalid IDs,
  states, or stale revisions without partial writes.
- `AC-PLAN-04` Revise requires a reason, preserves completed/skipped facts, marks
  replaced non-terminal work skipped, creates a valid new focal sequence, and exposes
  reason/revision in history/brief.
- `AC-PLAN-05` Complete fails until all steps are terminal and checked result/evidence
  are present; abandon preserves unfinished steps/reason; both terminal states reject
  further mutation.
- `AC-CLI-01` Flag and strict JSON forms share semantics; stdin and authorized `@PATH`
  work; unknown/oversized/out-of-root/invalid input returns stable JSON errors and no
  write.
- `AC-SCOPE-01` Only documented discovery reads accept `--scope all`; all mutations,
  show/brief, and every Todo/Plan command with `--changeset` reject invalid selectors
  before migration/write.
- `AC-DATA-01` New and v14-migrated temporary stores expose identical current schema;
  injected migration failure leaves version/data unchanged; existing v13->v14 migration
  still reaches the current version through the ordered chain.
- `AC-CONC-01` Two connections cannot duplicate a keyed create or overwrite a newer
  Todo/Plan revision; conflict responses identify the safe retry point.
- `AC-HOOK-01` Existing lifecycle output adds bounded readiness/counts/Plan tracking, stays
  bounded/read-only, creates no new platform registration, and meets the focused paired
  latency/query-plan gates.
- `AC-SKILL-01` `using-todo` and `using-plan` encode the approved triggers/boundaries;
  all native targets and three packaged integrations receive exact canonical bytes;
  refresh/uninstall preserve user bytes.
- `AC-DOC-01` English/Chinese/Agent docs match CLI help and clearly distinguish Todo,
  Plan, Work, temporal memory, and Wiki with no excluded automation promise.
- `AC-TEST-01` Every test in the matrix passes through the affected-surface command set;
  no test is weakened and no unrelated worktree change is included.

## Test Matrix

| Test | Requirement | Level | Input source | Expected result | Evidence |
| --- | --- | --- | --- | --- | --- |
| `TEST-TODO-01` | `REQ-TODO-01` | CLI/store | flag and JSON create fixtures | normalized durable entity; replay same ID | `cargo test --test todo_cli create` |
| `TEST-TODO-02` | `REQ-TODO-02` | integration | CJK/title/tag/cue/state fixtures | exact ordered list/search/show | `cargo test --test todo_cli query` |
| `TEST-TODO-03` | `REQ-TODO-03` | state machine | every allowed/forbidden transition | exact state/revision/result/reason | `cargo test --test todo_cli transition` |
| `TEST-TODO-04` | `REQ-CLI-01` | boundary | malformed JSON/text/tags/paths/limits | stable error, zero write | `cargo test --test todo_cli invalid` |
| `TEST-TODO-05` | `REQ-DATA-01` | storage | canonical rows and FTS | same discoverability after mutations | `cargo test --test todo_cli fts` |
| `TEST-PLAN-01` | `REQ-PLAN-01` | CLI/store | 1-step and 100-step create fixtures | valid focus/order/history | `cargo test --test plan_cli create` |
| `TEST-PLAN-02` | `REQ-PLAN-03` | integration | active plan with >20 terminal steps | bounded exact brief and omitted count | `cargo test --test plan_cli brief` |
| `TEST-PLAN-03` | `REQ-PLAN-02` | state machine | advance/block sequences | one focal step invariant | `cargo test --test plan_cli advance` |
| `TEST-PLAN-04` | `REQ-PLAN-04` | state machine | blocked plan plus replacement steps | terminal facts preserved; reasoned history | `cargo test --test plan_cli revise` |
| `TEST-PLAN-05` | `REQ-PLAN-05` | state machine | incomplete/complete/abandon fixtures | gates and terminal immutability | `cargo test --test plan_cli finish` |
| `TEST-PLAN-06` | `REQ-PLAN-02` | negative flow | wrong focal/next, duplicate IDs, >100 steps | atomic stable errors | `cargo test --test plan_cli invalid_transition` |
| `TEST-PLAN-07` | `REQ-PLAN-04` | storage | multi-revision Plan | monotonic one-row-per-revision history | `cargo test --test plan_cli history` |
| `TEST-PLAN-08` | `REQ-PLAN-03` | privacy/bounds | long details and many results | no raw reasoning; bounded payload | `cargo test --test plan_cli brief_bounds` |
| `TEST-CLI-01` | `REQ-CLI-01` | contract | help, flags, stdin, `@PATH`, JSON errors | documented grammar and JSON envelope | both new CLI test binaries |
| `TEST-SCOPE-01` | `REQ-SCOPE-01` | integration | project/global/all/changeset matrix | only allowed reads merge; all writes exact | both new CLI test binaries |
| `TEST-DATA-01` | `REQ-DATA-01` | migration | fresh, v15, v14, chained v13 fixture | schema/data/metadata parity at v16 | migration tests plus temporal test |
| `TEST-DATA-02` | `REQ-DATA-01` | failure injection | conflicting/broken DDL | transaction rollback to v14 | `cargo test --test plan_cli migration_failure` |
| `TEST-CONC-01` | `REQ-CONC-01` | concurrency | duplicate request IDs, two Store connections | replay or conflict, no duplicate | `cargo test --test todo_cli concurrency` |
| `TEST-CONC-02` | `REQ-CONC-01` | concurrency | two Plan briefs then competing mutation | stale revision rejected, winner intact | `cargo test --test plan_cli concurrency` |
| `TEST-HOOK-01` | `REQ-HOOK-01` | contract | missing/current/legacy stores | separate Todo and Plan readiness | `cargo test --test agent_hooks todo_and_plan` |
| `TEST-HOOK-02` | `REQ-HOOK-01` | safety/perf | seeded store, before/after snapshots, paired runs | no mutation; indexed counts; ratio <=1.25 | focused Hook test/benchmark case |
| `TEST-SKILL-01` | `REQ-SKILL-01` | policy | canonical Skill text | trigger/skip/state/safety clauses present | two policy shell tests |
| `TEST-SKILL-02` | `REQ-SKILL-01` | installer | all targets, shared paths, old manifests, user bytes | install/refresh/uninstall exact | `cargo test --test agent_cli` |
| `TEST-SKILL-03` | `REQ-SKILL-01` | packaging | canonical plus three mirrors | recursive file/byte parity | `node --test tests/integrations.mjs` |
| `TEST-DOC-01` | `REQ-DOC-01` | docs contract | help and three docs | commands/scopes/boundaries aligned | policy assertions and focused `rg` |
| `TEST-GATE-01` | `REQ-TEST-01` | affected surface | final diff | all listed commands green; no unrelated scope | `TASK-009` evidence |

## End-to-End Scenarios

### Deferred work survives and is explicitly resolved

1. Add an open Todo with Chinese title, two tags, cue, detail, and request ID.
2. Retry the same submission and receive the same ID without extra operation.
3. Find it by title, either tag, and cue in exact scope and `--scope all` search.
4. A stale update conflicts; a current-revision update succeeds.
5. Mark done with result, verify default open list excludes it, then reopen and verify
   terminal data is cleared.

### Current Plan resumes, adapts, and completes

1. Create a Plan with objective, done criteria, constraints, and four coarse steps.
2. Read `plan brief` in a new process and receive first focal/next/revision.
3. Advance two milestones with results.
4. Block the third; revise with explicit reason and two replacement steps.
5. Verify prior results remain, replaced work is skipped, new focal is exact.
6. Complete remaining steps; premature complete fails, checked result/evidence succeeds.
7. Verify terminal Plan cannot mutate and no Wiki/memory/Work rows were created.

### Agent integration remains reversible

1. Seed an isolated Agent home with user-owned config and one colliding sibling path.
2. Install all detected targets; verify three Skills and one unchanged Hook/MCP setup.
3. Refresh twice and observe unchanged bytes/idempotent status.
4. Uninstall and verify exact original user bytes and no owned leftovers.

## Security, Performance, and Rollback

- Boundary tests cover project-root `@PATH`, strict JSON, control characters, bound
  lengths, parameterized SQL payloads, and absence of secret/full-body Hook output.
- Query-plan fixtures prove indexed/FTS paths at defined scale. Hook paired timing uses
  real seeded Todo/Plan rows every feature run; it cannot pass on an untriggered path.
- Migration failure tests verify rollback. Installer restoration tests verify user
  bytes. No real database or installed Agent directory is used for acceptance.
- Old-binary downgrade remains backup restore; a forward schema repair requires a new
  explicit migration plan.

## Release Gates

This bundle authorizes no release. Before a later release workflow may start:

1. `TASK-009` affected-surface gate is green with recorded command outputs.
2. Schema/CLI/Skill/docs reviews find no requirement or trust-boundary drift.
3. Release owner chooses version/channels separately and follows repository release
   prechecks, cross-build, packaging, publication, and live readback rules.
4. Any real migration/install smoke receives the repository-required safety notice and
   execution-specific confirmation.

## Evidence Plan

- Record exact command, exit status, test count, and commit/diff identity in
  `06-handoff.md`; do not paste noisy full logs.
- Preserve failing red-test names from `TASK-001` and green outputs from owning tasks.
- Capture schema evidence using temporary DB `PRAGMA user_version`, table/index
  inventory, and representative rows—not direct edits.
- Capture installer evidence from isolated directory snapshots and manifest-owned paths.
- Use `git diff --stat`, `git diff --check`, and path review to prove bounded scope.

## Residual Risks

- LWC can enforce structural completion evidence but cannot know whether `done_when` is
  semantically true; `using-plan` and the operating Agent own that judgment.
- A v16 store cannot be opened by older binaries; release notes must make backup/upgrade
  compatibility explicit.
- Query fixtures cover planned scale, not unbounded enterprise task stores. Add
  cursor-based pagination or archival only after measured need.
- Research sources motivate behavior but do not prove product usability. User trial may
  justify smaller command aliases later; v1 keeps one explicit canonical grammar.

## 2026-08-22 Configuration Acceptance

- `AC-CONFIG-01` Fresh configuration reports Todo and Plan disabled; each can be
  enabled or overridden independently through global/project inheritance; disabled
  commands fail without writes; all-scope reads skip disabled stores; Hook and Skills
  advertise/use only enabled capabilities.
- `TEST-CONFIG-01` The focused Todo config test covers defaults, inheritance, override,
  errors, and filtered all-scope reads. The focused Hook test covers disabled,
  Todo-only, and Plan-only payloads. Skill policies enforce configuration self-gating.
- `AC-TRACK-01` An enabled three-step Plan advanced once yields revision 2, progress
  1/3, the second step as current, the third as next, and an exact brief command; the
  Hook omits objective and completed-result text. No active Plan yields no tracked object.
- `TEST-TRACK-01` `cargo test --test agent_hooks separate_todo_and_plan_readiness_tracks_current_plan`
  verifies the complete lifecycle flow using real CLI-created records.
- `AC-TODO-CHILD-TIME-01` Creating a direct child returns its parent and makes it
  discoverable from parent show and `--parent`; target times normalize to UTC and can
  be rescheduled/cleared under revision CAS. Todo-enabled Hook output includes exactly
  the oldest three due open items, omits closed/future items and cue/detail, and reports
  the exact omitted count. Disabled Todo contributes no Todo field or reminders.
- `TEST-TODO-CHILD-TIME-01` The focused Todo CLI child/time test and Hook due-reminder
  test exercise these rules through the real binary and isolated stores.
