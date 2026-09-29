#!/bin/sh
# Install the current user's native supervisor; does not touch any memory store.
set -eu
lwc_executable=${1:-$(command -v lwc)}
case "$lwc_executable" in /*) ;; *) echo 'Pass an absolute LWC executable path.' >&2; exit 1;; esac
[ -x "$lwc_executable" ] || { echo 'Executable unavailable.' >&2; exit 1; }
# Service configuration must survive the calling shell and must never contain tokens.
lwc_credentials=${LWC_TEAM_CREDENTIALS_FILE:-}
case "$lwc_credentials" in ''|/*) ;; *) echo 'Delegated credential path must be absolute.' >&2; exit 1;; esac
case "$(uname -s)" in
Darwin)
    lwc_label=io.lwc.team-sync
    [ -z "$lwc_credentials" ] || lwc_label="$lwc_label.$(printf '%s' "$lwc_credentials" | shasum -a 256 | cut -c1-16)"
    lwc_target="$HOME/Library/LaunchAgents/$lwc_label.plist"
    mkdir -p "$HOME/Library/LaunchAgents"
    lwc_service=$(mktemp "$HOME/Library/LaunchAgents/.lwc-sync.XXXXXX")
    trap 'rm -f "$lwc_service"' EXIT HUP INT TERM
    # plutil serializes values, so spaces and XML characters in paths are safe.
    /usr/bin/plutil -create xml1 "$lwc_service"
    /usr/bin/plutil -insert Label -string "$lwc_label" "$lwc_service"
    /usr/bin/plutil -insert ProgramArguments -json '[]' "$lwc_service"
    /usr/bin/plutil -insert ProgramArguments.0 -string "$lwc_executable" "$lwc_service"
    /usr/bin/plutil -insert ProgramArguments.1 -string space "$lwc_service"
    /usr/bin/plutil -insert ProgramArguments.2 -string supervise "$lwc_service"
    /usr/bin/plutil -insert EnvironmentVariables -json '{}' "$lwc_service"
    /usr/bin/plutil -insert EnvironmentVariables.HOME -string "$HOME" "$lwc_service"
    [ -z "$lwc_credentials" ] || /usr/bin/plutil -insert EnvironmentVariables.LWC_TEAM_CREDENTIALS_FILE -string "$lwc_credentials" "$lwc_service"
    /usr/bin/plutil -insert RunAtLoad -bool YES "$lwc_service"
    /usr/bin/plutil -insert KeepAlive -bool YES "$lwc_service"
    /usr/bin/plutil -insert ThrottleInterval -integer 10 "$lwc_service"
    chmod 600 "$lwc_service"
    mv "$lwc_service" "$lwc_target"
    trap - EXIT HUP INT TERM
    lwc_service="$lwc_target"
    launchctl bootout "gui/$(id -u)/$lwc_label" 2>/dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$lwc_service"
    printf 'Installed %s\n' "$lwc_service"
    ;;
Linux)
    # systemd quoted values escape quotes, backslashes and specifier/variable expansion.
    quote_unit() { printf '%s' "$1" | sed 's/\\/\\\\/g;s/"/\\"/g;s/%/%%/g;s/\$/$$/g'; }
    case "$lwc_executable$lwc_credentials" in *'
'*) echo 'Service paths cannot contain newlines.' >&2; exit 1;; esac
    lwc_label=lwc-team-sync
    [ -z "$lwc_credentials" ] || lwc_label="$lwc_label-$(printf '%s' "$lwc_credentials" | sha256sum | cut -c1-16)"
    lwc_service="$HOME/.config/systemd/user/$lwc_label.service"
    mkdir -p "$(dirname "$lwc_service")"
    umask 077
    {
        printf '[Unit]\nDescription=LWC automatic team memory synchronization\nAfter=network-online.target\n\n[Service]\n'
        printf 'ExecStart="%s" space supervise\n' "$(quote_unit "$lwc_executable")"
        [ -z "$lwc_credentials" ] || printf 'Environment="LWC_TEAM_CREDENTIALS_FILE=%s"\n' "$(quote_unit "$lwc_credentials")"
        printf 'Restart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n'
    } > "$lwc_service"
    systemctl --user daemon-reload
    systemctl --user enable --now "$lwc_label.service"
    printf 'Installed %s\n' "$lwc_service"
    ;;
*) echo 'Use install-sync-service.ps1 on Windows.' >&2; exit 1;;
esac
