---
name: using-practice
description: Use when creating or recovering a durable paper, attempt, grade, flashcard, scheduled review, mistake history, or goal-evidence record. Skip Tutor comprehension checks, lightweight diagnostics, and disposable questions.
---

# Using Practice

Practice preserves assessed work across context loss. Flashcards are Practice items.
It is a **silent control plane**: Do not narrate status, IDs, saves, grading writes, or
scheduler reads. Show only questions, answers, feedback, and meaningful progress.

Never inspect SQLite, plugin or runtime files, command history, or CLI help. Never
reread Skill files or text-search for state. Use only the public commands here.

## Enter or recover

Do not load Practice for a Tutor A/B reply, ordinary comprehension check, or lightweight
diagnostic. Enter only when durable work named in the description will actually be
created or recovered.

1. Inspect `LWC_READINESS.practice`. If disabled, explain the durable local-data
   boundary and ask once before
   `lwc --scope global config set --practice enabled`; explicit enablement is consent.
   Enabling does not download; first status may lazily install the pinned,
   hash-verified runtime.
2. Run `lwc practice status` once on entry, lost identity, compaction, or a
   pending/revision/owner error. Otherwise cache exact attempt/session/owner/latest
   revision; do not repeat status.
3. Recover an in-progress attempt and saved responses before creating another. Use a
   new stable `request_id` per operation; retry that operation with the same request ID,
   owner, and `if_revision`.
4. Cross-machine recovery requires the latest successful Sync receipt and exact-revision
   takeover; the old owner must stop writing.

## Durable flow

1. Resolve exact typed subject, goal, book, bank, set, paper, and item IDs. Precedence is
   explicit plan target, goal bank, then subject default. Invalid higher-precedence
   targets fail; never fuzzy-match.
2. Generated items stay draft until prompt, answer, rubric, and exact source ref are
   re-read and verified. Formal papers use verified revisions and report shortages
   without weakening the blueprint.
3. Start or resume the exact attempt. Save every response immediately with owner,
   request ID, and `if_revision`; submission freezes already durable responses.
   Abandonment preserves history. v1 accepts only choice, text, numeric, and flashcard
   self-rating; reject every other form.
4. Grade objective items deterministically. Grade subjective items against the frozen
   rubric and store concise rationale, confidence, method, and review state. Learner
   overrides append history.
5. Let the pinned scheduler compute review state and due dates. Respect time budget and
   retention target, expose deferred debt, and never edit due dates or silently control
   review.

Keep exact committed IDs and the failed next command when independent stores partially
succeed; never claim they are one transaction. Never copy Practice records into the
ordinary LWC Wiki or store hidden reasoning, prompts, logs, credentials, or secrets.
