# Bounded Source Diff and Direct Citation Review - Acceptance Plan

Plan ID: `source-diff-review`
Status: `executable`
Version: `2`
Updated: `2026-08-03T18:00:46+08:00`
Dependencies: [logic](02-logic-design.md), [constraints](04-constraints.md).

## Acceptance Criteria

- `AC-001` A one-character live edit produces `changed=true`, correct old/live
  hashes and bytes, and an untruncated unified hunk containing the exact removed
  and added lines; identical live bytes produce `changed=false` and empty text.
- `AC-002` Two immutable IDs compare without any live file, path, or permission;
  reversing IDs reverses the hunk and same-ID comparison is empty.
- `AC-003` The documented Agent flow retrieves every direct citer from one
  `has_more=false` refs snapshot, excludes a wikilink-only page, and labels
  results as review candidates. Any multi-call paginated scan is explicitly
  non-atomic and potentially incomplete regardless of whether repeated results
  happen to match; no automatic page/citation edit occurs.
- `AC-004` Live diff blocks ambiguous/untracked paths, unauthorized external or
  escaped paths, suspected secrets, non-regular/unavailable/invalid UTF-8 files,
  and live/database races with the specified stable error. Global live mode
  reads only its selected global store and never falls back to a project Wiki.
- `AC-005` Input, line, algorithm-time, and Unicode output caps are enforced;
  truncation is explicit; accepted benchmark cases meet latency/RSS gates.
- `AC-006` On a current v8 fixture, status, diff, and refs leave operations,
  DB/WAL bytes, sources, pages, citations, revisions, and projections unchanged;
  the existing legacy-open migration tests, all v0.5 command tests, and all
  benchmark gates remain green.
- `AC-007` English/Chinese docs and bundled/installed Skill expose the same
  command and safe workflow; Skill bootstrap validation passes locally and is
  absent from CI.
- `AC-008` Fixed feature data and all regression datasets/results are retained
  locally but untracked; every runner is proven to use the exact frozen v0.6.0
  candidate binary, and release/install evidence refers to that same commit and
  binary hash.

## Test Matrix

