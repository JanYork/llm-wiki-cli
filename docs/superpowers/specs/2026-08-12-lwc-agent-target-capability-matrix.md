# LWC AgentTarget capability matrix

Verified: 2026-08-12. This supersedes the three-strong/nine-weak model. Every
registered target is a strong adapter: LWC installs each official file-based
surface and explicitly reports official absence or UI ownership. Strong does
not mean that every host implements the same features.

The only Agent-facing MCP command is `lwc serve --mcp`. CodeGraph remains an
internal implementation detail behind LWC MCP and is never registered alone.

Modes: `I` installed, `B` extension bridge, `U` unsupported, `M` user-managed,
`N` not applicable. `G` and `L` are global and current-project scopes.

| Target | Scope | MCP | Skill | Hook | Instructions | Permissions | Official delivery paths |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Claude | G | I | I | I | I | I | `~/.claude.json`; `~/.claude/skills/using-lwc`; `~/.claude/settings.json`; `~/.claude/CLAUDE.md` |
| Claude | L | I | I | I | I | I | `.mcp.json`; `.claude/skills/using-lwc`; `.claude/settings.json`; `.claude/CLAUDE.md` |
| Codex | G | I | I | I | I | N | `$CODEX_HOME/config.toml`; `~/.agents/skills/using-lwc`; `$CODEX_HOME/hooks.json`; `$CODEX_HOME/AGENTS.md` |
| Codex | L | I | I | I | I | N | `.codex/config.toml`; `.agents/skills/using-lwc`; `.codex/hooks.json`; `AGENTS.md` |
| Pi | G | B | I | I | I | N | `~/.pi/agent/extensions/lwc.js`; `~/.pi/agent/skills/using-lwc`; guidance injected by the extension |
| Pi | L | B | I | I | I | N | `.pi/extensions/lwc.js`; `.pi/skills/using-lwc`; guidance injected by the extension |
| Cursor | G | I | I | I | M | N | `~/.cursor/mcp.json`; `~/.cursor/skills/using-lwc`; `~/.cursor/hooks.json`; User Rules stay Settings-owned |
| Cursor | L | I | I | I | I | N | `.cursor/mcp.json`; `.cursor/skills/using-lwc`; `.cursor/hooks.json`; `.cursor/rules/lwc.mdc` |
| OpenCode | G | I | I | I | I | N | `$XDG_CONFIG_HOME/opencode/opencode.jsonc`; `skills/using-lwc`; `plugins/lwc.js`; `AGENTS.md` |
| OpenCode | L | I | I | I | I | N | `opencode.jsonc`; `.opencode/skills/using-lwc`; `.opencode/plugins/lwc.js`; `AGENTS.md` |
| Hermes | G | I | I | I | I | N | `$HERMES_HOME/config.yaml`; `$HERMES_HOME/skills/using-lwc`; `hooks.pre_llm_call`; `$HERMES_HOME/SOUL.md` |
| Hermes | L | U | U | U | I | N | project `AGENTS.md`; no separate project install root for MCP, Skills or Shell Hooks |
| Gemini | G | I | I | I | I | N | `~/.gemini/settings.json`; `~/.gemini/skills/using-lwc`; `hooks.SessionStart`; `~/.gemini/GEMINI.md` |
| Gemini | L | I | I | I | I | N | `.gemini/settings.json`; `.gemini/skills/using-lwc`; `hooks.SessionStart`; `GEMINI.md` |
| Antigravity | G | I | I | I | I | N | `~/.gemini/config/plugins/lwc/`: `plugin.json`, `mcp_config.json`, `skills/using-lwc`, `hooks.json`, `rules/lwc.md` |
| Antigravity | L | I | I | I | I | N | `.agents/plugins/lwc/`: `plugin.json`, `mcp_config.json`, `skills/using-lwc`, `hooks.json`, `rules/lwc.md` |
| Kiro | G | I | I | I* | I | M | `$KIRO_HOME/settings/mcp.json`; `$KIRO_HOME/skills/using-lwc`; `$KIRO_HOME/hooks/lwc.json` |
| Kiro | L | I | I | I* | I | M | `.kiro/settings/mcp.json`; `.kiro/skills/using-lwc`; `.kiro/hooks/lwc.json` |
| Copilot VS Code | G | I | I | U | M | M | default user-profile `mcp.json`; personal instructions remain UI-owned; `~/.copilot/skills/using-lwc` |
| Copilot VS Code | L | I | I | I* | I | M | `.vscode/mcp.json`; `.github/skills/using-lwc`; `.github/hooks/lwc.json`; `.github/copilot-instructions.md` |
| Copilot CLI | G | I | I | I | I | M | `$COPILOT_HOME/mcp-config.json`; `$COPILOT_HOME/skills/using-lwc`; `$COPILOT_HOME/hooks/lwc.json`; `$COPILOT_HOME/copilot-instructions.md` |
| Copilot CLI | L | I | I | I | I | M | `.github/mcp.json`; `.github/skills/using-lwc`; `.github/hooks/lwc.json`; `.github/copilot-instructions.md` |
| Copilot JetBrains | G | I | I* | U | M | M | `$XDG_CONFIG_HOME\|~/.config/github-copilot/intellij/mcp.json`; personal instructions remain UI-owned; `~/.copilot/skills/using-lwc` |
| Copilot JetBrains | L | M | I* | U | I* | M | `.github/skills/using-lwc`; `.github/copilot-instructions.md`; MCP remains user-configured because the IDE exposes no stable repository file path |

