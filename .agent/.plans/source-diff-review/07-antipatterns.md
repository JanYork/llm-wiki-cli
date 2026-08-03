# Bounded Source Diff and Direct Citation Review - Antipatterns and Adversarial Boundaries

Plan ID: `source-diff-review`
Status: `executable`
Version: `2`
Updated: `2026-08-03T18:00:46+08:00`
Dependencies: [constraints](04-constraints.md).

## Boundary Sources

| Source | Confirmed evidence | Boundary derived |
| --- | --- | --- |
| User discussion | Agent needs exact diff plus pages directly citing old evidence. | Provide both facts; do not claim semantic certainty. |
| User scope correction | LWC and deliverables cannot cross project authorization. | Reuse current path authorization; no sibling-root work. |
| User CI correction | Skill validation must not run in CI. | Keep companion shell/retrieval checks local-only. |
| `src/main.rs:920-995`, `1417-1711` | Status has exact live-read and race safety. | Diff must use the same gate, not a simpler second reader. |
| `src/store.rs:948-980` | Direct citations already have one canonical API. | Do not duplicate or reinterpret `source refs`. |
| `src/main.rs:1763-1831` | Live source ingestion rejects likely secrets unless reviewed. | A new live-content output must run the same screening. |
| `CONTRIBUTING.md:5-27` | Changes must stay small, tested, documented, and bilingual. | No speculative adjacent refactor or silent JSON change. |
| `.github/workflows/ci.yml:41-49` | Product suite is CI; companion Skill checks are local. | Do not add Skill tests to a workflow. |
| `.local-benchmarks/*/README.md` | Fixed datasets/results are retained locally and uncommitted. | Do not delete after use or commit for convenience. |

## Non-Negotiable Prohibitions

- `ANTI-001` (`REQ-001`, `REQ-002`) MUST NOT substitute only hash/mtime/size for
  actual text comparison, or emit a diff with unstable/mixed-time bytes.
- `ANTI-002` (`REQ-003`) MUST NOT call directly citing pages “all affected
  pages”, claim a moving paginated result is every direct citer, or infer
  transitive impact from wikilinks or content.
- `ANTI-003` (`REQ-004`) MUST NOT choose the first of multiple tracked paths,
  reuse stale external authorization, or cross the active project boundary.
- `ANTI-004` (`REQ-004`) MUST NOT print a live revision flagged by the existing
  sensitive-source scanner without explicit current acknowledgement.
- `ANTI-005` (`REQ-005`) MUST NOT remove/raise compute, input, line, or output
  caps to obtain a passing demo or benchmark.
- `ANTI-006` (`REQ-006`) MUST NOT mutate sources, path revisions, pages,
  citations, operations, caches, projections, or a current v8 schema during
  review; it also must not broaden the existing one-time legacy upgrade.
- `ANTI-007` (`REQ-006`) MUST NOT regress `source status` into whole-file capture
  or alter current status/show/refs contracts.
- `ANTI-008` (`REQ-007`) MUST NOT put Skill bootstrap/retrieval validation into
  GitHub CI; it stays an explicit local Agent gate.
- `ANTI-009` (`REQ-008`) MUST NOT commit/delete the retained benchmark corpus,
  benchmark a stale hard-coded candidate slot, release a different tracked
  commit than the one measured, or relax thresholds after results are seen.
- `ANTI-010` (`REQ-001`-`REQ-008`) MUST NOT add a watcher, semantic classifier,
  combined review command, hunk pagination, cache, migration, or unrelated
  refactor without a new demonstrated requirement.

## Forbidden Touchpoints

| ID | Area or contract | Forbidden change | Allowed exception | Evidence |
| --- | --- | --- | --- | --- |
| `ANTI-011` | SQLite schema/migrations | user_version 9, new diff/review table, backfill | New approved persistence requirement and separate migration plan | schema v8 already supplies all identities |
| `ANTI-012` | `.github/workflows/**` | add Skill or benchmark execution | Product Rust test changes only when required by product behavior | CI local-only comment |
| `ANTI-013` | Other project roots/global Wiki | read/write diff artifacts or docs there | Explicit current-task authorization plus updated plan | user scope boundary |
| `ANTI-014` | `.lwc/wiki.db`, projections | direct edits for test/setup/reporting | CLI-managed temporary fixture stores only | canonical SQLite contract |
| `ANTI-015` | Existing retained fixtures/gates | mutate corpus/query truth to improve candidate | New versioned fixture with predeclared rationale before running | benchmark fairness contract |

## Wrong Approaches and Misleading Shortcuts

