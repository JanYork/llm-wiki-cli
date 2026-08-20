# Temporal Memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a bounded, normalized, useful temporal-memory capability with fast Agent recording, deterministic recall, retention, hints, metrics, and precise usage guidance.

**Architecture:** SQLite version 14 owns normalized append-only event tables and a separate contentless FTS5 index. `lwc remember` performs validated transactional recording, request retry idempotency, and retention; `lwc memory` exposes bounded reads, explicit feedback, status, and maintenance. Existing lifecycle Hooks remain read-only and only advertise capability; the canonical Skill teaches when to record or recall.

**Tech Stack:** Rust 2024, rusqlite/SQLite FTS5, clap, serde/serde_json, sha2, existing LWC scope/config/Hook infrastructure, Rust integration tests, shell/Node Skill parity tests.

**Execution environment:** Edit and inspect source only on the local checkout. Run
every Cargo compile, test, Clippy invocation, runtime acceptance, and benchmark
on the arm64 Pro over direct SSH at `muyouzhi@macmini-pro.v6.fhub.cn`. Stage the
working tree in an isolated Pro temporary directory and reuse a dedicated Cargo
target cache; do not modify the Pro's existing
`/Users/muyouzhi/workspace/my/lwc` checkout. Non-compiling local checks such as
`git diff --check` remain local.

---

## File map

- Create `src/store/temporal_memory.rs`: capsule types, validation, normalized persistence, FTS recall, relations, feedback, hints, retention, and status.
- Modify `src/store/mod.rs`: include the focused temporal-memory store module.
- Modify `src/store/types.rs`: bump canonical store version to 14.
- Modify `src/store/schema.rs`: add version-14 tables/indexes for fresh stores and validate their presence.
- Modify `src/store/migrations.rs`: transactional version-13 to version-14 migration.
- Modify `src/store/search_state.rs`: include temporal tables in exact changeset schema inventory while leaving their rows outside sparse changesets.
- Modify `src/config.rs`: version-5 layered memory configuration and validation.
- Modify `src/cli/definitions.rs`: add `remember`, `memory` subcommands, and memory config flags.
- Modify `src/cli/dispatch.rs`: route scoped write/read commands and merge `--scope all` recall results.
- Modify `src/cli/helpers.rs`: bounded inline/stdin/`@file` JSON input helper.
- Modify `src/agent.rs`: expose read-only temporal-memory readiness and commands.
- Modify `src/agent/install.rs`: bundle the new canonical capability reference for direct Agent installs.
- Create `tests/temporal_memory.rs`: real CLI acceptance and storage invariants.
- Create `tests/temporal_memory_benchmark.rs`: ignored deterministic usefulness/latency benchmark.
- Create `skills/using-lwc/references/temporal-memory.md`: standard capability guidance.
- Modify `skills/using-lwc/SKILL.md`, `skills/using-lwc/references/trigger-playbook.md`, `skills/using-lwc/references/operations-manual.md`, and `tests/using_lwc_policy.sh`: route and enforce the new guidance.
- Mechanically mirror canonical Skill files into `integrations/{codex-lwc,claude-lwc,pi-lwc}/skills/using-lwc/`.
- Modify `tests/agent_hooks.rs` and `tests/integrations.mjs`: readiness and packaged parity acceptance.
- Modify `tests/agent_cli.rs`: direct-install skill-tree acceptance.
- Modify `tests/cli/core.rs`, `tests/cli/graph.rs`, `tests/cli/source_diff.rs`, and `tests/storage_regressions.rs`: public command and config/store version contracts.
- Modify `README.md`, `README.zh-CN.md`, and `docs/agent-workflow.md`: public command and retention contract.
- Update project Wiki page `lwc-biomimetic-temporal-memory-design` only after implementation evidence is verified.

## Task 1: Lock CLI, schema, and configuration contracts

**Files:**
- Create: `tests/temporal_memory.rs`
- Modify: `src/store/types.rs`
- Modify: `src/store/schema.rs`
- Modify: `src/store/migrations.rs`
- Modify: `src/store/search_state.rs`
- Modify: `src/config.rs`
- Modify: `src/cli/definitions.rs`
- Modify: `src/cli/dispatch.rs`
- Modify: `src/cli/helpers.rs`
- Modify: `src/store/mod.rs`
- Create: `src/store/temporal_memory.rs`
- Modify: `tests/cli/core.rs`
- Modify: `tests/cli/graph.rs`
- Modify: `tests/cli/source_diff.rs`
- Modify: `tests/storage_regressions.rs`

- [ ] **Step 1: Write failing configuration and migration tests**

Add real-CLI tests proving:

