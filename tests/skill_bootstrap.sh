#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/lwc-skill-bootstrap.XXXXXX")"
system_tmp_root=""
skill_fixture="$test_root/using-lwc"
cp -R "$repo_root/skills/using-lwc" "$skill_fixture"
bootstrap="$skill_fixture/scripts/bootstrap.sh"

cleanup() {
  find "$test_root" -depth -delete
  if [ -n "$system_tmp_root" ]; then
    find "$system_tmp_root" -depth -delete
  fi
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
  local source="${2:-$test_root/mock-lwc}"
  mkdir -p "$(dirname "$destination")"
  cp "$source" "$destination"
  chmod +x "$destination"
}

cat > "$test_root/mock-lwc" <<'MOCK_LWC'
#!/bin/sh
set -eu

printf '%s\n' "$*" >> "${MOCK_LWC_LOG:?}"

if [ "$#" -eq 1 ] && [ "$1" = "--version" ]; then
  printf 'lwc 0.1.2\n'
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
  if [ "${MOCK_SCHEMA_FAIL:-0}" = "1" ]; then
    exit 65
  fi
  cp "$5" "$HOME/applied-global-schema.md"
  printf '{"ok":true}\n'
  exit 0
fi

printf 'unexpected mock lwc arguments: %s\n' "$*" >&2
exit 64
MOCK_LWC
chmod +x "$test_root/mock-lwc"

cat > "$test_root/mock-old-lwc" <<'MOCK_OLD_LWC'
#!/bin/sh
if [ "$#" -eq 1 ] && [ "$1" = "--version" ]; then
  printf 'lwc 0.1.1\n'
  exit 0
fi
exec "${MOCK_LWC_TEMPLATE:?}" "$@"
MOCK_OLD_LWC
chmod +x "$test_root/mock-old-lwc"

cat > "$test_root/mock-installer" <<'MOCK_INSTALLER'
#!/bin/sh
set -eu
: "${LWC_INSTALL_DIR:?}"
: "${MOCK_LWC_TEMPLATE:?}"
printf 'called\n' >> "${MOCK_INSTALLER_LOG:?}"
if [ "${MOCK_INSTALLER_FAIL:-0}" = "1" ]; then
  exit 22
fi
mkdir -p "$LWC_INSTALL_DIR"
cp "$MOCK_LWC_TEMPLATE" "$LWC_INSTALL_DIR/lwc"
chmod +x "$LWC_INSTALL_DIR/lwc"
MOCK_INSTALLER
cp "$test_root/mock-installer" "$skill_fixture/scripts/install-lwc.sh"
chmod +x "$skill_fixture/scripts/install-lwc.sh"

mock_bin="$test_root/mock-bin"
mkdir -p "$mock_bin"
cat > "$mock_bin/curl" <<'MOCK_CURL'
#!/bin/sh
printf 'bootstrap must not download executable shell code\n' >&2
exit 77
MOCK_CURL
chmod +x "$mock_bin/curl"

