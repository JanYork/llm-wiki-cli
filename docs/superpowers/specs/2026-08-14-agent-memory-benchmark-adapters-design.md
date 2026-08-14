# Agent Memory Benchmark Adapters Design

## Goal

Make LWC reproducibly runnable against three current evaluation surfaces:

1. LongMemEval V1 / LongMemEval-S retrieval and end-to-end QA;
2. Agent Memory Leaderboard (AML) synchronous Add/Search evaluation;
3. LongMemEval-V2 Small, with a path to Medium after Small is verified.

The first result must measure LWC itself. External model-assisted memory
compilation is a later operating point, enabled only when the pure-LWC result
shows that it is needed and the benchmark permits the exact model.

## Non-goals

- Do not add a writable public server or multi-tenant mode to the LWC product.
- Do not add embeddings, a vector database, or a new Rust dependency.
- Do not commit benchmark corpora, generated memories, model outputs, API keys,
  or leaderboard credentials.
- Do not submit a formal run, publish a score, expose a public endpoint, or
  spend judge/model credits without a separately reviewed execution step.
- Do not claim that the repository's existing local retrieval benchmark is a
  LongMemEval or AML result.

## Chosen approach

Use a small Python adapter package under `benchmarks/agent_memory/`. It invokes
the released `lwc` CLI as a black box and stores all benchmark state under a
caller-selected output root. This keeps benchmark-only protocol and lifecycle
code out of the Rust product while testing the same public interface users run.

Two operating points are possible:

- `fast`: pure LWC source storage and lexical passage retrieval. This is the
  required first run and the only initially implemented operating point.
- `accurate`: optional benchmark-permitted `gpt-4o-mini` compilation or rerank.
  It is added only after the fast report identifies a concrete accuracy gap;
  it must remain a separate, disclosed operating point.

## Components

### Shared LWC backend

`lwc_backend.py` owns the only translation between benchmark memory operations
and LWC commands.

- Convert every external scope identifier to a fixed SHA-256 directory name;
  never use an untrusted identifier as a path.
- Initialize one project Wiki per evaluation scope below the configured state
  root. LongMemEval V1 uses a question/evaluation-instance scope; AML uses the
  supplied `user_id`; LongMemEval-V2 uses the harness memory-instance scope.
- Serialize ordered messages or a trajectory to deterministic UTF-8 Markdown.
  Preserve roles, timestamps, session identifiers, and original text.
- Make Add idempotent by keying the file on the request or trajectory identity.
  Repeating identical content succeeds; reusing an identity with different
  content fails rather than silently revising evaluation history.
- Add the file as an immutable LWC Source. Add returns only after the source is
  searchable.
- Search with source-only passage retrieval, resolve returned evidence from the
  owning immutable Source, preserve ranking order, and return no more than the
  requested limit.
- Invoke `lwc` without a shell, parse JSON stdout, parse structured JSON stderr,
  apply a bounded timeout, and surface a stable adapter error.

The adapter does not run LWC's agent ingest/page-authoring loop in the fast
operating point. That loop requires an Agent and would otherwise make the
baseline depend on an undisclosed model and prompt.

### LongMemEval V1 runner

`longmemeval_v1.py` consumes the official cleaned JSON data.

For each evaluation instance it creates an isolated backend, adds every history
session, searches with the official question, and records ranked session IDs.
It reports session-level Recall@5, Recall@10, MRR, and query latency using the
official evidence session IDs, excluding abstention questions from retrieval
metrics as required by the upstream protocol. It writes retrieval JSONL in a
form that can be transformed into the official generation/evaluation pipeline.

The runner supports an explicit `--limit` for a non-scored smoke run. A report
with a limit is labelled partial and cannot be presented as a benchmark score.

### AML HTTP adapter

`aml_api.py` exposes `GET /health`, `POST /add`, and `POST /search` with the
current AML synchronous schemas. It uses only Python's standard library and the
shared backend.

- `/add` validates the exact required fields, stores all messages under the
  supplied `user_id`, waits for searchable persistence, and echoes the required
  identifiers.
- `/search` searches only that `user_id`, returns a JSON object containing a
  relevance-ordered `data` array, and never generates a final answer.
- Per-scope locks serialize writes without globally serializing independent
  evaluation scopes.
- Authentication is an optional fixed bearer/API key supplied through the
  environment. `/health` is always unauthenticated.
- A minimal Dockerfile builds the release LWC binary and starts the adapter.

The public service lifecycle remains benchmark-specific and does not become an
`lwc serve` product mode.

### LongMemEval-V2 backend

`longmemeval_v2.py` provides the official `Memory` subclass in its own
benchmark environment. `insert()` writes each full trajectory through the
shared backend. `query()` returns ranked non-empty text context items in the
official format. The first version deliberately does not return image items;
the result documentation must disclose this text-only limitation.

Small is run before Medium. Medium is attempted only after Small completes and
storage/runtime measurements show that it is operationally reasonable.

## Data and execution flow

```text
official dataset or AML request
        -> protocol-specific parser and validation
        -> shared LWC backend
        -> isolated project Wiki + immutable Source files
        -> LWC source passage search
        -> ranked evidence in the benchmark's native format
        -> official answerer, judge, metrics, or submission packager
```

All generated data lives below `.local-benchmarks/agent-memory/` by default and
must remain ignored by Git. An explicit output root may be used for large local
runs.

## Failure and integrity rules

- Invalid input fails before any write.
- A failed LWC command returns a redacted error; command arguments containing
  credentials are never logged.
- Duplicate Add retries cannot create duplicate memory.
- No Search can cross an evaluation scope, even when the query text is equal.
- Benchmark question IDs, gold answers, evidence labels, and evaluation metadata
  are never passed into Search as hidden retrieval hints.
- Partial, interrupted, or judge-free runs are labelled as such.
- Formal AML runs use a frozen commit/version. Formal LongMemEval-V2 packages
  use the official reader and judge models required by its package validator.
- Every runner records the exact upstream benchmark commit or evaluation-contract
  revision used; adapters target pinned revisions rather than a moving branch.

## Testing

Use Python `unittest` and temporary directories; no new test dependency.

1. Unit-test identifier hashing, deterministic serialization, request
   validation, idempotency, conflict detection, and metric aggregation.
2. Integration-test the shared backend against a real release-built `lwc`
   binary using two isolated scopes and deliberately overlapping vocabulary.
3. Start the AML server locally and exercise health, Add, Search, invalid input,
   duplicate Add, and cross-user isolation through HTTP.
4. Run LongMemEval V1 on a small official subset, then the complete cleaned
   LongMemEval-S retrieval set.
5. Import the V2 adapter in the official repository, run its smallest supported
   smoke tier/sample, then run Small when required model endpoints and judge
   credentials are present.
6. Run repository formatting, linting, tests, and release build before claiming
   completion.

## Acceptance

- One documented command runs every local smoke from a clean checkout.
- LongMemEval-S produces a complete, non-partial retrieval report for every
  non-abstention instance.
- AML local HTTP smoke proves synchronous persistence and scope isolation; its
  Docker image builds and passes the same smoke when Docker is available.
- LongMemEval-V2 loads the custom backend and completes an official harness
  smoke. Small and Medium status are reported separately and truthfully.
- Every generated report records the LWC version/commit, dataset identity,
  upstream benchmark revision, adapter configuration, completion state, and
  latency.
- External blockers such as missing keys, unavailable compute, closed submission
  windows, or organizer approval are reported as blockers to formal evaluation,
  never rewritten as local success.
