# LWC Todo and Current Plan for Agents - Architecture

Plan ID: `lwc-todo-plan-20260821`  
Status: `executable`  
Updated: `2026-08-21T19:00:49+08:00`

## Context and Boundaries

Todo and Plan are two new domain modules inside the existing `lwc` binary and the
existing Wiki SQLite database. They reuse LWC's scope resolution, connection policy,
JSON/error envelope, CJK tokenizer, transaction discipline, operation log, lifecycle
readiness function, and Agent installer. They do not become Wiki pages, Sources,
temporal events, Work jobs, graph nodes, or MCP mutations.

```text
Agent/user
   |
   +-- lwc todo ... ----> Todo CLI ----> Todo store tables/FTS
   |
   +-- lwc plan ... ----> Plan CLI ----> Plan store tables/FTS/history
   |
   +-- lifecycle Hook --> bounded counts and one active Plan continuity summary
   |
   +-- Agent install ---> using-lwc + using-todo + using-plan Skill files
```

## Current Architecture Evidence

- `src/cli/definitions.rs` owns Clap structures and trust-boundary input readers;
  `src/cli/dispatch.rs` owns scope/changeset checks and Store calls.
- `src/store/mod.rs` composes cohesive Store modules with `include!`; current schema
  constants live in `src/store/types.rs` and current schema version is 14.
- `src/store/schema.rs` creates new databases and validates required tables/columns;
  `src/store/migrations.rs` upgrades recognized versions in immediate transactions.
- `src/store/temporal_memory.rs` demonstrates strict Serde input, normalized SQLite,
  contentless FTS5, random IDs, request fingerprints, idempotent create replay,
  project/global/all reads, and transaction-local retention.
- `src/scope.rs` is authoritative for project/global/all resolution and project-root
  file authorization. `src/changeset.rs` provides the standard selector rejection.
- `src/agent.rs` emits one bounded lifecycle readiness response; `src/agent/install.rs`
  embeds canonical Skill bytes and uses manifests for exact restore/uninstall.
- Every target in `src/agent/targets/` supplies one `using-lwc` directory path.
  `integrations/{codex-lwc,claude-lwc,pi-lwc}` package their entire `skills/`
  directory, so sibling Skills do not require new package-manifest fields.
- Existing targeted tests are `tests/temporal_memory.rs`, `tests/agent_cli.rs`,
  `tests/agent_hooks.rs`, `tests/integrations.mjs`, and policy shell tests.

## Target Architecture

### `ARCH-CLI-01` Command boundary

Add `Todo { command: TodoCommand }` and `Plan { command: PlanCommand }` to the current
Clap tree. Flag creates cover fast Agent use; `--json JSON|-|@PATH` covers complete
structured input. Definitions remain declarative; dispatch performs selector/scope
checks and routes to Store methods. All normal success/error envelopes remain JSON.

### `ARCH-TODO-01` Todo persistence and search

Add one Store module, `src/store/todo.rs`, with strict inputs and these normalized
schema objects:

- `todo_items(id, request_id, fingerprint, title, cue, detail, state, result,
  cancel_reason, revision, created_at, updated_at, closed_at)`;
- `todo_tags(todo_id, tag_name)` with indexes by tag and Todo;
- contentless `todo_fts(todo_id, title_terms, tag_terms, cue_terms, detail_terms)`.

`request_id` has a partial unique index. Title, state, revision, and timestamps are
ordinary indexed columns. FTS is updated in the same transaction as its canonical row.

### `ARCH-PLAN-01` Plan persistence, ordered steps, and history

Add one Store module, `src/store/plan.rs`, with:

- `plans(id, request_id, fingerprint, title, objective, done_when, state, result,
  completion_evidence, done_when_checked, abandoned_reason, revision, created_at,
  updated_at, closed_at)`;
- `plan_tags(plan_id, tag_name)`;
- `plan_constraints(plan_id, ordinal, value)`;
- `plan_steps(plan_id, step_id, ordinal, title, status, verify, result, blocker,
  created_revision, updated_revision, created_at, updated_at)`;
- `plan_history(id, plan_id, revision, action, reason, step_id, result, created_at)`;
- contentless `plan_fts(plan_id, title_terms, tag_terms, objective_terms,
  constraint_terms, step_terms)`.

Completed/skipped steps remain in `plan_steps`; revision replaces only future/focal
work through explicit state transitions, so no JSON snapshot table is needed.
`plan_history` stores compact indexed audit facts, not hidden reasoning.

### `ARCH-DATA-01` One schema-version transition

Increase `USER_VERSION` once from 14 to 15. A shared
separate `create_todo_schema(Transaction)` and `create_plan_schema(Transaction)` helpers are called by bootstrap and by a single
v14->v15 migration followed by the v15->v16 Todo field migration. Writable migration
is atomic per version; failed DDL leaves the prior version and data intact. Read-only opens reject old stores. Both
schema validators include every new table and query its expected columns.

