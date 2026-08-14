# Agent Memory Benchmark Adapters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add reproducible adapters that run LWC against LongMemEval-S, the AML Add/Search contract, and the LongMemEval-V2 memory harness.

**Architecture:** Keep all benchmark protocol code in a standard-library Python package under `benchmarks/agent_memory/`. A single `LwcBackend` invokes the public `lwc` CLI against one isolated project Wiki per benchmark scope; thin protocol adapters translate LongMemEval V1, AML HTTP, and LongMemEval-V2 inputs and outputs without changing the Rust product.

**Tech Stack:** Python 3.11+ standard library, released `lwc` CLI, `unittest`, official LongMemEval and AML evaluation repositories, Docker only for the AML deployment artifact.

---

## File map

- Modify `.gitignore`: exclude local benchmark corpora, cloned upstreams, state, and reports.
- Create `benchmarks/agent_memory/lwc_backend.py`: safe scope isolation, deterministic memory files, idempotent Add, LWC command invocation, and ranked evidence retrieval.
- Create `benchmarks/agent_memory/longmemeval_v1.py`: cleaned LongMemEval-S retrieval runner and metric report.
- Create `benchmarks/agent_memory/aml_api.py`: synchronous AML health/Add/Search HTTP service.
- Create `benchmarks/agent_memory/longmemeval_v2.py`: official V2 `Memory` subclass backed by `LwcBackend`.
- Create `benchmarks/agent_memory/longmemeval_v2.patch`: one pinned upstream registration patch for the V2 harness.
- Create `benchmarks/agent_memory/test_adapters.py`: unit and real-CLI integration coverage for all adapters.
- Create `benchmarks/agent_memory/Dockerfile`: minimal AML source-build image.
- Create `benchmarks/agent_memory/README.md`: exact pinned setup, smoke, full-run, packaging, and safety commands.

### Task 1: Shared LWC backend

**Files:**
- Modify: `.gitignore`
- Create: `benchmarks/agent_memory/test_adapters.py`
- Create: `benchmarks/agent_memory/lwc_backend.py`

- [ ] **Step 1: Ignore generated benchmark state**

Add only:

```gitignore
.local-benchmarks/
```

- [ ] **Step 2: Write failing backend tests**

Add `unittest` cases that prove:

```python
def test_scope_path_is_fixed_hash_below_state_root(): ...
def test_add_is_idempotent_and_rejects_changed_retry(): ...
def test_search_returns_ranked_source_content_and_metadata(): ...
def test_search_never_crosses_scope(): ...
```

Use `tempfile.TemporaryDirectory`, the release/debug LWC binary selected by
`LWC_BENCH_BINARY`, and real subprocesses. Use two scopes containing the same
generic vocabulary but distinct unique needles.

- [ ] **Step 3: Run the tests and verify RED**

Run:

```bash
python3 -m unittest benchmarks.agent_memory.test_adapters.BackendTests -v
```

Expected: import failure because `lwc_backend.py` does not exist.

- [ ] **Step 4: Implement the minimal backend**

Implement these public values:

```python
@dataclass(frozen=True)
class Evidence:
    id: str
    content: str
    score: float
    created_at: str
    session_id: str

class AdapterError(RuntimeError): ...
class ConflictError(AdapterError): ...

class LwcBackend:
    def __init__(self, state_root: Path, binary: str | Path = "lwc", timeout: float = 120.0): ...
    def add(self, scope_id: str, memory_id: str, session_id: str,
            messages: list[dict[str, object]]) -> None: ...
    def add_many(self, scope_id: str,
                 memories: list[tuple[str, str, list[dict[str, object]]]]) -> None: ...
    def search(self, scope_id: str, query: str, limit: int) -> list[Evidence]: ...
```

Implementation constraints:

- Require non-empty scope, identity, session, role, content, and query values.
- Use SHA-256 digests for scope and memory directory/file names.
- Serialize Markdown deterministically with benchmark metadata headings and
  ordered role/timestamp lines.
- Create the scope root and run `lwc init` once. Keep all source files inside
  that scope root.
- For a retry, compare the existing file bytes. Identical bytes return without
  another `source add`; different bytes raise `ConflictError`.
