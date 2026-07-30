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
| graph | lint | maintenance | log                                      |
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
+---------------------+---------------------+---------------------------+
| SEARCH PIPELINE     | GRAPH ENGINE        | MARKDOWN PROJECTION       |
| custom tokenizer    | links / citations   | raw/ | wiki/ | index.md   |
| FTS5 + BM25         | neighbors / types   | log.md | overview.md      |
+---------------------+---------------------+---------------------------+
```

The persistent knowledge model has three logical layers:

| Layer | Contents | Contract |
| --- | --- | --- |
| Raw sources | Immutable snapshots of curated input | Add through `source`; never rewrite source truth. |
| Wiki | Agent-maintained pages, citations, and links | Update through `page`; keep claims source-grounded. |
| Schema and purpose | Maintenance rules and project intent | Guide every future ingest and revision. |

SQLite is canonical. The Markdown tree is a rebuildable projection for people
and tools such as Obsidian. Agents mutate knowledge through `lwc`, not by editing
`.lwc/wiki.db` or projected Markdown directly. Successful commands return JSON
on stdout; failures return structured JSON on stderr.

## Why This Is Not Just RAG

Traditional RAG retrieves raw chunks and reconstructs an answer for every query.
`lwc` instead maintains a persistent Wiki: sources remain immutable, useful
synthesis becomes durable pages, contradictions and links survive across
sessions, and better answers can be written back into the knowledge base.

The result is accumulated knowledge, not only retrieval.

## Installation

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
that makes `lwc` a proactive memory layer for substantive sessions. From a
local checkout, install it for Codex with:

```bash
mkdir -p "${CODEX_HOME:-$HOME/.codex}/skills"
cp -R skills/using-lwc "${CODEX_HOME:-$HOME/.codex}/skills/"
```

When triggered, the Skill:

- finds a compatible CLI or installs the official checksum-verified release;
- initializes global memory in `~/.lwc/` once;
- recalls bounded global and project context before repeated investigation;
- asks before creating project-local `.lwc/` state;
- separates project facts from reusable global knowledge;
- integrates sources and writes durable answers back into the Wiki.

Set `LWC_AUTO_INSTALL=0` to disable automatic CLI installation. Automatic
installation executes the reviewed installer bundled in the Skill, trusts this
repository and its GitHub Release publishing boundary, and verifies the
downloaded archive against `SHA256SUMS`; the checksum is integrity protection,
not publisher code signing. Release binaries cover x86_64/aarch64 macOS, glibc
Linux, and Windows through Git Bash. `SKILL.md` follows the Agent Skills
resource layout, while
`agents/openai.yaml` supplies OpenAI/Codex metadata. Other Agent harnesses may
load the Skill when they support that layout, but universal compatibility is
not assumed.

## Quick Start

### 1. Initialize a project Wiki

```bash
cd your-project
lwc init
printf '# Schema\nEvery factual page cites sources.\n' | lwc schema set -
printf '# Purpose\nBuild a durable project Wiki.\n' | lwc purpose set -
```

### 2. Add source material

```bash
lwc source add-dir docs/
```

### 3. Analyze and integrate one source

```bash
lwc ingest next --context-limit 50
lwc ingest analyze 1 --file analysis.md
```

Create at least one cited source-summary page before completing the ingest task:

```bash
lwc page put source-1 \
  --title "Source 1 Summary" \
  --kind source \
  --summary "What this source contributes" \
  --file source-summary.md \
  --source 1

lwc ingest complete 1
```

This `kind=source` step is required. A source is not considered integrated just because it was imported or because only concept/entity pages were updated.

### 4. Query the accumulated Wiki

```bash
lwc context --limit 50
lwc search "question keywords" --limit 20
lwc page show source-1
```

## Agent Workflow

The intended workflow is:

1. Collect immutable sources.
2. Claim one ingest task with `lwc ingest next`.
3. Read the returned schema, purpose, source snapshot, and bounded context.
4. Analyze before generating pages.
5. Write or revise durable pages with explicit `--source` citations.
6. Complete only after a cited `kind=source` summary exists.
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
lwc log --limit 20
```

Notes:

- `lint` reports deterministic structural problems and records the lint pass.
- `maintenance reindex` rebuilds derived search artifacts from SQLite.
- `maintenance materialize` rebuilds the projected Markdown tree from SQLite.
- Search queries are private by default; add `--record` only when you want the query wording stored in the durable operation log.

For backups, stop active `lwc` commands and copy the complete `.lwc/` directory.
Do not copy only `wiki.db` while a writer may still be using its WAL files.

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
