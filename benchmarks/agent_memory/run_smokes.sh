#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"

local_root="$repo_root/.local-benchmarks"
v1_root="$local_root/agent-memory/lme-v1"
v2_root="$local_root/upstreams/LongMemEval-V2"
v2_data="$local_root/agent-memory/lme-v2/official"
v2_venv="$local_root/venvs/lme-v2"
v2_revision="2cc8c540bdb87fe6761629b585e727e1c4704520"
v2_data_revision="f152293e235517d504809563c833d7190b8c713b"
lwc_binary="$repo_root/target/release/lwc"
lwc_commit="$(git rev-parse HEAD)"
v1_smoke_root="$v1_root/smoke-$lwc_commit"

cargo build --release --locked
mkdir -p "$v1_root"
if ! printf '%s  %s\n' \
  d6f21ea9d60a0d56f34a05b609c79c88a451d2ae03597821ea3d5a9678c3a442 \
  "$v1_root/longmemeval_s_cleaned.json" | shasum -a 256 -c - >/dev/null 2>&1; then
  curl -fL \
    https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/98d7416c24c778c2fee6e6f3006e7a073259d48f/longmemeval_s_cleaned.json \
    -o "$v1_root/longmemeval_s_cleaned.json"
fi
printf '%s  %s\n' \
  d6f21ea9d60a0d56f34a05b609c79c88a451d2ae03597821ea3d5a9678c3a442 \
  "$v1_root/longmemeval_s_cleaned.json" | shasum -a 256 -c -
PYTHONDONTWRITEBYTECODE=1 python3 -m benchmarks.agent_memory.longmemeval_v1 \
  --data "$v1_root/longmemeval_s_cleaned.json" \
  --state-root "$v1_smoke_root/state" \
  --output "$v1_smoke_root/report.json" \
  --upstream-revision 9e0b455f4ef0e2ab8f2e582289761153549043fc \
  --lwc-commit "$lwc_commit" \
  --lwc-binary "$lwc_binary" \
  --limit 5

LWC_BENCH_BINARY="$lwc_binary" PYTHONDONTWRITEBYTECODE=1 \
  python3 -m unittest benchmarks.agent_memory.test_adapters.AmlApiTests -v

if [[ ! -d "$v2_root/.git" ]]; then
  git clone https://github.com/xiaowu0162/LongMemEval-V2.git "$v2_root"
  git -C "$v2_root" checkout "$v2_revision"
fi
if [[ "$(git -C "$v2_root" rev-parse HEAD)" != "$v2_revision" ]]; then
  echo "LongMemEval-V2 checkout is not at $v2_revision" >&2
  exit 1
fi
if git -C "$v2_root" apply --check \
  "$repo_root/benchmarks/agent_memory/longmemeval_v2.patch" 2>/dev/null; then
  git -C "$v2_root" apply "$repo_root/benchmarks/agent_memory/longmemeval_v2.patch"
elif ! git -C "$v2_root" apply --reverse --check \
  "$repo_root/benchmarks/agent_memory/longmemeval_v2.patch" 2>/dev/null; then
  echo "LongMemEval-V2 patch is neither cleanly applicable nor already applied" >&2
  exit 1
fi

if [[ ! -x "$v2_venv/bin/python" ]]; then
  python3.11 -m venv "$v2_venv"
fi
if ! "$v2_venv/bin/python" -c 'import huggingface_hub, openai, PIL, tqdm, transformers' 2>/dev/null; then
  if [[ "$(uname -s)" == Darwin && "$(uname -m)" == x86_64 ]]; then
    "$v2_venv/bin/pip" install cryptography==46.0.7
  fi
  "$v2_venv/bin/pip" install -r "$v2_root/requirements.txt"
fi

export V2_DATA_ROOT="$v2_data"
export V2_DATA_REVISION="$v2_data_revision"
"$v2_venv/bin/python" - <<'PY'
import os
from pathlib import Path

from huggingface_hub import hf_hub_download