```rust
#[test]
fn memory_config_is_layered_validated_and_unsettable() { /* show/set/unset */ }

#[test]
fn version_13_store_migrates_temporal_tables_transactionally() { /* open old DB */ }
```

Assert built-in `enabled`, `365`, `268435456`, origin changes across global and
project values, disabled behavior, invalid zero limits, and `config unset
--memory`. For migration, initialize a store, remove only the future temporal
schema in test setup, set format/user version to 13, open it through the real
CLI, then assert version 14 and every new table/index.

Update existing hard-coded config/store version assertions and the exact sparse
changeset table inventory. Add a regression proving a changeset begun after
temporal events exist can still commit an unrelated page without removing or
copying temporal rows.

- [ ] **Step 2: Run focused tests and verify RED**

```bash
cargo test --test temporal_memory memory_config_is_layered_validated_and_unsettable -- --nocapture
cargo test --test temporal_memory version_13_store_migrates_temporal_tables_transactionally -- --nocapture
```

Expected: RED because memory flags/schema do not exist.

- [ ] **Step 3: Implement only schema and layered config**

Add `MemorySetting::{Disabled,Enabled,Inherit}` and:

```rust
pub struct MemorySettings {
    pub setting: MemorySetting,
    pub max_age_days: u32,
    pub max_bytes: u64,
}
```

Bump config version to 5 with explicit v4 migration. Mirror the existing trans
layering pattern; `config show` includes effective values and origin, `set`
requires `--memory` when memory-specific values are present, and `unset
--memory` restores inheritance.

Bump SQLite `USER_VERSION` to 14. Bootstrap and migration create the exact
normalized tables, ordinary indexes, aggregate state row, and contentless
`memory_fts`. Add no JSON payload column and no dependency.

- [ ] **Step 4: Re-run both tests and verify GREEN**

- [ ] **Step 5: Run affected existing config/migration tests**

```bash
cargo test --test cli config_ -- --nocapture
cargo test --test storage_regressions migration -- --nocapture
cargo test --test cli source_diff -- --nocapture
```

## Task 2: Record a normalized event with exact retry idempotency

**Files:**
- Modify: `tests/temporal_memory.rs`
- Modify: `src/store/temporal_memory.rs`
- Modify: `src/cli/definitions.rs`
- Modify: `src/cli/dispatch.rs`
- Modify: `src/cli/helpers.rs`

- [ ] **Step 1: Write failing record-path tests**

Cover separately:

```rust
#[test] fn remember_persists_each_semantic_channel_in_relational_rows() {}
#[test] fn remember_accepts_inline_stdin_and_scoped_at_file_json() {}
#[test] fn same_request_and_payload_is_idempotent_across_processes() {}
#[test] fn changed_request_replay_conflicts_without_mutation() {}
#[test] fn identical_capsules_without_the_same_request_id_remain_distinct() {}
#[test] fn remember_rejects_unknown_empty_or_malformed_capsules() {}
#[test] fn remember_rejects_scope_all_and_changesets() {}
```

Use Chinese values, null/omitted optional fields, ordered changes/evidence, and
one explicit relation to a prior event. Inspect SQLite only in the test to prove
that no opaque canonical payload exists and all child rows are ordered.

- [ ] **Step 2: Run the six focused tests and verify RED**

```bash
cargo test --test temporal_memory remember_ -- --nocapture
```

Expected: RED because `remember` is unknown.

- [ ] **Step 3: Implement minimal recording**

Use `#[serde(deny_unknown_fields)]` wire structs. Normalize timestamps through
SQLite, reject empty required/items, require one semantic channel/change, and
compute a canonical SHA-256 fingerprint excluding `request_id` and generated
timestamps. `request_id` is the only unique retry key; fingerprint is not
unique. Insert the event and child rows in one immediate transaction, build one
FTS row with existing tokenizer helpers, update aggregate counters, record an
operation without text, and return the stored event.

The bounded JSON helper accepts inline text, `-`, or `@PATH`; project file input
must resolve inside the active project root. Keep changesets unsupported for
event writes.

- [ ] **Step 4: Re-run record tests and verify GREEN**

## Task 3: Recall current history and preserve explicit revisions

**Files:**
- Modify: `tests/temporal_memory.rs`
- Modify: `src/store/temporal_memory.rs`
- Modify: `src/cli/definitions.rs`
- Modify: `src/cli/dispatch.rs`

- [ ] **Step 1: Write failing recall/show/feedback tests**

```rust
#[test] fn recall_is_bounded_cjk_searchable_and_time_filterable() {}
#[test] fn superseding_event_replaces_old_default_result_but_history_remains() {}
#[test] fn scope_all_merges_project_and_global_temporal_results() {}
#[test] fn feedback_is_append_only_and_adjusts_only_matching_results() {}
#[test] fn show_returns_the_complete_capsule_and_relations() {}
#[test] fn recall_scope_all_is_read_only_and_feedback_rejects_all_or_changesets() {}
```

