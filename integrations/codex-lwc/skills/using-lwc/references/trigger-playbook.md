# LWC Trigger Playbook

## Use when

Use this document when deciding whether LWC should activate, at session start or
after compaction, and at milestones where verified knowledge may deserve durable
write-back.

## Skip when

Skip LWC for spelling/formatting, a one-line literal edit, a self-contained
translation, or a fact with no project context or future reuse.

## Minimum workflow

Classify before calling tools:

| Trigger | LWC action |
| --- | --- |
| New substantive session | bootstrap once, bounded context, one search |
| Context compaction/resume | restore strong tags and only task-relevant memory |
| Research/debug/design | recall prior evidence/decisions before re-deriving |
| Before/when/changed/why/prior attempts | read `references/temporal-memory.md`, then bounded temporal recall |
| Meaningful verified event boundary | record one temporal capsule when future work may need the history |
| Stable current conclusion | update the Wiki; keep temporal memory as its history |
| Structural code question | check CodeGraph once; use it if ready |
| Document relationship question | check physical graph once; use it if ready |
| Non-Markdown source | configure one converter only when needed |
| Verified milestone | update an existing page or create one distinct page |
| Contradiction/staleness | inspect cited sources, revise or retract the claim |
| Task end | lint changed scope and run fixed retrieval acceptance |

The Automatic self-use loop is: classify, recall once, inspect current evidence,
solve, capture at milestones, validate, finish. Widen retrieval by one query,
kind, scope, or granularity at a time after a miss.

Read `references/temporal-memory.md` before the first temporal record or recall
decision. Temporal memory is history; the Wiki remains the first source for
current architecture, instructions, and stable facts.

Hooks are signals, not commands to mutate. At a lifecycle boundary, evaluate each
graph independently: CodeGraph requires a code-structure task plus code evidence
in the current working root; the physical document graph requires a document
relationship task plus document or Wiki evidence in the project root. Ask only
for applicable missing capabilities, and show the combined choices only when
both apply and are missing. Using Tutor, Book, or Practice for learning, reading,
or practice does not alone make CodeGraph applicable; modifying their source
code can when the task requires code structure and the working root contains
code evidence. Ask nothing for ordinary questions, sessions without a project
root, or when neither graph applies. Do not repeat the question in the same
project conversation.

## Consent boundaries

Automatic activation may read bounded authorized memory. It may not initialize a
missing Wiki, enable a graph, build a CodeGraph index, install a converter, or
write memory without the corresponding explicit or durable project authority.

## Completion evidence

- The task was correctly classified as use or skip.
- Bootstrap/recall/readiness checks ran at most once per working root unless state
  materially changed.
- Optional maintenance did not delay the deliverable.
- Any write-back is verified, durable, non-secret, and retrievable.
