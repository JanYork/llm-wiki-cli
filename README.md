<h1 align="center">LWC — Proactive Memory for AI Agents</h1>

<p align="center">
  <strong>Agent-driven · Persistent · Source-grounded</strong>
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@i-xor/lwc"><img alt="npm: @i-xor/lwc" src="https://img.shields.io/badge/npm-%40i--xor%2Flwc-CB3837?logo=npm"></a>
  <a href="https://crates.io/crates/lwc"><img alt="crates.io: lwc" src="https://img.shields.io/crates/v/lwc.svg"></a>
  <img alt="Node.js 22 or newer" src="https://img.shields.io/badge/node-%3E%3D22-5FA04E?logo=nodedotjs">
  <img alt="Platform: macOS, Linux, Windows" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-666666">
  <a href="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://skills.sh/janyork/llm-wiki-cli/using-lwc"><img alt="skills.sh: using-lwc" src="https://img.shields.io/badge/skills.sh-using--lwc-000000?logo=vercel"></a>
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> ·
  <a href="docs/readme/README.ja.md">日本語</a> · <a href="docs/readme/README.es.md">Español</a> ·
  <a href="docs/readme/README.pt-BR.md">Português (Brasil)</a> · <a href="docs/readme/README.fr.md">Français</a> ·
  <a href="docs/readme/README.ru.md">Русский</a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-social-preview.png" alt="LWC — Proactive Memory for AI Agents" width="100%">
</p>

`lwc` is an agent-driven proactive memory CLI for AI agents. It lets Agents
autonomously recall, maintain, and evolve persistent, source-grounded knowledge
across sessions.

**Works with Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes,
Antigravity, GitHub Copilot in VS Code, Copilot CLI, Copilot for JetBrains, and
pi.**

LWC turns curated documents into a durable Wiki. Agents reason and synthesize;
`lwc` preserves sources, pages, citations, links, indexes, and history so
knowledge compounds instead of being rediscovered from raw chunks on every
query.

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-overview-en.png" alt="LWC product overview" width="820">
</p>

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

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-source-grounding-en.png" alt="LWC source grounding and traceability" width="820">
</p>

### The Agent operates LWC

`lwc` is a machine interface for Agents, not a human-facing note-taking app. In
normal use, a human selects sources, states goals, asks questions, and reviews
answers or the projected Markdown. The Agent runs the CLI, manages scope,
integrates sources, maintains citations and links, and decides what is worth
recalling or writing back.

Do not manually drive the routine `lwc` workflow unless you are developing or
debugging the tool. Ask your Agent to activate the bundled canonical
`using-lwc` Skill instead—usually as `$using-lwc`.

## Recommended: Ask Your Agent to Set Up LWC

Paste this prompt into the Agent you use. It installs the global CLI, delegates
all supported host configuration to LWC's idempotent AgentTarget installer, and
uses native self-configuration only for an unregistered Agent.

<details>
<summary><strong>Copy the complete setup prompt</strong></summary>

