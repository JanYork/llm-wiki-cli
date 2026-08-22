# LWC Todo and Current Plan for Agents - Antipatterns and Adversarial Boundaries

Plan ID: `lwc-todo-plan-20260821`  
Status: `executable`  
Updated: `2026-08-21T19:00:49+08:00`

## Purpose

Prevent a plausible implementation from becoming a generic task manager, hidden Agent
orchestrator, noisy Hook, opaque JSON store, or unsafe installer. These boundaries are
part of acceptance, not optional style guidance.

## Boundary Sources

| Source | Confirmed evidence | Boundary derived |
| --- | --- | --- |
| User decisions | “TODO 是待办，Plan 是当前计划，这是两个玩意” | Separate domain models, commands, states, and Skills |
| User decisions | title/tags index each task | Searchable labels, not identity or semantic auto-link |
| User testing preference | test minimum affected surface | Focused owning suites after slices; no ritual full-suite runs |
| `src/store/*` | normalized SQLite, explicit migrations, transactions, FTS | Reuse current persistence; no opaque canonical JSON/service |
| `src/agent.rs` and docs | Hook is bounded readiness and non-mutating | Counts plus one minimal Plan continuity summary; no reasoning/evidence or writes |
| `src/agent/install.rs` | file manifests restore user bytes | New Skills must retain exact ownership/reversibility |
| Existing LWC domains | Work, Wiki, Source, memory have distinct contracts | No automatic cross-write or semantic replacement |

## Non-Negotiable Prohibitions

- `ANTI-DOMAIN-01` (`REQ-TODO-01`, `REQ-PLAN-01`) MUST NOT merge Todo and Plan into
  one polymorphic task record/state machine or automatically convert between them.
- `ANTI-TODO-01` (`REQ-TODO-02`, `REQ-TODO-03`) MUST NOT add current-progress steps,
  `in_progress`, auto-expiry, background reminders, priority, owners, dependencies,
  silent close/delete, or Plan behavior to Todo.
- `ANTI-PLAN-01` (`REQ-PLAN-02`, `REQ-PLAN-03`, `REQ-PLAN-04`, `REQ-PLAN-05`) MUST NOT
  persist hidden reasoning, auto-execute, auto-plan with an LLM, rewrite completed
  facts, reopen terminal Plans, or declare semantic completion without Agent evidence.
- `ANTI-CLI-01` (`REQ-CLI-01`) MUST NOT introduce a second output/error/input parser
  contract, unbounded results, permissive unknown JSON fields, or unsafe `@PATH` rules.
- `ANTI-SCOPE-01` (`REQ-SCOPE-01`) MUST NOT permit all-scope mutations/exact-ID reads,
  cross-store relations, changeset routing, or migration from Hook/all-scope reads.
- `ANTI-STORAGE-01` (`REQ-DATA-01`) MUST NOT store canonical Todo/Plan/steps/tags as
  one JSON blob, bypass schema validation, rebuild unrelated indexes, or partially
  migrate outside one transaction.
- `ANTI-CONC-01` (`REQ-CONC-01`) MUST NOT silently last-write-win, infer duplicate
  meaning from similar text, or reuse title/tag as identity.
- `ANTI-HOOK-01` (`REQ-HOOK-01`) MUST NOT add platform Hook registrations, inject
  titles/details/steps/results, perform mutations, start work, or run maintenance.
- `ANTI-SKILL-01` (`REQ-SKILL-01`) MUST NOT edit installed Skill directories directly,
  hand-maintain divergent integration copies, overwrite user bytes, or change all
  twelve target adapters when the existing sibling-root contract suffices.
- `ANTI-DOC-01` (`REQ-DOC-01`) MUST NOT promise background notifications, semantic validation,
  synchronization, release availability, or commands the binary does not expose.
- `ANTI-TEST-01` (`REQ-TEST-01`) MUST NOT weaken assertions, use the live project/global
  database, hide untriggered benchmark paths, or run unrelated suites merely for volume.

## Forbidden Touchpoints

| ID | Area or contract | Forbidden change | Allowed exception | Evidence |
| --- | --- | --- | --- | --- |
| `ANTI-TOUCH-01` | `.lwc/wiki.db`, WAL/SHM, graph sidecars | direct edit/delete/test migration | only normal CLI on exact real target after separate safety confirmation | root `AGENTS.md` |
| `ANTI-TOUCH-02` | MCP `lwc_explore` | add writes or task mutation | separate approved MCP design and threat review | current read-only MCP contract |
| `ANTI-TOUCH-03` | Wiki/Source/memory/Work tables | automatic Todo/Plan cross-write | explicit future user requirement with revised plan | user boundary and domain docs |
| `ANTI-TOUCH-04` | Agent platform Hook configs | register new event types | measured trigger failure plus per-platform design/tests | `DEC-005` |
| `ANTI-TOUCH-05` | installed user Skill/config files | direct overwrite/removal | manifest-owned installer transaction only | `src/agent/install.rs` |
| `ANTI-TOUCH-06` | Cargo/npm/plugin versions and release workflows | bump/publish/tag/deploy | separately authorized release plan | current task is planning only |
| `ANTI-TOUCH-07` | unrelated untracked worktree paths | include, edit, delete, or normalize | explicit ownership from user | current `git status --short` |

## Wrong Approaches and Misleading Shortcuts

