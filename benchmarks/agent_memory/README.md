# Agent-memory benchmark adapters

These adapters exercise the released `lwc` CLI without adding benchmark-only
behavior to the Rust product. Generated corpora, state, upstream clones, and
reports belong under `.local-benchmarks/`, which is ignored by Git.

## Tested contracts

| Benchmark | Upstream | Tested revision | Local mode |
| --- | --- | --- | --- |
| LongMemEval-S | `https://github.com/xiaowu0162/LongMemEval` | `9e0b455f4ef0e2ab8f2e582289761153549043fc` | complete retrieval metrics |
| Agent Memory Leaderboard | `https://github.com/AML-memory/agent-memory-leaderboard` | `5761ed58502d24153115cbdc010e44957cb18c3a` | synchronous Add/Search HTTP |
| LongMemEval-V2 | `https://github.com/xiaowu0162/LongMemEval-V2` | `2cc8c540bdb87fe6761629b585e727e1c4704520` | official no-model harness smoke |

Build the measured binary first:

```bash
cargo build --release --locked
```

To run every bounded local smoke from a clean checkout with one command:

```bash
benchmarks/agent_memory/run_smokes.sh
```

This downloads about 1.5 GB of pinned public text data. It runs a five-question
LongMemEval-S retrieval smoke, the direct AML HTTP tests, the official
LongMemEval-V2 one-question/one-trajectory no-model harness, and the V2 adapter
tests. It does not expose an endpoint, call a reader or judge, or submit a run.

## LongMemEval-S

The cleaned dataset is pinned by content hash. The tested Hugging Face dataset
revision is `98d7416c24c778c2fee6e6f3006e7a073259d48f`.

```bash
mkdir -p .local-benchmarks/agent-memory/lme-v1
curl -L \
  https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/98d7416c24c778c2fee6e6f3006e7a073259d48f/longmemeval_s_cleaned.json \
  -o .local-benchmarks/agent-memory/lme-v1/longmemeval_s_cleaned.json
shasum -a 256 .local-benchmarks/agent-memory/lme-v1/longmemeval_s_cleaned.json
```

Expected SHA-256:

```text
d6f21ea9d60a0d56f34a05b609c79c88a451d2ae03597821ea3d5a9678c3a442
```

A limited run is a smoke only and writes `partial=true`:

```bash
python3 -m benchmarks.agent_memory.longmemeval_v1 \
  --data .local-benchmarks/agent-memory/lme-v1/longmemeval_s_cleaned.json \
  --state-root .local-benchmarks/agent-memory/lme-v1/smoke-state \
  --output .local-benchmarks/agent-memory/lme-v1/smoke-report.json \
  --upstream-revision 9e0b455f4ef0e2ab8f2e582289761153549043fc \
  --lwc-commit "$(git rev-parse HEAD)" \
  --lwc-binary "$PWD/target/release/lwc" \
  --limit 5
```

The complete retrieval run omits `--limit`:

```bash
python3 -m benchmarks.agent_memory.longmemeval_v1 \
  --data .local-benchmarks/agent-memory/lme-v1/longmemeval_s_cleaned.json \
  --state-root .local-benchmarks/agent-memory/lme-v1/full-state \
  --output .local-benchmarks/agent-memory/lme-v1/full-report.json \
  --upstream-revision 9e0b455f4ef0e2ab8f2e582289761153549043fc \
  --lwc-commit "$(git rev-parse HEAD)" \
  --lwc-binary "$PWD/target/release/lwc"
```

Only a report with `complete=true`, `instances_processed=500`, and
`instances_retrieval_scored=470` is a complete LongMemEval-S retrieval result.
This runner measures retrieval, not downstream answer generation.

## Agent Memory Leaderboard Add/Search API

Start a local service:

```bash
python3 -m benchmarks.agent_memory.aml_api \
  --state-root .local-benchmarks/agent-memory/aml/state \
  --lwc-binary "$PWD/target/release/lwc" \
  --host 127.0.0.1 \
  --port 8080
```

Exercise the current synchronous contract:

```bash
curl -sS http://127.0.0.1:8080/health
curl -sS http://127.0.0.1:8080/add \
  -H 'Content-Type: application/json' \
  -d '{"request_id":"smoke-1","messages":[{"role":"user","content":"remember indigo-orchid"}],"user_id":"smoke-user","session_id":"smoke-session"}'
curl -sS http://127.0.0.1:8080/search \
  -H 'Content-Type: application/json' \
  -d '{"query":"indigo orchid","user_id":"smoke-user","top_k":5}'
```

Set `AML_MEMORY_API_KEY` to protect Add/Search. Clients may send it as
`Authorization: Bearer ...`, `Authorization: Token ...`, or `X-Api-Key`.
Health remains unauthenticated. Do not place a key in source or an image.

When Docker is available:

```bash
docker build -f benchmarks/agent_memory/Dockerfile -t lwc-aml:local .
docker run --rm -p 8080:8080 \
  -v "$PWD/.local-benchmarks/agent-memory/aml/docker-state:/data" \
  -e AML_MEMORY_API_KEY \
  lwc-aml:local
```

