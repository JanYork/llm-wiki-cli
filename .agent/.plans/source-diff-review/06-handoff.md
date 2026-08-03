# Bounded Source Diff and Direct Citation Review - Handoff

Plan ID: `source-diff-review`
Status: `in_progress`
Version: `2`
Updated: `2026-08-03T18:52:00+08:00`
Dependencies: all sibling documents and `plan-manifest.json`.

## Current Status

Phase: `TASK-005-candidate-ready`. The bounded diff command, shared live-read
safety, public/safety tests, bilingual docs, canonical Skill policy, v0.6
capability probe, and release-note workflow are implemented. All pre-freeze
product, installer, local-only Skill, feature-benchmark, and retained-regression
gates pass. No release candidate commit or final frozen-candidate matrix exists
yet.

This plan bundle is finalized for the release-candidate commit. Add and freeze
it with the complete tracked v0.6.0 change; after that commit, write measured
evidence only below the ignored `.local-benchmarks/` tree and in the final user
report. Any later tracked edit invalidates the frozen matrix.

## Goal and Non-goals

Add one bounded, read-only `source diff` command. Use the existing paginated
`source refs` command to list direct citation review candidates. Do not infer
semantic impact, mutate knowledge, add schema state, watch files, or build a
combined review engine.

## Read First

1. [requirements](00-requirements.md)
2. [architecture and JSON contract](01-architecture.md)
3. [logic/error semantics](02-logic-design.md)
4. [TDD execution order](03-implementation-plan.md)
5. [acceptance and benchmark gates](05-acceptance-plan.md)
6. [prohibited shortcuts](07-antipatterns.md)

## Confirmed Facts

- HEAD was `fdce47048471b2fb7e25f5ba96928da191d680d4` on `main`, tagged
  `v0.5.0` and synchronized with `origin/main` at planning time.
- `source status` already implements exact SHA-256 freshness, path lineage,
  external authorization, nonblocking file open, and file/database race checks.
- `source refs` already returns exact direct `page_sources` citations with
  deterministic ordering; one query is a read snapshot, while multiple offset-
  paginated CLI calls are deliberately labelled non-atomic/potentially
  incomplete.
- Schema v8 contains all required source/path/page/citation data; no migration
  is needed.
- No diff dependency is installed. The selected minimal dependency is
  `similar 3.1.1`, whose official API provides unified line diff and deadlines.
- Existing fixed benchmark data is retained under `.local-benchmarks/`, excluded
  through `.git/info/exclude`, and intentionally absent from Git/CI.
- Companion Skill checks are explicitly local-only in CI policy.

## Assumptions and Blockers

- `ASM-001` 8 MiB/200,000 lines is the initial safe text-diff ceiling; feature
  benchmarks must confirm its resource bound.
- `ASM-002` 20,000 default and 100,000 maximum output characters cover the first
  Agent workflow; explicit truncation prevents false completeness.
- `ASM-003` `similar 3.1.1` builds on every existing release target; locked
  product/release tests prove this before publication.
- Blockers: none that change architecture or acceptance. Benchmark or target
  failures become execution blockers and must not be hidden by changing gates.

## Completed Evidence

- Read repository code, tests, docs, workflows, release history, benchmark
  instructions, retained benchmark manifests/results, and project LWC context.
- Verified the current installed CLI is `lwc 0.5.0` and the active Wiki is
  `/Users/muyouzhi/lwc/.lwc/wiki.db` inside the authorized project root.
- Compared three implementation approaches and selected the smallest portable
  one: one pure renderer dependency plus reuse of current status/refs flows.
- Generated this eight-document plan bundle and rebuilt the manifest from the
  actual stable IDs.
- Ran an independent adversarial plan review and corrected the candidate/version
  dependency cycle, stale hard-coded benchmark-binary risk, citation concurrency
  boundary, deterministic race test design, pathological 8 MiB coverage, and
  TDD task ownership before declaring the plan executable.
- Red command: `cargo test --locked --test cli source_diff -- --nocapture`.
  Result: four tests failed with `error: unrecognized subcommand 'diff'`; 30
  unrelated CLI tests were filtered out.
- Red command: `cargo test --locked --test safety_workflows source_diff -- --nocapture`.
  Result: the double-acknowledgement safety test failed because Clap returned the
  same missing-subcommand error rather than the expected JSON safety contract.
- Red tests now lock one-character live comparison, immutable comparison,
  explicit multi-path selection, Unicode output limits, read-only operations,
  and independent external/sensitive acknowledgements.
- Renderer red command: `cargo test --locked source_diff::tests -- --nocapture`.
  Result: three tests failed at the explicit `todo!("bounded source diff renderer")`
  before `similar` or renderer code was added.
- Renderer green command: `cargo test --locked source_diff::tests -- --nocapture`.
  Result: 3 passed, including safe headers, Unicode truncation, and exact
  8 MiB/200,000-line boundaries.
- Focused green command: `cargo test --locked source_diff -- --nocapture`.
  Result: renderer 3/3, CLI 8/8, safety 1/1; all other filtered targets clean.
- Sibling regression command: `cargo test --locked source_status -- --nocapture`.
  Result: shared deterministic race unit 1/1, CLI status 5/5, read-only state
  safety 1/1.
