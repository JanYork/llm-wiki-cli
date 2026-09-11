---
name: using-discussion
description: Silently persist and recover visible multi-turn clarification, interviews, design Q/A and brainstorming, regardless of which Skill or prompt starts them. Use for ongoing discussion, not ordinary single-answer chat.
---

# Persistent discussion

When a user requests or enters iterative clarification/brainstorming, use this
protocol alongside the active domain Skill (including grill-me, grilling or
superpowers). No name allowlist. Do not initiate questions merely to use it.
Reuse the authorized project SQLite and current Hook's opaque agent context.
Never manufacture another context, initialize a missing Wiki, or install tooling
solely to record. An unavailable store is a capture limitation, not permission to
pretend the conversation is saved. Without a resolved context, disclose the gap.

Use `lwc contract discussion` once for the exact schema and example. CLI
`lwc discussion apply --json -` and MCP `lwc_discussion` action `apply` share it.
MCP always requires the current absolute projectPath. Never pass shell-interpolated
user text: serialize JSON to stdin or structured MCP arguments.

1. Recover `discussion current --context CONTEXT` before starting or after a
   compact/restart. No binding means start a discussion with a unique ID, title,
   description and stable request ID. The first `start` binds only this context.
2. Save each question and options with stable IDs **before displaying identical
   text**. Question preparation is not proof of display. Only report delivery
   with a host message reference; it remains reported evidence.
3. On each user reply, persist exact visible text before analysis or another
   question. An `answer` references its parent question; multiple messages have
   separate IDs. If association is ambiguous use `reply` and later `move` it to
   a question. Preserve multi-question original responses; never invent a split.
4. Read unassigned host replies first to avoid duplicate recording. Message IDs
   deduplicate supplied host events; unsupported/missing IDs fall back to this
   Agent protocol, never a guarantee of complete host capture. Do not reread
   full transcripts or store expanded prompts, tools, system text or reasoning.
5. Batch the preceding answer and next question when possible. Use the last
   receipt's revision and one stable request ID. On uncertain result retry the
   identical input; on conflict reload and reconcile. Never blindly replay with
   a new ID. Successful receipts are silent: no logging commentary is necessary.
6. On persistent failure, notify the user once that capture is incomplete and
   keep retrying only within the task's scope. Do not continue claiming durable
   capture. For excluded secrets submit a `gap` with a nonsensitive description;
   never store the secret in the reason, ID or retry log. This is an explicit
   exception for visible Discussion records, not general prompt logging.

## Local adjustment and completion

`revise` changes one item's current text, retaining original text and history;
require a reason. Represent answer fragments/options/summary conclusions as
separate stable items to edit at that granularity. `move` updates parent/ordinal;
combine moves and new questions to split or merge groups without deleting IDs.
`withdraw`/`restore` retain history. `metadata` changes title/description.
`confirm` is only for a user's explicit confirmation, with the visible basis in
reason. Never mark an Agent proposal confirmed. Summary refs must form a DAG.

Write the detailed summary as separately addressable `summary` items: background,
requirements/constraints, alternatives and tradeoffs, decisions and rationale,
user corrections, unresolved questions, risks and next actions. Every item cites
question/answer IDs. `revise` a stale summary with current refs after rechecking
its evidence; unrelated conclusions stay intact. Newly introduced topics may
require additional summary items. Do not equate a generated summary with user
approval or implementation authorization.

`close` requires current summaries and all active questions answered; explicitly
withdraw waived questions with the user's reason. `pause` preserves pending work
and releases binding. `resume` requires a reason; cross-context resume supplies
`from_context` explicitly (empty for an imported unbound discussion). Never guess
or silently take over another task. After closing, deliver the saved summary.

Use `list --context CONTEXT` to discover owned/imported discussions and `item ID ITEM --context CONTEXT` for one item. Read `show` and `history` using limit/offset; use `export` only for an explicitly
needed full JSON artifact. SQLite alone owns the records. Do not mirror raw Q/A
into Wiki or filesystem memory. Promote confirmed reusable knowledge separately
with Discussion item references when requested or justified by project policy.
