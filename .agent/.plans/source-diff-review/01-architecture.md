# Bounded Source Diff and Direct Citation Review - Architecture

Plan ID: `source-diff-review`
Status: `executable`
Version: `2`
Updated: `2026-08-03T18:00:46+08:00`
Dependencies: [requirements](00-requirements.md).

## Current Architecture Evidence

| Component | Current responsibility | Evidence |
| --- | --- | --- |
| `src/main.rs::SourceCommand` | Clap routing and JSON contracts for `status`, `show`, and `refs`. | `src/main.rs:336-443`, `src/main.rs:920-1024` |
| Live-source helpers | Resolve tracked paths, recheck project scope, open nonblocking, fingerprint, stream SHA-256, and reject mixed-time reads. | `src/main.rs:1417-1711` |
| `Store::source_status_targets` | Maps requested immutable IDs to every tracked path and current path head. | `src/store.rs:836-930` |
| `Store::source_show` / private `load_source` | Loads immutable UTF-8 source records and supports Unicode windows. | `src/store.rs:934-946`, `src/store.rs:2023-2050` |
| `Store::source_refs` | Reads direct `page_sources` citations with stable order and pagination. | `src/store.rs:948-980` |
| SQLite schema v8 | Stores immutable sources, path revisions, pages, and direct citations. | `src/store.rs:2758-2773`, `src/store.rs:2811-2821` |

The missing capability is text comparison. No new persistent entity is needed.

## Target Architecture

## Data Flow

```text
                     existing read-only Store
                    +--------------------------+
source diff OLD --->| snapshot OLD             |----+
       |             | optional snapshot NEW    |    |
       |             | tracked-path candidates  |    v
       |             +--------------------------+  source_diff.rs
       |                                              |
       | live mode                                    | bounded unified diff
       v                                              v
existing tracked-path resolver -> safe live read -> JSON response

source refs OLD -------------------------------> direct page candidates
                                                    (separate command)
```

### `ARCH-001` Add one public `source diff` subcommand

Interface:

```text
lwc source diff <SOURCE_ID> [--path <TRACKED_PATH>]
    [--to-source <SOURCE_ID>] [--max-chars <N>]
    [--allow-external-source] [--acknowledge-sensitive-source]
```

- Default mode compares the immutable `SOURCE_ID` to a current live tracked
  file.
- `--to-source` selects immutable-to-immutable mode and conflicts with `--path`,
  `--allow-external-source`, and `--acknowledge-sensitive-source`.
- `--path` is optional only when the old source maps to exactly one tracked
  path. Its value is the exact `tracked_path` returned by `source status`.
- `--max-chars` defaults to 20,000 and is limited to 100,000.

No `--all`, directory mode, color, write option, or automatic follow-up is
added.

### `ARCH-002` Isolate bounded rendering in `src/source_diff.rs`

One small module owns:

- fixed input byte/line ceilings;
- a one-second `similar::TextDiffConfig` deadline using line-oriented Myers;
- three-context-line unified formatting;
- Unicode-safe output truncation and counts;
- a pure response-ready result (`changed`, hashes supplied by caller, rendered
  text, returned/total chars, `truncated`).

This is a real boundary: it is deterministic for ordinary inputs, has no
filesystem or SQLite access, and is unit-testable. It does not introduce a
trait, factory, cache, or configurable algorithm.

### `ARCH-003` Reuse, do not fork, live-source safety

Refactor the existing live inspection so `source status` requests hash-only
and `source diff` requests hash-plus-UTF-8-content from the same prepared file
handle and fingerprint checks. Status must retain streaming/low-memory behavior;
only diff captures bytes, and only below the diff ceiling.

Selection sequence in live mode:

1. read one `source_status_targets(vec![old_id], false)` snapshot;
2. choose the sole path or match exact `--path`;
3. resolve/re-authorize the path and prepare one file handle;
4. read/hash/capture through that handle;
5. select the targets again and require equality;
6. if the live hash equals the old hash, return an empty diff without exposing
   content;
7. otherwise validate captured live UTF-8 with the existing sensitive-source
   scanner, requiring explicit current acknowledgement before returning flagged
   text;
8. render only after both filesystem and database heads are stable.

### `ARCH-004` Keep citation review on `source refs`

The canonical direct-citation API remains `Store::source_refs` and
`lwc source refs`. The Skill performs `status -> diff -> refs`, first with
`--limit 1000`. A response with `has_more=false` is one complete SQLite query
snapshot. If more than 1,000 citers require multiple CLI invocations, the Skill
collects and de-duplicates one complete observed scan but labels it non-atomic
and potentially incomplete. It never treats repeated offset scans as proof of a
single database snapshot. This avoids a new cursor/API abstraction, duplicate
page representations, and a false completeness claim across concurrent writes.

### `ARCH-005` No new persistence or mutation

Both modes use `Store::open_for_read`. A current v8 store stays read-only; the
existing one-time upgrade of a writable legacy store remains unchanged. There
is no schema v9, operation-log entry, projection update, source addition, page
edit, or acknowledgement state. The only new dependency is the pure Rust diff
renderer.

### `ARCH-006` Documentation and Skill are consumers

Update `README.md`, `README.zh-CN.md`, `docs/agent-workflow.md`, and the bundled
`skills/using-lwc` policy. Update the bootstrap capability check to require
`source diff --help` for v0.6. Skill smoke tests remain local-only.

### `ARCH-007` Benchmarks remain local artifacts

