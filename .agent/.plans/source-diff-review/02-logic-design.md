# Bounded Source Diff and Direct Citation Review - Logic Design

Plan ID: `source-diff-review`
Status: `executable`
Version: `2`
Updated: `2026-08-03T18:00:46+08:00`
Dependencies: [architecture](01-architecture.md).

## Domain Model and Identity

| Entity | Identity | Meaning |
| --- | --- | --- |
| Immutable source | `sources.id` plus SHA-256 `content_hash` | Exact UTF-8 evidence snapshot; never rewritten. |
| Tracked path | `source_path_revisions.tracked_path` | Mutable file identity observed across immutable revisions. |
| Path head | maximum `revision` for a tracked path | Latest snapshot LWC has observed, not necessarily current live bytes. |
| Live file | authorized canonical file handle plus pre/post fingerprint | Current bytes inspected without being persisted. |
| Direct review candidate | `page_sources(source_id, page_slug)` | Page explicitly citing the old immutable source. |
| Diff | ordered comparison from `from` to `to` | Read-only evidence for Agent judgment; not durable state. |

`SOURCE_ID` is always the left/old side. `--to-source` or the selected live
tracked path is the right/new side. Review candidates always cite the left
source ID.

## Core Rules

- `RULE-001` Diff direction is stable: `source:<old_id>` -> `live:<path>` or
  `source:<new_id>`. Reversing CLI argument order reverses the diff.
- `RULE-002` Live mode accepts exactly one tracked path. Zero paths returns
  `source_diff_untracked`; multiple paths without `--path` returns
  `source_diff_path_required` with sorted candidates; an unmatched `--path`
  returns `source_diff_path_not_found`.
- `RULE-003` `--to-source` conflicts with `--path`,
  `--allow-external-source`, and `--acknowledge-sensitive-source`. Snapshot
  mode never touches the filesystem.
- `RULE-004` Live mode reuses current scope/path validation. Project scope never
  falls back to a sibling or global Wiki; an external project path requires
  `--allow-external-source` on this invocation. Global scope reads only the
  selected global store and reports `scope=global`. Captured live text passes
  the existing sensitive-source scanner before output and requires a current
  `--acknowledge-sensitive-source` when flagged. If a project path is both
  external and sensitive, neither acknowledgement substitutes for the other;
  both flags are required.
- `RULE-005` A live comparison is accepted only when handle/path fingerprints
  are unchanged throughout the read and the before/after
  `SourceStatusTargets` snapshots are equal. Otherwise return
  `source_status_unstable` and no diff.
- `RULE-006` `changed` is exact SHA-256 inequality. Equal hashes skip diff
  computation and return an empty diff.
- `RULE-007` Each UTF-8 side must be <= 8 MiB and <= 200,000 logical lines.
  Line count is exactly `split_inclusive('\n').count()` (`0` for empty input,
  no phantom extra line after a terminal newline). Breach returns
  `source_diff_too_large` before algorithm execution.
- `RULE-008` Unified diff uses line-oriented Myers, a one-second deadline, three
  context lines, stable logical headers, and no color or timestamps. Path labels
  encode the backslash character as `\\`, LF as `\n`, CR as `\r`, tab as `\t`,
  and every other C0/DEL character as uppercase `\u00XX`; printable Unicode is
  unchanged.
- `RULE-009` `--max-chars` defaults to 20,000, must be 1..=100,000, and truncates
  only on a Unicode scalar boundary. Metadata always reports total and returned
  character counts and `truncated`.
- `RULE-010` An untruncated diff is valid unified text. A truncated diff is a
  preview and MUST NOT be described as complete or directly applicable.
- `RULE-011` `source refs` is the sole page-candidate query. Start with
  `--limit 1000`. `has_more=false` yields one complete query snapshot. If
  pagination is required, collect one full observed scan in sorted offset order
  and de-duplicate by slug, but label it non-atomic and potentially incomplete.
  Repeated equal scans do not prove snapshot completeness. Wikilink-only and
  semantically related pages are never included.
- `RULE-012` LWC never decides whether a difference changes knowledge. The
  Agent must inspect diff plus direct candidates, then deliberately choose to
  preserve or revise pages.
- `RULE-013` Diff/status/refs execute through `open_for_read`. Current v8 stores
  create no operations, projections, caches, acknowledgement rows, or schema
  changes; existing writable legacy stores may perform only their already-
  supported one-time upgrade before the read.

## State Transitions

There is no new state machine. Existing immutable/history semantics remain:

```text
path P: source A (rev 1) -> source B (rev 2) -> source A (rev 3)
```

## Data Semantics

- `source diff B --path P` compares snapshot B with the current live P even if
  P's database head is A.
- `source diff A --to-source B` compares the two snapshots without assuming
  either is a path head.
- `source refs B` lists only pages still citing B.
- No comparison changes revisions or makes a source “acknowledged”.

## `FLOW-001` Live Source Diff

1. Validate CLI conflicts and `max_chars`.
2. Open the selected store for read and load bounded old-source metadata/body.
3. Select `SourceStatusTargets` for `old_id`; apply `RULE-002`.
4. Resolve the selected tracked path and recheck external authorization.
5. Prepare one nonblocking regular-file handle with its fingerprint.
6. Stream at most the diff input ceiling while hashing and capturing UTF-8.
7. Recheck handle/path fingerprint and database target snapshot.
8. Compare SHA-256. If equal, return `changed=false` and no rendered text.
9. Otherwise decode UTF-8 and run the existing sensitive-source scan before
   output.
10. Render and Unicode-truncate according to `RULE-007`-`RULE-010`.
11. Return JSON. Do not mutate the store.

## `FLOW-002` Immutable Snapshot Diff

