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
Antigravity, and pi.**

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

The persistent knowledge model has four logical layers:

| Layer | Contents | Contract |
| --- | --- | --- |
| Raw sources | Immutable snapshots of curated input | Add through `source`; never rewrite source truth. |
| Wiki | Agent-maintained pages, citations, links, and provenance | Update through `page`; cite sources and classify durable non-source knowledge. |
| Temporal memory | Small normalized events about change, decisions, outcomes, and unresolved work | Record through `remember`; relate revisions explicitly and let configured retention evict ordinary history. |
| Schema and purpose | Maintenance rules and project intent | Guide every future ingest and revision. |

SQLite is canonical. The Markdown tree is a rebuildable projection for people
and tools such as Obsidian. Agents mutate knowledge through `lwc`, not by editing
`.lwc/wiki.db` or projected Markdown directly. Successful commands return JSON
on stdout; failures return structured JSON on stderr.

Read commands keep current-format stores read-only. When an older writable
store is opened by a newer CLI, its schema is migrated transactionally once
before the read proceeds.

## Hierarchical Recall and Knowledge Graph

Every current Source and Wiki page is deterministically indexed as passages and
sentences. SQLite remains authoritative; span FTS and an optional external
document graph are rebuilt indexes. Existing search stays document-only
unless a granularity is requested:

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="LWC memory graph" width="100%">
</p>

```bash
lwc search "projection consistency" --granularity sentence --type page
lwc search "projection consistency" --granularity passage
lwc search "projection consistency" --granularity all --group-by document
lwc span get <SPAN_ID>
lwc span expand <SPAN_ID> --before 1 --after 1 --children 20
```

Span locators contain the document fingerprint and segmentation version. A
locator from a replaced body fails with `stale_span` and reports prior/current
metadata; LWC never silently remaps it to similar text.

Use the bounded, typed graph API for exploration without requiring keywords:

```bash
lwc graph explore                         # representative macro view
lwc graph node page:projection-policy
lwc graph neighbors page:projection-policy --direction outgoing
lwc graph path page:implementation page:policy --max-depth 6
lwc graph impact page:policy --max-depth 4
lwc graph overview
lwc graph status
lwc graph verify
```

Automatic edges are limited to structural/evidential facts. Semantic claims
must be explicit and auditable:

```bash
lwc graph relation set page:implementation DEPENDS_ON page:policy \
  --provenance source-grounded --source 12 \
  --reason "Source 12 states the required policy" --confidence 0.95
lwc graph relation list --from page:implementation
lwc graph relation retract page:implementation DEPENDS_ON page:policy \
  --reason "The dependency was superseded"
```

Relation reasons are durable content: never put credentials, secrets, or raw
chain-of-thought in them.

SQLite documents remain authoritative. Graph storage is disabled by default;
enable exactly one external engine when traversal is needed. Configuration is
layered from built-in defaults through global and project files:

```bash
lwc config show
lwc config set --graph grafeo
lwc config set --graph surrealdb
lwc config set --graph disabled
lwc config unset --graph
```

Markdown conversion is a separate opt-in operation. `lwc init` reports the
same machine-readable setup guidance, but never installs or enables a
converter. Install one adapter, select it explicitly, convert to a new local
Markdown file, review it, and only then ingest it:

```bash
# Choose one adapter; both are disabled unless configured.
npm install --global @firecrawl/anydoc
lwc config set --trans anydoc

# Or:
python3 -m pip install 'markitdown[all]'
lwc config set --trans markitdown

lwc trans INPUT --output OUTPUT.md
lwc source add OUTPUT.md
```

