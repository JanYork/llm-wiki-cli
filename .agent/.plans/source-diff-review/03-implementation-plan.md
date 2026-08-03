# Bounded Source Diff and Direct Citation Review - Implementation Plan

Plan ID: `source-diff-review`
Status: `executable`
Version: `2`
Updated: `2026-08-03T18:00:46+08:00`
Dependencies: [logic](02-logic-design.md), [acceptance](05-acceptance-plan.md).

## Execution Strategy

Implement one vertical CLI slice through strict red-green-refactor. Preserve
the existing store and live-read contracts, add one pure renderer module, then
update Agent guidance and run feature plus regression benchmarks. Do not begin
`TASK-002` until `TASK-001` has captured meaningful red failures.

## Task Breakdown

## Phase 1 - Contract and Red Evidence

Entry: plan is approved and implementation is separately authorized.

### `TASK-001` Lock public command contracts in failing tests

- Goal: make each new CLI behavior fail for the right reason before product code
  changes, while retaining green characterization for the existing refs and
  read-only boundaries.
- Traces to: `REQ-001`-`REQ-006`; `ARCH-001`, `ARCH-003`, `ARCH-005`;
  `FLOW-001`, `FLOW-002`, `FLOW-004`; `AC-001`, `AC-002`, `AC-004`, `AC-006`;
  `TEST-001`, `TEST-004`-`TEST-007`, `TEST-009`-`TEST-014`, `TEST-016`,
  `TEST-017`, `TEST-019`, `TEST-021`-`TEST-023`.
- Preconditions: clean or understood worktree; v0.5.0 baseline binary retained.
- Inputs: `tests/cli.rs`, `tests/safety_workflows.rs`, `tests/core_parity.rs`,
  current v0.5 behavior and JSON fixtures.
- Expected files: `tests/cli.rs`, `tests/safety_workflows.rs`,
  `tests/core_parity.rs` only where the existing test boundary fits.
- Change: add atomic CLI/flow tests for both modes, one-character/no-change,
  boundary limits, path ambiguity, external/symlink/FIFO safety, global scope,
  read-only operations, existing direct refs, and help text. Do not edit
  production code. Renderer-unit, deterministic race, Skill-policy, docs, and
  benchmark tests remain owned by their later tasks.
- Verification:
  - `cargo test --locked --test cli source_diff -- --nocapture`
  - `cargo test --locked --test safety_workflows source_diff -- --nocapture`
  - `cargo test --locked --test core_parity source_diff -- --nocapture`
  - new-command CLI/safety cases must fail because `source diff` or its target
    behavior is absent, not because fixture setup is invalid; existing refs and
    read-only characterization cases must stay green.
- Rollback: remove only new test cases; no database/user state exists outside
  temporary fixtures.
- Done: red output is saved in the task evidence note, all tests listed above
  have owners, and existing refs/status characterizations remain green.

Exit: red evidence is valid. Stop if tests cannot express the JSON contract
without changing an unrelated public API.

## Phase 2 - Smallest Green Product Slice

### `TASK-002` Add the bounded pure diff renderer

- Goal: produce safe unified diff metadata from two accepted UTF-8 strings.
- Traces to: `REQ-001`, `REQ-002`, `REQ-005`; `ARCH-002`; `FLOW-002`, `FLOW-005`;
  `AC-001`, `AC-002`, `AC-005`; `TEST-002`, `TEST-003`, `TEST-016`-`TEST-018`.
- Preconditions: `TASK-001` red evidence.
- Inputs: approved contract, official `similar 3.1.1` API, existing Rust edition
  and release targets.
- Expected files: `Cargo.toml`, `Cargo.lock`, new `src/source_diff.rs`, and the
  single `mod source_diff;` declaration in `src/main.rs`.
- Change:
  1. create the module with atomic tests and a minimal `todo!()` contract, run
     them once to capture renderer-specific red evidence;
  2. only then add `similar = "3.1.1"` and implement fixed
     caps/deadline/context, stable labels, exact hash-equality short-circuit,
     unified rendering, and Unicode-safe truncation;
  3. use no traits or future configuration layer.
- Verification:
  - first renderer run fails at the explicit stub rather than fixture setup;
  - renderer unit tests for one-character, additions/deletions, missing final
    newline, CJK/emoji, exact header escaping, equal input, cap rejection,
    truncation, and complete reconstruction on small disjoint/repetitive input;
  - `cargo test --locked source_diff::tests -- --nocapture`.
