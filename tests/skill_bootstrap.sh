#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bootstrap="$repo_root/skills/using-lwc/scripts/bootstrap.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/lwc-skill-bootstrap.XXXXXX")"

cleanup() {
  find "$test_root" -depth -delete
}
trap cleanup EXIT HUP INT TERM

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

test -f "$bootstrap" || fail "missing $bootstrap"

json_assert() {
  local file="$1"
  local expression="$2"
  python3 - "$file" "$expression" <<'PY'
import json
import sys

path, expression = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    value = json.load(handle)

required = {
    "lwc_path",
    "lwc_version",
    "installed",
    "global_wiki",
    "global_initialized",
    "project_wiki",
    "project_root",
    "project_confidence",
    "project_evidence",
    "suggest_project_init",
}
missing = required.difference(value)
if missing:
    raise SystemExit(f"missing JSON keys: {sorted(missing)}")
if not eval(expression, {"__builtins__": {}}, {"value": value}):
    raise SystemExit(f"JSON assertion failed: {expression}\nvalue={value!r}")
PY
}

make_mock_lwc() {
  local destination="$1"
  mkdir -p "$(dirname "$destination")"
  cp "$test_root/mock-lwc" "$destination"
  chmod +x "$destination"
}

cat > "$test_root/mock-lwc" <<'MOCK_LWC'
#!/bin/sh
set -eu

printf '%s\n' "$*" >> "${MOCK_LWC_LOG:?}"

if [ "$#" -eq 1 ] && [ "$1" = "--version" ]; then
  printf 'lwc 0.1.1\n'
  exit 0
fi

if [ "$#" -eq 2 ] && [ "$1" = "init" ] && [ "$2" = "--help" ]; then
  printf 'Usage: lwc init --scope <SCOPE>\n'
  exit 0
fi

if [ "$#" -eq 3 ] && [ "$1" = "--scope" ] && [ "$2" = "global" ] &&
  [ "$3" = "init" ]; then
  mkdir -p "$HOME/.lwc"
  printf 'mock database\n' > "$HOME/.lwc/wiki.db"
  printf '{"ok":true}\n'
  exit 0
fi

if [ "$#" -eq 5 ] && [ "$1" = "--scope" ] && [ "$2" = "global" ] &&
  [ "$3" = "purpose" ] && [ "$4" = "set" ]; then
  cp "$5" "$HOME/applied-global-purpose.md"
  printf '{"ok":true}\n'
  exit 0
fi

if [ "$#" -eq 5 ] && [ "$1" = "--scope" ] && [ "$2" = "global" ] &&
  [ "$3" = "schema" ] && [ "$4" = "set" ]; then
  cp "$5" "$HOME/applied-global-schema.md"
  printf '{"ok":true}\n'
  exit 0
fi

printf 'unexpected mock lwc arguments: %s\n' "$*" >&2
exit 64
MOCK_LWC
chmod +x "$test_root/mock-lwc"

cat > "$test_root/mock-installer" <<'MOCK_INSTALLER'
#!/bin/sh
set -eu
: "${LWC_INSTALL_DIR:?}"
: "${MOCK_LWC_TEMPLATE:?}"
mkdir -p "$LWC_INSTALL_DIR"
cp "$MOCK_LWC_TEMPLATE" "$LWC_INSTALL_DIR/lwc"
chmod +x "$LWC_INSTALL_DIR/lwc"
MOCK_INSTALLER

mock_bin="$test_root/mock-bin"
mkdir -p "$mock_bin"
cat > "$mock_bin/curl" <<'MOCK_CURL'
#!/bin/sh
set -eu
printf '%s\n' "$*" >> "${MOCK_CURL_LOG:?}"
if [ "${MOCK_CURL_FAIL:-0}" = "1" ]; then
  exit 22
fi
output=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      output="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
if [ -n "$output" ]; then
  cp "${MOCK_INSTALLER:?}" "$output"
else
  cat "${MOCK_INSTALLER:?}"
fi
MOCK_CURL
chmod +x "$mock_bin/curl"

run_bootstrap() {
  local home="$1"
  local cwd="$2"
  local output="$3"
  local existing_bin="${4:-}"
  local curl_fail="${5:-0}"
  local case_bin="$home/case-bin"

  mkdir -p "$home" "$cwd" "$case_bin"
  if [ -n "$existing_bin" ]; then
    make_mock_lwc "$case_bin/lwc"
  fi

  (
    cd "$cwd"
    env \
      HOME="$home" \
      PATH="$case_bin:$mock_bin:/usr/bin:/bin" \
      MOCK_CURL_FAIL="$curl_fail" \
      MOCK_CURL_LOG="$home/curl.log" \
      MOCK_INSTALLER="$test_root/mock-installer" \
      MOCK_LWC_LOG="$home/lwc.log" \
      MOCK_LWC_TEMPLATE="$test_root/mock-lwc" \
      sh "$bootstrap"
  ) > "$output"
}

