# LWC Agent Onboarding and Readiness

## Use when

Use this document during LWC installation, first project use, Agent integration,
session start/compaction readiness, or when the project lacks physical document
graph or CodeGraph capability.

## Skip when

Skip onboarding after the Agent integration and required project capabilities are
already ready. Do not turn readiness into a prompt on every user message.

## Minimum workflow

Install with Agent detection and a chosen scope:

```bash
lwc agent install
lwc agent install --yes
lwc agent install --print-config codex
```

The installer configures the stable `lwc cg serve --mcp` launcher where the Agent
supports MCP, writes one bounded `LWC_AGENT_START`/`LWC_AGENT_END` instruction
block, and registers native lifecycle Hooks where verified. Codex, Claude Code,
and Pi have packaged integrations, but the registry is not limited to them.

At fresh init or a boundary Hook, inspect `LWC_READINESS`. When both graphs are
missing and the current task is substantive, ask once using this portable text
protocol; do not require checkbox support:

```text
1. Enable physical document graph and CodeGraph (recommended)
2. Enable physical document graph only
3. Enable CodeGraph only
4. Later
```

Choice `1` is one combined authorization. Execute existing commands immediately
after consent, skipping project init only when it already exists:

```bash
lwc --scope project init
lwc --scope project config set --graph grafeo
lwc --scope project work watch <ID>
lwc --scope project graph status
lwc --scope project graph verify
lwc --scope project cg init
lwc --scope project cg status
```

The physical graph Work and CodeGraph status are independent acceptance results.
Report either failure without hiding the other success. Choice `4` changes
nothing and the primary task continues.

## Consent boundaries

Agent detection, installation, Hook execution, and readiness facts are not graph
consent. Hooks are bounded local read-only checks: they never initialize a Wiki,
enable a graph, download CodeGraph, build an index, read transcripts, or write
memory. Native plugin trust remains a separate host/user decision.

## Completion evidence

- `lwc agent status` reports the intended target and global/local location.
- Reinstall/refresh is byte-idempotent; uninstall removes only owned MCP,
  markers, and Hooks.
- Graph authorization choices disappear once both capabilities have durable
  consent; bounded readiness facts still expose pending or failed initialization
  for routing and repair.
- The same numbered meaning works through plain text and optional native UI.