| Test | Requirement | Level | Input | Exact expected result |
| --- | --- | --- | --- | --- |
| `TEST-001` | `REQ-001` | CLI | `port=80` -> `port=81` | Exact one-hunk unified diff and changed hashes. |
| `TEST-002` | `REQ-001` | unit | insertion/deletion/replacement plus path label with CR/LF/tab | Correct non-injectable headers, ranges, signs, context=3. |
| `TEST-003` | `REQ-001` | unit | CJK + emoji one-character edit | Valid UTF-8; changed character visible. |
| `TEST-004` | `REQ-001` | CLI | identical snapshot/live bytes | `changed=false`, empty/zero/untruncated diff. |
| `TEST-005` | `REQ-002` | CLI | old/new immutable IDs, live file removed | Command succeeds and labels both sources. |
| `TEST-006` | `REQ-002`, `REQ-004` | CLI | same ID; reverse IDs; conflicting flags; isolated global snapshot and tracked live file beside a project Wiki | Empty same-ID, reversed hunk, Clap conflict; both global modes read only the global DB, live diff succeeds, and output reports `scope=global`. |
| `TEST-007` | `REQ-003` | flow | 1,000 direct citer pages | one `--limit 1000` query returns the complete sorted set with no omission/duplication. |
| `TEST-008` | `REQ-003` | local Skill policy | checked-in Skill text plus fixed 1,001-citer stable/changing response transcripts | local test locks limit/de-dup/non-atomic/incomplete wording; transcript simulation always labels multi-call results incomplete and never claims database concurrency was tested. |
| `TEST-009` | `REQ-004` | CLI | source observed at two paths | path-required error lists sorted candidates. |
| `TEST-010` | `REQ-004` | CLI | exact valid/invalid `--path` | chosen path works; unknown path rejected. |
| `TEST-011` | `REQ-004` | safety | authorized external source | blocked by default; explicit current acknowledgement succeeds. |
| `TEST-012` | `REQ-004` | safety | tracked path replaced by outside symlink | blocked unless current external acknowledgement. |
| `TEST-013` | `REQ-004` | safety | FIFO, missing, unreadable, invalid UTF-8 | no blocking; stable unavailable/UTF-8 errors. |
| `TEST-014` | `REQ-004` | safety | external live source gains credential marker | neither flag alone passes both gates; both current acknowledgements are required. |
| `TEST-015` | `REQ-004` | deterministic race | fingerprint/path swap and before/after `SourceStatusTargets` mismatch through existing helper seams | no sleeps/timing luck, no partial JSON, exact `source_status_unstable`. |
| `TEST-016` | `REQ-005` | unit/CLI | 8 MiB and 8 MiB+1 | boundary accepted; +1 rejected before diff. |
| `TEST-017` | `REQ-005` | unit/CLI | 200,000 and 200,001 lines | boundary accepted; +1 rejected. |
| `TEST-018` | `REQ-005` | unit/CLI | output around Unicode cap | no split scalar; exact total/returned/truncated. |
| `TEST-019` | `REQ-005` | CLI | max chars 0, 1, 100000, 100001 | defined valid/invalid-limit results. |
| `TEST-020` | `REQ-005` | unit/perf | untruncated 32 KiB plus truncated 1 MiB/8 MiB disjoint and repetitive fixed-width inputs | small outputs reconstruct fully; large previews have coherent bounded metadata and meet P95/RSS gates without a false full-reconstruction claim. |
| `TEST-021` | `REQ-006` | safety | status -> diff -> refs | operation count and canonical tables unchanged. |
| `TEST-022` | `REQ-006` | regression | existing source status/show/refs tests | outputs/behavior unchanged. |
| `TEST-023` | `REQ-006` | storage | current-v8 before/after DB/WAL/projection hashes plus legacy-open test | no current-store delta; existing legacy upgrade still passes. |
| `TEST-024` | `REQ-007` | docs | English/Chinese command examples | syntax/limits/errors semantically aligned. |
| `TEST-025` | `REQ-007` | local Skill | bootstrap against old/new mock CLI | old CLI upgrade path; new CLI capability accepted. |
| `TEST-026` | `REQ-007` | policy | CI workflow scan | no execution of `using_lwc_bootstrap.sh` or retrieval acceptance. |
| `TEST-027` | `REQ-008` | benchmark | fixed diff corpus, five alternating runs | correctness, latency, RSS, manifest gates pass. |
| `TEST-028` | `REQ-008` | regression | raw retrieval fixture | existing Recall/MRR/latency/storage gates unchanged. |
| `TEST-029` | `REQ-008` | regression | compiled/provenance fixtures | existing quality/read/write/storage gates unchanged. |
| `TEST-030` | `REQ-008` | regression | 758 MiB freshness dataset on v0.5.0 and frozen candidate | all nine absolute gates pass; for each scenario candidate median is within the locked normalized/10 ms formula and candidate RSS is within the locked relative/8 MiB formula. |
| `TEST-031` | `REQ-008` | release | six targets + installer smoke | all targets pass; installed v0.6.0 exposes diff. |

## TDD Evidence Rules

1. In `TASK-001`, before production edits, CLI `TEST-001`, `TEST-005`,
   `TEST-009`, `TEST-014`, `TEST-019`, and `TEST-021` must fail because the
   command/contract is absent. In `TASK-002`, renderer unit tests including
   `TEST-002`, `TEST-003`, and `TEST-018` must fail once against the explicit
   `todo!()` stub before the renderer is implemented.
2. Save commands and representative failure output in `06-handoff.md`; do not
   commit bulky logs.
3. Make one behavior cluster green at a time: pure renderer, snapshot mode,
   live mode, safety/races, Skill workflow.
4. Refactor only after focused green; rerun sibling status tests after the shared
   live-read refactor.
5. Completion requires atomic, flow, acceptance, benchmark, and residual-risk
   evidence—not merely a green unit suite.

## End-to-End Scenarios

Fixture:

1. initialize a temporary project;
2. add `docs/policy.md` containing `retry_limit = 3` as source A;
3. create 1,000 pages citing A and one page only wikilinking a citer;
4. change the live file to `retry_limit = 4` (same byte length);
5. run status, diff, and all refs pages;
6. inspect operation/table/projection state before and after.

Pass:

- status says `modified` with unequal hashes;
- diff shows only the relevant changed line plus context;
- one refs query returns all 1,000 direct citers exactly once and excludes the
  indirect page;
- neither LWC nor the Skill calls them definitively affected;
- no source/page/citation/revision/operation/projection changes occur.

Critical negative replay: replace the edited value with a known credential
prefix. Diff must fail closed without printing the token until the test supplies
the explicit sensitive acknowledgement. A separate fixed transcript in the
local Skill test covers 1,001 stable and changing paginated observations; both
must remain labelled non-atomic/potentially incomplete. This is policy
simulation, not a claim that the test executes an LLM or proves DB concurrency.

## Local Feature Benchmark

Location: `.local-benchmarks/lwc-source-diff/` (retained, ignored, never CI).

### Fixed data