1. Validate CLI conflicts and `max_chars`.
2. Open the store for read.
3. Load both source records only after their byte lengths pass the diff cap.
4. Compare immutable hashes; skip rendering when equal.
5. Render `source:<old_id>` -> `source:<new_id>` and return JSON.

The two source IDs may be unrelated; comparison itself does not assert lineage.

## `FLOW-003` Agent Review Workflow

1. Run targeted `source status <OLD_ID>` only when currentness matters.
2. If `filesystem_state=modified`, run `source diff <OLD_ID>` with the exact
   `--path` when required.
3. If `diff.truncated=true`, state that review is incomplete and retry up to the
   100,000-character ceiling before deciding.
4. Run `source refs <OLD_ID> --limit 1000 --offset 0`.
5. If `has_more=false`, present that point-in-time set as complete. Otherwise
   finish one paginated scan, de-duplicate by slug, and explicitly report that
   the observed candidate set is non-atomic and potentially incomplete.
6. Present results as “directly citing review candidates”, not “affected”.
7. Agent classifies the edit:
   - non-semantic: preserve pages and explain why no update is needed;
   - semantic: `source add` the same path, complete ingest, and deliberately
     revise only pages whose claims changed;
   - uncertain/unavailable: do not treat old claims as current evidence.

## Error and Edge Cases

### `FLOW-004` Failure and Race Handling

| Condition | Result | Retry / recovery |
| --- | --- | --- |
| Unknown source | existing `source_not_found` | Correct ID. |
| No tracked path in live mode | `source_diff_untracked` | Re-add intended file or use `--to-source`. |
| Multiple paths, no selector | `source_diff_path_required` | Choose exact candidate from message/status. |
| Selector is not a historical path for old source | `source_diff_path_not_found` | Use exact tracked path. |
| External/escaped path without current acknowledgement | existing `external_source_requires_acknowledgement` | Retry only with current authorization. |
| Missing, unreadable, non-regular live target | `source_diff_unavailable` with state in message | Restore/choose a readable regular file. |
| Invalid live UTF-8 | existing `invalid_utf8` | Review/redact/re-encode source; binary diff is out of scope. |
| Live text matches a sensitive marker | existing `possible_secret_detected` | Inspect without printing it; retry only with explicit sensitive acknowledgement. |
| Input byte/line cap exceeded | `source_diff_too_large` | Use a scoped external inspection; do not raise limits silently. |
| File/path/database head changes during read | existing `source_status_unstable` | Retry from the beginning. |
| Invalid output cap | existing `invalid_limit` | Use 1..=100000. |

Every error is all-or-nothing: stdout contains no partial success JSON and the
database remains unchanged.

## `FLOW-005` Output Bounding

1. Reject either side above the byte/line cap.
2. Configure the algorithm deadline before diff construction.
3. Render complete unified output for accepted input into bounded-process memory.
4. Count Unicode characters.
5. Return the first `max_chars` characters on a valid UTF-8 boundary.
6. Set `truncated = returned_chars < total_chars`.

The compute deadline can produce a non-minimal but complete replacement-style
script. Output truncation can make the preview non-applicable, which is why the
flag is a release gate.

## `FLOW-006` Verification and Release

1. Capture red tests before product edits.
2. Make focused tests green and refactor only introduced duplication.
3. Finish docs/Skill/version changes, run full locked product checks and local
   Skill smoke tests separately, then create one frozen v0.6.0 candidate commit.
4. Build `target/release/lwc` from that exact clean HEAD, record its SHA-256,
   copy it into every hard-coded retained `bin/lwc-current` slot, and assert the
   copied hash matches before timing.
5. Run fixed feature benchmarks and every retained regression suite, including
   provenance, against that exact candidate. Preserve corpus, query truth,
   scoring, and gates; only refresh ignored baseline identity metadata.
6. Review the frozen diff against every `REQ-*` and `ANTI-*` without tracked
   edits. If any tracked file changes, invalidate the evidence and return to
   step 3 for all checks and benchmarks.
7. Only then push/tag/release when authorized, install the released binary/Skill
   locally, and smoke-test published assets from the same commit.

## Flow Simulations

| Scenario | Input | Expected output | Rules |
| --- | --- | --- | --- |
| One character | old `port=80\n`, live `port=81\n` | `changed=true`; one unified hunk contains `-port=80` and `+port=81`. | `RULE-006`, `RULE-008` |
| Typo with two citing pages | one spelling edit; two `page_sources` rows | Diff shows spelling line; `source refs` returns both as candidates; no page is auto-edited. | `RULE-011`, `RULE-012` |
| Same bytes | old and live SHA equal | Empty diff and `changed=false`; no algorithm work. | `RULE-006` |
| Two aliases | old source was observed at `a.md` and `b.md` | Without `--path`, error lists both; exact selector compares only chosen path. | `RULE-002` |
| Old to new snapshot | `source diff 7 --to-source 21` | Stable source labels, no filesystem access, old-source refs queried separately. | `RULE-001`, `RULE-003` |
| Large generated file | 8 MiB+1 or 200,001 lines | Fast `source_diff_too_large`; no partial diff. | `RULE-007` |
| Long diff preview | total 50,000 chars, default cap | 20,000 returned, exact total, `truncated=true`. | `RULE-009`, `RULE-010` |
| Wikilink-only page | page links to a citing page but lacks source ID | Not returned by `source refs`; no claim of transitive impact. | `RULE-011` |
| Citations require pagination | more than 1,000 direct citers, with or without concurrent writes | Return the de-duplicated observed candidates labelled non-atomic/potentially incomplete; make no completeness claim. | `RULE-011`, `RULE-012` |
| External file gains a secret | project path is outside root and live text matches scanner | Each flag alone fails its own gate; only both current acknowledgements permit output. | `RULE-004` |
