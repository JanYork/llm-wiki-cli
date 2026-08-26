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
tutor_skill="$root/skills/using-tutor/SKILL.md"
test -f "$decision_fixtures"

route_from_skill() {
  scenario=$1
  awk -F ' *\\| *' -v scenario="$scenario" \
    '/^## Turn state$/ { section=1; next }
     /^## / && section { section=0 }
     section && $2 == "`" scenario "`" { print $3 "|" $4 "|" $5 }' "$tutor_skill" \
    | tr -d '`'
}

intent_from_skill() {
  scenario=$1
  awk -F ' *\\| *' -v scenario="$scenario" \
    '/^## Intent gate$/ { section=1; next }
     /^## / && section { section=0 }
     section && $2 == "`" scenario "`" { print $3 "|" $4 "|" $5 }' "$tutor_skill" \
    | tr -d '`'
}

mutation_from_skill() {
  mutation=$1
  awk -F ' *\\| *' -v mutation="$mutation" \
    '$2 == "`" mutation "`" { print $3 "|" $4 }' "$tutor_skill" \
    | tr -d '`'
}

shape_from_skill() {
  command=$1
  awk -F ' *\\| *' -v command="$command" \
    '/^## Known public shapes$/ { section=1; next }
     /^## / && section { section=0 }
     section && $2 == "`" command "`" { print $3 }' "$tutor_skill" \
    | tr -d '`'
}

preface_from_skill() {
  awk -F ' *\\| *' \
    '/^## Silent control plane$/ { section=1; next }
     /^## / && section { section=0 }
     section && $2 == "`phase/batch/wait`" { print $3 }' "$tutor_skill" \
    | tr -d '`'
}

while IFS="$(printf '\t')" read -r scenario state_source turn_flow practice; do
  case "$scenario" in \#*|'') continue ;; esac
  expected="$state_source|$turn_flow|$practice"
  case "$scenario" in
    explicit-intent|ambiguous-intent|ordinary-qa)
      actual=$(intent_from_skill "$scenario")
      ;;
    *)
      actual=$(route_from_skill "$scenario")
      ;;
  esac
  test "$actual" = "$expected" || {
    printf 'learning route failed: %s: expected %s, got %s\n' \
      "$scenario" "$expected" "${actual:-missing}" >&2
    exit 1
  }
done < "$decision_fixtures"

test "$(intent_from_skill explicit-intent)" = 'enter-directly|begin-teach-commit|skip'
test "$(intent_from_skill ambiguous-intent)" = 'ask-once|no-turn-until-answer|skip'
test "$(intent_from_skill ordinary-qa)" = 'outside-tutor|no-turn|skip'

# Hot replies must use the cached exact binding; only cold/recovery may read status.
test "$(route_from_skill hot)" = 'cached-exact-binding|begin-teach-commit|skip'
test "$(route_from_skill cold | cut -d '|' -f 1)" = 'status-once'
test "$(route_from_skill recovery | cut -d '|' -f 1)" = 'status-once'
test "$(route_from_skill practice-transition | cut -d '|' -f 3)" = 'enter'

# Idempotency keys are stable per mutation, never shared by begin and commit.
test "$(mutation_from_skill begin)" = 'new-stable-begin-key|same-mutation-only'
test "$(mutation_from_skill commit)" = 'new-stable-commit-key|same-mutation-only'
test "$(mutation_from_skill begin | cut -d '|' -f 1)" != \
  "$(mutation_from_skill commit | cut -d '|' -f 1)"

test "$(shape_from_skill 'lwc tutor subject create --json JSON')" = '{name,request_id}'
test "$(shape_from_skill 'lwc tutor session create --json JSON')" = '{subject_id,mode=learning/question/exam,request_id}'
test "$(shape_from_skill 'goal create')" = '{subject_id,statement,criteria[],request_id} optional'
test "$(shape_from_skill 'plan create')" = '{subject_id,goal_id,mode=fixed/adaptive/agent-led,deadline:string,weekly_minutes,core_content[],order[],pace,method,exercise_ratio=0..1,request_id} optional'
test "$(shape_from_skill 'lwc tutor turn begin --json JSON')" = '{session_id,owner,input,request_id}'
test "$(shape_from_skill 'lwc tutor turn commit TURN_ID --if-revision REV --json JSON')" = '{owner,reply,checkpoint,request_id}'
test "$(shape_from_skill checkpoint)" = '{kind=teaching,blocked_by=non-empty-string,hint_level,learner_attempted,explicit_answer_request,full_answer,feedback_evidence_refs,anchor}'
test "$(shape_from_skill anchor)" = '{current_node,mastered_nodes,current_mode,clearance_status,next_action}'
test "$(shape_from_skill 'goal/plan')" = 'optional-first-entry'
test "$(preface_from_skill)" = 'outcome-or-next-teaching-action-only;never-Tutor/using-tutor/Skill/LWC/storage/persistence/recording'

assert_no_command() {
  pattern=$1
  if grep -Eiq -- "$pattern" "$tutor_skill" "$root/skills/using-practice/SKILL.md"; then
    printf 'forbidden learning command found: %s\n' "$pattern" >&2
    exit 1
  fi
}

assert_no_command '`[^`]*(cat|find|ls|sed|awk|grep|rg)[^`]*(SKILL\.md|plugin|runtime)[^`]*`'
assert_no_command '`[^`]*(sqlite3?|\.sqlite)[^`]*`'
assert_no_command '`[^`]*lwc (tutor|practice)[^`]*--help[^`]*`'

grep -Fq -- "read the complete current Soul" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "turn begin" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "turn commit" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "exact Book and Practice IDs" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "objective, scientific, concrete, and non-sycophantic" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "exact observed response or improvement" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "ASCII" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "no whiteboard subsystem" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "silent control plane" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- 'must not be recorded as a turn' "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "Never inspect SQLite" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "Learning mode" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "Problem-solving mode" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "Exam mode" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "ordinary comprehension checks" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "hidden cognitive anchor" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "Never print the anchor" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "Do not narrate" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "first principles" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "Feynman" "$root/skills/using-tutor/SKILL.md"
grep -Fq -- "Socratic" "$root/skills/using-tutor/SKILL.md"
for plugin in tutor book practice; do
  grep -Fq -- "silent control plane" "$root/skills/using-$plugin/SKILL.md"
  grep -Fq -- "Do not narrate" "$root/skills/using-$plugin/SKILL.md"
done
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