- Rollback: remove the module/dependency and restore lockfile.
- Done: pure tests are green; no filesystem/SQLite code exists in the module.

### `TASK-003` Wire bounded snapshots and safe live reads into `source diff`

- Goal: make both CLI modes green without changing status/refs behavior.
- Traces to: `REQ-001`, `REQ-002`, `REQ-004`, `REQ-006`; `ARCH-001`, `ARCH-003`,
  `ARCH-005`; `FLOW-001`, `FLOW-002`, `FLOW-004`; `AC-001`, `AC-002`, `AC-004`,
  `AC-006`; `TEST-001`-`TEST-006`, `TEST-009`-`TEST-015`, `TEST-021`-`TEST-023`.
- Preconditions: `TASK-002` green.
- Inputs: `SourceCommand`, `source_status_targets`, private source loaders,
  `prepare_live_source`, `inspect_prepared_source`, `AppError` convention.
- Expected files: `src/main.rs`, `src/store.rs`; no schema/migration file.
- Change:
  1. add the Clap variant and response structs;
  2. add one bounded store loader that checks byte size before loading content;
  3. refactor existing prepared-live inspection to support hash-only status and
     bounded hash-plus-content diff through the same safety path;
  4. enforce exact path disambiguation and before/after target equality;
  5. map unavailable/limit/UTF-8/race states to the defined errors;
  6. add deterministic tests for file/path replacement and before/after target
     mismatch through the existing helper seams before changing their shared
     integration; do not add sleeps or timing-dependent hooks;
  7. keep both stores read-only.
- Verification:
  - rerun all focused commands from `TASK-001` until green;
  - `cargo test --locked source_status -- --nocapture` to prove sibling behavior;
  - inspect operation count before/after status, diff, and refs.
- Rollback: revert the command, loader, and live-read refactor together; schema
  remains v8 so no data rollback is required.
- Done: product `TEST-001`-`TEST-023` except Skill-owned `TEST-008` and
  benchmark-owned `TEST-020` are green, deterministic `TEST-015` proves both
  race classes, and v0.5 status response snapshots are byte-for-byte compatible
  apart from non-semantic JSON object ordering.

Exit: feature flow is green. Stop if status memory behavior now captures file
contents or any read command records an operation.

## Phase 3 - Agent Workflow and Documentation

### `TASK-004` Teach Agents the review flow without duplicating product logic

- Goal: ensure an Agent uses diff and direct refs correctly after a modified
  status result.
- Traces to: `REQ-003`, `REQ-007`; `ARCH-004`, `ARCH-006`; `FLOW-003`; `AC-003`,
  `AC-007`; `TEST-007`, `TEST-008`, `TEST-024`-`TEST-026`.
- Preconditions: product command green.
- Inputs: README source workflow, `docs/agent-workflow.md`, bundled Skill and
  memory policy, bootstrap feature probe, existing local Skill test script.
- Expected files: `README.md`, `README.zh-CN.md`, `docs/agent-workflow.md`,
  `skills/using-lwc/SKILL.md`, `skills/using-lwc/references/memory-policy.md`,
  `skills/using-lwc/scripts/bootstrap.sh`, `tests/using_lwc_bootstrap.sh`.
- Change: document `status -> diff -> refs -> judgment`; implement the
  `--limit 1000` one-query path and honest non-atomic pagination policy in the
  Skill; de-duplicate observed slugs but never claim a multi-call snapshot is
  complete; name candidates as direct citations; surface incomplete refs,
  truncation, and unavailable states; update bootstrap's v0.6 capability probe.
  Keep English/Chinese facts aligned.
- Verification:
  - product help test from `tests/cli.rs`;
  - local only: `./tests/using_lwc_bootstrap.sh`;
  - `rg -n "using_lwc_bootstrap" .github/workflows` must return no executable CI
    step; preserve the existing local-only comment.
- Rollback: revert docs/Skill/probe changes together; do not publish a Skill that
  requires an unreleased CLI.
- Done: `TEST-007`, `TEST-008`, and `TEST-024`-`TEST-026` pass locally; fixed
  >1,000-page transcripts lock the non-atomic/incomplete Skill policy without
  claiming a concurrency test; no Skill check was added to CI; and direct refs
  are never labelled proven semantic impact.

## Phase 4 - Objective Verification