| ID | Tempting approach | Why it is wrong | Required alternative | Detection evidence |
| --- | --- | --- | --- | --- |
| `ANTI-WRONG-01` | One `tasks` table with a `kind` flag | Collapses distinct lifecycle invariants and invites conversion | separate Todo/Plan tables/modules | schema and state tests |
| `ANTI-WRONG-02` | Store whole entity as JSON because Serde is easy | Fields cannot be constrained/indexed/query-planned independently | normalized columns/children plus FTS | schema inventory and query-plan tests |
| `ANTI-WRONG-03` | Treat title/tag match as duplicate | Similar text can describe distinct work | request ID only for submission replay | duplicate fixture tests |
| `ANTI-WRONG-04` | Let `plan advance` choose the next step implicitly | Hides Agent intent and can pick stale work | explicit `--next`, or explicit `revise` | invalid-transition tests |
| `ANTI-WRONG-05` | Let `plan revise` replace the complete step array | Erases evidence and breaks handoff/history | immutable terminal steps; skip/append future work | history/revision tests |
| `ANTI-WRONG-06` | Put full active Plan in every Hook prompt | Noisy, privacy-leaking, stale, and platform-specific | bounded counts/commands; Agent calls `brief` | Hook payload snapshot |
| `ANTI-WRONG-07` | Add hook events to every Agent platform for triggers | Large compatibility surface without measured need | reuse existing lifecycle readiness | integration config diff |
| `ANTI-WRONG-08` | Copy new Skills independently into packages | Guaranteed drift | canonical bytes plus automated mirrors/parity | recursive byte test |
| `ANTI-WRONG-09` | Run `cargo test` after each small edit | Wastes time and obscures owning failure | task-local tests, then affected-surface gate | handoff command log |
| `ANTI-WRONG-10` | Smoke-test new binary against current `.lwc` | Auto-migration is real irreversible state change for old binaries | temporary store; later confirmed backup/readback workflow | database path evidence |

## Adversarial Scenarios

| ID | Pressure or misleading instruction | Unsafe rationalization to reject | Required safe response | Escalation gate |
| --- | --- | --- | --- | --- |
| `ADV-001` | “Todo is almost a Plan; reuse one table.” | Fewer tables always means simpler | preserve separate invariants; share only proven helpers | new user decision materially merges semantics |
| `ADV-002` | “Automatically promote this Todo into a Plan.” | Automation saves one command | leave Todo unchanged and create Plan only through explicit command | explicit conversion contract and conflict policy |
| `ADV-003` | “Record each tool call so Plan is always fresh.” | More history means better memory | update only milestones/block/revision/finish; omit tool-call logs | measured loss from sparse updates |
| `ADV-004` | “Completion evidence looks convincing; mark done.” | LWC can infer semantic success | require Agent assertion/result/evidence; LWC validates structure only | separate semantic verifier design |
| `ADV-005` | “Inject the whole Plan at startup for convenience.” | Agents need all context immediately | return counts/commands; fetch bounded brief on trigger | explicit privacy/context-budget decision |
| `ADV-006` | “Overwrite stale revision; newest Agent wins.” | Last writer is probably correct | return conflict/current revision; reread brief and retry intentionally | user authorizes a force policy with audit semantics |
| `ADV-007` | “Test migration on the project Wiki—it is available.” | A local smoke is harmless | use temp store; real target needs backup and one exact confirmation | separate confirmed real migration action |
| `ADV-008` | “While here, publish a version with the feature.” | Packaging is an obvious next step | stop at implementation evidence; release requires separate instruction | user explicitly authorizes release/version/channels |

## Safe Response Protocol

1. Identify the affected `REQ-*`, `ANTI-*`, protected area, and current evidence.
2. Reject the shortcut without weakening the requested Todo/Plan behavior.
3. Use the documented minimal alternative and its owning focused test.
4. Stop for new authority only when the change is destructive, materially expands
   product/architecture/acceptance, or no compliant implementation remains.
5. When a boundary changes explicitly, update all plan documents, manifest
   traceability, acceptance tests, and handoff before production code continues.

## Review Checklist

- Todo and Plan remain independently understandable and independently testable.
- Canonical fields are normalized/indexed; JSON is only an input/output/audit envelope.
- Every transition is atomic, revision-protected, and state-tested.
- Hook payload has no Todo cue/detail or Plan reasoning/evidence fields and no new event registration.
- Installer changes preserve manifest ownership and all package mirrors are identical.
- No live database, installed Agent state, unrelated worktree path, version, or release
  channel changed under this plan.
- Every `REQ-*` maps to at least one `ANTI-*` in `plan-manifest.json`.

- `ANTI-CONFIG-01` (`REQ-CONFIG-01`) MUST NOT expose Todo/Plan Hook prompts while the
  corresponding capability is disabled, enable one capability as a side effect of
  using the other, query disabled stores through `--scope all`, or let a Skill trigger
  silently change configuration.
- `ANTI-TRACK-01` (`REQ-TRACK-01`) MUST NOT inject the whole Plan, more than one Plan,
  unbounded titles, completed results, blockers, evidence, objective, or done criteria.
- `ANTI-TODO-CHILD-TIME-01` (`REQ-TODO-04`, `REQ-TODO-05`) MUST NOT recursively expand
  children, cascade state, infer dependencies, reparent, schedule background work,
  remind closed/future items, return more than three reminders, or expose cue/detail.