Configuration accepts `--trans-timeout 1..900` and repeated
`--trans-arg=<value>` options for the selected adapter. LWC invokes the fixed
adapter executable directly, never falls back to the other adapter, accepts
local files only, caps input and output at 64 MiB, and never overwrites an
existing output. Keep credentials in the adapter's environment rather than in
LWC configuration. See the official [Anydoc](https://github.com/firecrawl/anydoc)
and [MarkItDown](https://github.com/microsoft/markitdown) documentation for
supported formats and optional flags.

OfficeCLI reading is a separate global, opt-in capability. When an Office read
is needed, enable it once; the first command downloads the pinned binary into
the versioned global LWC runtime cache and verifies its SHA-256:

```bash
lwc --scope global config set --office officecli
lwc office view report.docx text
lwc office get workbook.xlsx /Sheet1/A1 --json
lwc office query slides.pptx 'shape[fill=FF0000]'
lwc --scope global config set --office disabled
```

`lwc office` passes through OfficeCLI's `view`, `get`, `query`, `validate`,
`dump`, `raw`, and `help` commands, their arguments, output, and exit status.
All modifying, installer, plugin, resident, and server commands are rejected.
LWC disables OfficeCLI auto-update and resident mode, never falls back to a
binary from `PATH`, and does not delete the cached runtime when disabled. Read
commands may still create an explicitly requested derived output (`--out` or
`--save`) or open a browser; they do not modify the source Office document.
See [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) and
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

## Optional Learning Suite

Tutor, Book, and Practice are three fixed first-party plugins. Each is globally and
independently disabled by default:

```bash
lwc --scope global config set --tutor enabled
lwc --scope global config set --book enabled
lwc --scope global config set --practice enabled
```

The first runtime-backed `lwc tutor`, `lwc book`, or `lwc practice` operation lazily
downloads the same-version, checksum-verified binary for the current target into
`~/.lwc/runtime/<plugin>/<version>/<target>/`. LWC does not discover plugins on
`PATH` and exposes no generic plugin ABI. Canonical stores remain separate under
`~/.lwc/plugins/{tutor,book,practice}/`; disabling or replacing a runtime preserves
them. Sync inventories each store independently and can preserve validated canonical
data when a destination lacks that runtime.

- **Tutor** keeps durable teaching turns, learner evidence, goals, plans, and its
  private Soul/Wiki.
- **Book** accepts EPUB, TXT, Markdown, and text PDFs. HTML, scanned/OCR PDFs, MOBI,
  and AZW3 are unsupported; convert them outside Book first.
- **Practice** keeps versioned banks/items, immutable papers, attempts, immediate
  responses, grading, and FSRS review state.

The matching `using-tutor`, `using-book`, and `using-practice` Skills recover pending
work and carry exact IDs across plugins. Explicit learning intent starts the enabled
workflow; ambiguous intent causes one direct question. Ordinary factual Q&A stays
outside Tutor and is not recorded. If a required plugin is disabled, the Agent asks
once before enabling it unless the user explicitly requested enablement.

Plugin roots are private user data. They are not projected into the ordinary LWC Wiki,
and v1 provides no forget/clear/purge command. Disable, runtime replacement, archive,
correction, and Sync preserve canonical history; any future destructive purge requires
a separate, explicit design and authorization. See
[the Learning Suite contracts](docs/learning-suite-contracts.md) and
[the Agent workflow](docs/agent-workflow.md#optional-learning-suite).

Grafeo and embedded SurrealDB use disposable sidecars under `.lwc/`. Each
`graph-project` Work commits one current Source/Page and its owned links,
citations, and explicit relations before starting the next document. Updates
and deletions enqueue only touched documents; rebuild and resume use the same
document units. Historical source revisions remain immutable and are never
re-tokenized or projected. Use `work list`, `work status`, or `work watch` to
observe progress and `work resume` after interruption. `graph status` reports
the selected engine and projected document count; `graph verify` compares its
current document keys with SQLite.

## Installation

Most users should use the Agent setup prompt above. The manual commands below
are for maintainers, debugging, or Agent environments that cannot install the
companion Skill.

Install with Homebrew (prebuilt bottles are available for Apple silicon macOS
and x86_64 Linux):

```bash
brew install JanYork/tap/lwc
```

Install with npm (Node.js 22+):

```bash
npm install --global @i-xor/lwc
```

Install from crates.io:

```bash
cargo install --locked lwc
```

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
from [skills.sh](https://skills.sh/JanYork/llm-wiki-cli):

```bash
npx skills add JanYork/llm-wiki-cli --skill using-lwc -g
```

Or copy it from a local checkout into the current Agent runtime's user-level
Skills directory. For Codex:

```bash
mkdir -p "$HOME/.agents/skills"
cp -R skills/using-lwc "$HOME/.agents/skills/"
```

The canonical invocation is `$using-lwc`.

When triggered, the Skill:

- finds a compatible CLI or installs the official checksum-verified release;
- initializes global memory in `~/.lwc/` once;
- recalls bounded global and project context before repeated investigation;
- initializes the active project on explicit invocation, otherwise asks first;
- refuses project writes outside the current authorized workspace root;
- separates project facts from reusable global knowledge;
- integrates sources and writes durable answers back into the Wiki.

`SKILL.md` is a short router rather than a monolithic manual. It links one
focused teaching document for basic memory, trigger timing, active memory,
physical document graph, bounded Word Graph, CodeGraph, strong tags, document
conversion, Agent onboarding, and recovery/maintenance. Each document states
when to use and skip the capability, its minimum workflow, consent boundary, and
completion evidence.

The Skill normally discovers the active project from the current directory and
invokes the globally installed `lwc` command directly. `LWC_PROJECT_ROOT` is an
explicit boundary for a deliberately targeted project, not a prefix to export
for routine commands in the project you are already working in.

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

### Native Agent setup

LWC can detect supported Agents and install one unified read-only LWC MCP.
All 12 registered AgentTargets are strong adapters: each installs every
official file-based MCP, Skill, Hook, and Instructions surface available for
that host and scope, while UI-owned, preview, or unsupported surfaces are
reported explicitly.

```bash
lwc agent install --yes
lwc agent status --target all --location global
lwc agent install --print-config codex
lwc agent refresh --target codex,claude
lwc agent uninstall --target codex,claude --yes
```

`--yes` selects detected Agents, global scope, and each target's default
lifecycle/prompt Hooks. Use `--no-prompt-hook` to omit Claude's per-prompt Hook. The installed
entry is `lwc -> serve --mcp`; its single `lwc_explore`
tool defaults to bounded Wiki memory and accepts explicit `code`/`all` modes.
The requested `projectPath` must stay inside the workspace where the MCP host
started LWC. It never downloads or initializes CodeGraph. Repeated install and refresh are
byte-idempotent; uninstall restores only owned state and leaves project indexes
intact. Optional Codex, Claude Code, and Pi packages live under `integrations/`;
installing a package does not grant or bypass native trust. Do not combine the
direct installer and native package for the same Agent. Each native package
bundles the complete `using-lwc` Skill, so installation does not depend on a
third-party Skill manager or any maintainer-specific environment.

Pi exposes LWC MCP through its official extension bridge because Pi has no
built-in MCP. Other Targets register only `lwc serve --mcp`; CodeGraph stays an
internal LWC code-context plane and is never registered as a second Agent MCP.
Officially UI-owned trust and permission settings remain user-managed. Preview
surfaces are labeled as such, and partial project scopes install the supported
surfaces instead of weakening or rejecting the whole Target. Kiro global paths
honor `KIRO_HOME`.

The target interface, registry order, detection rules, and MCP paths follow
CodeGraph's MIT-licensed installer adapter design; LWC adds the unified LWC MCP,
per-surface capability reporting, Skills and Hooks, shared-file ownership, and
exact rollback.
See [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

Fresh project `lwc init` output and session/compaction Hooks expose bounded
`LWC_READINESS` facts for the Wiki, physical document graph, CodeGraph runtime
and project index, optional Office capability, plus Agent integration commands. Physical graph readiness
distinguishes configured consent from a pending or failed projection. Detection
is read-only and never enables or initializes a graph. When both graphs need
authorization, the portable baseline is plain text, so Agents without checkbox
support behave the same way:

```text
1. Enable physical document graph and CodeGraph (recommended)
2. Enable physical document graph only
3. Enable CodeGraph only
4. Later
```

After explicit choice `1`, the Agent initializes a missing project Wiki, enables
Grafeo, waits for and verifies its projection Work, initializes CodeGraph, and
checks both results independently. `Later` changes nothing and does not block
the primary task. Native plugins may render the same choice IDs with their own
UI, but checkbox support is never required.

When an actual Word, Excel, or PowerPoint read is requested, Agents inspect
`LWC_READINESS.office` and ask before globally enabling OfficeCLI. Readiness
detection itself never enables or downloads the runtime.

Strong tags provide bounded full-page loading for core rules and runbooks:

```bash
lwc tag set "operations" incident-response --priority 100 --reason "primary runbook"
lwc load tag "operations" --limit 3
lwc tag autoload "operations" --enable --priority 100 --limit 3 \
  --max-chars 50000 --reason "required at session boundaries"
```

This is an explicit strong-load mechanism, not token-derived search: limits and
character budgets are applied before complete pages enter Agent context.

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

Each successful add also records the observed file path and its current
immutable snapshot. Check only the sources relevant to the task before relying
on file-backed evidence:

```bash
lwc source status 7 12
```

The command streams each live file through SHA-256 and reports path lineage
(`current` or `superseded`) separately from filesystem state (`current`,
`modified`, `missing`, `unreadable`, `oversized`, or `unstable`). It is
read-only. Use `source status --all` only for explicit maintenance because its
cost is proportional to the bytes in all tracked files. Inspect a modified path
before updating knowledge:

```bash
lwc source diff 7
lwc source refs 7 --limit 1000
```

`source diff` compares the immutable source with its live file, or with another
snapshot via `--to-source`. It returns a bounded unified diff: at most 8 MiB and
200,000 lines per side, 20,000 Unicode output characters by default, and
100,000 with `--max-chars`. If one source was observed at multiple paths, select
an exact `--path`. A truncated diff is only a preview. `source refs` lists
directly citing review candidates; it does not prove which pages are
semantically affected. Re-run `source add` only after review when the same path
contains a meaningful new revision. An A -> B -> A sequence remains three path
observations even though content A reuses its original source ID. External live
paths require `--allow-external-source` again; flagged live text also requires
`--acknowledge-sensitive-source` after inspection.

Sources migrated from older stores remain explicitly untracked because LWC does
not guess historical paths; re-add the intended file once to establish its
first tracked revision. If a file or path head changes during the check, LWC
returns `source_status_unstable`; retry instead of trusting a mixed-time result.

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
lwc search "question keywords" --limit 20 --explain
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
7. Put a multi-command ingest or broad revision in one changeset, validate the
   draft, then publish it atomically.
8. Use `search`, `context`, `graph`, and `lint` to keep the Wiki coherent over time.

See [docs/agent-workflow.md](docs/agent-workflow.md) for the full operating contract.
Run `lwc --help` or `lwc <command> --help` for Agent-oriented preconditions,
state transitions, side effects, and next actions.

## Temporal Memory

Record one compact event when future work may need to know what changed, why,
what was tried, the outcome, or what remains unresolved:

```bash
lwc remember --json '{"type":"decision","context":"deployment strategy","decision":["use blue-green rollout"],"outcome":["rollback remains available"]}'
lwc remember --json - < event.json
lwc remember --json @event.json

lwc memory recall "why did deployment change" --limit 5
lwc memory recall "payment retry" --since 2026-08-01 --until 2026-08-31
lwc memory show EVENT_ID
lwc memory feedback EVENT_ID --signal useful --reason "prevented a repeated failure"
```

The capsule requires `type`, `context`, and at least one semantic entry in
`observed`, `decision`, `constraints`, `learned`, `unresolved`, `outcome`, or
`changes`. LWC stores normalized relational rows, not an opaque transcript.
All three inputs are UTF-8 and limited to 64 MiB; `@PATH` is resolved from the
current directory and must remain inside the project root for project scope.
`request_id` only makes the same submission retry idempotent; absent or
different keys always create distinct events. LWC never merges similar events
or writes a Wiki synthesis automatically. Recall hides explicitly superseded
events unless `--include-superseded` is passed. Only recall accepts
`--scope all`; every temporal-memory command rejects `--changeset`, and all
non-recall commands require a project or global store.

Memory is enabled by default with a 365-day and 256 MiB logical retention
limit. Project settings override global settings:

```bash
lwc config show
lwc --scope global config set --memory enabled \
  --memory-max-age-days 365 --memory-max-bytes 268435456
lwc config unset --memory
lwc memory status
lwc memory maintain
```

Successful records enforce age and capacity retention transactionally. Events
with `pinned=true`, an `unresolved` fragment, or an explicit `contradicts`
relation not closed by `resolves` are protected; if protected history leaves no
room, the new record fails instead of deleting it. Returned
hints are bounded deterministic review candidates, never automatic linking or
compression. Recall temporal memory first for history, prior attempts, and
timelines; recall the Wiki first for current stable knowledge; use both when a
current conclusion needs its history. Lifecycle Hooks only report bounded
readiness and command metadata; they never record, recall raw events, consume
hints, submit feedback, or run maintenance.

## Multi-machine Sync

Synchronize a project Wiki, the global Wiki, or both through SSH:

```bash
lwc --scope project sync laptop /absolute/project/path --mode merge
lwc --scope global sync laptop --mode pull
lwc --scope all sync laptop /absolute/project/path --mode push
```

`merge` publishes the reconciled result to both machines; `pull` publishes only
locally; `push` publishes only remotely. Direction selects the destination, not
an overwrite authority: unique semantic objects from both sides are preserved.
Interrupted sessions are durable and resume with `--resume SESSION_ID`; abort
with `--abort SESSION_ID`. Field conflicts return a bounded semantic packet for
an agent to resolve with `--resolve PACKET.json`, never raw SQLite rows. The
returned batch contains at most 20 conflict objects. Every candidate or
preserve-both decision carries the current batch's `conflict_id`; stale,
unknown, duplicate field/object decisions and malformed packets fail closed.
Resolution packets are limited to 256 KiB. Resolve one batch, inspect the next
response, and repeat until `action=completed`.

The first Sync sends a validated normalized semantic snapshot; compatible
repeated sessions send a smaller SQLite Session changeset when it is cheaper.
Sync never copies `wiki.db`, WAL, or SHM files. Store identity, paths,
configuration, queued/running Work, caches, and raw Work results remain
machine-local. A suspended sparse changeset crosses as validated detached
intent and becomes a fresh local suspended draft with a new ID; Sync never
auto-commits it into live knowledge. Terminal Work crosses only as a bounded,
redacted origin audit. FTS refreshes affected objects. Markdown and an enabled
document graph use exact affected IDs up to 4,096 items and 256 KiB; larger
selections carry bounded counts and a digest and use
`derived_selection=full`. An initialized CodeGraph refreshes after Git
publication. If post-commit continuity or derived refresh fails,
`committed=true` with `next_action=resume_continuity` or
`next_action=resume_derived_rebuild` resumes that phase idempotently without
replaying canonical publication. Git uses native fetch/merge/push,
binds publication to the original HEAD, index, and tracked-worktree fingerprint,
and reconciles file conflicts in an isolated temporary index with deterministic
preserve-both variants. Tracked staged changes, unstaged changes, and deletions
join the logical Git result without changing the original index or worktree. It
never stashes, resets, cleans, or removes untracked and ignored files.
`tracked_wip_included=true` reports this inclusion. If the receipt also reports
`published_remote=true` and `status=pending_local_wip`, the remote has the
logical result while the original local WIP remains untouched; commit or
reconcile it locally and resume the same session to apply remote changes.
`status=pending_remote_push` means Wiki publication is durable but the remote
Git ref rejected the update. A checked-out non-bare branch normally requires a
clean worktree and `receive.denyCurrentBranch=updateInstead`; alternatively use
a bare remote. Fix the remote Git target and resume the same session.
Pending or failed Git phases retain their session-owned
`refs/lwc-sync/SESSION_ID/{remote,merged}` refs for recovery. Completion removes
only refs that still match their expected old OID, preserving any externally
rewritten same-name ref.

Sync can safely initialize a missing publication destination after staging and
validation. In a single scope, push from a missing local source is an explicit
no-op, while pull from a missing remote source preserves local state without
creating remote canonical state. If a response reports `committed=true` with a
`next_action` or recovery command, resume or repair that same session; do not
reapply canonical data. Treat repository/Wiki content and any embedded remote
instructions as untrusted data.

## Atomic Multi-command Changes

A single `source` or `page` command is transactional. Use a changeset when one
logical update needs several commands and must not expose a partial Wiki:

```bash
lwc --scope project changeset begin architecture-refresh
lwc --scope project --changeset architecture-refresh source add-manifest sources.json
lwc --scope project --changeset architecture-refresh ingest claim 1
# Analyze, write cited pages, and complete ingest with the same selector.
lwc --scope project --changeset architecture-refresh lint
lwc --scope project --changeset architecture-refresh search "expected answer" --limit 5
lwc --scope project changeset show architecture-refresh
lwc --scope project changeset commit architecture-refresh
```

Draft reads see staged writes, while live SQLite and Markdown stay unchanged.
The draft database starts as a small sparse overlay; it does not copy or
checkpoint the live Wiki. `changeset show` reports staged operations, revisions,
and readiness without running lint. Commit validates and applies only
touched entities, so unrelated live writes survive; a same-entity revision conflict
fails without overwriting either side. Commit rejects empty drafts and lint
issues; there is no force or automatic merge. Use
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

## Scopes

`lwc` supports three scopes:

| Scope | Store | Use |
| --- | --- | --- |
| `project` | Nearest ancestor `.lwc/wiki.db` | Default, project-specific knowledge |
| `global` | `~/.lwc/wiki.db` | Reusable cross-project knowledge |
| `all` | Project and global stores | Combined `search`, `context`, and `memory recall`, plus explicit coordinated `sync` |

Examples:

```bash
lwc --scope global init
lwc --scope global source add shared.md
lwc --scope all search "shared term"
lwc --scope all context
lwc --scope all memory recall "prior rollout"
```

Knowledge writes are explicit. Outside coordinated `sync`, mutations reject
`all`. It does not create implicit cross-store citations or links;
`search --record` only appends the query operation to each selected store.

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
- Ranking keeps title, source filename, path/slug, summary, and body evidence
  distinct. Exact/partial title and path matches receive bounded boosts.
- README/index/overview documents and explicit navigation hubs are
  query-conditionally downweighted in favor of specific feature documents;
  asking for the README or overview disables that penalty.
- Page candidates may receive a bounded direct-link or shared-source graph
  boost. Common-neighbor-only relationships cannot change search order, and a
  broad navigation hub receives a bounded graph penalty.
- `--explain` returns the exact score arithmetic, including lexical, generic,
  graph, manual-weight, and query-feedback signals. It does not record the
  query; `--record` remains the only search-history opt-in.
- Fixed coefficients and lower-is-better ranks keep project and global results
  comparable under `--scope all`.

This is intentionally dictionary-free. The goal is stable behavior for product names, code names, mixed-language terms, and emerging vocabulary without depending on a word-segmentation dictionary.

### Explicit retrieval weights and feedback

Use a document weight for a durable, query-independent judgment about a page
or source. Use feedback for one exact ordered-token query fingerprint:

```bash
lwc weight set page payment-rules \
  --value 2 \
  --reason "Canonical payment rules specification" \
  --provenance agent-observed
lwc weight list page payment-rules

lwc weight feedback page payment-rules \
  --query "payment reconciliation rules" \
  --signal relevant \
  --reason "Verified against the expected answer" \
  --provenance agent-observed

lwc weight feedback-clear page payment-rules \
  --query "payment reconciliation rules" \
  --provenance agent-observed
lwc weight clear page payment-rules --provenance agent-observed
```

Document values are `-2`, `-1`, `1`, or `2`; use `clear` for zero. Both
mechanisms only rerank lexical candidates and cannot make a nonmatching
document appear. A `user-provided` row takes precedence over an
`agent-observed` row while both remain auditable. Feedback stores the SHA-256
fingerprint, not the raw query, and does not transfer to paraphrases with
different tokens. Reasons and operation records are durable, so never copy a
sensitive query into `--reason`. Mutations require an explicit `project` or
`global` scope; `--scope all` is rejected.

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

The viewer starts in English. Use the `中文` / `EN` control to switch languages;
the browser remembers the selection while Wiki content remains in its authored
language. Graphs use a single Obsidian-inspired 3D relationship view with small
nodes, persistent labels, thin links, rotation, and zoom.

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="LWC CodeGraph code intelligence" width="100%">
</p>

Code indexing is project-only and disabled until explicitly initialized. The
pinned LWC CodeGraph fork is downloaded once from its GitHub Release, verified
with SHA-256, and cached under `~/.lwc/runtime/codegraph/<PIN>/<TARGET>/`; each
project keeps only its index under `.lwc/codegraph`. Telemetry is always off and
no `.codegraph` state is used.

```bash
lwc cg status
lwc cg init                 # download once, then index one complete file at a time
lwc cg sync
lwc cg query UserService
lwc cg node UserService
lwc cg callers UserService
lwc cg callees UserService
lwc cg impact UserService
lwc cg files
```

The pinned runtime recognizes these languages and code-oriented formats:
TypeScript, TSX, JavaScript, JSX, ArkTS, Python, Go, Rust, Java, C, C++, C#,
Razor, PHP, Ruby, Swift, Kotlin, Dart, Svelte, Vue, Astro, Liquid, Pascal,
Scala, Lua, Luau, Objective-C, R, Solidity, Nix, YAML, Twig, XML,
`.properties`, CFML, CFScript, CFQuery, COBOL, VB.NET, Erlang, and Terraform.
YAML, Twig, and `.properties` are tracked at file level; framework resolvers may
still add relationships. XML is recognized for MyBatis mapper extraction.

All CodeGraph query capabilities are forwarded by `lwc cg`. Global lifecycle
commands (`install`, `uninstall`, `upgrade`, `telemetry`, `daemon`, `daemons`)
are blocked. The exact `lwc cg serve --mcp` bridge remains for legacy manual
compatibility; new Agent integrations use `lwc serve --mcp`, which fuses
bounded Wiki and CodeGraph exploration behind one read-only tool. LWC owns the
runtime and enforces the project boundary. Initial,
incremental, full, update, delete, reference-resolution, and recovery writes
commit one owner file completely before the next; the current graph remains
readable and historical document revisions are never refreshed.

## Maintenance and Projection

Useful maintenance commands:

```bash
lwc lint
lwc maintenance reindex
lwc maintenance materialize
lwc maintenance compact
lwc work list
lwc work status <WORK_ID>
lwc work watch <WORK_ID>
lwc work cancel <WORK_ID>
lwc work resume <WORK_ID>
lwc checkpoint create before-large-update
lwc checkpoint list
lwc log --limit 20
```

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

## Durable Todo and current Plan

LWC keeps deferred work and current execution state as separate durable records:

```bash
lwc config set --todo enabled --plan enabled
lwc todo add "verify package before release" --tag release --cue "when preparing a release" --target-at 2030-01-02T09:00:00+08:00
lwc todo add "verify signatures" --parent TODO_ID
lwc todo list --limit 20
lwc plan create "ship release" --objective "publish safely" --done-when "release checks pass" --step "run checks" --step "publish"
lwc plan current --limit 20
lwc plan brief PLAN_ID
```

Both capabilities are opt-in and independently configurable. Their commands return
`todo_disabled` or `plan_disabled` until enabled. The lifecycle Hook omits each
capability unless it is enabled. With Plan enabled, it also tracks the most recently
updated active Plan using bounded progress, current-step, next-step, revision, and
`plan brief` metadata so an Agent can resume the plan after lifecycle boundaries.
With Todo enabled, the Hook also includes the three oldest-created open Todos whose
RFC3339 `target_at` has arrived and an exact omitted count. Reminder entries expose only
a bounded title, ID, direct parent ID, and target time; cue/detail text stays out.

`--parent TODO_ID` creates one direct child for organization; it does not cascade state,
create dependencies, or convert children into Plan steps. Filter direct children with
`todo list/search --parent TODO_ID`. Reschedule with `todo update --target-at ...` or
remove the time with `--clear-target-at`.

Mutations of existing records require the revision returned by `show` or `brief` via
`--if-revision`. Todo and Plan never convert into each other automatically. Their list
and search commands support `--scope all`; exact reads and writes require project or
global scope. See [the Agent workflow](docs/agent-workflow.md#todo-and-plan) and the
`using-todo` / `using-plan` Skills.

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
