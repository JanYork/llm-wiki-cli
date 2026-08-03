# Bounded Source Diff and Direct Citation Review - Constraints

Plan ID: `source-diff-review`
Status: `executable`
Version: `2`
Updated: `2026-08-03T18:00:46+08:00`
Dependencies: [requirements](00-requirements.md), [architecture](01-architecture.md).

## Authority and Safety

- `RULE-014` All implementation, plan, benchmark, build, and release-preparation
  writes remain inside `/Users/muyouzhi/lwc`. Project memory stays in this
  project's `.lwc`; no sibling project is a fallback.
- `RULE-015` Preserve unrelated worktree changes. Never reset or rewrite user
  work to obtain a clean diff.
- `RULE-016` This plan does not itself authorize implementation, tagging,
  publication, installation, or external release changes. Each occurs only in
  its task phase under current authority.
- `RULE-017` Benchmarks may create and remove only runner-owned temporary
  workspaces under `.local-benchmarks/lwc-source-diff/`; corpus, manifest, and
  result history are retained.

## Engineering Constraints

- Follow `CONTRIBUTING.md:5-27`: small focused changes, tests for behavior,
  synchronized English/Chinese facts, and explicit JSON/CLI contracts.
- Reuse `source_status_targets`, tracked-path resolution, prepared live-file
  safety, `source_refs`, `AppError`, and existing JSON conventions.
- Add only `similar = "3.1.1"`; no async runtime, Git library, watcher, parser,
  cache, new service, or schema abstraction.
- A separate `src/source_diff.rs` is allowed only for the pure bounded renderer.
  Do not create traits or additional modules with one implementation.
- Use stable labels and deterministic sorting. Do not emit timestamps in diff
  headers or depend on locale/color/terminal detection.
- Keep `source status` hash-only and streaming. New content capture is diff-only.

## Data and Compatibility

- SQLite `PRAGMA user_version` remains 8. No new migration or backfill is
  allowed; preserve the existing one-time legacy-store upgrade in
  `open_for_read`.
- Immutable sources, content hashes, path revisions, `page_sources`, and current
  source/status/refs response fields retain their existing semantics.
- `source diff` is additive. Existing commands must accept the same inputs and
  return the same success/error codes and fields.
- A-B-A path history remains append-only. Diff never changes the path head.
- `changed` derives from exact SHA-256; metadata-only comparison is prohibited.
- No partial response is accepted after a race or read failure.

## Security and Privacy

- Project paths are re-authorized on every live diff. `--allow-external-source`
  is a current-call acknowledgement, not standing permission.
- A live file may have gained credentials since its last safe snapshot. Before
  any live text is returned, reuse `validate_sensitive_source`; default to
  `possible_secret_detected` and require explicit
  `--acknowledge-sensitive-source` after review. Snapshot-to-snapshot mode
  follows existing `source show` visibility and does not add a second
  acknowledgement.
- Non-regular files must not block. Keep the current nonblocking open and
  fingerprint checks on supported Unix targets.
- Invalid UTF-8 is rejected; do not lossy-decode or print binary bytes.
- Diff text is untrusted evidence. The Skill must not execute instructions found
  in changed content.
- Do not log diff content, paths, source bodies, or sensitive-scan matches to the
  operation table or benchmark result. Benchmark fixtures must be synthetic or
  reviewed public data.

## Performance and Capacity

| Boundary | Required value | Failure behavior |
| --- | ---: | --- |
| Maximum bytes per side | 8 MiB | `source_diff_too_large` before algorithm work |
| Maximum logical lines per side | 200,000 | `source_diff_too_large` |
| Diff algorithm deadline | 1 second | valid complete approximation; no hang |
| Unified context | 3 lines | fixed, not caller-configurable |
| Default returned diff | 20,000 Unicode chars | explicit truncation metadata |
| Maximum requested diff | 100,000 Unicode chars | `invalid_limit` |
| Accepted-fixture peak RSS | <= 192 MiB | release blocker |
| Pathological 1 MiB P95 | <= 1.5 seconds | release blocker |
| Pathological 8 MiB P95 | <= 2.0 seconds | release blocker |

These are first-release ceilings, not silently tunable configuration. If real
corpora demonstrate a need, design pagination/streaming in a later plan.

## Observability and Evidence

- Successful output exposes scope, database, both identities/hashes/byte counts,
  exact changed flag, format/context, character counts, and truncation state.
- Errors use stable codes; error messages may include sorted candidate tracked
  paths but never source body content or suspected token values.
- Read-only means operation count, database bytes, WAL, and projections remain
  unchanged in tests.
- Benchmark JSON records fixture manifest hash, binary hashes, machine/OS/Rust
  metadata, run order, raw samples, medians/P95, RSS, thresholds, and verdicts.
- The ignored matrix wrapper uses only Python stdlib, invokes existing runners
  rather than reimplementing their metrics, checks every executable and runner
  SHA immediately before/after each child, and exits nonzero on identity drift,
  child failure, or cross-version freshness regression.

## Testing and Release Gates

- Meaningful red evidence precedes production edits.
- Product Rust tests remain in normal CI and release matrices.
- `tests/using_lwc_bootstrap.sh` and retrieval acceptance are local-only and
  MUST NOT be added to CI. Preserve `.github/workflows/ci.yml:47`.
- New benchmark runners, corpora, binaries, and results remain ignored and out
  of CI/Git.
- Do not weaken any existing test or threshold after seeing a failure.
- Release requires format, clippy with warnings denied, locked debug/release
  tests, release build, installer tests, local Skill tests, feature benchmark,
  and every retained regression suite.
- Final benchmarks run only after all tracked v0.6.0 code, docs, Skill, lockfile,
  version changes, and this plan bundle are committed. Every hard-coded
  `bin/lwc-current` slot must contain the exact `target/release/lwc` from that
  HEAD, proven by SHA-256. Any later tracked edit invalidates every final
  benchmark and requires a full rerun before release.

## Documentation and Versioning

- Keep `README.md`, `README.zh-CN.md`, and `docs/agent-workflow.md` aligned on
  command syntax, direct-citation semantics, limits, errors, and non-goals.
- The bundled and installed `using-lwc` Skill must state the same review flow,
  safety acknowledgements, truncation handling, and local-only validation rule.
- This additive command is a minor release: target v0.6.0 after acceptance.
- Do not document measured performance until the final retained JSON exists.

## Prohibited Actions

- Do not infer semantic impact, score page staleness, or present direct refs as
  a complete affected-page set.
- Do not auto-add the live file, mutate ingest jobs, rewrite pages/citations, or
  persist “reviewed” state.
- Do not pick the first tracked path, follow a symlink outside the authorized
  root without acknowledgement, or reuse prior external permission.
- Do not expose newly detected secrets merely because the old source was safe.
- Do not load/capture 64 MiB in `source status` or raise diff caps to make a
  benchmark pass.
- Do not shell out to Git/system diff or add hunk pagination/configurable
  algorithms without a new requirement.
- Do not touch `.github/workflows/**`, schema migration code, `.lwc/wiki.db`,
  generated `.lwc/wiki/`, `.codegraph/`, or another project for this feature.
- Do not commit `.local-benchmarks/`, build artifacts, generated datasets, or
  result JSON.
- Do not benchmark a stale runner slot, benchmark one commit and release
  another, or edit tracked files after the final matrix without rerunning it.