- Citation boundary command:
  `cargo test --locked --test core_parity source_refs_returns_1000 -- --nocapture`.
  Result: one 1,000-row query returned all direct citers, sorted, with
  `has_more=false`, excluding an indirect wikilink-only page.
- Local-only Skill commands: `./tests/using_lwc_bootstrap.sh` and
  `./tests/using_lwc_policy.sh`. Results: 5 and 10 checks passed; repository CI
  still contains no Skill validation command.
- Release audit proved every historical Release body was generated with
  `--generate-notes` and, without associated PRs, collapsed to a Full Changelog
  link even though annotated tags had useful text. The workflow now requires an
  annotated tag with a nonblank body and publishes it with `--notes-from-tag`;
  `CONTRIBUTING.md` defines the user-facing section contract.
- Widest pre-freeze gates passed: `cargo fmt --check`, strict all-target Clippy,
  debug and release all-target tests, release build, installer tests, and both
  local-only Skill scripts. The suites contain 53 unit, 38 CLI, 8 core, 11
  production, 11 safety, and 3 storage tests; the one declared benchmark test
  remains ignored by design in both test profiles.
- The v0.5.0 baseline binary was reverified as `lwc 0.5.0` with SHA-256
  `1c883e19fb58e2672a6cee31c8d288ae04e1581c386b5f98b0f3345cffa67206`.
  Retained raw, compiled, provenance, and v0.5/v0.6 freshness preflights passed
  without a quality, storage, latency, or RSS regression.
- The ignored fixed feature corpus is
  `.local-benchmarks/lwc-source-diff/dataset-manifest.json` with SHA-256
  `ac2628c29b8f1a0cae0c125f859399a383970ca09df7bf3e5ebc7671c43fd518`.
  Both harness self-tests passed, and the full 12-scenario preflight passed its
  correctness, reconstruction, truncation, latency, rejection, and memory gates.
  These are pre-freeze checks only; the exact same matrix must run after commit.
- Local release-workflow preflight parsed the YAML, accepted the annotated
  v0.5.0 tag body, and confirmed Skill checks remain absent from CI.
- A pre-freeze independent review found two evidence gaps rather than product
  defects: the local policy script only linted wording, and renderer tests did
  not explicitly prove insertion/deletion plus an untruncated CJK/emoji edit.
  The policy test now executes fixed 1,001-citer stable and changing paginated
  transcripts, de-duplicates slugs, and labels both non-atomic/potentially
  incomplete; renderer tests now cover insertion, deletion, replacement, safe
  headers, and visible Unicode edits. Focused and widest gates passed again.
- A competing suggestion to require sensitive acknowledgement for unchanged
  live bytes was rejected: `FLOW-001` returns an empty diff before live text is
  exposed, matching the locked content-reveal boundary. Changed flagged live
  content still requires the explicit acknowledgement and remains tested.

## Next Action

Create the focused v0.6.0 release-candidate commit, require a clean tracked
worktree, build from that exact HEAD, copy the checksum-verified binary into all
documented candidate slots, and run the complete checksum-bound local matrix.
Any tracked correction restarts the full matrix.

## Resume Commands

```bash
git status --short
git rev-parse HEAD
python3 /Users/muyouzhi/.codex/skills/use-create-plan/scripts/validate_plan_bundle.py .agent/.plans/source-diff-review --strict
```

Then follow `TASK-005` in `03-implementation-plan.md` from the candidate-commit
step; do not rerun an earlier phase unless a tracked correction is required.

## Update Rules

- Update this file and `plan-manifest.json` after each phase with exact commands,
  results, changed files, blocker, and next task through `TASK-004`.
- In `TASK-005`, finalize tracked handoff/manifest content before creating the
  candidate commit. Store later benchmark/release verification in ignored local
  result JSON and the final user report. Do not create a self-invalidating
  post-benchmark documentation commit; any tracked correction restarts
  `TASK-005` and its full matrix.
- Keep requirements/logic authority in their owning documents; link instead of
  copying changed contracts here.
- Preserve superseded decisions with date/reason. Never silently relax a limit,
  test, or benchmark gate.
- Record local benchmark artifact paths, but never commit their data.

## Decision Log

- `DEC-001` 2026-08-03: keep direct pages on existing `source refs`; Skill
  orchestrates it after diff.
- `DEC-002` 2026-08-03: use `similar 3.1.1`, not shell commands or a custom diff
  algorithm.
- `DEC-003` 2026-08-03: require explicit path only on ambiguity; never pick one.
- `DEC-004` 2026-08-03: fixed line-oriented Myers/context/deadline; only output
  character cap is caller-adjustable.
- `DEC-005` 2026-08-03: no schema, cached diff, automatic page update, semantic
  impact, or hunk pagination in v0.6.
- `DEC-006` 2026-08-03: live diff reuses sensitive-source scanning before text
  output; current explicit acknowledgement is required to reveal a flagged
  live revision.
- `DEC-007` 2026-08-03: claim complete direct-citation candidates only from one
  `has_more=false` refs query. A paginated observation is always labelled non-
  atomic/potentially incomplete; repeated equal scans are not proof.
- `DEC-008` 2026-08-03: finish and commit v0.6.0 before final verification;
  benchmarks and release must use that exact commit/binary, and any tracked edit
  forces the whole final matrix to rerun.
