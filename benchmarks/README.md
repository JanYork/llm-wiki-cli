# Search benchmark

This repo ships an opt-in local benchmark for lexical source retrieval, import
cost, and storage amplification. Normal integration tests cover compiled-page
ranking and workflow correctness.

It is intentionally ignored in normal test runs.

## Inputs

- `LWC_BENCH_CORPUS`: required path to a local corpus directory
- `LWC_BENCH_QUERY_SET`: optional path to a JSONL query set
  - default: `benchmarks/query-set.example.jsonl`
- `LWC_BENCH_BINARY`: optional `lwc` binary for before/after comparisons
  - default: the binary built by the current Cargo test run

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
| Graph | structural-evidence-only candidates | unit and core parity tests |
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
