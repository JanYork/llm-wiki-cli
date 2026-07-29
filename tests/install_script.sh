#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
installer="$repo_root/install.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/lwc-installer-test.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local expected="$2"
  grep -Fq "$expected" "$file" || fail "$file does not contain: $expected"
}

command -v zip >/dev/null || fail "zip is required"
command -v unzip >/dev/null || fail "unzip is required"

release_dir="$test_root/release"
mock_bin="$test_root/bin"
mkdir -p "$release_dir" "$mock_bin"

targets=(
  x86_64-apple-darwin
  aarch64-apple-darwin
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  x86_64-pc-windows-msvc
  aarch64-pc-windows-msvc
)

for target in "${targets[@]}"; do
  archive="lwc-0.1.0-$target"
  stage="$release_dir/$archive"
  binary="lwc"
  extension="tar.gz"
  if [[ "$target" == *windows* ]]; then
    binary="lwc.exe"
    extension="zip"
  fi

  mkdir -p "$stage"
  printf '#!/bin/sh\nprintf "lwc 0.1.0\\n"\n' > "$stage/$binary"
  chmod +x "$stage/$binary"
  cp "$repo_root/README.md" "$repo_root/README.zh-CN.md" "$repo_root/LICENSE" "$stage/"

  if [[ "$extension" == "zip" ]]; then
    (cd "$release_dir" && zip -qr "$archive.zip" "$archive")
  else
    tar -C "$release_dir" -czf "$release_dir/$archive.tar.gz" "$archive"
  fi
  rm -rf "$stage"
done

(
  cd "$release_dir"
  for archive in *.tar.gz *.zip; do
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "$archive"
    else
      shasum -a 256 "$archive"
    fi
  done | sort -k2 > SHA256SUMS
)

cat > "$mock_bin/uname" <<'MOCK_UNAME'
#!/bin/sh
case "$1" in
  -s) printf '%s\n' "$MOCK_OS" ;;
  -m) printf '%s\n' "$MOCK_ARCH" ;;
  *) exit 2 ;;
esac
MOCK_UNAME

cat > "$mock_bin/curl" <<'MOCK_CURL'
#!/bin/sh
output=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      output="$2"
      shift 2
      ;;
    http://*|https://*)
      url="$1"
      shift
      ;;
    *)
      shift
      ;;
  esac
done

printf '%s\n' "$url" >> "$MOCK_CURL_LOG"
case "$url" in
  */releases/latest)
    printf '%s' "https://github.com/JanYork/llm-wiki-cli/releases/tag/v0.1.0"
    ;;
  */releases/download/v0.1.0/*)
    cp "$MOCK_RELEASE_DIR/${url##*/}" "$output"
    ;;
  *)
    printf 'unexpected URL: %s\n' "$url" >&2
    exit 22
    ;;
esac
MOCK_CURL

chmod +x "$mock_bin/uname" "$mock_bin/curl"

run_installer() {
  local os="$1"
  local arch="$2"
  local install_dir="$3"
  local release_fixture="${4:-$release_dir}"
  local log="$5"

  env \
    PATH="$mock_bin:$PATH" \
    HOME="$test_root/home" \
    LWC_INSTALL_DIR="$install_dir" \
    MOCK_OS="$os" \
    MOCK_ARCH="$arch" \
    MOCK_RELEASE_DIR="$release_fixture" \
    MOCK_CURL_LOG="$log" \
    sh "$installer"
}

matrix=(
  "Darwin|x86_64|x86_64-apple-darwin|lwc|tar.gz"
  "Darwin|arm64|aarch64-apple-darwin|lwc|tar.gz"
  "Linux|x86_64|x86_64-unknown-linux-gnu|lwc|tar.gz"
  "Linux|aarch64|aarch64-unknown-linux-gnu|lwc|tar.gz"
  "MINGW64_NT-10.0|x86_64|x86_64-pc-windows-msvc|lwc.exe|zip"
  "MSYS_NT-10.0|arm64|aarch64-pc-windows-msvc|lwc.exe|zip"
)

case_number=0
for entry in "${matrix[@]}"; do
  IFS='|' read -r os arch target binary extension <<< "$entry"
  case_number=$((case_number + 1))
  case_dir="$test_root/case-$case_number"
  install_dir="$case_dir/install"
  log="$case_dir/curl.log"
  mkdir -p "$case_dir"

  run_installer "$os" "$arch" "$install_dir" "$release_dir" "$log" > "$case_dir/out"
  test -x "$install_dir/$binary" || fail "missing installed binary for $os/$arch"
  test "$("$install_dir/$binary" --version)" = "lwc 0.1.0" || fail "wrong installed version"
  assert_contains "$log" "lwc-0.1.0-$target.$extension"
done

update_dir="$test_root/update/install"
update_log="$test_root/update/curl.log"
mkdir -p "$update_dir"
printf '#!/bin/sh\nprintf "lwc 0.0.0\\n"\n' > "$update_dir/lwc"
chmod +x "$update_dir/lwc"
run_installer Darwin x86_64 "$update_dir" "$release_dir" "$update_log" > "$test_root/update/out"
test "$("$update_dir/lwc" --version)" = "lwc 0.1.0" || fail "outdated binary was not updated"
assert_contains "$test_root/update/out" "Updated lwc"

: > "$update_log"
run_installer Darwin x86_64 "$update_dir" "$release_dir" "$update_log" > "$test_root/current.out"
test "$(wc -l < "$update_log" | tr -d ' ')" -eq 1 || fail "current version downloaded assets"
assert_contains "$test_root/current.out" "already installed"

bad_release="$test_root/bad-release"
cp -R "$release_dir" "$bad_release"
bad_asset="lwc-0.1.0-x86_64-apple-darwin.tar.gz"
awk -v asset="$bad_asset" '
  $2 == asset { $1 = "0000000000000000000000000000000000000000000000000000000000000000" }
  { print }
' "$bad_release/SHA256SUMS" > "$bad_release/SHA256SUMS.tmp"
mv "$bad_release/SHA256SUMS.tmp" "$bad_release/SHA256SUMS"
printf '#!/bin/sh\nprintf "lwc 0.0.0\\n"\n' > "$update_dir/lwc"
chmod +x "$update_dir/lwc"
if run_installer Darwin x86_64 "$update_dir" "$bad_release" "$test_root/bad.log" \
  > "$test_root/bad.out" 2> "$test_root/bad.err"; then
  fail "checksum mismatch succeeded"
fi
assert_contains "$test_root/bad.err" "Checksum verification failed"
test "$("$update_dir/lwc" --version)" = "lwc 0.0.0" || fail "checksum failure replaced existing binary"

if run_installer FreeBSD x86_64 "$test_root/unsupported-os" "$release_dir" "$test_root/os.log" \
  > "$test_root/os.out" 2> "$test_root/os.err"; then
  fail "unsupported OS succeeded"
fi
assert_contains "$test_root/os.err" "Unsupported operating system"

if run_installer Linux riscv64 "$test_root/unsupported-arch" "$release_dir" "$test_root/arch.log" \
  > "$test_root/arch.out" 2> "$test_root/arch.err"; then
  fail "unsupported architecture succeeded"
fi
assert_contains "$test_root/arch.err" "Unsupported architecture"

printf 'install script tests: 11 passed\n'
