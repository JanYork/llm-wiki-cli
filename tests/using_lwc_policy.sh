#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skill="$repo_root/skills/using-lwc/SKILL.md"
policy="$repo_root/skills/using-lwc/references/memory-policy.md"
manual="$repo_root/skills/using-lwc/references/operations-manual.md"
temporal="$repo_root/skills/using-lwc/references/temporal-memory.md"
metadata="$repo_root/skills/using-lwc/agents/openai.yaml"
skill_documents=("$skill" "$repo_root"/skills/using-lwc/references/*.md)

if grep -En '\"\$LWC\"|LWC=.decoded-lwc_path.|export LWC_PROJECT_ROOT=.decoded-project_root.' \
  "${skill_documents[@]}"; then
  printf 'using-lwc guidance must invoke global lwc directly from the current project\n' >&2
  exit 1
fi

for expected in \
  'lwc --scope all context --limit 25' \
  'lwc --scope all search "task terms" --limit 20' \
  'LWC_PROJECT_ROOT is only for an explicitly targeted project'; do
  grep -Fq -- "$expected" "${skill_documents[@]}" || {
    printf 'missing direct lwc invocation contract: %s\n' "$expected" >&2
    exit 1
  }
done

capabilities=(
  core-memory
  trigger-playbook
  active-memory
  document-graph
  word-graph
  code-graph
  strong-context
  document-conversion
  office-reading
  temporal-memory
  agent-onboarding
  recovery-maintenance
)

for capability in "${capabilities[@]}"; do
  document="$repo_root/skills/using-lwc/references/$capability.md"
  [[ -f "$document" ]] || {
    printf 'missing using-lwc capability document: %s\n' "$capability" >&2
    exit 1
  }
  grep -Fq -- "references/$capability.md" "$skill" || {
    printf 'using-lwc router does not link capability: %s\n' "$capability" >&2
    exit 1
  }
  for heading in \
    '## Use when' \
    '## Skip when' \
    '## Minimum workflow' \
    '## Consent boundaries' \
    '## Completion evidence'; do
    grep -Fq -- "$heading" "$document" || {
      printf 'capability %s is missing teaching section: %s\n' "$capability" "$heading" >&2
      exit 1
    }
  done
done

grep -Fq -- 'references/temporal-memory.md' \
  "$repo_root/skills/using-lwc/references/trigger-playbook.md" || {
  printf 'trigger playbook does not route temporal memory\n' >&2
  exit 1
}

for expected in \
  'Record when future work may need what changed, why, what was tried, the outcome, or what remains unresolved.' \
  'Skip routine progress, transient tool output, secrets, stable Wiki facts, and ordinary chat turns.' \
  'Recall temporal memory first for before, when, changed, why, prior attempts, repeated failures, unresolved work, or incident timelines.' \
  'Recall the Wiki first for current architecture, instructions, and stable facts.' \
  "lwc remember --json '{...}'" \
  'lwc memory recall "<query>" --limit 5' \
  'lwc memory show <EVENT_ID>' \
  'lwc memory feedback <EVENT_ID> --signal useful --reason "<reason>"' \
  'lwc memory status' \
  'lwc memory maintain'; do
  grep -Fq -- "$expected" "$temporal" || {
    printf 'missing temporal-memory guidance: %s\n' "$expected" >&2
    exit 1
  }
done

for expected in \
  '| `remember`, `memory`' \
  '## Temporal memory'; do
  grep -Fq -- "$expected" "$manual" || {
    printf 'operations manual does not expose temporal memory: %s\n' "$expected" >&2
    exit 1
  }
done

skill_lines="$(wc -l < "$skill" | tr -d ' ')"
((skill_lines <= 250)) || {
  printf 'using-lwc router is too large: %s lines (maximum 250)\n' "$skill_lines" >&2
  exit 1
}

grep -Fq -- 'description: Use when' "$skill" || {
  printf 'using-lwc description must state activation triggers\n' >&2
  exit 1
}

for expected in \
  'source diff <OLD_SOURCE_ID>' \
  'source refs <OLD_SOURCE_ID> --limit 1000 --offset 0' \
  'non-atomic and potentially incomplete' \
  'review candidates' \
  'diff.truncated=true' \
  '--max-chars 100000' \
  '--to-source <NEW_SOURCE_ID>' \
  'review candidates'; do
  grep -Fq -- "$expected" "${skill_documents[@]}" || {
    printf 'missing using-lwc policy contract: %s\n' "$expected" >&2
    exit 1
  }
done

for expected in \
  'Automatic self-use loop' \
  'Recall budget' \
  'Write-back triggers' \
  'Do not write' \
  'Graph activation recommendation' \
  'Code intelligence recommendation' \
  'use its read commands proactively' \
  'ask for consent before `cg init`' \
  'cg sync' \
  'single-file' \
  'literal edit' \
  'current dirty or uncommitted code' \
  'checked-out code' \
  'current implementation evidence' \
  'config show' \
  'Markdown conversion recommendation' \
  'config set --trans anydoc' \
  'config set --trans markitdown' \
  'trans INPUT --output OUTPUT.md' \
  'config set --office officecli' \
  'lwc office COMMAND' \
  'config set --graph grafeo' \
  'config set --graph surrealdb'; do
  grep -Fq -- "$expected" "${skill_documents[@]}" || {
    printf 'missing automatic LWC guidance: %s\n' "$expected" >&2
    exit 1
  }
done

for expected in \
  'Command families' \
  'Graph engine and document-granular Work' \
  'Optional Markdown conversion' \
  'config set --trans anydoc' \
  'config set --trans markitdown' \
  'trans INPUT --output OUTPUT.md' \
  'Project code intelligence' \
  'Route questions deliberately' \
  'Use the three LWC planes together' \
  'single-file literal' \
  'current dirty or uncommitted code' \
  'checked-out source is the current' \
  'Structured failure recovery' \
  'Safe recipes'; do
  grep -Fq -- "$expected" "$manual" || {
    printf 'missing LWC operations manual contract: %s\n' "$expected" >&2
    exit 1
  }
done

for expected in \
  'project code intelligence' \
  'inspect current code structurally'; do
  grep -Fq -- "$expected" "$metadata" || {
    printf 'missing CodeGraph skill metadata: %s\n' "$expected" >&2
    exit 1
  }
done

for document in "$skill" "$policy"; do
  for expected in \
    'changeset begin' \
    '--changeset <NAME>' \
    'changeset show' \
    'changeset commit' \
    'changeset discard' \
    'changeset rollback' \
    'changeset_conflict' \
    'changeset_frozen' \
    '--allow-lint-issues'; do
    grep -Fq -- "$expected" "$document" || {
      printf 'missing changeset policy contract in %s: %s\n' "$document" "$expected" >&2
      exit 1
    }
  done
done

for document in "$repo_root/docs/agent-workflow.md"; do
  for expected in \
    'changeset begin' \
    '--changeset' \
    'changeset show' \
    'changeset commit' \
    'changeset discard' \
    'changeset rollback'; do
    grep -Fq -- "$expected" "$document" || {
      printf 'missing changeset documentation contract in %s: %s\n' "$document" "$expected" >&2
      exit 1
    }
  done
done

if grep -ERn 'using_lwc_(bootstrap|policy)\.sh' "$repo_root/.github/workflows" >/dev/null; then
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

printf 'using-lwc policy tests: passed\n'
