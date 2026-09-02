# LWC project memory map

## Project identity

LWC is an Agent-first Rust CLI that compiles curated documents into a persistent, source-grounded Wiki. SQLite is canonical; Markdown, FTS, Grafeo/SurrealDB sidecars, and CodeGraph are derived or complementary planes.

## Read path

1. Search the project Wiki with task terms.
2. Use `lwc-project-overview` as the architecture and topic hub.
3. Use `lwc-code-module-map` for source-module ownership.
4. Use `lwc-operational-attention-points` before changes involving sources, changesets, Work, graphs, recovery, benchmarks, or releases.
5. Verify mutable implementation facts against checked-out code; sync/query CodeGraph for cross-file structural questions.

## Authoritative documentation

- `README.md` and its locale mirrors: product positioning, visual capability overview, basic installation, SEO entry points, and links to complete documentation; detailed commands deliberately live in the public Wiki.
- Public GitHub Wiki: complete user guides, architecture, workflows, configuration, command reference, and troubleshooting in English and Simplified Chinese.
- `docs/agent-workflow.md`: authoritative trust boundary, ingest, changesets, retrieval, maintenance, recovery, and projection contract.
- `SECURITY.md`: disclosure and local knowledge/sidecar security boundary.
- `CONTRIBUTING.md`: local checks, PR expectations, and release-tag rules.
- `benchmarks/README.md`: benchmark scope, metrics, sanitization, and fair comparison.
- `docs/superpowers/specs/2026-08-09-npm-distribution-design.md`: npm native-binary distribution and release order.
- `docs/superpowers/specs/2026-08-11-using-lwc-capability-guidance-design.md`: canonical Skill routing and readiness protocol.
- `docs/superpowers/specs/2026-08-12-lwc-agent-target-capability-matrix.md`: current AgentTarget capability authority; it supersedes the earlier strong/weak classification.
- `docs/superpowers/specs/2026-08-30-lwc-agent-signal-hooks-design.md`: frozen semantic-event, signal catalog, host delivery, Stop continuation, consent, privacy, budget, and acceptance contract.
- Wiki `todo-plan-harness-continuity`: local opaque Agent-context Plan/Todo tracking, session/child isolation, fail-closed Hook delivery, and prompt-level cross-context rejection.
- Wiki `lwc-sync-architecture`: non-overwriting SSH Sync semantics, normalized SQLite publication, Agent conflict resolution, Git continuity, and local derived-plane rebuild boundaries.

Translated READMEs, package summaries, integration summaries, and copied Skill trees are useful delivery mirrors but should not be ingested again when the canonical document already supplies the same evidence.

## Maintenance triggers

- Update the relevant Wiki topic when verified architecture, public behavior, safety boundaries, release contracts, or durable recovery lessons change.
- Re-ingest only source documents whose semantic content changed; use targeted `source status`, `source diff`, and `source refs` before revision.
- After Wiki changes, require project lint, fixed original/paraphrase retrieval checks, and independent Grafeo verification. After structural code changes, sync CodeGraph before current-structure claims.
- When the active project Plan changes or closes, update the durable-execution pointer below.
- Keep this file concise. Detailed facts belong in one focused Wiki page per durable concept; transcripts, transient logs, guesses, credentials, and raw chain-of-thought belong nowhere in the brain.

## Current durable execution

- No project Plan is active. Completed Plan `d60bf2cd634ef54db9b72cdcd70f61f9` (“Agent Plan/Todo Hook 会话隔离实现”), revision 10, delivered explicit local context tracking, schema v18, session/child-aware Hook and Stop filtering, host identity adapters, cross-context prompt rejection, Skills/docs, and Pro-only verification.
- Completed Plan `c6fd750f691a023894f10957cf29d35d` (“LWC 主动感知 Hook 全面优化”), revision 10, delivered the preceding Hook work and has no focal step.
- Revision 8 acceptance baseline: Clippy all-targets with warnings denied, the full Rust suite (main binary 256 passed/2 ignored plus integrations), 82 Agent CLI cases, 78 Hook cases, 7 Store Hook cases, and 5 Node integration cases passed; the later documentation, Skill, Wiki, memory, graph, lint, and fixed-retrieval closure is complete.
- Post-plan host addendum (2026-08-30): `--scope all` Hook reads isolate stale project/global stores so a valid peer store still supplies context and signals without writes; Pi 0.84.2 uses the official `session_before_compact` and `session_compact` events without a synthetic `session_compact_failed`. Real Codex smoke verified Skill discovery, SessionStart Plan context, first Stop continuation, and native repeat-guard release. Real Claude smoke verified Skill discovery and SessionStart Plan context, but its model service did not return, so Stop remains unverified on that host. Global Hook-only overlays may truthfully remain `modified` against an older full-install manifest or user-owned symlink state and must not be presented as `installed`.
- Cross-platform acceptance addendum (2026-08-31): Windows Hook reads accept canonical local verbatim-drive paths while rejecting UNC namespaces, preserve WAL/SHM bytes, and remain covered by the existing `windows-2025` runtime CI. The installer compatibility probe shares one deadline across process wait and bounded 64 KiB pipe drains; macOS/Linux runtime gates passed, while latest-source Windows GNU check/build/test-no-run linked 38 PE test executables. Windows no-run is compile/link evidence only, not a runtime PASS.
- Release closure (2026-09-01): immutable `v0.17.8`/`v0.17.9` failed tags exposed macOS Hook deadline/ZERO-busy-timeout and Windows fixture-portability gaps and produced no GitHub Release. Corrective `v0.17.10` at `ea625a1` passed release acceptance and all six native targets, published 26 GitHub assets, and completed public crates.io, Homebrew Tap (`5c57167`, CI `33516548466`), and npm `@i-xor/lwc@0.17.10` distribution with independent registry readback and a fresh-prefix npm install on Pro.
- Release closure (2026-09-02): `v0.17.11` at `f865af2` shipped Agent-context-isolated Plan/Todo Hooks. Release workflow `33638756269` passed acceptance and six native targets, published 26 GitHub assets with all 24 archives verified, and completed public crates.io, npm `@i-xor/lwc@0.17.11`, and Homebrew Tap (`b5a570d`, CI `33647863955`) readback. Local and Pro core plus Tutor/Book/Practice runtimes are `0.17.11`; user/AMC-managed suite symlinks remain ownership-preserved.
- Completed Plan `992709971591eafaf78b5a3a4a6c2281` (“弹性 Wiki 契约最小落地”) records the elastic Wiki contract delivery and acceptance evidence.

## Deliberately absent

There are no project-brain todos, activity logs, or operations journals yet. Create one only when current evidence demonstrates a continuity need, and update this discovery map at the same time.