The supersession fixture must prove pattern completion: a query that lexically
matches the old event still returns its explicit replacement and hides the old
event by default; `--include-superseded` returns both. A near-paraphrase about a
different entity must remain a separate row.

- [ ] **Step 2: Run recall tests and verify RED**

```bash
cargo test --test temporal_memory recall_ -- --nocapture
cargo test --test temporal_memory superseding_ -- --nocapture
cargo test --test temporal_memory feedback_ -- --nocapture
```

- [ ] **Step 3: Implement bounded deterministic reads**

Tokenize the query with existing CJK helpers, query `memory_fts`, load complete
capsules in one bounded batch, follow explicit `supersedes` relations, filter by
normalized since/until bounds, apply small explicit feedback adjustment, and
sort deterministically by current state, lexical rank, feedback, occurrence
time, scope, and ID. Apply configured age expiry as a read filter even before
physical maintenance. Recall remains read-only in every scope and mere
retrieval is neither persisted nor counted as useful feedback.

Validate every relation target and enum. Feedback inserts a new row and never
changes event text.

- [ ] **Step 4: Re-run recall tests and verify GREEN**

## Task 4: Enforce finite retention and emit review-only hints

**Files:**
- Modify: `tests/temporal_memory.rs`
- Modify: `src/store/temporal_memory.rs`

- [ ] **Step 1: Write failing retention/status/hint tests**

```rust
#[test] fn age_retention_evicts_only_expired_unprotected_events() {}
#[test] fn byte_budget_evicts_oldest_unprotected_and_rolls_back_when_blocked() {}
#[test] fn unresolved_pinned_and_open_contradiction_events_are_protected() {}
#[test] fn exact_context_cluster_yields_a_candidate_without_merging_events() {}
#[test] fn hints_are_bounded_cooled_down_and_pruned() {}
#[test] fn memory_status_reports_pressure_outcomes_and_no_event_text() {}
#[test] fn memory_maintain_rejects_scope_all_and_changesets() {}
```

Use very small configured byte limits and explicit timestamps so tests are
deterministic. Assert successful writes are retained, failed capacity writes
leave counters/rows unchanged, and operational logs never contain capsule text.

- [ ] **Step 2: Run retention tests and verify RED**

```bash
cargo test --test temporal_memory retention -- --nocapture
cargo test --test temporal_memory budget -- --nocapture
cargo test --test temporal_memory hint -- --nocapture
cargo test --test temporal_memory memory_status -- --nocapture
```

- [ ] **Step 3: Implement indexed retention and candidates**

Within the remember transaction, delete expired eligible rows, then evict oldest
eligible rows until the new event fits. Eligibility is expressed in SQL from
pinned/unresolved/open-contradiction state; no semantic score or scan. If room
cannot be made, return `memory_capacity_exceeded` and roll back. Keep logical
payload/event counters correct through explicit helper calls or SQLite triggers,
whichever is smaller and transactionally testable.

Generate only four deterministic hint classes from the spec, return at most
three, and record stable cooldown keys only during remember. Maintenance prunes
expired/orphan cooldown rows. No hint path merges or edits event content.

- [ ] **Step 4: Re-run retention/status/hint tests and verify GREEN**

## Task 5: Teach Agents without changing Hook safety

**Files:**
- Create: `skills/using-lwc/references/temporal-memory.md`
- Modify: `skills/using-lwc/SKILL.md`
- Modify: `skills/using-lwc/references/trigger-playbook.md`
- Modify: `skills/using-lwc/references/operations-manual.md`
- Modify: `tests/using_lwc_policy.sh`
- Modify: `src/agent.rs`
- Modify: `src/agent/install.rs`
- Modify: `tests/agent_hooks.rs`
- Modify: `tests/agent_cli.rs`
- Mirror: `integrations/{codex-lwc,claude-lwc,pi-lwc}/skills/using-lwc/`
- Verify: `tests/integrations.mjs`

- [ ] **Step 1: Write failing policy/readiness tests**

Require the capability reference, five standard teaching sections, router link,
record/skip/temporal-first/Wiki-first rules, exact commands, and parity across
all packaged integrations. Add a Hook assertion for bounded
`LWC_READINESS.memory` and prove Hook execution leaves memory tables/counters
unchanged. Add a direct `lwc agent install` assertion that the installed Skill
tree contains the byte-identical new reference, not only `SKILL.md`.

- [ ] **Step 2: Run affected tests and verify RED**

