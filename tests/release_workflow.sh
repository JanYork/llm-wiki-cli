#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="$repo_root/.github/workflows/release.yml"

if ! grep -Fq 'printf '\''%s\n'\'' "$notes" > "${RUNNER_TEMP}/release-notes.md"' "$workflow"; then
  echo 'release workflow must persist the validated annotation' >&2
  exit 1
fi

if ! grep -Fq -- '--notes-file "${RUNNER_TEMP}/release-notes.md"' "$workflow"; then
  echo 'release workflow must publish the persisted annotation' >&2
  exit 1
fi

if grep -Fq -- '--notes-from-tag' "$workflow"; then
  echo 'release workflow must publish the already validated annotation via --notes-file' >&2
  exit 1
fi

grep -Fq 'node --test tests/npm_package.mjs' "$workflow" || {
  echo 'release workflow must test the npm package' >&2
  exit 1
}
grep -Fq 'external_graph_rebuild_and_update_are_document_granular' "$workflow" || {
  echo 'release workflow must run the current graph benchmark' >&2
  exit 1
}
grep -Fq 'Smoke npm installer against release assets' "$workflow" || {
  echo 'release workflow must smoke the npm installer after asset publication' >&2
  exit 1
}
if grep -Fq 'npm publish' "$workflow"; then
  echo 'npm publication must remain a local maintainer action' >&2
  exit 1
fi

echo 'release workflow contract: PASS'