root = Path(os.environ["V2_DATA_ROOT"])
for filename in (
    "questions.jsonl",
    "haystacks/lme_v2_small.json",
    "trajectories.jsonl",
):
    hf_hub_download(
        repo_id="xiaowu0162/longmemeval-v2",
        repo_type="dataset",
        revision=os.environ["V2_DATA_REVISION"],
        filename=filename,
        local_dir=root,
    )
PY
(
  cd "$v2_data"
  printf '%s  %s\n' \
    0a3ae5ebea938c24d7800e1e0b0828e08ae1646f939a53853b2b8cdc08e292b7 questions.jsonl \
    9b5301defb23a088a5f06e45ff8d5f35e569d78305a66d492046a9fff9b46593 haystacks/lme_v2_small.json \
    363cec9a8e87aa8d9101ce4e600aadbf7031d674056ebe4f969e8424abc5f3c6 trajectories.jsonl \
    | shasum -a 256 -c -
)

smoke_root="$local_root/agent-memory/lme-v2/smoke"
mkdir -p "$smoke_root"
export V2_SMOKE_ROOT="$smoke_root"
export LWC_BENCH_BINARY="$lwc_binary"
"$v2_venv/bin/python" - <<'PY'
import json
import os
from pathlib import Path

data = Path(os.environ["V2_DATA_ROOT"])
out = Path(os.environ["V2_SMOKE_ROOT"])
question_id = "00aa905a"
trajectory_id = "f1b80250"

question = next(
    item
    for line in (data / "questions.jsonl").open(encoding="utf-8")
    if (item := json.loads(line))["id"] == question_id
)
haystacks = json.loads(
    (data / "haystacks/lme_v2_small.json").read_text(encoding="utf-8")
)
if trajectory_id not in haystacks[question_id]:
    raise RuntimeError("pinned trajectory is not in the pinned Small haystack")
trajectory = next(
    item
    for line in (data / "trajectories.jsonl").open(encoding="utf-8")
    if (item := json.loads(line))["id"] == trajectory_id
)

(out / "questions-one.json").write_text(json.dumps([question]), encoding="utf-8")
(out / "haystack-one.json").write_text(
    json.dumps({question_id: [trajectory_id]}), encoding="utf-8"
)
(out / "trajectories-one.json").write_text(json.dumps([trajectory]), encoding="utf-8")
PY

run_root="$(mktemp -d "$local_root/agent-memory/lme-v2/run.XXXXXX")"
export V2_RUN_ROOT="$run_root"
"$v2_venv/bin/python" - <<'PY'
import json
import os
from pathlib import Path

run = Path(os.environ["V2_RUN_ROOT"])
config = {
    "memory_type": "lwc",
    "memory_params": {
        "state_root": str(run / "state"),
        "lwc_binary": os.environ["LWC_BENCH_BINARY"],
        "search_limit": 10,
        "command_timeout_seconds": 120,
    },
}
(run / "lwc-memory-config.json").write_text(json.dumps(config, indent=2), encoding="utf-8")
PY

PYTHONPATH="$repo_root:$v2_root" PYTHONDONTWRITEBYTECODE=1 "$v2_venv/bin/python" \
  "$v2_root/evaluation/harness.py" \
  --domain web \
  --questions-path "$smoke_root/questions-one.json" \
  --haystack-path "$smoke_root/haystack-one.json" \
  --trajectories-path "$smoke_root/trajectories-one.json" \
  --memory-config-path "$run_root/lwc-memory-config.json" \
  --output-dir "$run_root/output" \
  --save-memory \
  --skip-evaluation

LME_V2_ROOT="$v2_root" LWC_BENCH_BINARY="$lwc_binary" \
  PYTHONPATH="$repo_root:$v2_root" PYTHONDONTWRITEBYTECODE=1 \
  "$v2_venv/bin/python" -m unittest \
  benchmarks.agent_memory.test_adapters.LongMemEvalV2Tests -v

printf 'All local agent-memory smokes passed. V2 output: %s\n' "$run_root/output"