`*` is an official preview or host-version-gated surface. Status reports it as
`configured_preview`: LWC wrote the official file, but host activation remains unverified. Kiro standalone Hook files require Kiro IDE 1.x or CLI v3,
and Copilot repository Hooks/JetBrains Skills require a host release that exposes
those preview surfaces.

## Official evidence

- Claude: [MCP](https://docs.anthropic.com/en/docs/claude-code/mcp), [hooks](https://docs.anthropic.com/en/docs/claude-code/hooks), [skills](https://docs.anthropic.com/en/docs/claude-code/skills).
- Codex: [MCP](https://learn.chatgpt.com/docs/extend/mcp), [hooks](https://learn.chatgpt.com/docs/hooks), [skills](https://learn.chatgpt.com/docs/build-skills), [AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md).
- Pi: [skills](https://pi.dev/docs/latest/skills), [extensions](https://pi.dev/docs/latest/extensions), [no built-in MCP](https://pi.dev/).
- Cursor: [Agent practices](https://cursor.com/blog/agent-best-practices), [rules](https://docs.cursor.com/context/rules).
- OpenCode: [skills](https://opencode.ai/docs/skills/), [plugins](https://dev.opencode.ai/docs/plugins/).
- Hermes: [configuration](https://hermes-agent.nousresearch.com/docs/user-guide/configuration), [skills](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills), [hooks](https://hermes-agent.nousresearch.com/docs/user-guide/features/hooks/).
- Gemini: [configuration](https://geminicli.com/docs/reference/configuration/), [hooks](https://geminicli.com/docs/hooks/reference/), [instructions](https://geminicli.com/docs/cli/gemini-md/).
- Antigravity: [plugins](https://www.antigravity.google/docs/plugins), [hooks](https://www.antigravity.google/docs/hooks), [skills](https://antigravity.google/docs/skills), [rules](https://antigravity.google/docs/ide-rules).
- Kiro: [agent configuration](https://kiro.dev/docs/cli/custom-agents/configuration-reference/), [MCP](https://kiro.dev/docs/cli/mcp/configuration/), [settings](https://kiro.dev/docs/cli/reference/settings/).
- Copilot: [customization matrix](https://docs.github.com/en/copilot/reference/customization-cheat-sheet), [hooks](https://docs.github.com/en/copilot/concepts/agents/hooks), [CLI reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference).

## Invariants

1. `adaptation=strong` is metadata only; capabilities are never inferred from it.
2. Install, status, print-config, refresh and uninstall use the same explicit modes and paths.
3. Supported partial scopes remain installable; missing surfaces do not make a Target weak.
4. LWC owns only its named MCP entry, marker block, canonical Skill and recognizable Hook/plugin entry. Refresh is byte-idempotent and uninstall preserves siblings.
5. UI-only rules, trust prompts and broad permissions are never fabricated or auto-approved.