### `TASK-005` Freeze v0.6.0 candidate, benchmark that exact binary, and retain evidence

- Goal: freeze the complete tracked v0.6.0 candidate, then prove that exact
  commit/binary is bounded/useful and does not degrade existing metrics.
- Traces to: `REQ-005`, `REQ-006`, `REQ-008`; `ARCH-007`; `FLOW-005`, `FLOW-006`;
  `AC-005`, `AC-006`, `AC-008`; `TEST-020`, `TEST-027`-`TEST-030`.
- Preconditions: Phases 1-3 green; all intended tracked product, test, docs,
  Skill, and lockfile edits except the final version/capability bump are
  complete; the v0.5.0 baseline binary has a recorded SHA-256 value.
- Inputs: retained `.local-benchmarks/lwc-source-freshness/` and
  `.local-benchmarks/lwc-regression-review/`, including the two ignored runners
  whose `baseline_commit` still names v0.4; fixed new fixtures defined in
  [acceptance](05-acceptance-plan.md).
- Expected files: tracked `Cargo.toml`, `Cargo.lock`, and existing version facts
  first reach v0.6.0; `.github/workflows/release.yml` and `CONTRIBUTING.md`
  enforce annotated, nonblank user-facing Release notes; local-only
  `.local-benchmarks/lwc-source-diff/` then gains
  its feature runner, matrix wrapper, manifest, fixtures, candidate binary, and
  timestamped JSON results. The ignored regression README/result metadata is
  refreshed for v0.5.0 -> v0.6.0 without changing its corpus, queries, scoring,
  or gates. No tracked file changes after the candidate commit.
- Change:
  1. bump every tracked version/capability fact to v0.6.0 and finish all tracked
     release documentation; finalize tracked handoff/manifest with the planned
     local evidence paths before freezing them;
  2. run all locked product, installer, and local-only Skill checks;
  3. include this finalized `.agent/.plans/source-diff-review/` bundle in one
     focused local release-candidate commit and require a fully clean worktree
     apart from ignored `.local-benchmarks/` artifacts;
  4. build `target/release/lwc` from that exact HEAD, record commit/version/hash,
     copy it into the hard-coded regression `bin/lwc-current`, and assert hashes
     match; copy the verified `lwc-v0.5.0` binary into `bin/lwc-baseline`; update
     exactly `run-raw-benchmark.py` and `provenance/run.py` baseline metadata
     from `7e19503...` to v0.5.0 commit
     `fdce47048471b2fb7e25f5ba96928da191d680d4`, and refresh the ignored README
     so it truthfully names the baseline/candidate hashes without changing
     corpus, query truth, measurement, scoring, or thresholds;
  5. create the reproducible five-run alternating feature benchmark, validate
     fixture hashes/states, and measure latency, peak RSS, correctness,
     truncation, and fast rejection; run its tampered-fixture self-check once;
  6. add one ignored stdlib-only matrix wrapper that invokes feature, v0.5/v0.6
     freshness, raw, compiled, and provenance runners; before and after every
     child it verifies all actual executable/runner hashes, captures result/log
     paths, applies the predeclared cross-version freshness formulas, and writes
     one checksum-linked verification JSON; preserve retained corpus, query
     truth, scoring, and gates; its self-test must catch a temporary identity
     swap and a synthetic freshness regression before the real matrix starts.
- Verification: exact commands and thresholds are in `05-acceptance-plan.md`.
- Rollback: benchmark workspaces are disposable; retain corpus/manifest/results
  and delete only generated temporary Wikis through the runner.
- Done: `TEST-020` and `TEST-027`-`TEST-030` pass; feature/regression records name
  the same v0.6.0 HEAD and candidate hash; no existing metric regresses; no
  tracked file changed after the commit; and `git status --short` shows no
  `.local-benchmarks` path.

Exit: objective evidence accepted. Stop on any quality/latency/storage/RSS gate
failure; diagnose rather than relax thresholds after seeing results.

## Phase 5 - Final Review and Release

### `TASK-006` Read-only final review, then release and install the frozen candidate

- Goal: ship one auditable v0.6.0 release and verify the installed Agent path.
- Traces to: `REQ-007`, `REQ-008`; `ARCH-006`, `ARCH-007`; `FLOW-006`; `AC-007`,
  `AC-008`; `TEST-031`.