# Missing CLI: install, initialize global memory, and suggest—but never create—
# project memory at the strong root.
fresh_home="$test_root/fresh-home"
project="$fresh_home/projects/acme"
nested="$project/src/deep"
mkdir -p "$project/.git" "$nested"
touch "$project/Cargo.toml"
run_bootstrap "$fresh_home" "$nested" "$fresh_home/first.json"
json_assert "$fresh_home/first.json" \
  'value["installed"] is True and value["global_initialized"] is True'
json_assert "$fresh_home/first.json" \
  'value["project_root"].endswith("/projects/acme") and value["project_confidence"] == "strong"'
json_assert "$fresh_home/first.json" \
  'value["suggest_project_init"] is True and value["project_wiki"] == ""'
test -f "$fresh_home/.lwc/wiki.db" || fail "global Wiki was not initialized"
test ! -e "$project/.lwc" || fail "bootstrap initialized project memory without consent"
cmp "$repo_root/skills/using-lwc/assets/global-purpose.md" \
  "$fresh_home/applied-global-purpose.md"
cmp "$repo_root/skills/using-lwc/assets/global-schema.md" \
  "$fresh_home/applied-global-schema.md"
grep -Fq 'releases/latest/download/install.sh' "$fresh_home/curl.log" ||
  fail "official installer URL was not used"

# Repeated bootstrap is idempotent and preserves user-edited global policy.
printf 'user purpose\n' > "$fresh_home/applied-global-purpose.md"
printf 'user schema\n' > "$fresh_home/applied-global-schema.md"
: > "$fresh_home/curl.log"
: > "$fresh_home/lwc.log"
run_bootstrap "$fresh_home" "$nested" "$fresh_home/second.json" present
json_assert "$fresh_home/second.json" \
  'value["installed"] is False and value["global_initialized"] is False'
test ! -s "$fresh_home/curl.log" || fail "idempotent run called installer"
! grep -Fq -- '--scope global init' "$fresh_home/lwc.log" ||
  fail "idempotent run reinitialized global Wiki"
test "$(cat "$fresh_home/applied-global-purpose.md")" = "user purpose" ||
  fail "idempotent run overwrote global purpose"
test "$(cat "$fresh_home/applied-global-schema.md")" = "user schema" ||
  fail "idempotent run overwrote global schema"

# Existing project memory resolves from a nested directory.
existing_home="$test_root/existing-home"
existing_project="$existing_home/work/known"
existing_nested="$existing_project/a/b"
mkdir -p "$existing_home/.lwc" "$existing_project/.git" \
  "$existing_project/.lwc" "$existing_nested"
touch "$existing_home/.lwc/wiki.db" "$existing_project/.lwc/wiki.db"
run_bootstrap "$existing_home" "$existing_nested" "$existing_home/state.json" present
json_assert "$existing_home/state.json" \
  'value["project_wiki"].endswith("/work/known/.lwc/wiki.db")'
json_assert "$existing_home/state.json" \
  'value["suggest_project_init"] is False'

# A recognized manifest is strong evidence even without Git.
manifest_home="$test_root/manifest-home"
manifest_project="$manifest_home/work/app"
mkdir -p "$manifest_home/.lwc" "$manifest_project/lib"
touch "$manifest_home/.lwc/wiki.db" "$manifest_project/package.json"
run_bootstrap "$manifest_home" "$manifest_project/lib" \
  "$manifest_home/state.json" present
json_assert "$manifest_home/state.json" \
  'value["project_confidence"] == "strong" and value["suggest_project_init"] is True'
json_assert "$manifest_home/state.json" \
  '"package.json" in value["project_evidence"]'

# Never suggest project state in home, Downloads, cache, or incidental folders.
excluded_home="$test_root/excluded-home"
mkdir -p "$excluded_home/.lwc" "$excluded_home/.git" \
  "$excluded_home/Downloads/sample/.git" "$excluded_home/.cache/sample/.git" \
  "$excluded_home/notes"
touch "$excluded_home/.lwc/wiki.db" "$excluded_home/notes/one.md"
for excluded in \
  "$excluded_home" \
  "$excluded_home/Downloads/sample" \
  "$excluded_home/.cache/sample" \
  "$excluded_home/notes"; do
  name="$(printf '%s' "$excluded" | tr '/.' '__')"
  output="$excluded_home/$name.json"
  run_bootstrap "$excluded_home" "$excluded" "$output" present
  json_assert "$output" \
    'value["suggest_project_init"] is False and value["project_root"] == ""'
done

# Installation failure is explicit and preserves an existing global Wiki.
failure_home="$test_root/failure-home"
failure_cwd="$failure_home/work/project"
mkdir -p "$failure_home/.lwc" "$failure_cwd/.git"
printf 'keep me\n' > "$failure_home/.lwc/wiki.db"
if run_bootstrap "$failure_home" "$failure_cwd" "$failure_home/out.json" "" 1 \
  2> "$failure_home/error"; then
  fail "failed installer returned success"
fi
test "$(cat "$failure_home/.lwc/wiki.db")" = "keep me" ||
  fail "failed installation damaged global Wiki"
test ! -e "$failure_home/.local/bin/lwc" ||
  fail "failed installation left a partial executable"

printf 'skill bootstrap tests: passed\n'