```bash
bash tests/using_lwc_policy.sh
cargo test --test agent_hooks temporal_memory -- --nocapture
cargo test --test agent_cli temporal_memory -- --nocapture
node --test tests/integrations.mjs
```

- [ ] **Step 3: Add minimal guidance and read-only readiness**

Write the dedicated reference with concise decision rules. Route it from the
canonical Skill and trigger playbook. Add effective setting/commands to
readiness; do not load raw events or candidates. Add the reference to
`src/agent/install.rs::SKILL_FILES`, then mechanically mirror the canonical
Skill directory to the three integration packages.

- [ ] **Step 4: Re-run the three affected tests and verify GREEN**

## Task 6: Prove usefulness with a focused benchmark

**Files:**
- Create: `tests/temporal_memory_benchmark.rs`
- Modify: `tests/temporal_memory.rs` only if the benchmark exposes a correctness gap.

- [ ] **Step 1: Write the ignored benchmark and observe its RED state**

Use a fixed synthetic fixture with at least 20 labeled temporal queries covering
freshness, supersession, bounds, protected eviction, false merges, and every
hint class. Run a control ordering (lexical-only event candidates) and the public
temporal CLI. Report JSON with quality counts and P50/P95 timings.

```bash
cargo test --release --test temporal_memory_benchmark \
  temporal_memory_benchmark_reports_json -- --ignored --nocapture
```

Expected initially: RED on one or more acceptance thresholds.

- [ ] **Step 2: Fix only demonstrated temporal gaps**

Do not tune against unrelated queries or add an embedding layer. Adjust the
smallest deterministic ordering/filter/hint rule that corrects a labeled case,
then rerun the focused correctness test covering that rule.

- [ ] **Step 3: Require benchmark gates**

Require `false_merge_rate=0`, protected survival/bounded precision `=1`, stale
suppression `=1`, fresh top-1 `>=0.90`, hint precision `=1`, hint recall
`>=0.95`, and record P95 regression `<=1.25x` control across five in-process
runs. Preserve the JSON report in command output, not the repository.

## Task 7: Public docs, durable Wiki, and final impact audit

**Files:**
- Modify: `README.md`
- Modify: `README.zh-CN.md`
- Modify: `docs/agent-workflow.md`
- Update via CLI: project Wiki page `lwc-biomimetic-temporal-memory-design`

- [ ] **Step 1: Document only shipped commands and boundaries**

Describe JSON input forms, normalized schema, exact retry rule, scope/config,
retention deletion semantics, recall/feedback/status/maintenance, and Agent
routing. State explicitly that semantic duplicate detection and automatic Wiki
synthesis do not exist.

- [ ] **Step 2: Run fresh targeted verification**

Run all Cargo, shell-test, Node-test, and runtime commands below in the isolated
Pro test directory. The local machine must not compile or execute tests.

```bash
cargo fmt --check
cargo clippy --bin lwc --test temporal_memory --test agent_hooks -- -D warnings
cargo test --test temporal_memory -- --nocapture
cargo test --test agent_hooks temporal_memory -- --nocapture
cargo test --test agent_cli temporal_memory -- --nocapture
bash tests/using_lwc_policy.sh
node --test tests/integrations.mjs
cargo test --test cli every_public_command_exposes_renderable_help -- --nocapture
cargo test --test cli config_ -- --nocapture
cargo test --test storage_regressions migration -- --nocapture
cargo test --release --test temporal_memory_benchmark \
  temporal_memory_benchmark_reports_json -- --ignored --nocapture
git diff --check
```

Do not run `cargo test --all-targets` or unrelated benchmark/adapters.

- [ ] **Step 3: Verify runtime acceptance**

In a temporary project, run init, config show/set/unset, three JSON input forms,
same/changed request replay, explicit supersession, CJK recall, feedback,
forced retention, status, and maintenance. Inspect structured output and SQLite
table inventory; preserve no temporary fixture in the repository.

- [ ] **Step 4: Sync structural evidence and Wiki**

```bash
lwc --scope project cg sync
lwc --scope project cg impact Store
lwc --scope project cg query "temporal memory remember recall"
```

Update the existing design Wiki page to replace proposal language with verified
behavior, while preserving provenance and links. Wait for any returned Work,
then run:

```bash
lwc --scope project lint
lwc --scope project search "LWC 时序记忆如何记录、召回与淘汰" --limit 5
lwc --scope project search "事件胶囊 request_id 容量保护" --limit 5
lwc --scope project graph verify
```

- [ ] **Step 5: Audit every acceptance requirement before completion**

Map each design requirement to current source, focused test output, runtime
output, benchmark metric, docs, and Wiki evidence. Leave the goal active if any
evidence is missing or indirect.