- 4 KiB mostly-equal text with one-character middle edit;
- 1 MiB mostly-equal text with one-character middle edit;
- 32 KiB completely disjoint unique lines and highly repetitive sparse edits for
  full untruncated reconstruction checks;
- 1 MiB completely disjoint unique lines;
- 1 MiB highly repetitive lines with sparse edits;
- 8 MiB accepted mostly-equal boundary fixture;
- 8 MiB completely disjoint unique lines;
- 8 MiB highly repetitive lines with sparse edits;
- 8 MiB+1 byte and 200,001-line rejection fixtures;
- UTF-8/CJK/emoji truncation fixture.

Generated accepted fixtures use lines no longer than 96 Unicode characters;
one-character cases lock the edited line and character index near the middle.
`dataset-manifest.json` records path, scenario, bytes, line count, maximum line
length, edit position where applicable, and SHA-256 for old/live/new variants.
The runner verifies every value before measuring and fails on any command/state/
diff mismatch. Accepted 8 MiB fixtures stay below the separate 200,000-line
ceiling. A local `--self-test` tampers only a temporary fixture copy and must
prove manifest and expected-result mismatches exit nonzero.

### Measurement method

- Retain v0.5.0 and the frozen candidate release binary with SHA-256. Copy the
  candidate built from exact clean HEAD into every runner's hard-coded
  `bin/lwc-current` slot and fail before timing unless its hash is identical.
- Candidate `source diff` is compared with system diff; v0.5.0 has no diff
  command. Compare v0.5.0 versus v0.6.0 only for the shared `source status`
  path and retained regression suites. Warm applicable binaries; alternate five
  runs and use medians/P95, never the best run.
- Lock the comparator to `LC_ALL=C /usr/bin/diff -U 3 OLD LIVE`; record
  `/usr/bin/diff --version` and its SHA-256. Exit `1` is the expected
  “differences found” result; any other non-zero code invalidates the sample.
- Redirect command output to files and parse JSON outside timed setup.
- Sample peak RSS using the existing macOS technique from the freshness runner.
- Record OS, CPU, Rust, binary hashes, manifest hash, run order, and thresholds.
- Record the feature runner's own SHA-256. Refresh the ignored regression README
  and the exact `baseline_commit` fields in `run-raw-benchmark.py` and
  `provenance/run.py` to
  `fdce47048471b2fb7e25f5ba96928da191d680d4` before the run; record both runner
  hashes and do not alter corpus, query truth, scoring, or thresholds.
- The checksum-linked verification JSON records each runner SHA and every actual
  executable argument or hard-coded slot SHA before and after its invocation.
  This external record is authoritative for runners that do not emit binary
  identity themselves; any mismatch invalidates the whole matrix.
- Implement that record in ignored `run-matrix.py`, using Python stdlib only.
  The wrapper invokes the existing runners; it does not reproduce their scoring.
  It writes per-child command, start/end hashes, exit status, stdout/stderr log
  paths, emitted result path/hash, parsed verdicts, and final aggregate verdict.
  A child process never starts when its expected identity is already wrong. Its
  local `--self-test` must detect a temporary identity swap and a synthetic
  freshness regression without touching retained inputs.

### Feature gates

- Every untruncated small/medium diff must reconstruct the expected new text in
  the harness; pure renderer tests prove complete edit semantics for each
  pattern. Truncated large cases verify hashes/bytes, valid UTF-8, exact returned
  count, `total_chars > returned_chars`, and `truncated=true`; they are never
  claimed to reconstruct an unreturned tail.
- 4 KiB and 1 MiB sparse medians must be <= `system diff median * 3 + 50 ms`.
- Disjoint/repetitive 1 MiB P95 must be <= 1.5 seconds.
- Mostly-equal, disjoint, and repetitive 8 MiB P95 must be <= 2.0 seconds and
  peak RSS must be <= 192 MiB.
- Over-byte and over-line rejection median must be <= 100 ms and perform no
  algorithm/output work.
- No threshold may be relaxed after results are observed; a failure returns to
  implementation or explicitly blocks release.

## Release Gates

Release is blocked unless every feature gate, existing regression gate, locked
product check, local-only Skill check, and six-target release check below passes
without weakening a predeclared assertion or threshold.

### Existing Regression Gates

- `.local-benchmarks/lwc-regression-review/run-raw-benchmark.py`: Recall@5/10
  and MRR must not fall; search P95 <= +10%; database/Wiki <= +10%; import <=
  +15% plus 5 ms.
- `.local-benchmarks/lwc-regression-review/run-compiled-benchmark.py` and
  provenance runner: keep their checked-in-local fixed query/quality and
  latency/storage thresholds unchanged.
