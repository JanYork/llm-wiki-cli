#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skill="$root/skills/using-todo/SKILL.md"
for expected in 'name: using-todo' 'lwc config show' 'todo.setting' 'lwc config set --todo enabled' 'A Skill trigger is not consent' 'lifecycle Hook' 'at most three' 'omitted count' 'lwc todo add' '--target-at RFC3339' '--parent TODO_ID' '--clear-target-at' 'lwc todo show' '--if-revision' '--scope all' 'never be converted'; do grep -Fq -- "$expected" "$skill"; done
for integration in codex-lwc claude-lwc pi-lwc; do diff -ru "$root/skills/using-todo" "$root/integrations/$integration/skills/using-todo"; done
