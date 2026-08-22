# LWC Todo and Current Plan for Agents - Constraints

Plan ID: `lwc-todo-plan-20260821`  
Status: `executable`  
Updated: `2026-08-21T19:00:49+08:00`

## Authority and Safety

- `RULE-SAFE-01` Planning does not authorize implementation, migration of a real
  database, installation, release, publication, deployment, or external communication.
- `RULE-SAFE-02` Temporary test stores are allowed during implementation. Before the
  first real v14->v15 database open, follow repository safety policy: resolve exact
  command/target/scope/impact/recovery/reversibility, create a recoverable backup or
  checkpoint, then obtain one execution-specific confirmation.
- `RULE-SAFE-03` Do not edit `.lwc/wiki.db`, WAL/SHM, graph sidecars, Agent install
  manifests, or installed Skill directories directly. Use code, CLI, and installer
  paths under test.
- `RULE-SAFE-04` Preserve unrelated untracked/modified files and preexisting Agent
  configuration. Installer tests must run in isolated homes.

## Engineering Constraints

- Rust 2024, existing dependencies only. Prefer current helpers and SQLite constraints
  over new abstractions or dependencies.
- Keep Todo and Plan in cohesive Store modules; do not add a generic task framework,
  repository/service trait, scheduler, background worker, or provider abstraction.
- Clap definitions remain in `src/cli/definitions.rs`; routing remains in
  `src/cli/dispatch.rs`; SQL state truth remains in Store modules.
- Reuse the current JSON error envelope and operation-log pattern. Every error code is
  stable, machine-readable, and tested.
- Keep `src/main.rs` and test-structure limits green. Split modules only by existing
  responsibility boundaries, not speculative reuse.
- No change to MCP's single read-only `lwc_explore`, viewer, Work, Wiki, temporal
  memory, Office, conversion, graph, changeset, or configuration command semantics.

## Data and Compatibility

- One forward migration v14->v15; bootstrap and migration call the same schema helper.
- Migration must be one immediate transaction. Metadata and `PRAGMA user_version`
  change only after all tables/indexes/FTS are valid.
- Existing rows, source IDs, pages, memory events, operation history, changesets, and
  search indexes remain unchanged.
- All Todo/Plan child rows have foreign keys with deliberate cascade from their owning
  entity. No v1 delete command exposes that cascade.
- Create identity uses optional `request_id` plus canonical fingerprint. Mutation
  identity uses opaque ID plus mandatory revision CAS.
- Read-only old stores are not silently migrated by Hook or `--scope all`; follow the
  current store-open contract.
- Old binaries reject v15. Downgrade is backup restore, not an in-place down migration.
- Response field names/states/error codes are versioned public CLI contracts once
  released; docs and Skills must use exact names.

## Security and Privacy

- Treat titles, details, cues, constraints, results, reasons, and evidence as user data.
  Do not persist secrets, credentials, raw prompts, hidden reasoning, or transient tool
  logs; Skills must say so explicitly.
- Project `@PATH` JSON must remain inside the canonical project root; global scope and
  stdin/inline semantics reuse the current trust-boundary helper and 64 MiB cap.
- All SQL values are bound parameters. Dynamic SQL is limited to verified fixed schema
  identifiers already used by Store internals.
- Hook returns counts, at most three bounded due Todo summaries, and one bounded active Plan continuity summary. Todo cue/detail and
  Plan objective/done criteria/constraints/verification/results/blockers/evidence/tags
  do not cross into lifecycle context.
- No network request, subprocess execution, file write outside existing Agent install
  paths, or automatic action is part of Todo/Plan commands.

## Performance and Capacity

- Add indexes for Todo state/update ordering, Todo tags, Plan state/update ordering,
  Plan tags, Plan step order/status, and Plan history by plan/revision.
- Search uses FTS5, never `%query%` scans over canonical text. List/current queries use
  indexed filters and bounded `LIMIT/OFFSET`.
- Cross-scope reads query at most `limit + offset` candidates per store before merge;
  they never load entire stores.
- Limits: list/search/current `1..=1000`, default 100; Plan steps `1..=100`; brief
  terminal summaries at most 20; JSON input at most existing 64 MiB.
- Focused query-plan tests seed at least 10,000 Todos and 1,000 Plans and reject full
  scans on the primary list/search/current paths. A focused Hook timing fixture compares
  counts-enabled readiness with the existing readiness control over paired runs; median
  paired p95 ratio must be <=1.25 and every feature run must actually query seeded
  Todo/Plan state.
- No full benchmark suite is required unless these focused gates fail or implementation
  changes shared search/tokenizer code.

## Observability

- Successful mutations append one existing `operations` row with action, entity ID,
  revision, and non-secret summary. Plan also appends normalized `plan_history`.
- Responses expose action (`created`, `updated`, `unchanged`, transition name), scope,
  database, entity ID, state, revision, and timestamps.
- Conflicts return expected/current revision or request key context without echoing full
  sensitive payloads.
- Reads, Hook calls, and unchanged idempotent replays do not create operations.
- Tests use counts/hashes/selected rows as evidence; do not dump complete Todo/Plan
  bodies into logs when a structural assertion suffices.

## Documentation Rules

- Update `README.md` and `README.zh-CN.md` together with equivalent facts and examples.
- Update `docs/agent-workflow.md` with trigger/skip/query/update/completion rules and
  the exact Hook boundary.
- Canonical Skills are authoritative for Agent behavior; integration copies must be
  byte-identical, not manually paraphrased.
- Do not claim release availability, installed state, background reminder delivery, semantic
  validation, or cross-domain synchronization.
- Implementation progress belongs in this bundle's manifest/handoff until complete.

## Prohibited Actions

- Do not merge Todo and Plan into one state machine or infer automatic conversion.
- Do not encode canonical entities/steps/tags as one opaque JSON blob.
- Do not add Todo `in_progress`, Plan backlog semantics, priority/owner/dependencies,
  Plan reopen, delete/archive/purge, or configurable retention in v1. Todo `target_at`
  and its fixed bounded Hook reminder are the sole time/reminder capability.
- Do not add new platform Hook events, lifecycle mutations, unbounded Hook content,
  standalone schedulers/notifications, or writable MCP tools.
- Do not silently resolve revision conflicts, overwrite terminal steps, or weaken
  completion gates.
- Do not run migration or feature commands against the live project/global database
  merely to smoke-test implementation.
- Do not run unrelated full test suites after every change; expand tests only when an
  actual edited shared path or discovered caller justifies it.
- Do not bump versions, commit, push, tag, publish crates/npm/plugins/Homebrew, install
  locally, or deploy under this plan without a separate user instruction.

- `RULE-CONFIG-01` Built-in Todo and Plan settings are independently disabled. A Hook
  or Skill trigger cannot enable either capability. Disabled capabilities contribute
  no Hook fields, and all-scope reads cannot expose their records.
