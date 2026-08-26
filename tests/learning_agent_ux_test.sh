#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
checker="$root/tests/learning_agent_ux_check.py"
fixtures="$root/tests/fixtures/learning_agent_ux"

reject() {
  scenario=$1
  transcript=$2
  if python3 "$checker" "$scenario" "$transcript" >/dev/null 2>&1; then
    printf 'expected rejection: %s\n' "$transcript" >&2
    exit 1
  fi
}

accept() {
  scenario=$1
  transcript=$2
  python3 "$checker" "$scenario" "$transcript"
}

reject no-code-init "$fixtures/bad_no_code_init.jsonl"
reject no-code-init "$fixtures/bad_no_code_hidden_cg.jsonl"
reject no-code-init "$fixtures/bad_no_code_control_persisted.jsonl"
reject no-code-init "$fixtures/bad_no_code_speculative_plan.jsonl"
reject steady-state "$fixtures/bad_steady_state.jsonl"
reject steady-state "$fixtures/bad_steady_state_skill_read.jsonl"
reject steady-state "$fixtures/bad_steady_state_order.jsonl"
reject steady-state "$fixtures/bad_steady_state_identity.jsonl"
reject steady-state "$fixtures/bad_steady_state_input.jsonl"
reject steady-state "$fixtures/bad_steady_state_commentary.jsonl"
accept no-code-init "$fixtures/good_no_code_init.jsonl"
accept steady-state "$fixtures/good_steady_state.jsonl"
accept steady-state "$fixtures/good_steady_state_commentary.jsonl"

grep -Fq " && python3 '" "$root/tests/learning_agent_ux_live.sh" || {
  printf 'live gate must fail fast when either checker rejects\n' >&2
  exit 1
}

printf 'learning Agent UX contract tests: passed\n'
