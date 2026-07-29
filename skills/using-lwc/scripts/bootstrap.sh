#!/bin/sh
set -eu

installer_url="https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh"

die() {
  printf 'using-lwc bootstrap: %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 ||
    die "required command not found: $1"
}

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

usable_lwc() {
  candidate="$1"
  candidate_version="$("$candidate" --version 2>/dev/null || true)"
  printf '%s\n' "$candidate_version" |
    grep -Eq '^lwc [0-9]+\.[0-9]+\.[0-9]+' || return 1
  "$candidate" init --help 2>&1 | grep -q -- '--scope'
}

add_evidence() {
  if [ -n "$project_evidence" ]; then
    project_evidence="${project_evidence},$1"
  else
    project_evidence="$1"
  fi
}

is_excluded_root() {
  case "$1" in
    "$home_dir"|\
    "$home_dir/Downloads"|"$home_dir/Downloads/"*|\
    "$home_dir/Desktop"|"$home_dir/Desktop/"*|\
    "$home_dir/.cache"|"$home_dir/.cache/"*|\
    "$home_dir/Library/Caches"|"$home_dir/Library/Caches/"*)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

: "${HOME:?using-lwc bootstrap: HOME is not set}"
need dirname
need grep
need sed
need mktemp

skill_dir="$(
  CDPATH= cd "$(dirname "$0")/.." >/dev/null 2>&1
  pwd -P
)"
purpose_file="$skill_dir/assets/global-purpose.md"
schema_file="$skill_dir/assets/global-schema.md"
[ -f "$purpose_file" ] || die "missing $purpose_file"
[ -f "$schema_file" ] || die "missing $schema_file"

home_dir="$(
  CDPATH= cd "$HOME" >/dev/null 2>&1
  pwd -P
)" || die "cannot resolve HOME"
cwd="$(pwd -P)"

installed=false
global_initialized=false
work_dir=""

cleanup() {
  if [ -n "$work_dir" ] && [ -d "$work_dir" ]; then
    rm -rf "$work_dir"
  fi
}
trap cleanup EXIT HUP INT TERM

lwc_path="$(command -v lwc 2>/dev/null || true)"
if [ -z "$lwc_path" ] || ! usable_lwc "$lwc_path"; then
  need curl
  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/using-lwc-install.XXXXXX")" ||
    die "cannot create temporary directory"
  installer="$work_dir/install.sh"
  curl --proto '=https' --tlsv1.2 -fsSL "$installer_url" -o "$installer" ||
    die "failed to download the lwc installer"
  LWC_INSTALL_DIR="$home_dir/.local/bin" sh "$installer" >&2 ||
    die "lwc installation failed"
  lwc_path="$home_dir/.local/bin/lwc"
  usable_lwc "$lwc_path" || die "installed lwc failed its compatibility check"
  installed=true
fi

lwc_version="$("$lwc_path" --version)"
global_wiki="$home_dir/.lwc/wiki.db"
if [ ! -f "$global_wiki" ]; then
  "$lwc_path" --scope global init >/dev/null ||
    die "failed to initialize global memory"
  [ -f "$global_wiki" ] || die "global initialization did not create $global_wiki"
  "$lwc_path" --scope global purpose set "$purpose_file" >/dev/null ||
    die "failed to set global memory purpose"
  "$lwc_path" --scope global schema set "$schema_file" >/dev/null ||
    die "failed to set global memory schema"
  global_initialized=true
fi

project_wiki=""
project_root=""
project_confidence="none"
project_evidence=""
suggest_project_init=false

cursor="$cwd"
while :; do
  if [ "$cursor" = "$home_dir" ]; then
    break
  fi
  if [ -f "$cursor/.lwc/wiki.db" ]; then
    project_wiki="$cursor/.lwc/wiki.db"
    project_root="$cursor"
    project_confidence="existing"
    project_evidence=".lwc/wiki.db"
    break
  fi
  parent="$(dirname "$cursor")"
  [ "$parent" != "$cursor" ] || break
  cursor="$parent"
done

if [ -z "$project_wiki" ]; then
  cursor="$cwd"
  while :; do
    if [ "$cursor" = "$home_dir" ]; then
      break
    fi

    if ! is_excluded_root "$cursor"; then
      project_evidence=""
      strong=false

      if [ -e "$cursor/.git" ]; then
        add_evidence ".git"
        strong=true
      fi

      for marker in \
        Cargo.toml package.json pyproject.toml go.mod pom.xml \
        build.gradle build.gradle.kts Gemfile composer.json; do
        if [ -f "$cursor/$marker" ]; then
          add_evidence "$marker"
          strong=true
        fi
      done

      for marker_path in "$cursor"/*.sln "$cursor"/*.xcodeproj; do
        if [ -e "$marker_path" ]; then
          add_evidence "$(basename "$marker_path")"
          strong=true
        fi
      done

      if [ "$strong" = true ]; then
        project_root="$cursor"
        project_confidence="strong"
        suggest_project_init=true
        break
      fi

      readme=""
      for readme_name in README.md README README.txt README.rst; do
        if [ -f "$cursor/$readme_name" ]; then
          readme="$readme_name"
          break
        fi
      done
      if [ -n "$readme" ]; then
        for content_dir in src docs tests; do
          if [ -d "$cursor/$content_dir" ]; then
            project_root="$cursor"
            project_confidence="weak"
            project_evidence="${readme},${content_dir}/"
            break
          fi
        done
      fi
    fi

    [ -z "$project_root" ] || break
    parent="$(dirname "$cursor")"
    [ "$parent" != "$cursor" ] || break
    cursor="$parent"
  done
fi

printf '{'
printf '"lwc_path":"%s",' "$(json_escape "$lwc_path")"
printf '"lwc_version":"%s",' "$(json_escape "$lwc_version")"
printf '"installed":%s,' "$installed"
printf '"global_wiki":"%s",' "$(json_escape "$global_wiki")"
printf '"global_initialized":%s,' "$global_initialized"
printf '"project_wiki":"%s",' "$(json_escape "$project_wiki")"
printf '"project_root":"%s",' "$(json_escape "$project_root")"
printf '"project_confidence":"%s",' "$(json_escape "$project_confidence")"
printf '"project_evidence":"%s",' "$(json_escape "$project_evidence")"
printf '"suggest_project_init":%s' "$suggest_project_init"
printf '}\n'
