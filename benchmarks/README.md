# Search and graph benchmarks

This repo ships an opt-in local benchmark for lexical source retrieval, import
cost, and storage amplification. Normal integration tests cover compiled-page
ranking and workflow correctness.

It is intentionally ignored in normal test runs.

`tests/graph_benchmark.rs` additionally builds a deterministic generated Wiki
and reports hierarchy indexing time, graph/node/span/delta counts, database and
retained-sidecar growth, sentence/passage search, span expansion,
neighbors/path/impact/overview p95 latency, GraphQLite full projection, and one
document-replacement projection. Run a release benchmark with:

```bash
LWC_GRAPH_BENCH_DOCS=100 LWC_GRAPH_BENCH_SAMPLES=30 \
  cargo test --release --test graph_benchmark \
  graph_benchmark_reports_latency_growth_projection_and_bounds \
  -- --ignored --nocapture
```

The release-mode assertions enforce the documented latency budgets. The JSON
records OS, architecture, build mode, corpus size, counts, projection state,
per-table canonical page usage, and separate canonical/delta/sidecar
measurements. Generated inputs live only in a temporary directory.

A verified macOS x86_64 release run on the default 100-document/30-sample
fixture reported 1,275 ms canonical indexing for a 100 KiB replacement, 75 ms
incremental GraphQLite projection, 162.5 ms net overview p95 (all other net
query/traversal p95 values below 48 ms), and 3.76x canonical storage growth.
The write budgets intentionally retain complete reverse-dependent co-occurrence
updates and exact deltas; the user approved this bounded latency tradeoff because
its correctness benefit outweighs the former 750/500 ms targets. The compact
representation otherwise trades only persisted co-occurrence display precision
(six decimal places); exact position bytes, exact aggregate evidence, rank
order, locators, and rebuild semantics remain intact.

## Inputs

- `LWC_BENCH_CORPUS`: required path to a local corpus directory
- `LWC_BENCH_QUERY_SET`: optional path to a JSONL query set
  - default: `benchmarks/query-set.example.jsonl`
- `LWC_BENCH_BINARY`: optional `lwc` binary for before/after comparisons
  - default: the binary built by the current Cargo test run

The runner explicitly allows the user-selected sanitized corpus as an external
source because its temporary Wiki is created in a different directory.

The benchmark imports only raw sources into a temporary project Wiki, runs the
query set with the default `search --limit 10`, and prints one JSON report with:

- import timing
- p50 / p95 query latency
- Recall@5 / Recall@10
- MRR
- `.lwc/wiki.db`, `.lwc/wiki.db-wal`, and `.lwc/` size before and after
  `maintenance compact` when the tested binary supports it

## Query set format

One JSON object per line:

```json
{"query":"sqlite durability","expected_paths":["storage/sqlite.md"],"note":"storage topic"}
```

Fields:

- `query`: search query text
- `expected_paths`: one or more corpus-relative path suffixes expected to match
- `note`: optional free-text label

Ground truth is matched against the imported source origin path suffix, not an
internal source id.

## Suite coverage

| Layer | Check | Runner |
| --- | --- | --- |
| Raw retrieval | Recall@5/10, MRR, P50/P95 | ignored local benchmark |
| Compiled retrieval | page-first ranking, paired-source suppression, type/kind filters | `tests/cli.rs` |
| Ingest quality gate | source + non-source integration, explicit exception | `tests/cli.rs`, `tests/core_parity.rs` |
| Large sources | Unicode-safe resumable windows | `tests/cli.rs` |
| Graph | hierarchy, semantic lifecycle, bounded traversal, rslg/GraphQLite parity, p95 budgets | `tests/cli.rs`, `src/graph_backend.rs`, ignored graph benchmark |
| Storage | contentless FTS5, migrations, lint, WAL compaction | `tests/storage_regressions.rs`, production tests |

Because the benchmark creates no Wiki pages, default search is a raw-only
workload for both legacy and current binaries. Page-first behavior has
different ground truth and is tested separately.

## Example run

```bash
mkdir -p /tmp/lwc-bench-corpus/storage /tmp/lwc-bench-corpus/lang
cat > /tmp/lwc-bench-corpus/storage/sqlite.md <<'EOF'
SQLite uses a write-ahead log to improve concurrent reads during writes.
EOF
cat > /tmp/lwc-bench-corpus/lang/rust.md <<'EOF'
Rust ownership and borrowing prevent data races without a garbage collector.
EOF
cat > /tmp/lwc-bench-corpus/lang/python.md <<'EOF'
Python list comprehensions provide compact list transformation syntax.
EOF

LWC_BENCH_CORPUS=/tmp/lwc-bench-corpus \
cargo test search_benchmark_reports_json_for_local_corpus -- --ignored --nocapture
```

Use a public or sanitized corpus. Do not point the benchmark at private
material you are not allowed to snapshot into a temporary wiki.

## Fair comparisons

- Build both candidates with release optimizations and set
  `LWC_BENCH_BINARY` explicitly.
- Use the same machine, corpus snapshot, query set, and idle-state conditions.
- Run each candidate at least three times; compare the median run rather than
  selecting the best result.
- Do not accept a latency or storage win that reduces Recall@5/10 or MRR.
- Keep private corpora and reviewed ground-truth files outside Git.
