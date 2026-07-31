#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bootstrap="$repo_root/skills/using-lwc/scripts/bootstrap.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/using-lwc-bootstrap-test.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

home="$test_root/home"
outer="$home/work"
project="$outer/project"
nested="$project/src"
outside="$home/outside"
mock_bin="$test_root/bin"
runtime_tmp="$test_root/runtime-tmp"
mkdir -p "$outer/.lwc" "$nested" "$outside" "$mock_bin" "$runtime_tmp"
: > "$outer/.lwc/wiki.db"

cat > "$mock_bin/lwc" <<'MOCK_LWC'
#!/bin/sh
case "$*" in
  "--version")
    printf 'lwc 0.2.0\n'
    ;;
  "init --help")
    printf '%s\n' '--scope'
    ;;
  "--help")
    printf '%s\n' 'Set LWC_PROJECT_ROOT to bound project discovery.'
    ;;
  "--scope global init")
    mkdir -p "$HOME/.lwc"
    : > "$HOME/.lwc/wiki.db"
    ;;
  "--scope global purpose set "*|"--scope global schema set "*)
    ;;
  *)
    printf 'unexpected lwc call: %s\n' "$*" >&2
    exit 2
    ;;
esac
MOCK_LWC
chmod +x "$mock_bin/lwc"

project_root="$(cd "$project" && pwd -P)"
output="$(
  cd "$nested"
  HOME="$home" \
    PATH="$mock_bin:$PATH" \
    TMPDIR="$runtime_tmp" \
    LWC_AUTO_INSTALL=0 \
    LWC_PROJECT_ROOT="$project" \
    sh "$bootstrap"
)"

case "$output" in
  *"\"project_wiki\":\"\""*) ;;
  *) fail "bootstrap reused an ancestor Wiki outside LWC_PROJECT_ROOT: $output" ;;
esac
case "$output" in
  *"\"project_root\":\"$project_root\""*) ;;
  *) fail "bootstrap did not keep the configured project root: $output" ;;
esac

mkdir -p "$project/.lwc" "$nested/.lwc"
: > "$project/.lwc/wiki.db"
: > "$nested/.lwc/wiki.db"
conflict="$(
  cd "$nested"
  HOME="$home" \
    PATH="$mock_bin:$PATH" \
    TMPDIR="$runtime_tmp" \
    LWC_AUTO_INSTALL=0 \
    LWC_PROJECT_ROOT="$project" \
    sh "$bootstrap"
)"
case "$conflict" in
  *"\"project_wiki\":\"\""*"\"scope_conflict\":true"*) ;;
  *) fail "bootstrap selected one of multiple in-scope Wikis: $conflict" ;;
esac

if (
  cd "$outside"
  HOME="$home" \
    PATH="$mock_bin:$PATH" \
    TMPDIR="$runtime_tmp" \
    LWC_AUTO_INSTALL=0 \
    LWC_PROJECT_ROOT="$project" \
    sh "$bootstrap"
) >"$test_root/outside.out" 2>"$test_root/outside.err"; then
  fail "bootstrap accepted a working directory outside LWC_PROJECT_ROOT"
fi

if (
  cd "$home"
  HOME="$home" \
    PATH="$mock_bin:$PATH" \
    TMPDIR="$runtime_tmp" \
    LWC_AUTO_INSTALL=0 \
    LWC_PROJECT_ROOT="$home" \
    sh "$bootstrap"
) >"$test_root/home.out" 2>"$test_root/home.err"; then
  fail "bootstrap accepted the home directory as LWC_PROJECT_ROOT"
fi

printf 'using-lwc bootstrap tests: 4 passed\n'
