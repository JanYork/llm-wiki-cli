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

echo 'release workflow contract: PASS'
