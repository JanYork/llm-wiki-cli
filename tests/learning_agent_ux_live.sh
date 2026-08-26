#!/bin/sh
set -eu

test $# -eq 2 || {
  printf 'usage: learning_agent_ux_live.sh HOST REMOTE_ROOT\n' >&2
  exit 2
}

host=$1
remote_root=$2
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output=$(mktemp -d /tmp/lwc-agent-ux-live.XXXXXX)
printf 'live evidence directory: %s\n' "$output"

python3 "$root/tests/learning_agent_ux_rpc.py" "$host" "$remote_root" "$output"
ssh -6 "$host" "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:\$PATH; export PATH; mkdir -p '$remote_root/acceptance'"
rsync -a "$root/tests/learning_agent_ux_check.py" "$output/" "$host:$remote_root/acceptance/"
ssh -6 "$host" "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:\$PATH; export PATH; python3 '$remote_root/acceptance/learning_agent_ux_check.py' no-code-init '$remote_root/acceptance/no_code_init.jsonl' && python3 '$remote_root/acceptance/learning_agent_ux_check.py' steady-state '$remote_root/acceptance/steady_state.jsonl'"

printf 'live evidence retained locally at %s and remotely at %s/acceptance\n' "$output" "$remote_root"