### `ARCH-SCOPE-01` Scope resolver

Mutations and exact-ID reads open one project/global Store. `todo list/search` and
`plan current/list/search` may resolve project plus global stores for `--scope all`,
tag every returned row with its origin scope, merge deterministically, then apply the
global limit. Todo/Plan reject `--changeset` before opening a store.

### `ARCH-HOOK-01` Existing Hook extension

Extend the current readiness payload, not platform registrations. If a current store
is readable, include only `ready`, `open_count`/`active_count`, and command strings
(`list`/`current`, `search`, `add`/`create`). Missing or legacy stores report bounded
readiness facts without migration. No titles, tags, steps, results, or mutations are
returned.

### `ARCH-SKILL-01` Three canonical Skill bundles

Create canonical `skills/using-todo/` and `skills/using-plan/` beside
`skills/using-lwc/`, each with `SKILL.md` and `agents/openai.yaml`. Refactor the
installer's singular `SKILL_FILES` into a small named bundle list anchored at the
existing `using-lwc` target directory; its parent receives the two sibling Skills.
Target adapters keep their established paths. Existing manifests continue tracking
every byte-written path, so refresh/uninstall restoration remains exact.

Mirror both new Skill directories under:

- `integrations/codex-lwc/skills/`;
- `integrations/claude-lwc/skills/`;
- `integrations/pi-lwc/skills/`.

## Component Responsibilities

| Component | Current responsibility | Planned responsibility | Evidence |
| --- | --- | --- | --- |
| `src/cli/definitions.rs` | Global command grammar and bounded input | Declare Todo/Plan subcommands and arguments | current `Remember`/`Memory` commands |
| `src/cli/dispatch.rs` | Scope, changeset, Store routing | Enforce Todo/Plan scope matrix and merge reads | current memory dispatch |
| `src/store/todo.rs` | absent | Todo validation, transactions, FTS, response shaping | target `ARCH-TODO-01` |
| `src/store/plan.rs` | absent | Plan state machine, history, brief, FTS | target `ARCH-PLAN-01` |
| Store schema/migration files | v14 bootstrap/migration/validation | v15 Todo/Plan plus v16 child/time migration | `src/store/types.rs`, schema, migrations |
| `src/agent.rs` | bounded readiness Hook | add counts, due Todo summaries, and Plan tracking | existing Hook trust boundary |
| `src/agent/install.rs` | one embedded Skill bundle | install three byte-owned bundles | existing manifest restoration |
| canonical/integration Skills | `using-lwc` guidance | add distinct Todo and Plan policies | user decision |
| README/workflow docs | user and Agent contracts | expose command and boundary contracts | `README*.md`, `docs/agent-workflow.md` |

## Data Flow

```text
Flag or strict JSON input
  -> Clap parse
  -> reject --changeset / resolve allowed scope
  -> normalize and validate
  -> BEGIN IMMEDIATE
  -> request_id or revision conflict check
  -> canonical normalized rows + FTS + operation/history row
  -> COMMIT
  -> stable JSON envelope with scope, database, entity, revision, action
```

```text
Bounded read query
  -> resolve one or two read-only stores
  -> indexed filter/FTS query per store
  -> attach origin scope
  -> deterministic merge and global limit
  -> stable JSON envelope
```

## Interface Contracts

### Todo CLI

```text
lwc todo add TITLE [--tag TAG]... [--cue TEXT] [--detail TEXT] [--request-id ID]
lwc todo add --json JSON|-|@PATH
lwc todo list [--state open|done|cancelled] [--tag TAG] [--limit N] [--offset N]
lwc todo search QUERY [--state ...] [--tag TAG] [--limit N] [--offset N]
lwc todo show TODO_ID
lwc todo update TODO_ID --if-revision N [--title TEXT] [--cue TEXT|--clear-cue]
  [--detail TEXT|--clear-detail] [--add-tag TAG]... [--remove-tag TAG]...
lwc todo done TODO_ID --if-revision N --result TEXT
lwc todo cancel TODO_ID --if-revision N --reason TEXT
lwc todo reopen TODO_ID --if-revision N
```

`list` defaults to `state=open`, `limit=100`, `offset=0`; `search` defaults to all
states. `add --json` accepts the same semantic fields plus optional `request_id`.

### Plan CLI

