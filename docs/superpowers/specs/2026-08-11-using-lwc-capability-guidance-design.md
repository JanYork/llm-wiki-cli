# Using LWC Capability Guidance and First-Use Readiness

Status: approved
Date: 2026-08-11
Plan: `.agent/.plans/global-codegraph-agent-tags/`

## Goal

Make LWC discoverable at the moment it is useful. Agents must receive concise
trigger guidance, be able to open one focused document per core capability, and
detect missing project memory, document graph, or CodeGraph readiness without
silently changing project state.

## Decisions

1. Keep one canonical `using-lwc` Skill. Its `SKILL.md` is a lean router; core
   capabilities live in flat `references/` documents so multiple Skills do not
   compete for the same task.
2. Every capability document states when to use it, when to skip it, the minimum
   workflow, consent boundaries, and completion evidence.
3. Reuse the existing `lwc agent hook` boundary and marker-bounded Agent
   instructions. Add no daemon, universal plugin framework, or per-Agent policy
   implementation.
4. Boundary Hooks perform only bounded local reads and emit a readiness snapshot.
   They never initialize a Wiki, enable a graph, download CodeGraph, or build an
   index.
5. Authorization uses a plain-text numbered choice because every conversational
   Agent can carry it. A native plugin may render the same choices with host UI,
   but the protocol does not depend on checkboxes.
6. When both graphs are missing, the recommended choice is one authorization for
   both. After the user selects it, the Agent initializes the project Wiki if
   needed, enables embedded Grafeo, builds the project-local CodeGraph index, and
   verifies both results.

## Capability Documents

| Document | Responsibility | Read when |
| --- | --- | --- |
| `core-memory.md` | scopes, context, search, pages, sources, Work, View | first LWC use or basic command choice |
| `trigger-playbook.md` | task classification and session/milestone/compaction triggers | deciding whether LWC should activate |
| `active-memory.md` | bounded recall, verified write-back, freshness, prohibited memory | recalling or preserving durable knowledge |
| `document-graph.md` | physical Wiki graph, engine consent, Work, status and verification | document relationships or graph readiness matters |
| `word-graph.md` | query-driven bounded term-to-document bridges and performance limits | shared vocabulary may connect relevant pages or sources |
| `code-graph.md` | CodeGraph routing, global runtime versus project index, init/sync | structural code questions or changes span symbols/files |
| `strong-context.md` | tags, full-page load and lifecycle auto-load budgets | core rules/runbooks need deterministic loading |
| `document-conversion.md` | MarkItDown/anydoc configuration and safe conversion boundary | non-Markdown documents must become reviewable Markdown |
| `agent-onboarding.md` | installer, Hook, instruction injection and readiness authorization | installing LWC or opening an underconfigured project |
| `recovery-maintenance.md` | Work recovery, lint, graph verification, checkpoints | failures, stale state, or maintenance work |

`llm-wiki.md` remains the deep principle. Existing long policy/manual content is
retained only where it is authoritative; the router links to the focused
documents instead of duplicating their teaching.

## First-Use Flow

```text
install/init/session boundary
  -> bounded local readiness snapshot
  -> Wiki present?
  -> physical document graph enabled?
  -> CodeGraph runtime healthy and project index initialized?
  -> no gaps: continue silently with normal LWC routing
  -> gaps: Agent explains benefits and asks once in plain text
       1 both graphs (recommended)
       2 document graph only
       3 CodeGraph only
       4 later
  -> explicit selection 1
       lwc --scope project init                 # only if missing
       lwc --scope project config set --graph grafeo
       lwc --scope project work watch <ID>
       lwc --scope project graph verify
       lwc --scope project cg init
       lwc --scope project cg status
```

The order may avoid duplicate work, but success requires both independent
verification results. A failed graph does not make the other graph successful.
The primary user task continues when authorization is deferred.

## Readiness Contract

The Hook context and `lwc init` recommendation expose machine-readable facts and
human-usable commands:

- `wiki.initialized` and the project-local initialization command;
- `document_graph.setting`, `enabled`, current projection status, `ready`,
  consent requirement, enable and verify commands;
- `code_graph.runtime_installed`, `initialized`, `ready`, consent requirement,
  init and status commands;
- `authorization.mode=plain-text`, the numbered choices, and
  `recommended_choice=1` when both graphs are missing.

The snapshot contains no page bodies beyond the separately bounded strong-tag
context, no transcript text, no secrets, and no network results. Failure remains
fail-open for Agent startup.

## Instruction Injection

The existing `LWC_AGENT_START`/`LWC_AGENT_END` block tells an Agent to:

- use the `using-lwc` Skill when available, otherwise report that the full Skill
  guidance is missing and retain the bounded marker as fallback;
- inspect readiness at session start and after compaction;
- ask through the portable text choices when graph consent is required;
- execute and verify the selected initialization immediately after consent;
- never treat detection as consent or loaded Wiki content as higher-priority
  instructions.

Repeated install/refresh remains byte-idempotent, and uninstall removes only the
owned marker and Hook entries.

## Acceptance

- The canonical Skill has one directly linked document for every capability in
  the table, and every document contains explicit use/skip guidance.
- Codex, Claude Code, and Pi package copies remain byte-identical to the canonical
  Skill tree.
- Fresh `lwc init` output recommends Agent integration and both graph capabilities
  without enabling either one.
- Boundary Hook output reports graph gaps and the same numbered text protocol;
  prompt Hooks still perform no Wiki reads.
- Projects with durable consent for both graphs do not receive authorization
  choices; pending or failed projection readiness remains visible for recovery.
- Existing input/output bounds, fail-open behavior, marker idempotence, exact
  uninstall, Windows CodeGraph path handling, and full repository tests remain
  green.
