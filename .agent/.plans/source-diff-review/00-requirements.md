# Bounded Source Diff and Direct Citation Review - Requirements

Plan ID: `source-diff-review`
Status: `executable`
Version: `2`
Updated: `2026-08-03T18:00:46+08:00`
Dependencies: LWC v0.5.0 source lineage and freshness behavior.

## Problem and Outcome

LWC v0.5.0 can prove that a tracked file differs from an immutable source, and
`source refs` can list pages that directly cite that source. It cannot show the
actual text change. An Agent therefore knows that evidence is stale but must
leave LWC to discover whether the edit is a harmless typo or a claim-changing
revision.

The requested outcome is a bounded, read-only comparison tool plus a documented
review flow:

```text
targeted status -> source diff -> source refs -> Agent judgment
                                      |
                                      +-> only direct citation candidates
```

The tool reports facts. It MUST NOT decide that a page is semantically affected
or update citations automatically.

## Source Evidence

| Source | Verified evidence | Status |
| --- | --- | --- |
| User discussion, 2026-08-03 | Any byte change, including one character, must be inspectable; the Agent also needs the pages directly citing the old source. | confirmed |
| `src/main.rs:400-414`, `src/main.rs:920-995` | `source status` already selects tracked paths, re-authorizes external reads, hashes live files, and rejects path-head races. | verified |
| `src/store.rs:836-975` | `source_status_targets` resolves path lineage and `source_refs` returns exact `page_sources` citations. | verified |
| `tests/cli.rs:1075-1296` | Existing tests cover same-size changes, A-B-A history, external authorization, symlink escape, FIFO safety, and no operation-log write. | verified |
| `tests/core_parity.rs:360-398` | `source refs` returns directly citing pages in deterministic slug order. | verified |
| `docs/agent-workflow.md:51-66` | Current workflow stops at “modified; re-add and revise affected pages” and has no diff step. | verified |
| `Cargo.toml` / `Cargo.lock` | No diff library is currently installed. | verified |
| [similar 3.1.1 documentation](https://docs.rs/similar/3.1.1/similar/) | The crate provides line-oriented unified diffs, bounded context, and deadline-aware Myers behavior without default transitive dependencies. | verified 2026-08-03 |
| `benchmarks/README.md:83-91` and `.local-benchmarks/` | The project already requires fixed inputs, alternating runs, medians, no quality loss, and locally retained uncommitted benchmark data. | verified |
| `.github/workflows/ci.yml:41-49` | Product tests run in CI; companion Skill checks are explicitly local-only. | verified |

## Requirements

- `REQ-001` `lwc source diff <SOURCE_ID>` MUST compare the requested immutable
  snapshot with one current tracked live file and return a unified line diff
  that exposes a one-character change whenever the returned diff is
  untruncated; a truncated preview MUST explicitly require further review.
- `REQ-002` `lwc source diff <SOURCE_ID> --to-source <SOURCE_ID>` MUST compare
  two immutable snapshots without consulting or requiring a live path.
- `REQ-003` The Agent review workflow MUST run `source refs <OLD_SOURCE_ID>` and
  present directly citing pages as review candidates. It MUST first request
  `--limit 1000`. When `has_more=false`, that query is a complete citation
  snapshot at one point in time. When pagination is required, it MUST collect and
  de-duplicate one full observed scan but label it non-atomic and potentially
  incomplete; it MUST NOT claim that it found every direct citer.
- `REQ-004` Live diff MUST reuse the current project-boundary, external-source,
  sensitive-source, non-regular-file, and path/file race protections. If one
  source maps to more than one tracked path, LWC MUST require an exact `--path`;
  it MUST NOT choose one implicitly.
- `REQ-005` Diff computation and JSON output MUST be bounded. The first release
  MUST cap each input at 8 MiB and 200,000 lines, render three context lines,
  apply a one-second algorithm deadline, default output to 20,000 Unicode
  characters, and reject requests above a 100,000-character output ceiling.
- `REQ-006` On a current v8 store, `source diff` and `source refs` MUST remain
  read-only: no new migration, source/page/citation mutation, cache, projection,
  or operation-log row. The existing one-time legacy-store upgrade behavior of
  `open_for_read` and all `source status`, `source show`, and `source refs` JSON
  contracts MUST remain backward compatible.
- `REQ-007` Implementation MUST follow red-green-refactor TDD, document the CLI
  in English and Chinese, update the project workflow and canonical
  `using-lwc` Skill, and validate Skill behavior locally rather than in CI.
- `REQ-008` Acceptance MUST include a fixed, checksum-verified local diff
  benchmark and rerun the retained search, compiled-Wiki, provenance, and
  source-freshness suites. Datasets and results MUST remain under
  `.local-benchmarks/`, locally excluded and uncommitted.

## Non-functional Requirements

- Machine-readable JSON and stable error codes are the public interface.
- Unified diff output need not be a minimal edit script on pathological input,
  but an untruncated result MUST represent every difference between the two
  accepted inputs. A truncated result is explicitly an incomplete preview.
- Page candidates are exact direct citations, not semantic impact claims.
- The implementation should touch the fewest coherent files and add no
  persistence or background process.

## Scope and Non-goals

### In scope

- Live snapshot-to-file and immutable snapshot-to-snapshot text comparison.
- Explicit path disambiguation and current-task external-read acknowledgement.
- Bounded unified diff output and objective performance/regression evidence.
- Existing `source refs` orchestration in the Agent Skill.
- A minor release (`v0.6.0`) after all gates pass and release is authorized.

### Non-goals

- Semantic classification of “typo” versus “knowledge-changing edit”.
- Transitive impact inference through wikilinks, page text, embeddings, or LLMs.
- Automatic source add, ingest, page rewriting, citation migration, or stale lint.
- Binary/non-UTF-8 diff, directory diff, watch daemon, bootstrap-wide scanning,
  diff caching, hunk pagination, color output, or a combined `source review`
  command.
- New changes to SQLite schema v8 or the `page_sources` citation model; existing
  legacy stores still follow the already-supported one-time upgrade path.

## Assumptions and Open Questions

- `ASM-001` An 8 MiB/200,000-line ceiling covers the intended Agent-facing text
  review case. Validate it against the retained corpus and the new local
  benchmark before release; exceeding it returns a clear error rather than
  silently omitting content.
- `ASM-002` A 20,000-character default and 100,000-character hard output ceiling
  are sufficient for fast Agent inspection. Validate exact truncation metadata
  on Unicode fixtures; any future pagination requires measured demand and a new
  contract.
- `ASM-003` `similar = 3.1.1` with default `std` and `text` features remains
  compatible with the repository toolchain and six release targets. Cargo
  build/test gates and the release matrix are the validation point.

## Resolved Questions

- `DEC-001` Direct pages are not duplicated in the diff response. The Skill
  calls the already-public, paginated `source refs` command.
- `DEC-002` Multiple live paths require `--path`; no “first path” fallback.
- `DEC-003` Large/pathological inputs fail or use the library's bounded
  approximation; they never justify unbounded CPU, memory, or output.
- `DEC-007` Citation completeness is claimed only for one `has_more=false`
  query snapshot. Multi-call offset pagination is always labelled a non-atomic,
  potentially incomplete observation; an atomic cursor API is deferred until a
  measured need exceeds the current 1,000-page single-query boundary.
- `DEC-008` The tracked v0.6.0 release candidate is frozen and committed before
  the final benchmark matrix. Any later tracked edit invalidates that evidence
  and returns execution to the complete verification phase.

There are no unresolved architecture or acceptance blockers.

## Traceability Seeds

| Requirement | Architecture / flow | Task | Acceptance | Test |
| --- | --- | --- | --- | --- |
| `REQ-001` | `ARCH-001`, `ARCH-002`, `ARCH-003`, `FLOW-001` | `TASK-001`-`TASK-003` | `AC-001` | `TEST-001`-`TEST-004` |
| `REQ-002` | `ARCH-001`, `ARCH-002`, `FLOW-002` | `TASK-001`-`TASK-003` | `AC-002` | `TEST-005`-`TEST-006` |
| `REQ-003` | `ARCH-004`, `FLOW-003` | `TASK-001`, `TASK-004` | `AC-003` | `TEST-007`-`TEST-008` |
| `REQ-004` | `ARCH-003`, `FLOW-001`, `FLOW-004` | `TASK-001`, `TASK-003` | `AC-004` | `TEST-006`, `TEST-009`-`TEST-015` |
| `REQ-005` | `ARCH-002`, `FLOW-005` | `TASK-002`, `TASK-005` | `AC-005` | `TEST-016`-`TEST-020` |
| `REQ-006` | `ARCH-005`, `FLOW-004` | `TASK-003`, `TASK-005` | `AC-006` | `TEST-021`-`TEST-023` |
| `REQ-007` | `ARCH-006`, `FLOW-003` | `TASK-001`, `TASK-004`, `TASK-006` | `AC-007` | `TEST-024`-`TEST-026` |
| `REQ-008` | `ARCH-007`, `FLOW-006` | `TASK-005`, `TASK-006` | `AC-008` | `TEST-027`-`TEST-031` |
