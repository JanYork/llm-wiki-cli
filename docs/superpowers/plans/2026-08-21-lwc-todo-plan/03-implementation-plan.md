# LWC Todo and Current Plan for Agents - Implementation Plan

Plan ID: `lwc-todo-plan-20260821`  
Status: `executable`  
Updated: `2026-08-21T19:00:49+08:00`

## Execution Strategy

Implement one vertical slice at a time with test-first contracts. Reuse current
helpers and dependencies. Run only the smallest affected tests after each task; run
the consolidated affected-surface gate after all slices. Use temporary test stores,
never the repository's live `.lwc/wiki.db`. Publishing/versioning is a separate task.

## Task Breakdown

### `TASK-001` Freeze CLI, JSON, state, and migration contracts with failing tests

- Goal: turn the approved requirements into executable red tests before production
  behavior exists.
- Traces to: all `REQ-*`; `ARCH-CLI-01`, `ARCH-DATA-01`; all `FLOW-*`; all `AC-*`;
  all `TEST-*`.
- Preconditions: implementation separately authorized; read current helper signatures
  and test harness once more at the checked-out commit.
- Inputs: this bundle, `tests/temporal_memory.rs`, `tests/agent_cli.rs`,
  `tests/agent_hooks.rs`, `tests/integrations.mjs`, `src/store/tags.rs`.
- Expected files: new `tests/todo_cli.rs`, `tests/plan_cli.rs`; focused additions to
  `tests/agent_hooks.rs`, `tests/agent_cli.rs`, and `tests/integrations.mjs`; new
  `tests/using_todo_policy.sh`, `tests/using_plan_policy.sh`.
- Change: add production-shaped create/query/transition/conflict/scope/migration/
  Hook/install fixtures. Assert stable error codes and no-write snapshots. Record the
  exact existing helper reused for scoped JSON input and tag validation.
- Verification: `cargo test --test todo_cli` and `cargo test --test plan_cli` fail only
  because Todo/Plan commands are absent; policy tests fail only because Skills are
  absent. Existing adjacent tests remain green.
- Rollback: remove only the new test files/hunks.
- Done: every acceptance row has a failing assertion and no architecture question is
  discovered. If helper behavior contradicts `ASM-001`/`ASM-002`, update this bundle
  before production code.

### `TASK-002` Add the v15 normalized schema and transactional migration

- Goal: satisfy `REQ-DATA-01` without changing command behavior yet.
- Traces to: `REQ-DATA-01`, `REQ-CONC-01`; `ARCH-DATA-01`, `ARCH-TODO-01`,
  `ARCH-PLAN-01`; `FLOW-TODO-CREATE`, `FLOW-PLAN-CREATE`; `AC-DATA-01`;
  `TEST-DATA-01`, `TEST-DATA-02`.
- Preconditions: schema tests from `TASK-001`; current version confirmed as 14.
- Inputs: exact schema in `01-architecture.md`; existing temporal-memory migration and
  failure-injection fixture.
- Expected files: `src/store/types.rs`, `src/store/schema.rs`,
  `src/store/migrations.rs`, new schema helper in either `src/store/todo.rs` or one
  separate Todo and Plan schema helpers, `src/store/mod.rs`, focused test fixtures.
- Change: introduce v15 tables/indexes/FTS, bootstrap helper, one v14->v15 immediate
  migration, metadata update, writable/read-only validation, and no unrelated rebuild.
- Verification: `cargo test --test todo_cli migration`, `cargo test --test plan_cli migration`,
  and the v13->v14 temporal migration test remain green; injected DDL failure proves
  old rows and `user_version=14` survive.
- Rollback: revert code before release. Never down-migrate a real v15 database; restore
  a pre-upgrade backup/checkpoint or ship a forward repair migration.
- Done: new and migrated temporary stores have identical required schema; incomplete
  schema is rejected and failed migration is atomic.

### `TASK-003` Implement the complete Todo vertical slice

- Goal: deliver the Todo backlog without coupling it to Plan or other LWC domains.
- Traces to: `REQ-TODO-01`, `REQ-TODO-02`, `REQ-TODO-03`, `REQ-CLI-01`,
  `REQ-SCOPE-01`, `REQ-CONC-01`; `ARCH-CLI-01`, `ARCH-TODO-01`,
  `ARCH-SCOPE-01`; `FLOW-TODO-CREATE`, `FLOW-TODO-QUERY`,
  `FLOW-TODO-TRANSITION`, `FLOW-SCOPE-READ`; `AC-TODO-01..03`, `AC-CLI-01`,
  `AC-SCOPE-01`, `AC-CONC-01`; `TEST-TODO-01..05`, `TEST-SCOPE-01`,
  `TEST-CONC-01`.
