#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skill="$root/skills/using-plan/SKILL.md"
for expected in 'name: using-plan' 'lwc config show' 'plan.setting' 'lwc config set --plan enabled' 'A Skill trigger is not consent' 'lifecycle Hook' 'plan.tracking' 'current step' 'next step' 'lwc plan brief' '--if-revision' 'advance' 'done criteria' '--scope all' 'never be converted'; do grep -Fq -- "$expected" "$skill"; done
for integration in codex-lwc claude-lwc pi-lwc; do diff -ru "$root/skills/using-plan" "$root/integrations/$integration/skills/using-plan"; done
