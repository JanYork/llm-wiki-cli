<h1 align="center">lwc</h1>

<p align="center">
  <strong>Persistent, source-grounded wikis for LLM agents.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a>
</p>

`lwc` is an agent-first CLI that turns curated documents into a durable Wiki.
The agent reasons and synthesizes; `lwc` preserves sources, pages, citations,
links, indexes, and history so knowledge compounds instead of being rediscovered
from raw chunks on every query.

## LWC Is Agent Memory, Not RAG

RAG and LWC can both help an LLM work with external documents, but they keep
state in different places. A typical RAG request retrieves raw chunks and builds
one answer at query time:

```text
query -> retrieve chunks -> generate answer
```

LWC keeps the useful work between requests:

```text
task -> recall maintained Wiki -> reason from sources and prior synthesis
     -> write durable improvements back
```

Retrieval is one operation inside LWC, not its organizing principle. The durable
artifact is a source-grounded Wiki whose pages, citations, links,
contradictions, and history are revised as knowledge changes. LWC therefore
does not require embeddings or a vector database, and it does not discard each
synthesis after answering. It can complement RAG, but it is not query-time RAG.

### The Agent operates LWC

`lwc` is a machine interface for Agents, not a human-facing note-taking app. In
normal use, a human selects sources, states goals, asks questions, and reviews
answers or the projected Markdown. The Agent runs the CLI, manages scope,
integrates sources, maintains citations and links, and decides what is worth
recalling or writing back.

Do not manually drive the routine `lwc` workflow unless you are developing or
debugging the tool. Ask your Agent to activate the bundled `using-lwc` Skill
instead—usually as `$using-lwc`. The setup below also registers `$using-wiki`
where the Agent runtime supports named Skill commands.

## Recommended: Ask Your Agent to Set Up LWC

Paste this prompt into the Agent you use. It uses that Agent's own native
settings to install the CLI and user-level Skills, initialize global memory,
and add a minimal session-start reminder when Hooks are available—all without
overwriting existing configuration.

<details>
<summary><strong>Copy the complete setup prompt</strong></summary>

```text
Configure LWC completely for the current user and the Agent runtime executing
this prompt. Perform the work and verify it; do not merely describe commands
for me to run.

Source of truth:
- https://github.com/JanYork/llm-wiki-cli
- https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc

Requirements:
1. Read the repository README, SECURITY.md, the complete
   skills/using-lwc/SKILL.md, and every directly required script/reference
   before executing it. Record the source commit SHA used for installation.
2. Use your own native user-level locations and mechanisms for Skills, global
   instructions, and lifecycle Hooks. Do not ask the user to identify your
   configuration files, assume another Agent's filenames, or configure other
   installed Agent runtimes unless explicitly requested.
3. Preserve all existing user configuration. Before changing an existing file
   or Skill, create a timestamped backup; make every edit idempotent and do not
   duplicate blocks when this prompt is run again.
4. Install or update `skills/using-lwc` as the canonical user-level Skill. If
   the runtime supports named Skill commands, register `$using-lwc` from that
   canonical Skill and `$using-wiki` as a thin alias that delegates to it. Do
   not copy the implementation into two independently maintained Skills. If the
   runtime uses different invocation syntax, expose the closest native aliases
   and report their exact names. Verify both entry points when supported.
5. From the current host-authorized workspace root, run the canonical Skill's
   bootstrap exactly as its instructions require. Let it install the official
   SHA-256-verified LWC release when needed and initialize ~/.lwc global memory.
   Validate the returned JSON, the absolute lwc path, `lwc --version`, and the
   global Wiki. Do not initialize any project Wiki unless the user explicitly
   requests it for that project; never use global memory as a fallback for
   project-specific writes.
6. Apply the smallest complementary integration through your own native
   configuration:
   - Add a concise LWC routing rule to your additive user-level global
     system/developer instructions. Do not replace your built-in prompt.
   - Merge one LWC section into your user-level global instruction file,
     whatever that file is named. Enclose it with the exact comments
     `<!-- LWC_START -->` and `<!-- LWC_END -->`. On later runs, replace only
     the content inside those markers and leave every user-owned line outside
     them untouched. If only one marker exists, stop rather than guessing where
     user content ends. The section should require use of the canonical LWC
     Skill for substantive project, research, planning, debugging, decision,
     or document-ingest work; recall before re-deriving; write back durable
     findings; keep project and global scopes separate; respect the authorized
     workspace boundary; and never store secrets, raw chain-of-thought,
     transient logs, or unsupported guesses.
   - When you support lifecycle Hooks, create or merge one native user-level
     session-start Hook. It should only add a brief reminder to evaluate and use
     the LWC Skill at suitable times. The Hook must not read, initialize, or
     mutate a project Wiki itself. Do not add a per-prompt Hook, replace
     unrelated Hooks, or bypass Hook trust review. If you do not support Hooks,
     rely on the global instructions and report that limitation instead of
     inventing a mechanism.
7. Keep the Skill, global instructions, and Hook short and complementary instead
   of repeating the full policy. The canonical Skill remains authoritative.
8. Validate every changed config file and Hook executable, confirm existing
   config is preserved, and run the smallest safe smoke checks. Use only your
   native supported configuration rather than inventing keys or filenames.

Finish with a concise report containing: detected Agent runtime, installed LWC
version and path, source commit, installed Skill and alias paths, global Wiki
path, Hook/config files changed, backup paths, validation performed, unsupported
integrations, and anything that requires a new Agent session or normal Hook
trust approval.
```