- Preconditions: `TASK-002` schema green.
- Inputs: Todo CLI contract, rules, current memory input/FTS/idempotency/scope patterns.
- Expected files: `src/store/todo.rs`, `src/store/mod.rs`,
  `src/cli/definitions.rs`, `src/cli/dispatch.rs`, `tests/todo_cli.rs`.
- Change: implement strict inputs, normalization, create replay/conflict, indexed
  list/search/show, update, lifecycle transitions, revision CAS, operation log, FTS,
  cross-scope merge, and stable JSON/errors.
- Verification: `cargo test --test todo_cli`; run `cargo fmt --all -- --check` after the
  slice. Assert every failure leaves canonical/FTS/operations counts unchanged.
- Rollback: revert the Todo module/CLI hunks while retaining v15 schema only if Plan
  work already depends on it; otherwise revert `TASK-002` too before release.
- Done: all Todo tests pass, Plan tests remain red only for missing Plan behavior, and
  Wiki/memory/Work tables remain byte/logically unchanged in fixtures.

### `TASK-004` Implement the complete Plan vertical slice

- Goal: deliver current-plan create/resume/advance/block/revise/finish semantics.
- Traces to: `REQ-PLAN-01..05`, `REQ-CLI-01`, `REQ-SCOPE-01`, `REQ-CONC-01`;
  `ARCH-CLI-01`, `ARCH-PLAN-01`, `ARCH-SCOPE-01`; `FLOW-PLAN-CREATE`,
  `FLOW-PLAN-RESUME`, `FLOW-PLAN-ADVANCE`, `FLOW-PLAN-REVISE`,
  `FLOW-PLAN-COMPLETE`, `FLOW-SCOPE-READ`; `AC-PLAN-01..05`, `AC-CLI-01`,
  `AC-SCOPE-01`, `AC-CONC-01`; `TEST-PLAN-01..08`, `TEST-SCOPE-01`,
  `TEST-CONC-02`.
- Preconditions: `TASK-002`; reuse shared text/tag/JSON helpers only where semantics
  are identical.
- Inputs: Plan CLI contract, state rules, current transaction/FTS patterns.
- Expected files: `src/store/plan.rs`, `src/store/mod.rs`,
  `src/cli/definitions.rs`, `src/cli/dispatch.rs`, `tests/plan_cli.rs`.
- Change: implement normalized create, current/list/search/show, bounded brief,
  atomic advance/block, reasoned revise with preserved terminal steps, completion/
  abandon gates, revision history, FTS, operation log, all-scope discovery, errors.
- Verification: `cargo test --test plan_cli`; then rerun `cargo test --test todo_cli`
  because CLI/store shared files changed.
- Rollback: revert Plan module/CLI hunks. Do not remove schema from a migrated real
  database; use pre-release code reversion only on untouched fixtures.
- Done: Plan state machine and concurrency assertions pass; Todo remains green; no
  hidden reasoning or automatic cross-domain write is persisted.

### `TASK-005` Extend bounded lifecycle readiness using the existing Hook

- Goal: expose useful discovery triggers without new hook registrations or task text.
- Traces to: `REQ-HOOK-01`; `ARCH-HOOK-01`; `FLOW-PLAN-RESUME`;
  `AC-HOOK-01`; `TEST-HOOK-01`, `TEST-HOOK-02`.
- Preconditions: Todo/Plan store reads available.
- Inputs: current readiness payload and non-mutation tests in `tests/agent_hooks.rs`.
- Expected files: `src/agent.rs`, `tests/agent_hooks.rs`, and only if the schema-facing
  helper belongs there, `src/store/todo.rs`/`plan.rs`.
- Change: add readiness/count/command objects using read-only current-store queries;
  omit contents and return diagnostics for missing/legacy stores.
- Verification: `cargo test --test agent_hooks`; snapshot database tables, operations,
  WAL presence/size as the existing test permits, and row counts before/after Hook.
- Rollback: remove the two readiness fields; no data rollback exists because Hook is
  read-only.
- Done: counts are correct, payload is bounded, no new platform Hook type appears,
  and Hook causes no migration or mutation.

### `TASK-006` Add canonical Skills and native installer ownership

- Goal: make `using-todo` and `using-plan` available to all existing native Agent
  targets with exact refresh/uninstall behavior.
- Traces to: `REQ-SKILL-01`; `ARCH-SKILL-01`; `FLOW-SKILL-INSTALL`;
  `AC-SKILL-01`; `TEST-SKILL-01`, `TEST-SKILL-02`.
