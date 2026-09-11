---
name: using-plan
description: Use when an Agent needs to create, resume, advance, block, revise, complete, or abandon a durable current execution plan in LWC.
---

# Using LWC Plan

Reuse current readiness/config facts; run `lwc config show` only when the setting is unknown or stale. Continue only when `plan.setting` is `enabled`. A Skill trigger is not consent to enable Plan: when disabled, do not run Plan commands; explain that the user can opt in with `lwc config set --plan enabled`. A lifecycle Hook with a resolved `agent_context` includes only Plans explicitly tracked by that Agent context. Treat any Plan progress reminder for another context as unrelated and ignore it.

Use Plan only for the current coarse execution plan. It is independent from Todo and must never be converted to or from a Todo automatically.

- Create one objective, explicit done criteria, constraints, and ordered coarse steps.
- After creating or explicitly claiming a Plan, bind it with `lwc --scope project|global plan track PLAN_ID --context CONTEXT_ID`, using only the opaque ID from the current Hook's `LWC_READINESS.agent_context`. Never infer or copy another Agent's context.
- Resume with `lwc plan brief PLAN_ID`; it is bounded and contains no hidden reasoning.
- Treat Hook `plan.tracking` and `plan.additional_trackings` as continuity cues for this context only. Follow the current step and planned next step, but call `brief` before any mutation.
- Use exact, idempotent `plan untrack PLAN_ID --context CONTEXT_ID` before deliberately switching a context to another Plan; a conflicting `track` never replaces the current association.
- Before mutation, inspect the current revision and pass `--if-revision`.
- `advance` completes the focal step with a result and explicitly selects the next pending step.
- `block` records a concrete blocker. `revise` requires a reason and CAS revision. Inspect `lwc contract plan-revise` for its full schema and example.
- Prefer a targeted revision of `objective`, `done_when`, `constraints`, or `updates` by stable step ID. Use `current_step` when changing focus. The replacement `steps` form supersedes unfinished work and cannot be mixed with ID updates.
- Use `disposition: waived` with an explicit user-instruction `basis` for waived work; use `superseded` for replaced work. Neither means verification passed. Terminal steps cannot be rewritten.
- Mutation receipts show changed steps; `lwc plan show PLAN_ID` and `--full` expose full records. `lwc plan history PLAN_ID` returns revision evidence.
- `lwc plan reconcile PLAN_ID --from plan.md` is read-only: it returns title-matched event candidates and a textual document comparison. It does not infer semantic agreement or advance the plan. Review evidence before a CAS mutation.
- Keep execution state in this Plan. File maps should point to it, not mirror its progress. Use Wiki for stable knowledge and Memory for historical evidence.
- Complete only after all steps are terminal, evidence is supplied, and done criteria were checked.
- Use project/global for exact reads and writes. Use `--scope all` only for current/list/search.
- On a revision or request conflict, reload and reconcile; never overwrite blindly.
- Do not use `--changeset` with Plan commands.
