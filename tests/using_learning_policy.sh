#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

check_skill() {
  plugin=$1
  skill="$root/skills/using-$plugin/SKILL.md"
  metadata="$root/skills/using-$plugin/agents/openai.yaml"
  test -f "$skill"
  test -f "$metadata"
  grep -Fq -- "name: using-$plugin" "$skill"
  grep -Fq -- "LWC_READINESS.$plugin" "$skill"
  grep -Fq -- "lwc $plugin status" "$skill"
  grep -Fq -- "lwc --scope global config set --$plugin enabled" "$skill"
  grep -Fq -- "recover" "$skill"
  grep -Fq -- "request_id" "$skill"
  grep -Fq -- "if_revision" "$skill"
  grep -Fq -- "ordinary LWC Wiki" "$skill"
  grep -Fq -- "hidden reasoning" "$skill"
  grep -Fq -- "display_name:" "$metadata"
  grep -Fq -- "default_prompt:" "$metadata"

  for integration in codex-lwc claude-lwc pi-lwc; do
    diff -ru "$root/skills/using-$plugin" "$root/integrations/$integration/skills/using-$plugin"
  done
}

check_skill tutor
check_skill book
check_skill practice

decision_fixtures="$root/tests/fixtures/using_learning_decisions.tsv"
test -f "$decision_fixtures"

decide() {
  plugin=$1
  scenario=$2
  case "$scenario" in
    ambiguous_intent|disabled_without_consent)
      printf 'ask_once|-|no-new-work'
      ;;
    pending_recovery)
      printf 'recover|lwc %s status|no-new-work' "$plugin"
      ;;
    *)
      return 1
      ;;
  esac
}

while IFS="$(printf '\t')" read -r plugin scenario expected; do
  case "$plugin" in \#*|'') continue ;; esac
  actual=$(decide "$plugin" "$scenario")
  test "$actual" = "$expected" || {
    printf 'decision fixture failed: %s %s: expected %s, got %s\n' \
      "$plugin" "$scenario" "$expected" "$actual" >&2
    exit 1
  }
done < "$decision_fixtures"

grep -Fq -- "read the complete current Soul" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "turn begin" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "turn commit" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "exact Book and Practice IDs" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "objective, scientific, concrete, and non-sycophantic" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "exact observed response or improvement" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "ASCII" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "no whiteboard subsystem" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "read next" "$root/skills/using-book/SKILL.md"
grep -Fq -- "read commit" "$root/skills/using-book/SKILL.md"
grep -Fiq -- "search, show, and peek never advance coverage" "$root/skills/using-book/SKILL.md"
grep -Fq -- "cannot" "$root/skills/using-book/SKILL.md"
grep -Fq -- "advance coverage while expired" "$root/skills/using-book/SKILL.md"
grep -Fiq -- "save every response immediately" "$root/skills/using-practice/SKILL.md"
grep -Fq -- "fuzzy" "$root/skills/using-practice/SKILL.md"
grep -Fq -- "due dates" "$root/skills/using-practice/SKILL.md"
grep -Fq -- "choice, text, numeric, and flashcard" "$root/skills/using-practice/SKILL.md"

for plugin in tutor book practice; do
  grep -Fq -- "latest successful Sync receipt" "$root/skills/using-$plugin/SKILL.md"
  grep -Fq -- "old owner must stop writing" "$root/skills/using-$plugin/SKILL.md"
done

printf 'using-learning policy tests: passed\n'