- Preconditions: CLI help/output stable from `TASK-003`/`TASK-004`.
- Inputs: canonical `using-lwc` layout, current installer manifest rules, policy tests.
- Expected files: new `skills/using-todo/{SKILL.md,agents/openai.yaml}` and
  `skills/using-plan/{SKILL.md,agents/openai.yaml}`; `src/agent/install.rs`;
  `tests/agent_cli.rs`; new policy tests.
- Change: write concise trigger/skip/command/state/concurrency policies. Generalize the
  embedded bundle list and sibling install paths without editing twelve target adapters.
  Test install, unchanged reinstall, refresh, collision, shared paths, exact uninstall,
  and old-manifest upgrade.
- Verification: `bash tests/using_todo_policy.sh`, `bash tests/using_plan_policy.sh`,
  and `cargo test --test agent_cli`.
- Rollback: manifest-aware uninstall in fixtures; code rollback removes only new
  canonical bundles/generalization before release.
- Done: every native target receives all three byte-identical Skills and user-owned
  bytes are restored exactly.

### `TASK-007` Mirror Skills into packaged integrations and enforce parity

- Goal: keep Codex, Claude, and Pi distributed suites aligned with canonical Skills.
- Traces to: `REQ-SKILL-01`, `REQ-TEST-01`; `ARCH-SKILL-01`;
  `FLOW-SKILL-INSTALL`; `AC-SKILL-01`, `AC-TEST-01`; `TEST-SKILL-03`.
- Preconditions: canonical Skill content green.
- Inputs: canonical new directories and current integration package roots.
- Expected files: `integrations/{codex-lwc,claude-lwc,pi-lwc}/skills/using-todo/**`,
  matching `using-plan/**`, `tests/integrations.mjs`; package READMEs only if they
  enumerate Skill names.
- Change: copy exact canonical bytes and generalize parity test to all three Skill
  names. Keep existing package manifests because they already include `skills/`.
- Verification: `node --test tests/integrations.mjs`; compare recursive file lists and
  bytes for every canonical/integration bundle.
- Rollback: remove new mirror directories and parity hunks; existing using-lwc package
  remains unchanged.
- Done: three integrations package all three Skills with exact parity and existing MCP/
  Hook assertions remain green.

### `TASK-008` Document user and Agent contracts

- Goal: expose the distinction and complete supported command surface without
  promising excluded automation.
- Traces to: `REQ-DOC-01`; `ARCH-CLI-01`, `ARCH-SCOPE-01`, `ARCH-HOOK-01`;
  all user flows; `AC-DOC-01`; `TEST-DOC-01`.
- Preconditions: CLI help generated from final command definitions.
- Inputs: `README.md`, `README.zh-CN.md`, `docs/agent-workflow.md`, Skill policies.
- Expected files: those three docs; integration README files only when they list the
  bundled Skills.
- Change: add compact Todo/Plan sections, examples, state/scope/revision rules,
  Hook boundary, and separation from Work/memory/Wiki. Keep bilingual facts aligned.
- Verification: policy/parity tests plus focused `rg` assertions for every subcommand,
  scope rule, and forbidden auto-conversion/cross-write promise; `git diff --check`.
- Rollback: revert documentation hunks only.
- Done: docs match `--help` and acceptance contracts with no release claim.

### `TASK-009` Run the affected-surface gate and prepare implementation handoff

- Goal: prove the feature is implementation-complete without publishing it.
- Traces to: `REQ-TEST-01` and every other requirement through its test;
  `AC-TEST-01`; all `TEST-*`.
- Preconditions: `TASK-002..008` green independently.
- Inputs: final diff and commands below.
- Expected files: only evidence updates in this bundle if implementation policy tracks
  progress here; no version, release, package publication, or deployment file.
- Change: no feature code; inspect diff for scope, run consolidated focused gate,
  update manifest/handoff with exact results and residual risk.
- Verification: commands in `## Verification Commands`, all exit zero; inspect
  `git diff --stat`, `git diff --check`, and unrelated preexisting worktree state.
- Rollback: fix the owning slice; do not weaken tests or widen scope to mask failure.
- Done: all affected tests pass, every acceptance row has evidence, no blocker remains,
  and release remains a separately authorized next workflow.

## Dependency Order

```text
TASK-001 contracts
    -> TASK-002 schema
       -> TASK-003 Todo ----+
       -> TASK-004 Plan ----+-> TASK-005 Hook
                            +-> TASK-006 native Skills
                                  -> TASK-007 package mirrors
TASK-003 + TASK-004 + TASK-005 + TASK-007 -> TASK-008 docs -> TASK-009 gate
```

## File Impact

