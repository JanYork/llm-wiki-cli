---
name: using-plan
description: Use when an Agent needs to create, resume, advance, block, revise, complete, or abandon a durable current execution plan in LWC.
---

# Using LWC Plan

First run `lwc config show`. Continue only when `plan.setting` is `enabled`. A Skill trigger is not consent to enable Plan: when disabled, do not run Plan commands; explain that the user can opt in with `lwc config set --plan enabled`. While Plan is enabled, the lifecycle Hook includes the active count and bounded `plan.tracking` for the most recently updated active Plan: progress, current step, next step, revision, and a `plan brief` command.

Use Plan only for the current coarse execution plan. It is independent from Todo and must never be converted to or from a Todo automatically.

- Create one objective, explicit done criteria, constraints, and ordered coarse steps.
- Resume with `lwc plan brief PLAN_ID`; it is bounded and contains no hidden reasoning.
- Treat Hook `plan.tracking` as a continuity cue. Follow its current step and planned next step, but call its `brief` command before any mutation.
- Before mutation, inspect the current revision and pass `--if-revision`.
- `advance` completes the focal step with a result and explicitly selects the next pending step.
- `block` records a concrete blocker. `revise` requires a reason and replaces only unfinished work.
- Complete only after all steps are terminal, evidence is supplied, and done criteria were checked.
- Use project/global for exact reads and writes. Use `--scope all` only for current/list/search.
- On a revision or request conflict, reload and reconcile; never overwrite blindly.
- Do not use `--changeset` with Plan commands.
