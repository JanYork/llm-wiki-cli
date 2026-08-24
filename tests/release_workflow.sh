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
test "$(grep -c '^            target:' "$workflow")" -eq 6 || {
  echo 'release workflow must retain the six supported targets' >&2
  exit 1
}
grep -Fq 'cargo build --locked --release --target ${{ matrix.target }} --bins' "$workflow" || {
  echo 'release workflow must build core and all fixed learning binaries' >&2
  exit 1
}
grep -Fq 'for plugin in tutor book practice; do' "$workflow" || {
  echo 'release workflow must package all three fixed learning runtimes' >&2
  exit 1
}
grep -Fq 'archive="lwc-${plugin}-${version}-${{ matrix.target }}"' "$workflow" || {
  echo 'Unix learning archive names must match the lazy runtime contract' >&2
  exit 1
}
grep -Fq 'cp "target/${{ matrix.target }}/release/lwc-${plugin}" "dist/${archive}/"' "$workflow" || {
  echo 'Unix learning archives must contain their single fixed binary' >&2
  exit 1
}
grep -Fq '$archive = "lwc-$plugin-$version-${{ matrix.target }}"' "$workflow" || {
  echo 'Windows learning archive names must match the lazy runtime contract' >&2
  exit 1
}
grep -Fq 'Copy-Item "target\${{ matrix.target }}\release\lwc-$plugin.exe" $stage' "$workflow" || {
  echo 'Windows learning archives must contain their single fixed binary' >&2
  exit 1
}
grep -Fq 'cp README.md README.zh-CN.md LICENSE "dist/${archive}/"' "$workflow" || {
  echo 'the core archive contract must remain unchanged' >&2
  exit 1
}
for document in README.md README.zh-CN.md docs/agent-workflow.md docs/learning-suite-contracts.md; do
  for plugin in tutor book practice; do
    grep -Fq -- "lwc --scope global config set --$plugin enabled" "$repo_root/$document" || {
      echo "$document must document global $plugin enablement" >&2
      exit 1
    }
  done
done
grep -Fq -- 'HTML, scanned/OCR PDFs, MOBI,' "$repo_root/README.md" || {
  echo 'README must state the Book format boundary' >&2
  exit 1
}
grep -Fq -- 'v1 provides no forget/clear/purge command' "$repo_root/README.md" || {
  echo 'README must state the deferred purge boundary' >&2
  exit 1
}
grep -Fq -- '意图含糊时只直接询问' "$repo_root/README.zh-CN.md" || {
  echo 'Chinese README must state the Agent ask boundary' >&2
  exit 1
}
if grep -Fq 'npm publish' "$workflow"; then
  echo 'npm publication must remain a local maintainer action' >&2
  exit 1
fi

echo 'release workflow contract: PASS'
