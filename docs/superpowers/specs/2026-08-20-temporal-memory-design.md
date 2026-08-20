# LWC Temporal Memory Design

## Status

Implemented on 2026-08-20 after the preceding design discussion. This document
is the shipped behavior contract for the normalized SQLite model, CLI, retention
policy, Agent routing, and read-only Hook boundary.

## Goal

Add a low-friction temporal-memory layer to LWC so an Agent can persist a
sparse event capsule in one fast command, recall bounded historical context,
and receive deterministic consolidation candidates without maintaining logs or
running a background model.

The feature succeeds only if it improves time-sensitive recall while preserving
LWC's existing SQLite authority, scope isolation, auditability, and read-only
MCP/Hook safety boundaries.

## Non-goals

- Do not store chat transcripts or raw chain-of-thought.
- Do not treat one `summary` or opaque JSON blob as the canonical memory.
- Do not infer semantic duplicates, hidden causality, importance, or Wiki
  synthesis.
- Do not merge two events because their text is similar.
- Do not add a daemon, embedded LLM, vector database, temporal SQLite extension,
  or a new dependency.
- Do not mix temporal events into the existing Wiki/source FTS result set.
- Do not make lifecycle Hooks mutate memory.

## Commands

### Record

```bash
lwc remember --json '{...}'
lwc remember --json - < event.json
lwc remember --json @event.json
```

`remember` accepts one UTF-8 JSON event capsule as an inline value, stdin (`-`),
or a local file (`@PATH`). All three forms have identical validation, scope,
size limits, and response semantics; stdin/file forms avoid shell escaping for
non-coding Agent bridges. The minimum valid capsule has
non-empty `type`, non-empty `context`, and at least one useful semantic entry in
`observed`, `decision`, `constraints`, `learned`, `unresolved`, `outcome`, or
`changes`.

Optional fields are `request_id`, `occurred_at`, `valid_from`, `valid_to`,
`pinned`, `evidence`, and explicit `relations`. Unknown fields are rejected.
Timestamps are accepted in a SQLite-recognized ISO-8601 form and normalized to
UTC. LWC supplies `id` and `recorded_at`.

`request_id` is an idempotency key for one submission attempt:

- the same key and same canonical capsule returns the original event with
  `created=false`;
- the same key with different content returns `memory_request_conflict` and
  changes nothing;
- absent or different keys always create distinct events, even when the text is
  identical or similar.

The successful response includes the event, retention effects, current storage
pressure, and at most three deterministic consolidation hints. Recording never
runs recall or semantic linking first.

### Recall and inspect

```bash
lwc memory recall "why did the deployment change" --limit 5
lwc memory recall "payment retry" --since 2026-08-01 --until 2026-08-31
lwc memory show EVENT_ID
lwc memory status
```

`recall` uses a separate contentless FTS5 index with LWC's existing CJK-bigram
tokenization. It supports project, global, and merged `--scope all` reads. It is
bounded, read-only, and excludes events superseded by an explicit `supersedes`
relation unless `--include-superseded` is supplied.

Ranking is deterministic and explainable: lexical match is primary; explicit
useful/not-useful feedback adjusts already-matching results; occurrence time and
current/superseded state break ties. Mere retrieval never strengthens memory.
Each result reports why it matched and returns the event's semantic channels,
changes, evidence, and relations without reconstructing an opaque transcript.

### Feedback

```bash
lwc memory feedback EVENT_ID --signal useful --reason "prevented a repeated failed attempt"
```

Feedback is append-only and requires `useful` or `not-useful` plus a non-empty
reason. It supplies observable reuse metrics and a bounded rank adjustment; it
does not rewrite the event.

### Maintenance

```bash
lwc memory maintain
```

Maintenance enforces the same configured retention policy used after a
successful `remember`. It reports counts and logical payload bytes removed. It
never claims semantic compression.

## Canonical relational model

SQLite remains authoritative. Schema version 14 adds:

- `memory_events`: identity, request id and fingerprint, type, context,
  occurrence/record/validity timestamps, pinned flag, and logical payload bytes.
- `memory_fragments`: ordered `observed`, `decision`, `constraint`, `learned`,
  `unresolved`, and `outcome` text.
- `memory_changes`: ordered subject, before/after values, and reason.
- `memory_evidence`: ordered domain-neutral reference and optional excerpt.
- `memory_relations`: explicit event-to-event `supersedes`, `contradicts`,
  `resolves`, `supports`, or `related` edge plus optional basis.
- `memory_feedback`: append-only useful/not-useful reuse outcomes.
- `memory_hint_state`: operational cooldown state for emitted candidate keys.
- `memory_state`: one aggregate row for attempts, inserted events, idempotent
  replays, eviction counts, event count, and logical payload bytes.
- `memory_fts`: derived contentless FTS5 index containing context and all
  searchable semantic text.

Foreign keys cascade only when a configured retention policy evicts an event.
Corrections and revisions otherwise append a new event and explicit relation;
they never update old semantic content.

`request_id` is the only unique idempotency key and is unique only inside one
store (project or global). The canonical capsule fingerprint is non-unique and
exists solely to decide whether a replay of that same `request_id` has identical
content. It must never suppress an insert with an absent or different
`request_id`, including a byte-identical capsule.

The migration is transactional, upgrades existing version-13 stores, and adds
the same schema to fresh stores. Existing Wiki/source rows and indexes remain
unchanged.

## Configuration and retention

Configuration remains layered: a project value overrides global, which
overrides the built-in default.