- Preconditions: the exact frozen candidate commit passes `TASK-005`; its
  tracked worktree remains clean; push/tag/release authority is current.
- Inputs: complete diff, test/benchmark JSON, release workflow, installer,
  locally installed Skill path.
- Expected files: none before publication. This task is read-only against tracked
  files; release metadata/assets are external outputs of the frozen commit.
- Change: independently review requirements, antipatterns, final diff, test and
  benchmark evidence; confirm HEAD/hash/worktree are unchanged; push main and
  tag; observe all six release targets; smoke-install published assets; update
  the canonical local CLI and Skill from the released commit. If review finds
  any issue requiring a tracked edit, invalidate the candidate and return to
  `TASK-005`; do not patch and continue releasing.
- Verification:
  - verify recorded commit and binary SHA-256 still match HEAD and runner slots;
  - verify the tracked worktree remains clean and evidence has every gate;
  - no tracked edit occurs during final review;
  - published installer reports `lwc 0.6.0`, and `lwc source diff --help` works.
- Rollback: before publication, withhold the commit/tag and return to
  `TASK-005`. After publication, never move the tag; issue a corrective version.
  Local install can reinstall v0.5.0 using its verified asset.
- Done: Git/main/tag/release/local install all identify the same commit and
  binary hash; worktree is clean except intentionally ignored local benchmarks.

## Dependency Order

```text
TASK-001 red contract
    -> TASK-002 pure renderer
    -> TASK-003 CLI/store/live integration
    -> TASK-004 docs + local Skill workflow
    -> TASK-005 freeze candidate + all checks/benchmarks
    -> TASK-006 read-only final review + release
```

## Tracked File Impact

| Path | Planned reason |
| --- | --- |
| `src/source_diff.rs` | Small pure bounded renderer and unit tests. |
| `src/main.rs` | Command, JSON response, live read reuse, path/race orchestration. |
| `src/store.rs` | Bounded immutable source loader; no schema change. |
| `Cargo.toml`, `Cargo.lock` | One diff dependency and eventual v0.6.0 version. |
| `tests/cli.rs` | Public command, output, path, Unicode, and read-only flow tests. |
| `tests/safety_workflows.rs` | Authority and side-effect regression tests. |
| `tests/core_parity.rs` | Direct citation candidate boundary if not already fully covered. |
| README pair / `docs/agent-workflow.md` | User and Agent workflow contract. |
| bundled `skills/using-lwc/**` | Canonical Agent procedure and v0.6 capability probe. |
| `tests/using_lwc_bootstrap.sh`, `tests/using_lwc_policy.sh` | Local-only Skill capability and workflow regressions. |
| `.github/workflows/release.yml`, `CONTRIBUTING.md` | Publish the annotated tag body and reject blank/lightweight release tags. |
| `.agent/.plans/source-diff-review/**` | Reviewed execution contract, frozen before the candidate benchmark. |

Other workflows, SQLite schema/migrations, projections, and unrelated modules
are explicitly not expected to change.

## Verification Commands

Focused red/green and widest release commands are authoritative in
`05-acceptance-plan.md`. Every task repeats its exact subset; no phase may replace
those commands with a generic “tests pass” claim.

Before retained scripts run, copy the accepted v0.5.0 binary to their documented
baseline slot and the exact release-mode binary from the frozen v0.6.0 HEAD to
`.local-benchmarks/lwc-regression-review/bin/lwc-current`. Assert candidate/source
hash equality, record both versioned hashes, and run raw, compiled, provenance,
and freshness suites. Any later tracked edit invalidates all final results.

## Rollback and Recovery

The feature has no data migration or persistent runtime state. Before release,
rollback is a focused code/docs/dependency revert. After a published immutable
tag, ship a corrective release rather than moving the tag. Benchmark runners
delete only their own temporary Wikis and retain fixed fixtures/results.

## Progress Protocol

Through `TASK-004`, update `06-handoff.md` and `plan-manifest.json` with exact
red/green commands, worktree state, blockers, and next task. At the start of
`TASK-005`, finalize those tracked documents with planned ignored evidence paths
before the candidate commit. After the commit, write measured results only to
ignored `.local-benchmarks/lwc-source-diff/results/` verification records and
the user-facing final report; do not mutate the tracked plan to record them.
Any required tracked correction invalidates the candidate and returns to the
start of `TASK-005`. Never erase superseded decisions; mark and explain them
before the freeze.