```text
lwc plan create TITLE --objective TEXT --done-when TEXT [--tag TAG]...
  [--constraint TEXT]... --step TITLE [--step TITLE]... [--request-id ID]
lwc plan create --json JSON|-|@PATH
lwc plan current [--tag TAG] [--limit N] [--offset N]
lwc plan list [--state active|completed|abandoned] [--tag TAG] [--limit N] [--offset N]
lwc plan search QUERY [--state ...] [--tag TAG] [--limit N] [--offset N]
lwc plan show PLAN_ID
lwc plan brief PLAN_ID
lwc plan advance PLAN_ID --if-revision N --done STEP_ID --result TEXT --next STEP_ID
lwc plan block PLAN_ID --if-revision N --step STEP_ID --reason TEXT
lwc plan revise PLAN_ID --if-revision N --reason TEXT --json JSON|-|@PATH
lwc plan complete PLAN_ID --if-revision N --result TEXT --evidence TEXT --done-when-checked
lwc plan abandon PLAN_ID --if-revision N --reason TEXT
```

Create assigns stable step IDs and marks the first step `in_progress`; the rest are
`pending`. `revise` carries replacement future steps and a focal step ID; it cannot
edit completed/skipped step facts. `advance` requires `--next` while a future pending
step exists and forbids it when none remains.

### JSON and errors

- Structured inputs use `serde(deny_unknown_fields)` and the existing UTF-8, 64 MiB,
  stdin, relative `@PATH`, and project-root containment rules.
- Every returned entity contains `id`, `scope`, `state`, `revision`, and timestamps.
- Stable domain errors include `todo_not_found`, `todo_request_conflict`,
  `todo_revision_conflict`, `invalid_todo_transition`, `plan_not_found`,
  `plan_request_conflict`, `plan_revision_conflict`, `invalid_plan_transition`, and
  `plan_completion_incomplete`.

## Architecture Decisions

- `DEC-001` Todo and Plan are separate tables, commands, state machines, and Skills;
  shared labels do not justify one polymorphic “task” abstraction.
- `DEC-002` Reuse SQLite/rusqlite/FTS5 and existing helpers; add no dependency,
  daemon, service, or external store.
- `DEC-003` Use normalized current rows plus compact Plan history. Do not store the
  canonical entity as opaque JSON and do not retain hidden reasoning.
- `DEC-004` Require revisions for mutations of existing entities. Fast create stays
  one command; safety against silent multi-Agent overwrite is more important than
  saving one numeric argument.
- `DEC-005` Reuse the existing lifecycle Hook and add only bounded metadata. More
  platform hook registrations are deferred until a measured trigger gap exists.
- `DEC-006` Do not add configuration or retention. Todo/Plan state remains until an
  explicit future archival/deletion design is authorized.

## Risks

- `RISK-001` Schema migration could strand v14 stores. Mitigation: one immediate
  transaction, failure-injection test, preserved v14 version/data, bootstrap parity.
- `RISK-002` FTS rows could drift from canonical data. Mitigation: update in the same
  transaction and assert rebuild/query parity in focused tests.
- `RISK-003` Two Agents could overwrite progress. Mitigation: mandatory revision CAS,
  state predicates in SQL, and conflict tests using two connections.
- `RISK-004` Plan revision could erase useful completed work. Mitigation: completed
  and skipped steps are immutable; revision changes only focal/future steps and appends
  history.
- `RISK-005` Hooks could become noisy or leak task text. Mitigation: fixed three-item
  Todo reminder cap, bounded titles, no cue/detail, read-only connection, and tests.
- `RISK-006` Installer refactor could remove user-owned files. Mitigation: keep the
  existing manifest/snapshot path ownership model and extend exact restoration tests
  to all three Skill directories and shared Agent targets.
- `RISK-007` Todo/Plan could blur into Wiki, memory, or Work. Mitigation: explicit
  non-goals, separate tables, no automatic cross-write, Skill policy tests.

### `ARCH-CONFIG-01` Independent capability gates

Config v6 adds layered `todo` and `plan` settings using the existing project/global
resolution model. CLI dispatch rejects disabled exact-scope commands; all-scope queries
filter disabled stores. The existing Hook builds only enabled capability fields. Skills
self-gate with `config show` and never write configuration merely because they trigger.

### `ARCH-TRACK-01` Bounded Plan continuity

The read-only Hook selects one active Plan by `updated_at DESC, id`, computes structural
step counts, and returns only its bounded title, current/next step summaries, revision,
and brief command. It does not load Plan reasoning/evidence fields into lifecycle context.

### `ARCH-TODO-CHILD-TIME-01` Direct children and due reminders

Store v16 adds nullable `parent_id` and normalized UTC `target_at` columns plus parent
and due-reminder indexes. Parent creation validates an existing Todo in the same
transaction. The existing read-only Hook queries only open rows at or before SQLite's
current UTC time, orders by `created_at,rowid`, and returns at most three bounded
summaries plus an exact omitted count, only while Todo is enabled.
