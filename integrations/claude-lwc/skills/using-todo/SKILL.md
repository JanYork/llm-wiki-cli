---
name: using-todo
description: Use when an Agent needs to capture, find, update, finish, cancel, or reopen durable deferred work in LWC.
---

# Using LWC Todo

First run `lwc config show`. Continue only when `todo.setting` is `enabled`. A Skill trigger is not consent to enable Todo: when disabled, do not run Todo commands; explain that the user can opt in with `lwc config set --todo enabled`. The lifecycle Hook includes Todo counts and commands only while Todo is enabled. It also includes at most three open Todos whose `target_at` has arrived, ordered by oldest creation time, plus an exact omitted count. Treat these reminders as cues to inspect the Todo; they contain no cue/detail text.

Use Todo only for independent future or deferred work. It is not the current execution plan and must never be converted to or from a Plan automatically.

- Add: `lwc todo add TITLE --tag TAG --cue TEXT --target-at RFC3339 --request-id ID`.
- Add one direct child with `--parent TODO_ID`. Parentage is organization only: it does not cascade completion, create dependencies, or turn children into Plan steps.
- Discover: `lwc todo list --limit 20`, `lwc todo list --parent TODO_ID`, or `lwc todo search QUERY --limit 20`.
- Inspect before mutation: `lwc todo show TODO_ID`; pass its revision with `--if-revision`.
- Reschedule with `todo update ... --target-at RFC3339`; remove the time with `--clear-target-at`.
- Finish or cancel with a specific result/reason; use `reopen` before changing a closed Todo.
- Use project/global for exact reads and writes. Use `--scope all` only for list/search.
- On a revision or request conflict, reload and reconcile; never overwrite blindly.
- Do not use `--changeset` with Todo commands.
