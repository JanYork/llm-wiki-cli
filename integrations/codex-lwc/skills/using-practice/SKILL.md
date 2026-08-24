---
name: using-practice
description: Use for durable questions, banks, papers, attempts, grading, flashcards, or scheduled review. Skip disposable one-off questions.
---

# Using Practice

Use Practice when answers, grading, mistakes, or review history must survive context
loss. Flashcards are Practice items, not a separate plugin. Skip disposable one-off
questions that need no durable history.

## Enter or recover

1. Inspect `LWC_READINESS.practice`. It is bounded readiness only, not consent to
   enable, install, read answers, grade, or mutate review state.
2. If disabled, explain the durable local-data boundary and ask once before running
   `lwc --scope global config set --practice enabled`. An explicit enable request
   already supplies consent. Enabling does not download; the first
   `lwc practice status` may lazily install the fixed hash-verified runtime.
3. Run `lwc practice status`. Recover an in-progress attempt and its saved responses
   before creating another. Retry with the same `request_id`, owner, and `if_revision`.
4. After cross-machine recovery, require the latest successful Sync receipt and run an
   explicit exact-revision takeover. The old owner must stop writing after takeover.

## Practice flow

1. Resolve exact typed subject, goal, book, bank, set, paper, and item IDs. Target
   precedence is explicit plan target, goal bank, then subject default. A missing or
   invalid higher-precedence target fails; never use fuzzy title/tag fallback.
2. Generated items remain draft until prompt, answer, rubric, and exact source ref are
   re-read and verified. Formal papers use verified revisions only and return an exact
   shortage instead of relaxing the blueprint.
3. Start or resume the exact attempt. Save every response immediately with its owner,
   request ID, and `if_revision`; submission freezes already durable responses and is
   not the save boundary. Abandonment preserves history.
   v1 accepts exactly four response forms: choice, text, numeric, and flashcard
   self-rating. Reject image, audio, file, code, or any fifth form.
4. Objective grading stays deterministic. Subjective grades must use the frozen rubric
   and store concise user-visible rationale, confidence, method, and review state.
   Learner overrides append history rather than replacing it.
5. Let the pinned scheduler compute review state and due dates. Respect the learner's
   time budget and desired retention, and expose deferred debt; an Agent never edits
   due dates or controls silently.

Keep committed IDs and the exact failed next command when cross-plugin orchestration
partially succeeds. Do not pretend independent stores are one transaction or copy
Practice records into the ordinary LWC Wiki. Never store hidden reasoning, system
prompts, tool logs, credentials, secrets, or unsupported response forms.