- Use one `source add-manifest` command for `add_many`.
- Invoke commands as an argument list with `LWC_PROJECT_ROOT` set only on that
  subprocess. Never invoke a shell.
- Run `lwc search QUERY --type source --granularity passage --limit LIMIT`.
- Resolve each result's owning Source identifier, call `source show`, and return
  the complete immutable Source content and origin metadata in ranking order.
- Parse LWC structured JSON stderr into `AdapterError`; never include ambient
  environment values in an error.

- [ ] **Step 5: Run backend tests and verify GREEN**

```bash
cargo build --release
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
  python3 -m unittest benchmarks.agent_memory.test_adapters.BackendTests -v
```

Expected: all backend tests pass.

- [ ] **Step 6: Commit**

```bash
git add .gitignore benchmarks/agent_memory/lwc_backend.py benchmarks/agent_memory/test_adapters.py
git commit -m "feat: add isolated LWC benchmark backend"
```

### Task 2: LongMemEval-S retrieval runner

**Files:**
- Modify: `benchmarks/agent_memory/test_adapters.py`
- Create: `benchmarks/agent_memory/longmemeval_v1.py`

- [ ] **Step 1: Write failing dataset and metric tests**

Create a two-instance synthetic dataset containing one normal question and one
`_abs` abstention question. Prove that the runner:

- preserves `haystack_session_ids`, dates, roles, and contents;
- excludes abstention from retrieval denominators;
- computes Recall@5, Recall@10, MRR, p50, and p95 correctly;
- marks `--limit` reports as partial;
- records dataset SHA-256, upstream revision, LWC version, and completion count.

- [ ] **Step 2: Run the tests and verify RED**

```bash
python3 -m unittest benchmarks.agent_memory.test_adapters.LongMemEvalV1Tests -v
```

Expected: import failure because `longmemeval_v1.py` does not exist.

- [ ] **Step 3: Implement the runner**

Provide:

```bash
python3 -m benchmarks.agent_memory.longmemeval_v1 \
  --data PATH/longmemeval_s_cleaned.json \
  --state-root .local-benchmarks/agent-memory/lme-v1/state \
  --output .local-benchmarks/agent-memory/lme-v1/report.json \
  --upstream-revision REV [--limit N]
```

For every question, batch-add its sessions with the session ID as memory ID,
search the unchanged official question, map results back to session IDs, write
one retrieval JSONL record per question, and atomically replace the final JSON
report. Retrieval metrics skip all `_abs` questions. Any `--limit` makes
`complete=false` and `partial=true`.

- [ ] **Step 4: Run the tests and verify GREEN**

```bash
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
  python3 -m unittest benchmarks.agent_memory.test_adapters.LongMemEvalV1Tests -v
```

Expected: all V1 tests pass.

- [ ] **Step 5: Commit**

```bash
git add benchmarks/agent_memory/longmemeval_v1.py benchmarks/agent_memory/test_adapters.py
git commit -m "feat: add LongMemEval retrieval runner"
```

### Task 3: AML synchronous Add/Search service

**Files:**
- Modify: `benchmarks/agent_memory/test_adapters.py`
- Create: `benchmarks/agent_memory/aml_api.py`
- Create: `benchmarks/agent_memory/Dockerfile`

- [ ] **Step 1: Write failing AML validation and HTTP tests**

Exercise a real ephemeral `ThreadingHTTPServer` with `urllib.request` and prove:

- unauthenticated `GET /health` returns 2xx;
- valid Add echoes `success`, `request_id`, `user_id`, and `session_id` only
  after Search can retrieve the content;
- duplicate identical Add succeeds and changed reuse returns 409;
- malformed input returns 422;
- Search returns `{"data": [...]}` with required non-empty `id` and `content`;
- Search respects `top_k` and cannot return another user's unique memory;
- configured Bearer authentication protects Add/Search but not Health.

- [ ] **Step 2: Run the tests and verify RED**

```bash
python3 -m unittest benchmarks.agent_memory.test_adapters.AmlApiTests -v
```

Expected: import failure because `aml_api.py` does not exist.

- [ ] **Step 3: Implement AML HTTP service**

