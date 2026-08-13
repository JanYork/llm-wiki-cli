# LWC — Proactive Memory for AI Agents

**Agent-driven · Persistent · Source-grounded**

![LWC — Proactive Memory for AI Agents](https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-social-preview.png)

`lwc` is an agent-driven proactive memory CLI for AI agents. It lets Agents
autonomously recall, maintain, and evolve persistent, source-grounded knowledge
across sessions.

**Works with Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes,
Antigravity, and pi.**

## Install

```bash
npm install --global @i-xor/lwc
lwc --version
```

The installer downloads the matching binary from the exact GitHub Release and
verifies its SHA-256 checksum before use. Supported platforms are x64/arm64
macOS, glibc Linux, and Windows.

## What LWC Keeps

- a maintained Wiki instead of disposable query-time answers;
- immutable sources, citations, links, provenance, contradictions, and history;
- local-first SQLite knowledge with rebuildable Markdown and optional graphs;
- bounded recall and verified write-back across Agent sessions.

Read the [complete documentation](https://github.com/JanYork/llm-wiki-cli#readme)
or use the bundled
[`using-lwc` Agent Skill](https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc).