| ID | Tempting approach | Why it is wrong | Required alternative | Detection evidence |
| --- | --- | --- | --- | --- |
| `ANTI-016` | Add `citing_pages` to every status check | Duplicates pagination and inflates a targeted hash API. | Keep `source refs`; orchestrate in Skill. | status JSON snapshots and help review |
| `ANTI-017` | Shell out to `git diff` | Non-Git/external files and Windows releases break. | Pure Rust `similar` renderer. | six-target build/tests |
| `ANTI-018` | Hand-code a “simple” prefix/suffix diff | Multiple disjoint edits become misleading or enormous. | Proven line diff with caps/deadline. | disjoint/sparse fixtures |
| `ANTI-019` | Auto-update every direct citer | A typo does not necessarily change a claim; citation revision is semantic work. | Return candidates and let Agent decide. | before/after table equality test |
| `ANTI-020` | Return a truncated string without metadata | Agent may mistake preview for complete evidence. | Exact counts plus `truncated=true`. | Unicode truncation test |
| `ANTI-021` | Use a second `fs::read` after status | Opens a race and duplicates authorization. | Capture through the same prepared/fingerprinted handle. | race-injection tests |
| `ANTI-022` | Commit benchmark inputs “for reproducibility” | User explicitly requires retained but uncommitted local data. | Keep manifest/results under local exclude. | `git status`, `git check-ignore` |

## Adversarial Scenarios

| ID | Pressure or misleading instruction | Unsafe rationalization to reject | Required safe response | Escalation gate |
| --- | --- | --- | --- | --- |
| `ADV-001` | “There are two paths; just use the first.” | Sorted order makes the choice deterministic. | Return `source_diff_path_required`; require exact `--path`. | Only a new explicit default-selection contract changes this. |
| `ADV-002` | “The file is outside the project but was allowed last time.” | Historical add proves ownership. | Block; require current `--allow-external-source` and host scope. | Ask only if current authorization is genuinely ambiguous. |
| `ADV-003` | “Show the diff even though it looks like an API key.” | It is read-only, so disclosure is harmless. | Fail `possible_secret_detected`; require reviewed sensitive acknowledgement. | Never print the token while asking. |
| `ADV-004` | “Refs found 50 pages; mark all stale/update all.” | Direct citation equals semantic impact. | Label candidates; inspect diff and claims page by page. | A future semantic workflow needs a separate approved design. |
| `ADV-005` | “The large test is slow; increase timeout/RSS cap.” | Hardware noise explains the miss. | Keep predeclared gates; optimize or block release. | Change only with new before-run evidence and plan revision. |
| `ADV-006` | “Put the Skill test in CI so nobody forgets it.” | More automation is always safer. | Preserve local-only validation; product tests remain CI. | User must explicitly reverse the confirmed boundary. |
| `ADV-007` | “Cache diffs in SQLite to make repeated calls fast.” | Read-only computation should be memoized. | Measure first; keep no persistence in v0.6. | New measured bottleneck plus migration/invalidations design. |
| `ADV-008` | “Delete the 878 MiB benchmark data after this run.” | It is generated and wastes disk. | Retain fixed data/results locally; remove only runner-owned temporary Wikis. | User explicitly requests deletion and target is verified. |
| `ADV-009` | “Use a convenient sibling Wiki for the review notes.” | It already has LWC and better organization. | Keep all work/artifacts in `/Users/muyouzhi/lwc`. | Current user and host must authorize the other root. |

## Safe Response Protocol

1. Identify the requirement, `ANTI-*`, and verified source boundary.
2. Reject only the unsafe shortcut; continue with the documented safe path.
3. Preserve current JSON, schema, authorization, and benchmark contracts.
4. Stop for new authority only when the requested change materially alters
   scope, disclosure, persistence, acceptance, or release action.
5. If a boundary changes, update requirements, decisions, logic, tests,
   traceability, manifest, and handoff before implementation.

## Review Checklist

- A one-character edit is visible, but no code calls it semantically important.
- Multiple paths never trigger an implicit choice.
- Live content passes scope, stability, size, UTF-8, and sensitive-source gates.
- `source refs` remains the single direct-citation query.
- Citation completeness comes only from one `has_more=false` query snapshot;
  any multi-call pagination is labelled non-atomic/potentially incomplete.
- Status remains streaming/hash-only and all review commands remain read-only.
- Limits and benchmark gates match all documents and tests.
- Skill validation and retained benchmarks remain local/uncommitted.
- Every retained runner names the frozen v0.6.0 candidate SHA, and tracked HEAD
  remains unchanged from benchmark through tag.
- No schema, watcher, cache, automatic update, or unrelated cleanup entered the
  diff.