A formal AML evaluation additionally requires a stable public HTTPS endpoint
and an issued AML Eval Key. Running the local service does not submit, score,
or publish anything. Review endpoint exposure, storage lifetime, bandwidth,
and evaluation cost before starting a formal run.

## LongMemEval-V2

The first adapter is deliberately text-only: `text_only=true`. It accepts
`query_image` for protocol compatibility but returns no image context.

Clone and prepare the pinned official harness:

```bash
git clone https://github.com/xiaowu0162/LongMemEval-V2.git \
  .local-benchmarks/upstreams/LongMemEval-V2
git -C .local-benchmarks/upstreams/LongMemEval-V2 \
  checkout 2cc8c540bdb87fe6761629b585e727e1c4704520
python3.11 -m venv .local-benchmarks/venvs/lme-v2
.local-benchmarks/venvs/lme-v2/bin/pip install \
  -r .local-benchmarks/upstreams/LongMemEval-V2/requirements.txt
git -C .local-benchmarks/upstreams/LongMemEval-V2 apply \
  "$PWD/benchmarks/agent_memory/longmemeval_v2.patch"
```

On Intel macOS, if the newest `cryptography` release has no wheel, install a
compatible wheel such as `cryptography==46.0.7` before the requirements.

`run_smokes.sh` downloads the pinned dataset revision
`f152293e235517d504809563c833d7190b8c713b`, verifies the three required file
hashes, and extracts one real Small question plus one trajectory from its
official haystack under `.local-benchmarks/agent-memory/lme-v2/smoke/`:

```text
questions-one.json
haystack-one.json
trajectories-one.json
```

The script generates a per-run memory config. For a manual run, save this as
`lwc-memory-config.json` under the smoke directory with resolved absolute paths:

```json
{
  "memory_type": "lwc",
  "memory_params": {
    "state_root": "/absolute/path/to/.local-benchmarks/agent-memory/lme-v2/state",
    "lwc_binary": "/absolute/path/to/target/release/lwc",
    "search_limit": 10,
    "command_timeout_seconds": 120
  }
}
```

Run the no-model smoke:

```bash
LME_V2_ROOT="$PWD/.local-benchmarks/upstreams/LongMemEval-V2"
PYTHONPATH="$PWD:$LME_V2_ROOT" \
.local-benchmarks/venvs/lme-v2/bin/python "$LME_V2_ROOT/evaluation/harness.py" \
  --domain web \
  --questions-path .local-benchmarks/agent-memory/lme-v2/smoke/questions-one.json \
  --haystack-path .local-benchmarks/agent-memory/lme-v2/smoke/haystack-one.json \
  --trajectories-path .local-benchmarks/agent-memory/lme-v2/smoke/trajectories-one.json \
  --memory-config-path .local-benchmarks/agent-memory/lme-v2/smoke/lwc-memory-config.json \
  --output-dir .local-benchmarks/agent-memory/lme-v2/harness-output \
  --save-memory \
  --skip-evaluation
```

For a full Small or Medium run, first use the official data workflow:

```bash
LME_V2_ROOT="$PWD/.local-benchmarks/upstreams/LongMemEval-V2"
DATA_ROOT="$PWD/.local-benchmarks/agent-memory/lme-v2/data"
.local-benchmarks/venvs/lme-v2/bin/python "$LME_V2_ROOT/data/download_data.py" --data-root "$DATA_ROOT" --revision f152293e235517d504809563c833d7190b8c713b
.local-benchmarks/venvs/lme-v2/bin/python "$LME_V2_ROOT/data/prepare_data.py" --data-root "$DATA_ROOT" --mode symlink
.local-benchmarks/venvs/lme-v2/bin/python "$LME_V2_ROOT/data/validate_data.py" --data-root "$DATA_ROOT" --tier small
```

The current public snapshot is roughly 1.2 GB of trajectories plus 5.9 GB of
screenshots. A scored run also requires the fixed Qwen3.5-9B reader endpoint
and GPT-5.2 judge configuration required by the package validator. Full reader,
judge, packaging, and submission commands are intentionally a separately
reviewed execution because they spend external resources and may publish an
artifact. Use the upstream `leaderboard/README.md` only after both `web` and
`enterprise` runs are complete.

## Verification

The deterministic adapter suite uses a real LWC binary:

```bash
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
python3 -m unittest discover -s benchmarks/agent_memory -p 'test_*.py' -v
```

V2-specific tests additionally need the pinned, patched upstream and its
Python environment:

```bash
LME_V2_ROOT="$PWD/.local-benchmarks/upstreams/LongMemEval-V2" \
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
PYTHONPATH="$PWD:$PWD/.local-benchmarks/upstreams/LongMemEval-V2" \
.local-benchmarks/venvs/lme-v2/bin/python -m unittest \
  benchmarks.agent_memory.test_adapters.LongMemEvalV2Tests -v
```

`smoke` means protocol compatibility on a bounded subset. `partial` means a
dataset run stopped by an explicit limit. `complete` means every expected
instance was processed. `scored` requires the benchmark's official answer and
judge pipeline. `submitted` requires successful organizer ingestion and is
never implied by a local report.