```text
Configure LWC completely for this user. Perform and verify the work; do not
merely describe commands for me to run.

Source of truth:
- https://github.com/JanYork/llm-wiki-cli
- https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc

Requirements:
1. Read this README, `SECURITY.md`, and `skills/using-lwc/SKILL.md`. Install the
   official checksum-verified release if `lwc` is not globally callable; never
   prefix routine commands with a private binary path or `LWC_PROJECT_ROOT`.
2. Run `lwc --version`, initialize global memory once with
   `lwc --scope global init` when missing, then run `lwc agent install --yes`.
   This command detects installed supported Agents and safely installs their
   MCP, Skill, Hook and Instructions using official locations. Do not recreate
   that logic manually or install a native package for the same Agent as well.
3. Inspect `lwc agent status --target all --location global`. Restart affected
   Agents and complete their normal Hook trust review where required. Do not
   initialize a project Wiki or either graph without explicit project consent.
4. If the current runtime is not one of LWC's registered AgentTargets, use its
   official user-level conventions to install the canonical `using-lwc` Skill,
   an additive instruction block, `lwc serve --mcp`, and a bounded session Hook
   only where those surfaces are officially supported. Preserve existing
   configuration, remain idempotent, and report unsupported surfaces instead of
   inventing paths or keys.

Finish with the LWC version, detected and configured Targets, status results,
files changed, unsupported surfaces, and any restart or trust action remaining.
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

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-architecture-en.png" alt="LWC architecture" width="100%">
</p>

LWC keeps four distinct layers so durable knowledge stays traceable:

| Layer | Purpose |
| --- | --- |
| Raw sources | Immutable snapshots of curated evidence |
| Wiki | Agent-maintained pages, citations, links, and provenance |
| Temporal memory | Compact records of changes, decisions, outcomes, and unresolved work |
| Schema and purpose | Project-specific rules that guide future maintenance |

SQLite is canonical. Markdown, full-text indexes, and optional graph stores are
rebuildable projections. Agents update knowledge through the CLI; successful
operations return structured JSON that can be audited and resumed.

[Read the architecture overview →](https://github.com/JanYork/llm-wiki-cli/wiki/Architecture-Overview)

## Hierarchical Recall and Knowledge Graph

LWC indexes Sources and Wiki pages at document, passage, and sentence levels.
Agents can start with a small answer-shaped context, expand the exact span only
when necessary, and detect stale locators after content changes.

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="LWC memory graph" width="100%">
</p>

The optional document graph connects pages, sources, citations, links, and
explicit semantic relations. SQLite remains authoritative, while Grafeo or
SurrealDB provides a rebuildable traversal layer. Explicit relations keep their
reason, provenance, confidence, and source evidence.

### Document Conversion and Office Reading

Optional Anydoc or MarkItDown adapters convert supported local files into
reviewable Markdown before ingestion. OfficeCLI provides a separate,
consent-based, read-only path for Word, Excel, and PowerPoint files. Neither
capability is silently installed or enabled, and source Office files are never
modified.

Passage search also indexes the enclosing H2-H6 heading path as an independent
field. Heading context can improve matching, but it is never prepended to the
returned snippet or included in its byte range, so citations and span locators
still point to exact canonical body text. Store migration 17 rebuilds only this
derived span/search index.

[Explore retrieval and indexing →](https://github.com/JanYork/llm-wiki-cli/wiki/Retrieval-and-Indexing) ·
[Document graph →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Knowledge-Graph) ·
[Document conversion →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Conversion)

## Optional Learning Suite

Tutor, Book, and Practice are independent first-party capabilities, disabled by
default and backed by separate private stores:

- **Tutor** keeps teaching turns, learner evidence, goals, plans, and a private
  Soul/Wiki.
- **Book** imports supported books in verified source order for complete,
  grounded reading.
- **Practice** keeps versioned question banks, papers, attempts, grading,
  flashcards, and FSRS review state.

Each runtime is downloaded lazily, pinned to the LWC version, and verified by
checksum. Disabling a capability preserves its canonical data. Agent Skills
handle recovery and persistence without exposing routine bookkeeping to the
learner.

[Read the Learning Suite contracts →](docs/learning-suite-contracts.md)

## Installation

Most users need only one package command:

    npm install --global @i-xor/lwc

Homebrew, crates.io, checksum-verified GitHub releases, and local Cargo builds
are also supported.

[Installation and upgrade guide →](https://github.com/JanYork/llm-wiki-cli/wiki/Installation-and-Upgrades)

## Companion Agent Skill

The bundled [using-lwc Skill](skills/using-lwc) turns LWC into a proactive
memory layer. It recalls bounded context, keeps project and global knowledge
separate, integrates sources, preserves citations, and writes back only verified
knowledge worth reusing.

Install it from [skills.sh](https://skills.sh/JanYork/llm-wiki-cli):

    npx skills add JanYork/llm-wiki-cli --skill using-lwc -g

The canonical trigger is <code>$using-lwc</code>. The Skill is runtime-neutral
and includes focused guidance for memory, document graphs, Word Graph,
CodeGraph, strong tags, conversion, onboarding, recovery, and maintenance.

### Native Agent setup

LWC detects supported Agents and installs their available MCP, Skill, Hook, and
Instructions surfaces through idempotent AgentTarget adapters:

    lwc agent install --yes

The unified read-only MCP exposes bounded Wiki memory and optional code context
without widening the active workspace. The 12 registered targets are Claude
Code, Cursor, Codex, OpenCode, Hermes, Gemini CLI, Antigravity, Kiro, GitHub
Copilot in VS Code, Copilot CLI, Copilot for JetBrains, and pi.

`agent status` reports verified `hook_capabilities` separately from the native
events actually written as `installed_hook_events` for that scope. Narrow shell
consent is enforced as an ask only for Claude Code, Cursor, global Hermes, and
Antigravity; Codex receives advisory additional context and cannot enforce the
ask. Unsupported consent events are not installed and return an exact no-op.
Only Claude Code and Codex may continue an actionable active Plan from `Stop`,
once per native loop guard; other Stop surfaces are not simulated. Refresh and
uninstall remove only LWC-owned hook entries, including entries inside shared
groups, while preserving sibling and user configuration.

Graph capabilities remain consent-aware: document relationships require the
physical graph, code-structure tasks require CodeGraph, and neither is enabled
merely because its runtime exists. Office reading follows the same explicit
consent boundary.

[AgentTarget integration →](https://github.com/JanYork/llm-wiki-cli/wiki/AgentTarget-Installation-and-Integration)

## Quick Start

Humans normally describe the goal and review the result; the Agent operates the
CLI. The complete walkthrough lives in the
[Quick Start Wiki page](https://github.com/JanYork/llm-wiki-cli/wiki/Quick-Start).

### 1. Initialize a project Wiki

The Agent creates a project-local Wiki and defines its purpose and maintenance
rules. Project state is excluded locally from Git unless versioning it was an
explicit choice.

### 2. Add source material

Curated files become immutable, deduplicated snapshots. LWC tracks their live
paths and can report whether the current file is unchanged, modified, missing,
or superseded.

### 3. Analyze and integrate one source

The Agent reads the complete bounded source, writes a cited source summary,
updates shared knowledge, and completes the ingest only after both layers are
consistent.

### 4. Query the accumulated Wiki

Search is page-first and source-grounded. Agents retrieve maintained answers
first, then open exact source evidence when a claim needs verification.

## Agent Workflow

The normal loop is short:

1. Recall relevant maintained knowledge.
2. Inspect current sources or code when freshness matters.
3. Make the smallest verified update.
4. Validate retrieval, links, and applicable graph projections.

Broad revisions use an atomic changeset. See
[the full Agent workflow](docs/agent-workflow.md) for trust boundaries,
preconditions, recovery, and completion evidence.

## Temporal Memory

Temporal memory records compact events about what changed, why a decision was
made, what was tried, the outcome, and what remains unresolved. It complements
the Wiki: temporal recall explains history; the Wiki represents current stable
knowledge.

Retention is bounded and protects pinned, unresolved, and open contradiction
records. Events are normalized rather than stored as raw chat transcripts, and
similar events are never silently merged.

[Persistent memory guide →](https://github.com/JanYork/llm-wiki-cli/wiki/Persistent-Memory)

## Multi-machine Sync

Sync reconciles project memory, global memory, or both over SSH while keeping
semantic Wiki state separate from Git publication. Merge preserves unique
objects from both sides; conflicts are returned as bounded packets for explicit
resolution.

Sessions are durable and resumable. LWC never copies live SQLite database,
WAL, or SHM files, never resets the working tree, and keeps canonical
publication separate from rebuildable search and graph projections.

[Sync workflow and safety contract →](docs/agent-workflow.md)

## Atomic Multi-command Changes

Changesets keep a multi-step knowledge update invisible until it has been
reviewed and validated. Commit publishes only touched canonical entities in one
transaction; unrelated live work survives, and same-entity revision conflicts
fail closed.

A successful commit records an exact inverse patch for supported operations,
enabling guarded rollback without replacing the whole Wiki.

Draft reads see staged writes, while live SQLite and Markdown stay unchanged.
The draft database starts as a small sparse overlay; it does not copy or
checkpoint the live Wiki. `changeset show` reports staged operations, revisions,
and readiness without running lint. Commit validates and applies only
touched entities, so unrelated live writes survive; a same-entity revision conflict
fails without overwriting either side. Commit rejects empty drafts and blocking
lint errors; warnings and information remain review guidance and do not block
publication. There is no force or automatic merge. Use
`--allow-lint-issues --reason "reviewed pre-existing debt"` only for audited
debt that the changeset did not introduce. After commit, rerun the same fixed
retrieval checks against live state. Commit freezes the reviewed draft before
publication; `changeset_frozen` blocks any later staged write. Retry the same
commit for recovery, or discard after a reported conflict—never add more work
to a frozen draft.

```bash
lwc --scope project changeset discard architecture-refresh
lwc --scope project changeset rollback <CHANGESET_ID>
```

Discard touches only an uncommitted draft. Commit writes a checksummed inverse
patch containing only touched entities and returns the exact rollback ID;
rollback restores only those entities and refuses if one changed again. Project
and global changesets are separate, `--scope all` is invalid, and `init`,
`maintenance`, `checkpoint`, and nested changeset commands reject
`--changeset`. Drafts never create a second Markdown projection. If a structured
error reports `committed=true` with cleanup or materialization work remaining,
do not repeat the knowledge changes; run the returned recovery action.

Sparse commit currently has exact patches for Source add/ingest, Page
put/remove, schema, purpose, and recorded search operations. Retrieval-weight
and explicit semantic-relation mutations fail before checkpointing or taking a
live write lock with `changeset_sparse_unsupported`; apply those as direct
single-entity transactions until their sparse inverse patches are available.

[Changesets guide →](https://github.com/JanYork/llm-wiki-cli/wiki/Changesets)

## Scopes

| Scope | Use |
| --- | --- |
| project | Knowledge owned by the nearest project Wiki |
| global | Reusable knowledge shared across projects |
| all | Combined read-only recall and coordinated Sync |

Writes always target one explicit store. LWC never creates implicit
cross-project citations or links.

[Scopes and project discovery →](https://github.com/JanYork/llm-wiki-cli/wiki/Scopes-and-Project-Discovery)

## Search and CJK

Search is lexical, deterministic, and page-first. It keeps title, path, summary,
body, provenance, and graph evidence distinct; supports page/source/kind
filters; and can explain the exact score arithmetic.

CJK text uses adjacent bigrams plus useful unigrams, while Latin text uses
lowercased alphanumeric terms. This dictionary-free design remains stable for
product names, code symbols, mixed-language text, and emerging vocabulary.

### Explicit retrieval weights and feedback

Auditable document weights capture durable importance. Query-specific feedback
reranks only matching candidates and stores a fingerprint instead of the raw
query. Neither mechanism can make unrelated content appear.

[Search and context guide →](https://github.com/JanYork/llm-wiki-cli/wiki/Search-and-Context)

## Read-only Viewer and CodeGraph

`lwc view` starts a foreground, loopback-only project inspector and opens the
browser. It serves one embedded TS + Lit application—no CDN and no Node runtime
at use time—and exposes GET/HEAD APIs only. Pages, sources, Markdown, the
knowledge graph, and the optional code graph are read from the current project
without migration, refresh, or graph construction:

```bash
lwc view
lwc view --port 4173 --no-open
```

Page detail uses the canonical title, summary, kind, provenance, citations, and
timestamps. The viewer suppresses only a leading body H1 that matches the
canonical title and derives a local table of contents from three or more H2-H4
headings. These presentation rules never rewrite canonical content or projected
Markdown.

The viewer starts in English. Use the `中文` / `EN` control to switch languages;
the browser remembers the selection while Wiki content remains in its authored
language. Graphs use a single Obsidian-inspired 3D relationship view with small
nodes, persistent labels, thin links, rotation, and zoom.

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="LWC CodeGraph code intelligence" width="100%">
</p>

CodeGraph is project-only and explicitly initialized. It answers questions
about symbols, callers, callees, dependencies, files, and impact while keeping
telemetry disabled and graph writes atomic per owner file.

The pinned runtime recognizes TypeScript, TSX, JavaScript, JSX, ArkTS, Python,
Go, Rust, Java, C, C++, C#, Razor, PHP, Ruby, Swift, Kotlin, Dart, Svelte, Vue,
Astro, Liquid, Pascal, Scala, Lua, Luau, Objective-C, R, Solidity, Nix, YAML,
Twig, XML, .properties, CFML, CFScript, CFQuery, COBOL, VB.NET, Erlang, and
Terraform.

[Viewer guide →](https://github.com/JanYork/llm-wiki-cli/wiki/Read-Only-Viewer) ·
[CodeGraph guide →](https://github.com/JanYork/llm-wiki-cli/wiki/Code-Graph)

## Maintenance and Projection

Lint, reindexing, Markdown materialization, compaction, checkpoints, and graph
projection are explicit operations. Long-running work is durable, observable,
resumable, and applied in bounded document units.

SQLite remains canonical throughout. Search indexes, Markdown, and graph stores
can be rebuilt without rewriting source history or current Wiki knowledge.

`lwc lint` keeps `total` and `counts` as all-issue compatibility fields and
adds `blocking_total` for errors plus `advisory_total` for warnings and
information. Deterministic Markdown guidance currently reports conflicting or
duplicate body H1s, the first heading-level jump, and a long unsectioned prose
region; only integrity errors block changeset commit.

Notes:

- Maintenance commands return a durable `work` immediately. Read progress with
  `work status`, or use `work watch` and inspect `work.result` after success.
  Schema v10 to v11 migration uses the same mechanism automatically, so normal
  commands never perform that migration inline.
- `lint` is read-only by default. Add `--record` only when the lint pass belongs
  in durable operation history.
- `maintenance reindex` rebuilds derived search artifacts from SQLite.
- `maintenance materialize` rebuilds the projected Markdown tree from SQLite.
- `maintenance compact` only attempts a WAL truncate checkpoint; it does not
  hide a full FTS optimization. Run it while the Wiki is idle and inspect
  `busy` plus `after_bytes`. A busy reader returns promptly without changing
  canonical content.
- Search queries are private by default; add `--record` only when you want the query wording stored in the durable operation log.

`lwc checkpoint create <NAME>` uses SQLite's online backup API. Restore with
`lwc checkpoint restore <NAME>`; LWC first creates a `pre-restore-*` safety
checkpoint and then rebuilds the projection. Use `source remove <ID>` and
`page remove <SLUG>` for guarded deletion: sources with citations and pages
with inbound links are refused. Removing the current source for a tracked path
stops tracking that path instead of silently exposing an older revision as
current.

For a multi-source ingest or broad page replacement, prefer a changeset over a
manual checkpoint: successful commit writes a sparse inverse patch, publishes
only touched canonical entities in one transaction, and incrementally
materializes changed Markdown. Commit attempts a WAL truncate after publication;
`wal_checkpointed=false` means an active reader prevented it and does not mean
the canonical commit failed.

For an external filesystem backup, stop active `lwc` commands and copy the
complete `.lwc/` directory. Do not copy only `wiki.db` while a writer may still
be using its WAL files.

[Maintenance and diagnostics →](https://github.com/JanYork/llm-wiki-cli/wiki/Maintenance-and-Diagnostics)

## Benchmark Suite

The opt-in benchmark measures import time, search latency, Recall@5/10, MRR, and
storage on a caller-supplied sanitized corpus. Fair comparisons fix the machine,
corpus, query set, and run conditions, then compare repeated-run medians.

[Benchmark methodology →](benchmarks/README.md)

## Durable Todo and current Plan

Todo stores deferred work; Plan stores the currently executing objective,
ordered steps, progress, and revision. They are independent, opt-in
capabilities and never convert into each other automatically.

Bounded lifecycle context isolates Plan and Todo progress by Agent session and,
where the host exposes it, by subagent. Each Agent sees only explicitly tracked
work for its opaque context; detailed commands and host capability limits live
in the workflow guide.

[Todo and Plan workflow →](docs/agent-workflow.md#todo-and-plan)

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