run_bootstrap() {
  local home="$1"
  local cwd="$2"
  local output="$3"
  local existing_bin="${4:-}"
  local installer_fail="${5:-0}"
  local auto_install="${6:-1}"
  local schema_fail="${7:-0}"
  local case_bin="$home/case-bin"

  mkdir -p "$home" "$home/tmp" "$cwd" "$case_bin"
  case "$existing_bin" in
    old) make_mock_lwc "$case_bin/lwc" "$test_root/mock-old-lwc" ;;
    present) make_mock_lwc "$case_bin/lwc" ;;
  esac

  (
    cd "$cwd"
    env \
      HOME="$home" \
      TMPDIR="$home/tmp" \
      PATH="$case_bin:$mock_bin:/usr/bin:/bin" \
      LWC_AUTO_INSTALL="$auto_install" \
      MOCK_INSTALLER_FAIL="$installer_fail" \
      MOCK_SCHEMA_FAIL="$schema_fail" \
      MOCK_INSTALLER_LOG="$home/installer.log" \
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
test -s "$fresh_home/installer.log" ||
  fail "bundled installer was not used"

# Repeated bootstrap is idempotent and preserves user-edited global policy.
printf 'user purpose\n' > "$fresh_home/applied-global-purpose.md"
printf 'user schema\n' > "$fresh_home/applied-global-schema.md"
: > "$fresh_home/installer.log"
: > "$fresh_home/lwc.log"
run_bootstrap "$fresh_home" "$nested" "$fresh_home/second.json" present
json_assert "$fresh_home/second.json" \
  'value["installed"] is False and value["global_initialized"] is False'
test ! -s "$fresh_home/installer.log" || fail "idempotent run called installer"
! grep -Fq -- '--scope global init' "$fresh_home/lwc.log" ||
  fail "idempotent run reinitialized global Wiki"
test "$(cat "$fresh_home/applied-global-purpose.md")" = "user purpose" ||
  fail "idempotent run overwrote global purpose"
test "$(cat "$fresh_home/applied-global-schema.md")" = "user schema" ||
  fail "idempotent run overwrote global schema"

# An incompatible version is replaced once, then the installed compatible
# binary becomes the selected command.
old_home="$test_root/old-home"
old_cwd="$old_home/notes"
mkdir -p "$old_home/.lwc" "$old_cwd"
touch "$old_home/.lwc/wiki.db"
run_bootstrap "$old_home" "$old_cwd" "$old_home/state.json" old
json_assert "$old_home/state.json" \
  'value["installed"] is True and value["lwc_version"] == "lwc 0.1.2"'
json_assert "$old_home/state.json" \
  'value["lwc_path"].endswith("/.local/bin/lwc")'
test -s "$old_home/installer.log" || fail "incompatible lwc was not replaced"

# A stale PATH entry must not cause a download on every later session.
: > "$old_home/installer.log"
run_bootstrap "$old_home" "$old_cwd" "$old_home/repeat.json" old
json_assert "$old_home/repeat.json" \
  'value["installed"] is False and value["lwc_version"] == "lwc 0.1.2"'
json_assert "$old_home/repeat.json" \
  'value["lwc_path"].endswith("/.local/bin/lwc")'
test ! -s "$old_home/installer.log" ||
  fail "managed compatible lwc was ignored in favor of a stale PATH entry"

# Git Bash installs an .exe; the stable managed path must be recognized.
windows_home="$test_root/windows-home"
windows_cwd="$windows_home/notes"
mkdir -p "$windows_home/.lwc" "$windows_cwd"
touch "$windows_home/.lwc/wiki.db"
make_mock_lwc "$windows_home/.local/bin/lwc.exe"
run_bootstrap "$windows_home" "$windows_cwd" "$windows_home/state.json"
json_assert "$windows_home/state.json" \
  'value["installed"] is False and value["lwc_path"].endswith("/.local/bin/lwc.exe")'
test ! -s "$windows_home/installer.log" ||
  fail "compatible managed lwc.exe triggered an installation"

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

# Machine-readable output remains valid for unusual but legal path names.
json_home="$test_root/json-home"
json_project="$json_home/work/"$'line\nbreak'
mkdir -p "$json_home/.lwc" "$json_project/.git"
touch "$json_home/.lwc/wiki.db"
run_bootstrap "$json_home" "$json_project" "$json_home/state.json" present
json_assert "$json_home/state.json" \
  'value["project_confidence"] == "strong" and "\n" in value["project_root"]'

# A nearby weak candidate must not hide the enclosing strong project root.
shadow_home="$test_root/shadow-home"
shadow_project="$shadow_home/work/repository"
shadow_candidate="$shadow_project/examples/guide"
mkdir -p "$shadow_home/.lwc" "$shadow_project/.git" \
  "$shadow_candidate/docs/deep"
touch "$shadow_home/.lwc/wiki.db" "$shadow_candidate/README.md"
run_bootstrap "$shadow_home" "$shadow_candidate/docs/deep" \
  "$shadow_home/state.json" present
json_assert "$shadow_home/state.json" \
  'value["project_root"].endswith("/work/repository") and value["project_confidence"] == "strong"'
json_assert "$shadow_home/state.json" \
  'value["project_evidence"] == ".git"'

# A weak candidate remains available when no strong ancestor exists.
weak_home="$test_root/weak-home"
weak_project="$weak_home/work/notes"
mkdir -p "$weak_home/.lwc" "$weak_project/docs/deep"
touch "$weak_home/.lwc/wiki.db" "$weak_project/README.md"
run_bootstrap "$weak_home" "$weak_project/docs/deep" \
  "$weak_home/state.json" present
json_assert "$weak_home/state.json" \
  'value["project_root"].endswith("/work/notes") and value["project_confidence"] == "weak"'
json_assert "$weak_home/state.json" \
  'value["suggest_project_init"] is False'

# Never suggest project state in home, Downloads, cache, or incidental folders.
excluded_home="$test_root/excluded-home"
mkdir -p "$excluded_home/.lwc" "$excluded_home/.git" \
  "$excluded_home/Downloads/sample/.git" "$excluded_home/.cache/sample/.git" \
  "$excluded_home/tmp/sample/.git" "$excluded_home/notes"
touch "$excluded_home/.lwc/wiki.db" "$excluded_home/notes/one.md"
for excluded in \
  "$excluded_home" \
  "$excluded_home/Downloads/sample" \
  "$excluded_home/.cache/sample" \
  "$excluded_home/tmp/sample" \
  "$excluded_home/notes"; do
  name="$(printf '%s' "$excluded" | tr '/.' '__')"
  output="$excluded_home/$name.json"
  run_bootstrap "$excluded_home" "$excluded" "$output" present
  json_assert "$output" \
    'value["suggest_project_init"] is False and value["project_root"] == ""'
done

# macOS resolves /var/tmp to /private/var/tmp; both forms stay excluded.
if [ -d /var/tmp ]; then
  system_tmp_root="$(mktemp -d /var/tmp/lwc-skill-system-tmp.XXXXXX)"
  mkdir -p "$system_tmp_root/.git"
  run_bootstrap "$excluded_home" "$system_tmp_root" \
    "$excluded_home/system-tmp.json" present
  json_assert "$excluded_home/system-tmp.json" \
    'value["suggest_project_init"] is False and value["project_root"] == ""'
fi

# Explicit opt-out blocks network installation without touching global data.
optout_home="$test_root/optout-home"
optout_cwd="$optout_home/work"
mkdir -p "$optout_home/.lwc" "$optout_cwd"
printf 'keep me\n' > "$optout_home/.lwc/wiki.db"
if run_bootstrap "$optout_home" "$optout_cwd" "$optout_home/out.json" "" 0 0 \
  2> "$optout_home/error"; then
  fail "LWC_AUTO_INSTALL=0 returned success without a compatible CLI"
fi
grep -Fq 'LWC_AUTO_INSTALL=0' "$optout_home/error" ||
  fail "automatic-install opt-out did not explain the failure"
test "$(cat "$optout_home/.lwc/wiki.db")" = "keep me" ||
  fail "automatic-install opt-out damaged global Wiki"
test ! -e "$optout_home/installer.log" ||
  fail "automatic-install opt-out invoked the installer"

# A policy write failure remains recoverable without overwriting an unrelated
# pre-existing global Wiki.
recovery_home="$test_root/recovery-home"
recovery_cwd="$recovery_home/work"
mkdir -p "$recovery_home"
if run_bootstrap "$recovery_home" "$recovery_cwd" \
  "$recovery_home/failed.json" present 0 1 1 \
  2> "$recovery_home/error"; then
  fail "failed global schema write returned success"
fi
test -f "$recovery_home/.lwc/wiki.db" ||
  fail "recoverable global initialization did not retain its database"
run_bootstrap "$recovery_home" "$recovery_cwd" \
  "$recovery_home/recovered.json" present
json_assert "$recovery_home/recovered.json" \
  'value["global_initialized"] is True'
cmp "$repo_root/skills/using-lwc/assets/global-purpose.md" \
  "$recovery_home/applied-global-purpose.md"
cmp "$repo_root/skills/using-lwc/assets/global-schema.md" \
  "$recovery_home/applied-global-schema.md"

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
