# Discussion

Discussion stores visible iterative clarification and brainstorming in the existing Wiki SQLite database (schema 19). It is independent of the initiating Skill. Titles, descriptions, grouped questions and answers, original text, revisions, summary evidence and local bindings are persistent. Markdown/JSON exports are projections.

## Recording and recovery

Use `lwc contract discussion` for the shared CLI/MCP schema. Submit serialized JSON through `lwc discussion apply --json -` or MCP `lwc_discussion` with the authorized projectPath. Reuse the current opaque Agent context, stable request IDs and receipt revision. Identical retries are idempotent; conflicting requests, stale revisions and invalid batches fail atomically.

The using-discussion Skill records questions before display and exact visible answers before continuing. Optional host capture supplements an already bound discussion only when an exact raw prompt and native message ID are available. It saves unassigned replies for explicit association; it does not infer which question a reply answers. Synthetic Hook tests verify this path, not universal live-host transcript coverage. Unsupported hosts rely on the Agent protocol. Missing capture must be disclosed; secret-like input is rejected using the existing scanner and represented by a nonsensitive gap.

Read `current --context CONTEXT` for a bounded checkpoint; `list --context CONTEXT` discovers owned and imported unbound discussions. Use `show ID --context CONTEXT --offset 0 --limit 50`, `item ID ITEM --context CONTEXT`, and paginated `history`. `export` explicitly returns the full record.

## Smallest-granularity changes

Stable item IDs address questions, options, answer fragments and summary conclusions. Operations include start, question, answer, option, reply, gap, summary, revise, withdraw, restore, move, confirm, delivery, metadata, pause, resume and close. A reason is required for corrections and lifecycle adjustments. Originals and transaction inputs remain preserved; no repeated full history snapshots are written. Batch moves and new questions express regrouping, splitting and merging atomically.

Summary items cite existing evidence IDs. Corrections invalidate related conclusions transitively, including answers depending on a changed question; unrelated conclusions remain fresh. Cyclic references are rejected. The Agent writes detailed background, constraints, alternatives, decisions, corrections, unresolved issues and next actions. Confirmation is separate from summary generation. Closing requires answered active questions, fresh summaries and coverage of active questions and gaps, with no unassigned replies. Closing grants no implementation authorization.

Pause releases the binding. Explicit cross-context resume supplies the previous context; imported unbound discussions use an empty from_context. Archives and Sync preserve records/history but exclude local bindings. Imported local CAS revisions advance to reject stale writes. Raw discussion text is not automatically added to curated Wiki retrieval.

## Bounds and acceptance

Inputs are limited to 1 MiB, 64 operations per transaction and 64 KiB per text item. A current discussion is bounded to 10,000 items and 16 MiB. Reads paginate at most 100 items. Context isolation is not authentication. Host capture is supplementary and cannot guarantee delivery or completeness.

See [acceptance evidence](discussion-acceptance.md). The candidate is not globally installed or published. Codex native plugin packaging is documented in [its integration README](../integrations/codex-lwc/README.md); other hosts retain their installation routes.
