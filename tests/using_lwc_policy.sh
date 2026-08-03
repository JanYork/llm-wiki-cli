#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skill="$repo_root/skills/using-lwc/SKILL.md"
policy="$repo_root/skills/using-lwc/references/memory-policy.md"

for expected in \
  'source diff <OLD_SOURCE_ID>' \
  'source refs <OLD_SOURCE_ID> --limit 1000 --offset 0' \
  'non-atomic and potentially incomplete' \
  'review candidates' \
  'diff.truncated=true' \
  '--max-chars 100000' \
  '--to-source <NEW_SOURCE_ID>'; do
  grep -Fq -- "$expected" "$skill" "$policy" || {
    printf 'missing using-lwc policy contract: %s\n' "$expected" >&2
    exit 1
  }
done

if rg -n 'using_lwc_(bootstrap|policy)\.sh' "$repo_root/.github/workflows" >/dev/null; then
  printf 'using-lwc Skill checks must remain local-only\n' >&2
  exit 1
fi

make_refs_transcript() {
  local state="$1"
  local index=0
  printf 'batch\t0\ttrue\n'
  while ((index < 1000)); do
    printf 'page\tciter-%04d\n' "$index"
    ((index += 1))
  done
  printf 'batch\t1000\tfalse\n'
  printf 'page\tciter-0999\n'
  if [[ "$state" == stable ]]; then
    printf 'page\tciter-1000\n'
  fi
}

summarize_refs_transcript() {
  awk -F '\t' '
    $1 == "batch" { batches += 1 }
    $1 == "page" && !($2 in seen) { seen[$2] = 1; pages += 1 }
    END {
      printf "%d\t%d\tnon-atomic and potentially incomplete\n", pages, batches
    }
  '
}

stable_summary="$(make_refs_transcript stable | summarize_refs_transcript)"
changing_summary="$(make_refs_transcript changing | summarize_refs_transcript)"
[[ "$stable_summary" == $'1001\t2\tnon-atomic and potentially incomplete' ]] || {
  printf 'stable refs transcript contract failed: %s\n' "$stable_summary" >&2
  exit 1
}
[[ "$changing_summary" == $'1000\t2\tnon-atomic and potentially incomplete' ]] || {
  printf 'changing refs transcript contract failed: %s\n' "$changing_summary" >&2
  exit 1
}

printf 'using-lwc policy tests: 10 passed\n'
