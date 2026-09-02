---
name: using-todo
description: Use when an Agent needs to capture, find, update, finish, cancel, or reopen durable deferred work in LWC.
---

# Using LWC Todo

First run `lwc config show`. Continue only when `todo.setting` is `enabled`. A Skill trigger is not consent to enable Todo: when disabled, do not run Todo commands; explain that the user can opt in with `lwc config set --todo enabled`. A lifecycle Hook with a resolved `agent_context` includes only Todos explicitly tracked by that Agent context. Treat any Todo progress reminder for another context as unrelated and ignore it.

Use Todo only for independent future or deferred work. It is not the current execution plan and must never be converted to or from a Plan automatically.

- Add: `lwc todo add TITLE --tag TAG --cue TEXT --target-at RFC3339 --request-id ID`.
- After adding or explicitly claiming a Todo, bind it with `lwc --scope project|global todo track TODO_ID --context CONTEXT_ID`, using only the opaque ID from the current Hook's `LWC_READINESS.agent_context`. One context may track multiple Todos; never infer or copy another Agent's context.
- Add one direct child with `--parent TODO_ID`. Parentage is organization only: it does not cascade completion, create dependencies, or turn children into Plan steps.
- Discover: `lwc todo list --limit 20`, `lwc todo list --parent TODO_ID`, or `lwc todo search QUERY --limit 20`.
- Inspect before mutation: `lwc todo show TODO_ID`; pass its revision with `--if-revision`.
- Reschedule with `todo update ... --target-at RFC3339`; remove the time with `--clear-target-at`.
- Finish or cancel with a specific result/reason; use `reopen` before changing a closed Todo.
- Stop reminders with exact, idempotent `todo untrack TODO_ID --context CONTEXT_ID`; this removes only that context/Todo pair.
- Use project/global for exact reads and writes. Use `--scope all` only for list/search.
- On a revision or request conflict, reload and reconcile; never overwrite blindly.
- Do not use `--changeset` with Todo commands.