| Area | Verified files | Planned additions/changes |
| --- | --- | --- |
| CLI | `src/cli/definitions.rs`, `src/cli/dispatch.rs` | Todo/Plan command enums and routing |
| Store composition | `src/store/mod.rs`, `src/store/types.rs` | include new modules; v15 constant |
| Schema | `src/store/schema.rs`, `src/store/migrations.rs` | bootstrap, migration, validation |
| Domain | absent | `src/store/todo.rs`, `src/store/plan.rs`; optional cohesive schema helper only if it avoids duplication |
| Hook | `src/agent.rs` | bounded counts/readiness/commands |
| Native installer | `src/agent/install.rs`, `src/agent/targets/*.rs` | bundle generalization in installer; target files are review-only unless evidence disproves the sibling-path design |
| Canonical Skills | `skills/using-lwc/` | new sibling `using-todo/`, `using-plan/` |
| Integration Skills | three `integrations/*-lwc/skills/using-lwc/` trees | two sibling trees per integration |
| Tests | existing files listed in tasks | new Todo/Plan CLI and policy tests; focused Hook/installer/parity updates |
| Docs | `README.md`, `README.zh-CN.md`, `docs/agent-workflow.md` | contract sections only |

## Verification Commands

Run per task as listed, then once at `TASK-009`:

```bash
cargo fmt --all -- --check
cargo check --locked
cargo test --test todo_cli
cargo test --test plan_cli
cargo test --test temporal_memory version_13_store_migrates_temporal_tables_transactionally
cargo test --test storage_regressions
cargo test --test agent_hooks
cargo test --test agent_cli
node --test tests/integrations.mjs
bash tests/using_todo_policy.sh
bash tests/using_plan_policy.sh
git diff --check
```

Do not run unrelated graph, viewer, Office, conversion, browser, benchmark, or release
suites after each edit. If an edited shared file or discovered caller expands the real
impact, add only its owning focused test binary and record why in `06-handoff.md`.

## Rollback and Recovery

- Before publication: revert only Todo/Plan-owned code/docs/tests; preserve unrelated
  worktree changes.
- Schema: migration is forward-only. Temporary test stores may be discarded. A real
  store requires an explicit safety notice and backup/checkpoint before first v15 open;
  rollback restores that backup because an older binary rejects v15.
- Installer: rely on existing per-target manifests to restore exact original bytes.
  Never delete an untracked Skill directory without ownership evidence.
- Partial transaction: immediate transactions must roll back canonical, FTS, history,
  and operation rows together. Tests assert no partial success.
- Release/package/deploy: not authorized by this plan; no recovery procedure is needed
  until a separate release plan resolves version and channels.

## Progress Protocol

After each task, update `plan-manifest.json` and `06-handoff.md` with the completed
task, exact command results, discovered callers, blockers, and next task. Change a
confirmed requirement or architecture decision only with evidence and update all
traceability/acceptance/antipattern entries in the same change.

### `TASK-010` Add independent opt-in capability gates

- Traces to: `REQ-CONFIG-01`; `ARCH-CONFIG-01`; `FLOW-CAPABILITY-GATE`;
  `AC-CONFIG-01`; `TEST-CONFIG-01`; `ANTI-CONFIG-01`.
- Change: add config v6 Todo/Plan settings, CLI gating, enabled-store all-scope
  filtering, conditional Hook fields, and Skill self-gating.
- Verification: targeted config/Hook tests, full affected regressions, Skill
  policy/parity checks, and format/check/diff gates.

### `TASK-011` Track active Plan continuity in lifecycle Hook

- Traces to: `REQ-TRACK-01`; `ARCH-TRACK-01`; `FLOW-PLAN-TRACK`;
  `AC-TRACK-01`; `TEST-TRACK-01`; `ANTI-TRACK-01`.
- Change: select the latest active Plan read-only and emit bounded progress,
  current/next steps, revision, and brief command only when Plan is enabled.
- Verification: Red/Green real CLI-to-Hook flow, Hook privacy/config suite, Skill
  policy/parity, and affected format/check/diff gates.

### `TASK-012` Add direct child Todo and target-time reminders

- Traces to: `REQ-TODO-04`, `REQ-TODO-05`; `ARCH-TODO-CHILD-TIME-01`;
  `FLOW-TODO-CHILD-TIME`; `AC-TODO-CHILD-TIME-01`; `TEST-TODO-CHILD-TIME-01`;
  `ANTI-TODO-CHILD-TIME-01`.
- Change: migrate store v15 to v16, add parent/time CLI and JSON fields, direct-child
  reads, and Todo-enabled bounded due reminders in the existing Hook.
- Verification: focused Todo CLI and Hook Red/Green, full Todo/Hook integration files,
  Todo Skill policy/parity, locked check, formatting, JSON, and diff checks.
