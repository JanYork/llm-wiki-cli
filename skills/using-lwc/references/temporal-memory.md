# LWC Temporal Memory

## Use when

Record when future work may need what changed, why, what was tried, the outcome, or what remains unresolved.
Record once at a meaningful boundary, not after every tool call.

Recall temporal memory first for before, when, changed, why, prior attempts, repeated failures, unresolved work, or incident timelines.
Recall the Wiki first for current architecture, instructions, and stable facts.
Use both when a current conclusion needs its history, then verify against current evidence.

## Skip when

Skip routine progress, transient tool output, secrets, stable Wiki facts, and ordinary chat turns.
Also skip guesses, repeated wording, and a result that will not matter after the current task.

## Minimum workflow

Record one small normalized capsule in one command:

```bash
lwc remember --json '{...}'
lwc remember --json '{"type":"决策","context":"部署回滚","decision":["恢复上一稳定版本"],"unresolved":["确认失败请求是否需要重放"]}'
```

Use `request_id` only to retry the same write safely. Different or absent
request IDs always create separate events; LWC does not semantically merge them.

Read narrowly and give feedback only when usefulness is known:

```bash
lwc memory recall "<query>" --limit 5
lwc memory show <EVENT_ID>
lwc memory feedback <EVENT_ID> --signal useful --reason "<reason>"
lwc memory status
lwc memory maintain
```

Normal recording already enforces age and capacity limits, so do not run
`memory maintain` after each event. Returned hints are review candidates only.
Turn one into Wiki knowledge only after current work establishes a reusable
conclusion; otherwise resolve, pin, or ignore it without rewriting history.

## Consent boundaries

- Lifecycle Hooks may advertise readiness, but must not record, recall raw
  events, consume hints, or run maintenance.
- Use project scope for project history. Use global scope only with explicit or
  durable authority for genuinely cross-project history.
- Do not store secrets, raw chain-of-thought, or unverified claims.
- Never edit temporal tables, FTS rows, counters, or cooldown state directly.

## Completion evidence

- A record response identifies the event and reports retention/hints without a
  second Agent maintenance step.
- A recall is bounded and its important claims are checked against current code
  or authoritative sources.
- Usefulness feedback is explicit; retrieval alone never counts as success.
- Any Wiki synthesis is separately validated under the normal Wiki workflow.