Add a retained `.local-benchmarks/lwc-source-diff/` corpus, manifest, runner,
matrix wrapper, and results. The directory is already covered by
`.git/info/exclude`; it must not enter Git or CI. The wrapper runs each retained
suite, verifies every executable/runner hash immediately before and after its
child process, compares v0.5/v0.6 freshness results, and writes one checksum-
linked verification JSON. Existing retained corpus, query truth, scoring, and
gates are replayed unchanged. The ignored raw/provenance runner baseline
metadata is updated from its old v0.4 label to the verified v0.5.0 commit because
the baseline slot is intentionally advanced; runner hashes are recorded.

## JSON Contract

Proposed response, locked by `TEST-001` before implementation:

```json
{
  "scope": "project",
  "database": "/project/.lwc/wiki.db",
  "from": {
    "kind": "source",
    "source_id": 7,
    "content_hash": "...",
    "bytes": 123
  },
  "to": {
    "kind": "live",
    "tracked_path": "docs/design.md",
    "head_source_id": 7,
    "head_revision": 1,
    "content_hash": "...",
    "bytes": 124
  },
  "changed": true,
  "diff": {
    "format": "unified",
    "context_lines": 3,
    "text": "--- source:7\n+++ live:docs/design.md\n@@ ...",
    "returned_chars": 130,
    "total_chars": 130,
    "truncated": false
  }
}
```

Snapshot mode changes only `to`:

```json
{"kind":"source","source_id":21,"content_hash":"...","bytes":124}
```

Headers contain stable logical labels, not absolute database paths or current
timestamps. Tracked-path labels encode backslash as `\\`, LF as `\n`, CR as
`\r`, tab as `\t`, and every other C0/DEL character as uppercase `\u00XX`;
printable Unicode remains unchanged. A filename therefore cannot inject a
header or hunk line.
`changed=false` returns `diff.text=""`, zero counts, and `truncated=false`.

## Architecture Decisions

- `DEC-001` Reuse separate `source refs` rather than add a combined
  `source review` command or duplicate pages inside the diff response.
- `DEC-002` Add `similar = "3.1.1"`. Stdlib has no correct line-diff engine;
  shelling out to Git/POSIX `diff` is not portable across the release matrix;
  a custom Myers implementation adds more correctness and maintenance risk.
- `DEC-003` Use line-oriented Myers with the library deadline and unified
  formatter. Character-level diff is not needed: a one-character edit is still
  visible in its changed line.
- `DEC-004` Use fixed context/algorithm/deadline constants. Only output size is
  caller-adjustable because Agent token budgets vary.
- `DEC-005` Preserve the v0.5 path identity model. `--path` selects an existing
  tracked identity; it does not accept a new arbitrary file.
- `DEC-006` No new schema/version migration. This is a read-only view over
  existing sources, path revisions, and citations; the existing writable legacy
  `open_for_read` upgrade remains unchanged.
- `DEC-007` Keep the existing refs API: one <=1,000-row query can be complete at
  its snapshot; paginated observations are explicitly non-atomic/incomplete
  rather than adding a transaction/session token without demonstrated demand.
- `DEC-008` Freeze and commit the complete v0.6.0 candidate before final
  benchmarks; release uses that exact commit and binary or reruns the matrix.

## Rejected Alternatives

| Alternative | Rejection reason |
| --- | --- |
| Add citing pages to every `source status` row | Inflates a fast freshness contract and duplicates pagination semantics. |
| New semantic impact graph/LLM classifier | Cannot prove whether a textual change invalidates a claim; beyond the user request. |
| Automatically update old page citations | Violates immutable evidence and requires semantic judgment. |
| Shell out to `git diff` or `diff` | Fails for non-Git/external sources and weakens Windows portability. |
| Hand-write a diff algorithm | More code and correctness risk than one focused dependency. |
| Store diffs in SQLite or operation history | Read-only review does not justify durable state or schema churn. |
| Add hunk pagination now | No measured need; bounded truncation plus adjustable cap covers the first release. |

## Risks

- `RISK-001` Pathological diff inputs consume CPU or memory. Mitigation: 8 MiB,
  200,000-line, one-second compute, and 100,000-character output ceilings;
  benchmark repetitive/full-replacement fixtures.
- `RISK-002` Refactoring live reads regresses low-memory `source status`.
  Mitigation: hash-only mode, existing freshness tests, and unchanged 758 MiB
  benchmark gates.
- `RISK-003` Timeout-based Myers output may be non-minimal. Mitigation: contract
  requires complete difference visibility only when output is untruncated, not
  minimal hunks; verify reconstruction and label truncated output a preview.
- `RISK-004` A truncated diff could be mistaken for complete. Mitigation:
  explicit `returned_chars`, `total_chars`, and `truncated`; Skill must surface
  truncation before judgment.
- `RISK-005` Users confuse direct citation candidates with all affected pages.
  Mitigation: field and docs consistently say “directly citing review
  candidates”; tests exclude wikilink-only pages.
- `RISK-006` New dependency or CLI surface breaks a release target. Mitigation:
  locked dependency, local cross-platform product tests, and six-target release
  matrix before publication.
- `RISK-007` Citation rows can change between offset-paginated CLI calls.
  Mitigation: prefer one 1,000-row snapshot; otherwise label the de-duplicated
  observed scan non-atomic and potentially incomplete.
- `RISK-008` A retained runner can silently benchmark an old hard-coded
  `bin/lwc-current`. Mitigation: copy the exact frozen release binary into every
  runner slot and have the matrix wrapper assert all executable/runner SHA-256
  values immediately before and after every child.