```bash
lwc --scope global config set \
  --memory enabled \
  --memory-max-age-days 365 \
  --memory-max-bytes 268435456
```

Built-in defaults are enabled, 365 days, and 256 MiB of logical event payload.
Project scope may override or inherit them. `max_age_days` and `max_bytes` must
be positive when memory is enabled.

`lwc config show` reports the effective memory setting, effective limits, and
`built-in`/`global`/`project` origin alongside the existing configuration.
`lwc --scope project config unset --memory` restores global-or-built-in
inheritance; global unset restores the built-in defaults. Memory options are
deployment-local and remain unavailable inside a changeset.

The configured policy is durable authorization for automatic eviction of
ordinary temporal events. It does not authorize deleting Sources or Wiki pages.

After each successful record, LWC performs indexed, deterministic maintenance
inside the same transaction:

1. remove unprotected events older than `max_age_days`;
2. if the new capsule would exceed `max_bytes`, remove the oldest unprotected
   events until it fits;
3. if protected events leave insufficient capacity, roll back the new event and
   return `memory_capacity_exceeded` rather than silently losing it.

An event is protected when it is pinned, has an unresolved fragment, or
participates in an unresolved explicit contradiction. LWC does not guess
whether evidence is unique. Automatic deletion records only identifiers,
counts, and byte totals in the operation log, never event text.

Recall applies the same age rule as a query filter, so expired ordinary events
stop appearing even when no later write has yet physically removed them.
`remember` and explicit maintenance perform the physical deletion.

This design prevents unbounded logical growth without relying on semantic
deduplication. SQLite/WAL physical reclamation remains the separate existing
`lwc maintenance compact` operation.

## Consolidation hints

LWC may identify review candidates; it may not consolidate them itself. A hint
is deterministic and explicitly explains its trigger:

- at least five events share the exact normalized type and context;
- an explicit contradiction or supersession chain needs review;
- an unresolved event is at least 14 days old;
- temporal storage pressure is at least 80 percent.

Hints are computed and emitted only during an existing `remember` mutation, so
read-only lifecycle Hooks stay read-only. At most three are returned. A stable
candidate key and seven-day cooldown prevent repeated reminders. Similar text
alone never creates a hint and never causes a merge. Automatic and explicit
maintenance delete expired cooldown rows and candidate rows that no longer map
to retained events, so operational hint state is bounded too.

Agent guidance instructs the caller to turn a candidate into a Wiki synthesis
only when current work has actually established a reusable conclusion. The
Agent may instead add a resolving event, pin it, or ignore the candidate.

## Agent routing guidance

The canonical `using-lwc` Skill gains
`skills/using-lwc/references/temporal-memory.md`, linked from both `SKILL.md` and
the trigger playbook. The reference uses the repository's standard `Use when`,
`Skip when`, `Minimum workflow`, `Consent boundaries`, and `Completion evidence`
sections. Packaged integration copies remain byte-identical and the existing
Skill policy/parity tests cover the new file.

Record once at a meaningful boundary when future work may need to know what
changed, why, what was tried, the outcome, or what remains unresolved. Skip
routine progress, repeated wording, transient tool output, secrets, stable
current facts already represented in the Wiki, and every ordinary chat turn.

Recall temporal memory first for questions about `before`, `when`, `changed`,
`why`, prior attempts, repeated failures, unresolved work, or incident
timelines. Recall the Wiki first for current architecture, instructions, and
stable facts. Use both when a current conclusion needs its history; inspect a
Source when exact authoritative evidence matters.

Lifecycle context may describe the availability and recommended commands, but
Hooks must not write, consume hints, or expose raw events automatically.

Canonical Skill files remain under `skills/using-lwc`; packaged integration
copies must remain byte-identical.

## Effect and metrics

`lwc memory status` reports operationally useful measures:

- retained/protected/superseded event counts and logical payload bytes;
- configured age/byte limits and pressure ratio;
- record attempts, inserted events, idempotent replays, feedback counts, and
  age/capacity evictions;
- pending consolidation-candidate count without event bodies.

A focused ignored benchmark compares ordinary lexical ordering with temporal
ordering on a deterministic synthetic fixture. Acceptance metrics are:

- fresh top-1 accuracy for time-sensitive queries;
- stale contradiction suppression;
- bounded recall precision;
- protected-event survival after forced eviction;
- false semantic merge rate;
- hint precision/recall;
- record and recall P50/P95 latency.

The initial acceptance gates are:

- false semantic merge rate `0.0`;
- protected survival and bounded recall precision `1.0`;
- stale contradictions never outrank their explicit replacement by default;
- fresh top-1 accuracy at least `0.90` on the labeled fixture;
- hint precision `1.0` and recall at least `0.95` for deterministic triggers;
- median record P95 no worse than 1.25x the same fixture with maintenance
  thresholds inactive.

Counts stored and raw database size are diagnostics, not proof of usefulness.

## Impact-scoped verification

Implementation follows TDD and runs only affected checks:

- focused store unit tests for validation, migration, idempotency, eviction,
  relations, ranking, hints, and metrics;
- one real-CLI integration target for command/scope/config/error contracts;
- Agent Hook/CLI and integration-parity tests only when their canonical
  guidance or context changes;
- the dedicated ignored temporal benchmark;
- `cargo fmt --check`, targeted Clippy for changed targets, `git diff --check`,
  Wiki lint/retrieval, and CodeGraph sync/impact verification.

No full repository test suite is part of this feature acceptance unless a
targeted result exposes a broader regression.