Use `BaseHTTPRequestHandler` + `ThreadingHTTPServer`. Validate the current AML
contract exactly. Use a lock registry keyed by the hashed `user_id` so writes
within one scope serialize while independent scopes run concurrently. Read
configuration only from command arguments/environment:

```text
--host (default 127.0.0.1)
--port (default 8080)
--state-root (required)
--lwc-binary (default lwc)
AML_MEMORY_API_KEY (optional)
```

Do not log request bodies or credentials. Map validation to 422, identity
conflict to 409, authentication to 401, and backend failures to redacted 500.

- [ ] **Step 4: Add the minimal Docker image**

Use a Rust builder stage to compile `lwc --release`, then a Python 3.11 slim
runtime containing only the binary and `benchmarks/agent_memory`. Start
`python -m benchmarks.agent_memory.aml_api` and expose 8080. Do not bake keys
or benchmark data into the image.

- [ ] **Step 5: Run tests and local service smoke**

```bash
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
  python3 -m unittest benchmarks.agent_memory.test_adapters.AmlApiTests -v
```

Then start on port 0 through the test helper and repeat one Add/Search pair.
Expected: tests pass and the unique memory is returned.

- [ ] **Step 6: Build and smoke Docker when available**

```bash
docker build -f benchmarks/agent_memory/Dockerfile -t lwc-aml:local .
```

If Docker is unavailable, record `not_run: docker_unavailable`; do not call the
image verified.

- [ ] **Step 7: Commit**

```bash
git add benchmarks/agent_memory/aml_api.py benchmarks/agent_memory/Dockerfile benchmarks/agent_memory/test_adapters.py
git commit -m "feat: add AML Add Search adapter"
```

### Task 4: LongMemEval-V2 memory module

**Files:**
- Modify: `benchmarks/agent_memory/test_adapters.py`
- Create: `benchmarks/agent_memory/longmemeval_v2.py`
- Create: `benchmarks/agent_memory/longmemeval_v2.patch`

- [ ] **Step 1: Write failing V2 adapter tests**

Against a clone of the pinned V2 revision, verify that:

- the patch applies cleanly and imports/registers `memory_type="lwc"`;
- `insert()` stores a complete trajectory without benchmark gold metadata;
- `query()` returns a list of non-empty `{type: "text", value: ...}` items;
- separate memory instances use separate state roots;
- `_save_backend()` and `_load_backend()` preserve/recover the LWC state path;
- `query_image` is accepted but does not leak its path into text evidence.

- [ ] **Step 2: Run the tests and verify RED**

```bash
LME_V2_ROOT=.local-benchmarks/upstreams/LongMemEval-V2 \
  python3 -m unittest benchmarks.agent_memory.test_adapters.LongMemEvalV2Tests -v
```

Expected: import/patch failure because the V2 adapter does not exist.

- [ ] **Step 3: Implement and register the adapter**

`longmemeval_v2.py` imports `Memory`, `MemoryContextItem`, and
`register_memory` from the pinned upstream package, defines `LwcMemory`, and
accepts only these memory params:

```json
{
  "state_root": "/absolute/path",
  "lwc_binary": "/absolute/path/to/lwc",
  "search_limit": 10,
  "command_timeout_seconds": 120
}
```

Use a per-instance random scope identifier generated at construction; do not
use query context or question metadata as a retrieval hint. `_save_backend()`
copies the instance's adapter-owned scope directory below the official memory
output directory, and `_load_backend()` restores from that relative copy, so a
saved memory is relocatable and does not retain its creator's absolute path.

The pinned patch changes only `memory_modules/__init__.py` to import the LWC
class from this repository via `PYTHONPATH`. Full runs invoke
`evaluation/harness.py` directly with a `memory_type: lwc` config, avoiding a
wider patch to `evaluation/run_eval.py`.

- [ ] **Step 4: Run V2 adapter tests and official no-model harness smoke**

Use one official question/haystack and:

```bash
PYTHONPATH="$PWD:$LME_V2_ROOT" \
python3 "$LME_V2_ROOT/evaluation/harness.py" \
  --domain web \
  --questions-path PATH/questions-one.json \
  --haystack-path PATH/haystack-one.json \
  --trajectories-path PATH/trajectories.jsonl \
  --memory-config-path PATH/lwc-memory-config.json \
  --output-dir PATH/output \
  --save-memory --skip-evaluation
```

