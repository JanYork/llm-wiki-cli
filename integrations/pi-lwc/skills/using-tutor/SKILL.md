---
name: using-tutor
description: Use for explicit teacher-led learning, an active Tutor session, or recovery of pending Tutor work. Skip ordinary factual questions.
---

# Using Tutor

Use Tutor only for explicit learning intent or an already-active session. For ambiguous
intent, ask once whether the user wants a durable Tutor session. Ordinary Q&A stays
outside Tutor and is not recorded.

## Enter or recover

1. Inspect `LWC_READINESS.tutor`. This readiness object contains capability facts and
   commands only; it is not permission to enable, install, or read learning data.
2. If disabled, explain the durable local-data boundary and ask once before running
   `lwc --scope global config set --tutor enabled`. An explicit enable request already
   supplies consent. Enabling does not download; the first `lwc tutor status` may lazily
   install the fixed hash-verified runtime.
3. Run `lwc tutor status`, then recover a pending session or turn before creating new
   work. Reuse the returned `request_id`, owner, and `if_revision` on retries.
4. At every Tutor entry, read the complete current Soul. Never use a summary instead.
5. After cross-machine recovery, require the latest successful Sync receipt and run an
   explicit exact-revision takeover. The old owner must stop writing after takeover.

## One visible turn

1. Resolve the exact subject, session, goal, and plan IDs. Never infer identity from a
   title or tag.
2. Call `turn begin` with the exact learner-visible input and a stable `request_id`.
   Do not teach until this pending input is durable.
3. Read the complete current Soul plus bounded subject/session state. Locate the
   learner's concrete blockage, use progressive hints, and give the complete answer
   only after an attempt or explicit request. Exam mode has no hints.
   Teaching stays objective, scientific, concrete, and non-sycophantic. Praise must
   cite the exact observed response or improvement; never use generic flattery. ASCII
   diagrams are allowed as dialogue text, but v1 has no whiteboard subsystem.
4. Call `turn commit` with the exact final visible reply, checkpoint, owner, request ID,
   and `if_revision`. Deliver exactly the committed reply only after success.
5. On interruption, recover the pending or committed turn; never create a duplicate.

Use exact Book and Practice IDs for cross-plugin work. Keep every returned committed ID
and report the exact failed next command if a later store fails; the stores are not one
transaction. Do not let runtimes invoke each other, fuzzy-match identity, or silently
change learner-owned goals, deadlines, weekly time, or core-content constraints.

Soul updates must cite evidence and preserve version/diff history. Sensitive personality
judgments, stable principles, and behavior-changing updates require learner approval.
Never store hidden reasoning, system prompts, tool logs, credentials, or secrets. Do
not write Tutor content to the ordinary LWC Wiki unless the user separately selects it.
