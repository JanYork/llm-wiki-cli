# LWC Todo and Current Plan for Agents - Requirements

Plan ID: `lwc-todo-plan-20260821`  
Status: `executable`  
Updated: `2026-08-21T19:00:49+08:00`

## Problem and Outcome

Agents using LWC need two small durable control surfaces that survive sessions without
mixing unlike concepts:

- **Todo** is a future/deferred backlog. It answers “what useful work remains?”
- **Plan** is the current execution plan for already-authorized work. It answers “what
  are we doing now, what is done, what is blocked, and what comes next?”

The outcome is a project/global SQLite-backed CLI plus two installable Skills. Agents
can create, find, resume, revise, and finish work through stable JSON contracts while
users can inspect the same state. The implementation is successful when the focused
acceptance matrix in `05-acceptance-plan.md` passes without changing Wiki, Work,
temporal-memory, MCP, or release semantics.

## Source Evidence

| Source | Evidence | Status |
| --- | --- | --- |
| User decisions, 2026-08-21 | Todo is future/deferred work; Plan is the current execution plan; they are separate; both use title and tags as indexes; add `using-todo` and `using-plan` Skills. | verified |
| `src/cli/definitions.rs`, `src/cli/dispatch.rs` | Clap owns command grammar; successful commands return JSON; scoped JSON input already supports inline, stdin, and `@PATH`. | verified |
| `src/store/types.rs`, `src/store/schema.rs`, `src/store/migrations.rs` | SQLite schema is currently v14, bootstrapped and migrated transactionally, with explicit schema validation. | verified |
| `src/store/temporal_memory.rs` | Existing patterns cover normalized tables, FTS5, random stable IDs, `request_id` idempotency, immediate transactions, and project/global reads. | verified |
| `src/agent.rs`, `src/agent/install.rs`, `src/agent/targets/` | One read-only lifecycle readiness path serves twelve Agent targets; native install currently bundles one canonical `using-lwc` Skill. | verified |
| `skills/using-lwc/`, `integrations/*-lwc/skills/`, `tests/integrations.mjs` | Canonical Skill bytes are mirrored into Codex, Claude, and Pi packages and checked for parity. | verified |
| [ReAct](https://arxiv.org/abs/2210.03629) | Action plans should be updated from observations instead of treated as immutable scripts. | verified |
| [Magentic-One](https://arxiv.org/abs/2411.04468) | Long tasks benefit from explicit progress tracking and replanning after errors or stalls. | verified |
| [Agent Planning Benchmark](https://arxiv.org/abs/2606.04874) | Planning needs feedback-conditioned refinement and robust behavior under broken, irrelevant, or infeasible actions. | verified |
| [AdaPlan-H](https://aclanthology.org/2026.findings-acl.77/) and [COMPASS](https://aclanthology.org/2026.acl-long.152/) | Coarse-to-fine plans and concise evolving context motivate bounded steps plus a compact resume brief. | verified |
| [Prospective memory offloading](https://pubmed.ncbi.nlm.nih.gov/36201839/) | Explicit external cues motivate Todo's optional cue field; silent deletion is excluded. | verified |

## Requirements

- `REQ-TODO-01` Todo MUST be an independent durable backlog entity with a stable ID,
  action-oriented title, zero or more tags, optional cue and detail, revision,
  timestamps, and state `open`, `done`, or `cancelled`.
- `REQ-TODO-02` Agents MUST be able to add, list, search, show, and update Todos by
  title/tag/cue without scanning unrelated Wiki or temporal-memory records.
- `REQ-TODO-03` Todo completion, cancellation, and reopening MUST be explicit,
  atomic, idempotent for already-achieved state, and retain result or reason where
  applicable.
- `REQ-PLAN-01` Plan MUST be an independent current-execution entity with a stable
  ID, title, tags, objective, non-empty `done_when`, constraints, revision, timestamps,
  and state `active`, `completed`, or `abandoned`.
- `REQ-PLAN-02` An active Plan MUST contain 1..=100 ordered, stable-ID steps; exactly
  one step is focal (`in_progress` or `blocked`) until every remaining step is terminal.
- `REQ-PLAN-03` `plan brief` MUST return bounded resume context: objective,
  `done_when`, constraints, completed results, focal step/blocker, next step, and
  revision without raw chain-of-thought.
- `REQ-PLAN-04` Agents MUST advance one milestone atomically and MUST be able to
  revise future work with an explicit reason while preserving completed/skipped work
  and append-only revision history.
- `REQ-PLAN-05` Completing a Plan MUST require a non-empty result, no pending,
  in-progress, or blocked steps, and an explicit assertion that `done_when` was
  checked; LWC stores the assertion and evidence text but does not judge semantic
  correctness.
- `REQ-CLI-01` `lwc todo ...` and `lwc plan ...` MUST follow existing structured
  stdout/stderr contracts, bounded pagination, strict input validation, and the
  existing inline/stdin/`@PATH` JSON boundary for structured creates and revisions.
- `REQ-SCOPE-01` All mutations and exact-ID reads MUST use one `project` or `global`
  scope; only bounded list/search/current reads may accept `--scope all`; every Todo
  and Plan command MUST reject `--changeset`.
- `REQ-DATA-01` Canonical storage MUST be normalized SQLite with indexed fields and
  FTS5 search, added by one transactional schema migration with bootstrap and
  read-only validation parity.
- `REQ-CONC-01` Create retries MUST support `request_id` idempotency, and mutations
  of existing entities MUST use the returned revision as compare-and-swap input so
  concurrent Agents cannot silently overwrite each other.
- `REQ-HOOK-01` The existing lifecycle Hook MUST expose Todo and Plan as separate
  top-level objects and MAY expose bounded Todo open counts and Plan tracking metadata.
  For the most recently updated active Plan it MAY include the
  Plan ID/title/revision, structural progress, current and next step ID/title/status,
  and a brief command. It MUST omit Todo cue/detail and Plan objective, done criteria,
  constraints, verification text, results, blockers, evidence, and tags; it MUST NOT
  mutate state or register new platform hook types.
- `REQ-SKILL-01` Canonical `using-todo` and `using-plan` Skills MUST teach trigger,
  skip, query, milestone-update, completion, safety, and handoff behavior and MUST be
  installed/refreshed/uninstalled byte-identically for every currently supported
  Agent integration and native package.
- `REQ-DOC-01` English and Chinese user docs plus Agent workflow docs MUST explain
  the Todo/Plan distinction, command surface, scopes, concurrency, and boundaries
  from Work, temporal memory, and Wiki.
- `REQ-TEST-01` Focused tests MUST cover schema migration, CLI/store state machines,
  scope and concurrency failures, Hook non-mutation, Skill policy, installer
  restoration/idempotency, and package parity; unrelated graph/view/Office suites
  MUST NOT be run after each slice.

## Scope and Non-goals

### In scope

- `lwc todo add|list|search|show|update|done|cancel|reopen`.
- `lwc plan create|current|list|search|show|brief|advance|block|revise|complete|abandon`.
- Project/global persistence, bounded cross-scope discovery, JSON input/output,
  concurrency protection, Hook readiness summaries, new Skills, docs, and focused
  tests.

### Non-goals

- No Todo priority, recurrence, assignee, dependency graph, automatic expiry, reminder
  daemon, scheduler, or silent deletion. The optional target time and bounded Hook
  reminder are the only deadline/reminder behavior.
- No Plan auto-execution, LLM planning engine, hidden reasoning capture, arbitrary
  DAG, nested plans, owner assignment, or autonomous completion judgment.
- No automatic Todo-to-Plan or Plan-to-Todo conversion.
- No automatic write to Wiki, temporal memory, Source, Work, or operation outside
  the Todo/Plan transaction; explicit Agent decisions remain required.
- No writable MCP tool, viewer editing, additional lifecycle hook registration,
  new dependency, or release/version change in this feature plan.

## Assumptions and Open Questions

- `ASM-001` Todo/Plan tag syntax will reuse the verified `1..=128` non-control
  character validation rule from `src/store/tags.rs`, but task tags remain in their
  own join tables and do not become Wiki strong-load tags. `TASK-001` locks this in
  with boundary tests before schema code.
- `ASM-002` Existing random 16-byte lowercase hexadecimal IDs, CJK-aware tokenizer,
  `operations` audit entries, and 64 MiB JSON input helper are sufficient. `TASK-001`
  verifies each reuse point; any mismatch updates this plan before implementation.
- No unresolved product or architecture question blocks `TASK-001`. Release version,
  packaging publication, and deployment are intentionally outside this plan.

## Traceability Seeds

The complete per-requirement matrix is in `plan-manifest.json`. Representative paths:

- `REQ-TODO-01` -> `ARCH-TODO-01` -> `FLOW-TODO-CREATE` -> `TASK-002` -> `AC-TODO-01` -> `TEST-TODO-01`
- `REQ-PLAN-04` -> `ARCH-PLAN-01` -> `FLOW-PLAN-ADVANCE` -> `TASK-004` -> `AC-PLAN-04` -> `TEST-PLAN-04`
- `REQ-SKILL-01` -> `ARCH-SKILL-01` -> `FLOW-SKILL-INSTALL` -> `TASK-007` -> `AC-SKILL-01` -> `TEST-SKILL-01`

## 2026-08-22 Configuration Amendment

- `REQ-CONFIG-01` Todo and Plan MUST be independently opt-in with built-in defaults of
  disabled, global/project inheritance, domain-specific disabled errors, and all-scope
  reads that include only enabled stores. The lifecycle Hook MUST omit each disabled
  capability entirely. Skills MUST inspect effective configuration and MUST NOT treat
  semantic triggering as consent to enable a capability.
- `REQ-TRACK-01` When Plan is enabled and an active Plan exists, lifecycle readiness
  MUST expose one deterministic bounded `plan.tracking` for the most recently updated
  Plan, including progress, current step, planned next step, revision, and brief command.
- `REQ-TODO-04` A Todo MAY reference one existing direct parent Todo. Exact reads MUST
  expose direct children and list/search MUST support direct-parent filtering. Parentage
  is immutable organization only: no recursive expansion, cascade, dependency, or Plan conversion.
- `REQ-TODO-05` A Todo MAY carry a normalized RFC3339 `target_at`. When and only when
  Todo is enabled, lifecycle readiness MUST expose the oldest-created three due open
  Todos and the exact number omitted. Closed/future Todos and cue/detail text MUST not appear.