- `.local-benchmarks/lwc-source-freshness/run.py`: all nine cases satisfy
  candidate <= `shasum * 1.25 + 25 ms` and RSS <= 128 MiB. Run it once with
  v0.5.0 and once with the candidate on the same machine/session. For every
  category/state, define `baseline_ratio = v0.5_candidate_ms / v0.5_shasum_ms`
  and `allowed_ms = max(v0.5_candidate_ms + 10,
  v0.6_shasum_ms * baseline_ratio * 1.10)`; require
  `v0.6_candidate_ms <= allowed_ms`. For each reported RSS, also require the
  existing 128 MiB ceiling and
  `v0.6_rss <= max(v0.5_rss + 8 MiB, v0.5_rss * 1.10)`.
- Product suite counts may increase; no pre-existing test may be skipped,
  weakened, or deleted.

## Verification Commands

Focused during implementation:

```bash
cargo test --locked source_diff -- --nocapture
cargo test --locked source_status -- --nocapture
cargo test --locked --test safety_workflows source_diff -- --nocapture
```

Widest product/local gates:

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release --all-targets
cargo build --locked --release
./tests/install_script.sh
./tests/using_lwc_bootstrap.sh
test "$(.local-benchmarks/lwc-regression-review/bin/lwc-v0.5.0 --version)" = "lwc 0.5.0"
test "$(shasum -a 256 .local-benchmarks/lwc-regression-review/bin/lwc-v0.5.0 | awk '{print $1}')" = "1c883e19fb58e2672a6cee31c8d288ae04e1581c386b5f98b0f3345cffa67206"
test "$(target/release/lwc --version)" = "lwc 0.6.0"
cp .local-benchmarks/lwc-regression-review/bin/lwc-v0.5.0 .local-benchmarks/lwc-regression-review/bin/lwc-baseline
cp target/release/lwc .local-benchmarks/lwc-regression-review/bin/lwc-current
test "$(shasum -a 256 target/release/lwc | awk '{print $1}')" = "$(shasum -a 256 .local-benchmarks/lwc-regression-review/bin/lwc-current | awk '{print $1}')"
rg -n 'fdce47048471b2fb7e25f5ba96928da191d680d4' .local-benchmarks/lwc-regression-review/run-raw-benchmark.py .local-benchmarks/lwc-regression-review/provenance/run.py
python3 .local-benchmarks/lwc-source-diff/run.py --self-test
python3 .local-benchmarks/lwc-source-diff/run-matrix.py --self-test
python3 .local-benchmarks/lwc-source-diff/run-matrix.py --baseline .local-benchmarks/lwc-regression-review/bin/lwc-v0.5.0 --candidate .local-benchmarks/lwc-regression-review/bin/lwc-current
```

Before the copy, record the frozen HEAD, `lwc --version`, and candidate hash.
After the copy, record the v0.5.0 baseline hash and candidate-slot hash. After
all commands, require an unchanged HEAD and no tracked worktree delta; otherwise
discard the verdict and rerun this entire block from the new candidate commit.

The Skill shell test is deliberately outside repository CI. Product Rust tests
remain eligible for CI.

## Evidence Plan

Before the candidate commit, record planned paths and the completed TDD/product
evidence in the tracked handoff/manifest. After the freeze, record the following
in a checksum-linked ignored verification JSON plus the user-facing final
report, not by editing tracked plan files:

- red and final green commands;
- candidate commit and release binary SHA-256;
- feature manifest/result paths and verdicts;
- feature/matrix/regression runner hashes and both successful harness self-checks;
- unchanged regression result paths and metric deltas;
- six-target release conclusion and published asset checksums;
- installed CLI/Skill paths and smoke output;
- `git status --short` proving datasets/results are untracked and worktree state
  is understood.

## Residual Risks

- A deadline-bounded diff can be less readable/minimal on adversarial text, but
  must still expose a complete replacement when untruncated.
- Direct citations cannot identify semantic/transitive impact; human/Agent
  judgment remains required by design.
- More than 1,000 citations require multiple CLI snapshots; offset pagination
  cannot make a moving database atomic. It therefore reports observed
  de-duplicated candidates as potentially incomplete rather than promising
  completeness. A future atomic cursor is justified only by measured demand.
- Diff preview above 100,000 characters is intentionally unsupported in v0.6;
  demand for hunk pagination is future evidence, not a hidden release promise.
- A change inside an exceptionally long single line may require the 100,000-
  character retry or an authorized external inspection; v0.6 deliberately does
  not add inline character refinement. `truncated=true` prevents a false claim
  that such a preview is complete.