</details>

## Origin and Acknowledgements

`lwc` implements the [LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)
pattern proposed by Andrej Karpathy: an LLM incrementally builds and maintains a
persistent, interlinked Wiki instead of reconstructing knowledge from raw
documents for every query. The CLI architecture and selected implementation
details also draw inspiration from
[`nashsu/llm_wiki`](https://github.com/nashsu/llm_wiki).

This project adapts those ideas into an agent-first Rust CLI backed by SQLite.

## Core Design

```text
+-----------------------------------------------------------------------+
|                              AGENT PLANE                              |
+-----------------------------------------------------------------------+
| User Task -> LLM Agent -> using-lwc Skill                             |
|                         trigger | bootstrap | recall | write-back     |
+-----------------------------------------------------------------------+
                                   |
                          JSON / stdin / files
                                   v
+-----------------------------------------------------------------------+
|                               CLI LAYER                               |
+-----------------------------------------------------------------------+
| clap command router                                                   |
| init | schema | purpose | source | page | ingest | search | context   |
| graph | lint | maintenance | checkpoint | log                         |
+-----------------------------------------------------------------------+
                                   |
                                   v
+----------------------------------+------------------------------------+
| SCOPE RESOLVER                   | IMPORT / VALIDATION                |
| project | global | all (merge)   | UTF-8 | size | ext | symlink       |
+----------------------------------+------------------------------------+
                                   |
                                   v
+-----------------------------------------------------------------------+
|                             SQLITE STORE                              |
+-----------------------------------------------------------------------+
| Canonical | WAL | foreign keys | transactions | migrations            |
| meta | sources | pages | page_sources | links | ingest_jobs           |
| operations | search_fts                                               |
+-----------------------------------------------------------------------+
                                   |
                                   v
+-----------------------+-----------------------+-----------------------+
| SEARCH PIPELINE       | GRAPH ENGINE          | MARKDOWN PROJECTION   |
| CJK n-grams + Latin   | links + citations     | raw/ + wiki/          |
| contentless FTS5/BM25 | structural evidence   | index/log/overview    |
+-----------------------+-----------------------+-----------------------+
```

The persistent knowledge model has three logical layers:

| Layer | Contents | Contract |
| --- | --- | --- |
| Raw sources | Immutable snapshots of curated input | Add through `source`; never rewrite source truth. |
| Wiki | Agent-maintained pages, citations, links, and provenance | Update through `page`; cite sources and classify durable non-source knowledge. |
| Schema and purpose | Maintenance rules and project intent | Guide every future ingest and revision. |

SQLite is canonical. The Markdown tree is a rebuildable projection for people
and tools such as Obsidian. Agents mutate knowledge through `lwc`, not by editing
`.lwc/wiki.db` or projected Markdown directly. Successful commands return JSON
on stdout; failures return structured JSON on stderr.

Read commands keep current-format stores read-only. When an older writable
store is opened by a newer CLI, its schema is migrated transactionally once
before the read proceeds.

## Installation

Most users should use the Agent setup prompt above. The manual commands below
are for maintainers, debugging, or Agent environments that cannot install the
companion Skill.

Install from GitHub:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | sh
```

The installer supports x86_64/aarch64 macOS, glibc Linux, and Windows Git Bash,
verifies the release checksum, and installs or updates `lwc`.
It uses `~/.local/bin` by default, or updates an existing copy in
`~/.local/bin` or `~/.cargo/bin`. To choose another directory:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | LWC_INSTALL_DIR="$HOME/bin" sh
```

Alternatively, build and install from GitHub with Cargo:

```bash
cargo install --locked --git https://github.com/JanYork/llm-wiki-cli
```

Or install a local checkout:

```bash
git clone https://github.com/JanYork/llm-wiki-cli.git
cd llm-wiki-cli
cargo install --locked --path .
```

## Companion Agent Skill

The repository includes [`skills/using-lwc`](skills/using-lwc), an Agent Skill
that makes `lwc` a proactive memory layer for substantive sessions. Install it
in the current Agent runtime's user-level Skills directory. For Codex, from a
local checkout:

```bash
mkdir -p "${CODEX_HOME:-$HOME/.codex}/skills"
cp -R skills/using-lwc "${CODEX_HOME:-$HOME/.codex}/skills/"
```

The canonical invocation is `$using-lwc`. The setup prompt above also creates
the optional `$using-wiki` alias without duplicating the Skill implementation.

When triggered, the Skill:

- finds a compatible CLI or installs the official checksum-verified release;
- initializes global memory in `~/.lwc/` once;
- recalls bounded global and project context before repeated investigation;
- initializes the active project on explicit invocation, otherwise asks first;
- refuses project writes outside the current authorized workspace root;
- separates project facts from reusable global knowledge;
- integrates sources and writes durable answers back into the Wiki.

The Skill uses `LWC_PROJECT_ROOT` first as the canonical authorized workspace
boundary, then narrows it to the selected active project; discovery and
initialization cannot traverse above it.

Set `LWC_AUTO_INSTALL=0` to disable automatic CLI installation. Automatic
installation executes the reviewed installer bundled in the Skill, trusts this
repository and its GitHub Release publishing boundary, and verifies the
downloaded archive against `SHA256SUMS`; the checksum is integrity protection,
not publisher code signing. Release binaries cover x86_64/aarch64 macOS, glibc
Linux, and Windows through Git Bash. `SKILL.md` follows the Agent Skills
resource layout, while
`agents/openai.yaml` supplies OpenAI/Codex metadata. The CLI itself is
runtime-neutral: any Agent that can execute it and load or adapt the Skill's
instructions can use LWC. Skill commands, global instructions, and Hooks remain
runtime-specific, so the setup prompt detects and configures the current host.

## Quick Start

This section documents the CLI protocol that the Agent executes. Humans do not
need to run these commands during normal use.

### 1. Initialize a project Wiki

```bash
cd your-project
lwc init
printf '# Schema\nEvery page declares provenance; source-grounded claims cite sources.\n' | lwc schema set -
printf '# Purpose\nBuild a durable project Wiki.\n' | lwc purpose set -
```

Project initialization adds the project-relative `.lwc/` path to Git's local
`info/exclude` file when needed, without changing the repository `.gitignore`.
Use `lwc init --no-git-exclude` only when the Wiki is intentionally versioned.

### 2. Add source material

```bash
lwc source add-dir docs/
```

Files without an explicit title use their source origin as a stable,
human-readable fallback. Identical bytes are deduplicated by SHA-256.
Project sources that resolve outside the active Wiki root require
`--allow-external-source`. High-confidence credential markers are rejected
unless the reviewed source is explicitly acknowledged with
`--acknowledge-sensitive-source`.

For a curated atomic import, paths in a JSON manifest resolve from the
manifest's directory:

```json
{
  "sources": [
    {"path": "ARCHITECTURE.md", "title": "Architecture contract"},
    {"path": "src/store.rs", "title": "SQLite store"}
  ]
}
```

```bash
lwc source add-manifest lwc-sources.json
```

### 3. Analyze and integrate one source

```bash
lwc ingest next --context-limit 50 --source-max-chars 100000
lwc ingest analyze 1 --file analysis.md
```

Use `lwc ingest claim 7` when a manifest or scheduler already selected an exact
pending source ID.

If `source_window.has_more` is true, continue reading from
`source_window.next_offset_chars`:

```bash
lwc source show 1 --offset-chars 100000 --max-chars 100000
```

Create a cited source-summary page and integrate its contribution into at least
one non-source page before completing the ingest task:

```bash
lwc page put source-1 \
  --title "Source 1 Summary" \
  --kind source \
  --summary "What this source contributes" \
  --file source-summary.md \
  --source 1

lwc page put durable-concept \
  --title "Durable Concept" \
  --kind concept \
  --summary "How this source changes shared knowledge" \
  --file concept.md \
  --source 1

lwc ingest complete 1
```

Both layers are required: the source page is a navigation and provenance aid;
the non-source page makes knowledge compound. If a source genuinely changes no
shared page, complete it with a specific audited explanation:

```bash
lwc ingest complete 1 \
  --no-derived-pages-reason "Duplicate evidence; existing synthesis already covers every supported claim"
```

Source citations automatically expose `source-grounded` provenance. For
durable knowledge that comes from the user, an Agent observation, or an
explicit hypothesis, repeat `--provenance` as needed instead of inventing a
source:

```bash
lwc page put architecture-decision \
  --title "Architecture decision" \
  --kind query \
  --summary "Accepted constraint and remaining uncertainty" \
  --file decision.md \
  --provenance user-provided \
  --provenance hypothesis
```

`page put` replaces the complete citation and explicit-provenance sets. Read
the existing page first, then repeat every still-valid `--source` and
non-source `--provenance` value. Do not pass `source-grounded` explicitly; it is
derived from citations. Provenance is returned by page reads, context, search,
source references, and Markdown projection, but does not change search ranking.

### 4. Query the accumulated Wiki

```bash
lwc context --limit 50
lwc search "question keywords" --limit 20
lwc search "concept only" --type page --kind concept
lwc search "exact evidence" --type source
lwc page show source-1
```

## Agent Workflow

The intended workflow is:

1. Collect immutable sources.
2. Claim one ingest task with bounded `lwc ingest next`, or `ingest claim <ID>`
   when the source was selected explicitly.
3. Read every returned source window, plus the schema, purpose, and bounded context.
4. Analyze before generating pages.
5. Write or revise a source summary and shared durable pages with explicit `--source` citations.
6. Complete only after both integration gates pass, or record why no shared page should change.
7. Use `search`, `context`, `graph`, and `lint` to keep the Wiki coherent over time.

See [docs/agent-workflow.md](docs/agent-workflow.md) for the full operating contract.
Run `lwc --help` or `lwc <command> --help` for Agent-oriented preconditions,
state transitions, side effects, and next actions.

## Scopes

`lwc` supports three scopes:

| Scope | Store | Use |
| --- | --- | --- |
| `project` | Nearest ancestor `.lwc/wiki.db` | Default, project-specific knowledge |
| `global` | `~/.lwc/wiki.db` | Reusable cross-project knowledge |
| `all` | Project and global stores | Combined `search` and `context` only |

Examples:

```bash
lwc --scope global init
lwc --scope global source add shared.md
lwc --scope all search "shared term"
lwc --scope all context
```

Knowledge writes are explicit. `all` does not create implicit cross-store citations
or links; `search --record` only appends the query operation to each selected store.

## Search and CJK

Search is lexical and deterministic.

- Search terms are plain text, not raw FTS syntax.
- `--type auto` is the default: compiled pages rank first, paired raw sources
  are hidden, and raw sources provide fallback recall.
- Use `--type page`, `--type source`, or `--type all` to select a layer.
  Repeat `--kind` to restrict page results, such as
  `--kind concept --kind synthesis`.
- Multi-character CJK query terms use adjacent bigrams; the index also retains
  non-stopword unigrams so one-character queries remain searchable.
- Latin text is tokenized into lowercased alphanumeric terms.
- Ranking uses fixed title/summary/body weights so project and global results remain comparable under `--scope all`.

This is intentionally dictionary-free. The goal is stable behavior for product names, code names, mixed-language terms, and emerging vocabulary without depending on a word-segmentation dictionary.

## Maintenance and Projection

Useful maintenance commands:

```bash
lwc lint
lwc maintenance reindex
lwc maintenance materialize
lwc maintenance compact
lwc checkpoint create before-large-update
lwc checkpoint list
lwc log --limit 20
```

Notes:

- `lint` is read-only by default. Add `--record` only when the lint pass belongs
  in durable operation history.
- `maintenance reindex` rebuilds derived search artifacts from SQLite.
- `maintenance materialize` rebuilds the projected Markdown tree from SQLite.
- `maintenance compact` optimizes the contentless FTS5 index and attempts a
  WAL truncate checkpoint. Run it while the Wiki is idle and inspect `busy`
  plus `after_bytes`.
- Search queries are private by default; add `--record` only when you want the query wording stored in the durable operation log.

`lwc checkpoint create <NAME>` uses SQLite's online backup API. Restore with
`lwc checkpoint restore <NAME>`; LWC first creates a `pre-restore-*` safety
checkpoint and then rebuilds the projection. Use `source remove <ID>` and
`page remove <SLUG>` for guarded deletion: sources with citations and pages
with inbound links are refused.

For an external filesystem backup, stop active `lwc` commands and copy the
complete `.lwc/` directory. Do not copy only `wiki.db` while a writer may still
be using its WAL files.

## Benchmark Suite

The opt-in benchmark imports a local UTF-8 corpus into a temporary Wiki and
reports import time, search P50/P95, Recall@5/10, MRR, and storage before/after
compaction. Ground truth is a JSONL file of queries and expected
corpus-relative paths:

```bash
cargo build --release
LWC_BENCH_CORPUS=/path/to/sanitized-corpus \
LWC_BENCH_QUERY_SET=/path/to/query-set.jsonl \
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
cargo test --test search_benchmark -- --ignored --nocapture
```

Normal `cargo test --all-targets` covers page-first search, type/kind filters,
UTF-8 source windows, ingest completion gates, graph precision, migrations,
lint, and WAL compaction. See [benchmarks/README.md](benchmarks/README.md) for
the workload contract and fair before/after comparison rules.

## Limits and Non-Goals

Current design constraints:

- single-machine, single-user knowledge base;
- UTF-8 text workflow;
- bounded input size of 64 MiB per schema, purpose, source, or page body;
- lexical search, not semantic vector retrieval.

Deliberate non-goals for this CLI:

- no built-in LLM calls;
- no vector database;
- no daemon or background service;
- no web UI or desktop UI;
- no direct database editing contract.

If the projected Markdown drifts, rebuild it. If the SQLite schema is wrong, fix it through the CLI and migrations, not by hand.

## Contributing

Issues and pull requests are welcome, especially around:

- agent workflow ergonomics;
- deterministic projection behavior;
- durable citation and page maintenance contracts;
- search quality for multilingual technical corpora.

Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.
Report security issues according to [SECURITY.md](SECURITY.md).

## License

Licensed under the [Apache License 2.0](LICENSE).