Expected: the official harness builds and saves an LWC memory without a reader
or judge endpoint.

- [ ] **Step 5: Commit**

```bash
git add benchmarks/agent_memory/longmemeval_v2.py benchmarks/agent_memory/longmemeval_v2.patch benchmarks/agent_memory/test_adapters.py
git commit -m "feat: add LongMemEval V2 memory adapter"
```

### Task 5: Reproducible operator guide and public-data runs

**Files:**
- Create: `benchmarks/agent_memory/README.md`
- Modify: `benchmarks/README.md`

- [ ] **Step 1: Write the operator guide**

Document:

- exact upstream repository URLs and tested revisions;
- the official cleaned LongMemEval-S download URL and SHA-256 capture command;
- release build and complete V1 retrieval command;
- AML local smoke, Docker, authentication, compatibility smoke, and formal
  submission boundaries;
- V2 clone, patch, data preparation, no-model harness smoke, Small web +
  enterprise full commands, and leaderboard package commands clearly labelled
  as a future, separately reviewed execution;
- the initial V2 adapter's explicit `text_only=true` limitation and lack of
  returned image context;
- expected external resources (reader endpoint, judge key, AML Eval Key/public
  endpoint) without embedding their values;
- output meanings: complete, partial, smoke, full, scored, and submitted.

Link it from `benchmarks/README.md` without presenting external results as part
of the existing local source-search benchmark.

- [ ] **Step 2: Run all deterministic tests**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
  python3 -m unittest discover -s benchmarks/agent_memory -p 'test_*.py' -v
```

Expected: every command exits 0.

- [ ] **Step 3: Run complete public LongMemEval-S retrieval**

Download the official cleaned dataset into `.local-benchmarks`, record its
SHA-256 and the upstream commit, then run without `--limit`. Require the output
record count to equal the pinned dataset length, the retrieval-scored count to
equal the dataset's computed non-abstention count, `complete=true`, and a real
metrics/latency report before describing it as complete.

- [ ] **Step 4: Run AML local and Docker smoke**

Require the same Add/Search/isolation assertions against the direct service and
the container. If Docker is missing, preserve direct-service evidence and mark
container status unverified.

- [ ] **Step 5: Run V2 official harness smoke and inspect full-run readiness**

Clone the pinned upstream revision, apply the adapter patch, download/prepare
the official Small data, and run the `--skip-evaluation` harness smoke. Inspect
only the presence (not value) of reader/judge endpoint variables and required
executables. Record readiness without running Small web/enterprise readers,
judges, or the official submission packager; those spend external resources
and require a separate reviewed execution step. Always record `text_only=true`
and leave the full score unclaimed at this stage.

- [ ] **Step 6: Commit documentation**

```bash
git add benchmarks/agent_memory/README.md benchmarks/README.md
git commit -m "docs: document agent memory benchmark runs"
```

### Task 6: Final completion audit

**Files:**
- Inspect all files and generated reports; modify only if verification finds a defect.

- [ ] **Step 1: Verify repository scope**

```bash
git status --short
git diff --check HEAD~5..HEAD
git log --oneline -6
```

Confirm unrelated pre-existing untracked files were not added.

- [ ] **Step 2: Verify every acceptance item against current evidence**

Check the design acceptance list one item at a time: clean-checkout command,
complete V1 report, AML direct/Docker smoke, V2 import/harness smoke, report
metadata, and truthful external blocker labels.

- [ ] **Step 3: Preserve durable project knowledge only if verified and reusable**

If the completed adapters establish stable benchmark commands/results, update
the existing LWC quality/benchmark Wiki topic through an audited changeset,
then lint, run fixed original/paraphrase retrieval checks, commit the changeset,
and verify Grafeo independently. Skip memory mutation when no stable fact is
ready.

- [ ] **Step 4: Report actual completion state**

Report local adapter/test/run evidence separately from formal remote scoring
and leaderboard publication. Do not call the goal complete while a required,
available run remains unexecuted.
