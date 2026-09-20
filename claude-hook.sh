#!/usr/bin/env bash
# herdr-pacer — claude-hook.sh
# Wraps (or unwraps) the Claude Code statusLine with claude-statusline.sh, which
# passes the line through untouched — cc-pacer keeps drawing it — and reports the
# session's context window to Herdr for the agent's sidebar row.
#
# Usage: ./claude-hook.sh install | remove | status
# The previous command is carried inside the new one, so `remove` restores it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
SETTINGS="${CLAUDE_SETTINGS_PATH:-$HOME/.claude/settings.json}"
WRAPPER="$ROOT/claude-statusline.sh"

command -v jq > /dev/null || { echo "jq is required" >&2; exit 1; }
mkdir -p "$(dirname "$SETTINGS")"
[[ -f $SETTINGS ]] || printf '{}\n' > "$SETTINGS"

current=$(jq -r '.statusLine.command // ""' "$SETTINGS")
inner=$(sed -n "s|^HERDR_PACER_STATUSLINE='\(.*\)' '.*claude-statusline.sh'$|\1|p" <<< "$current")
wrapped=0
[[ $current == *claude-statusline.sh* ]] && wrapped=1

case "${1:-status}" in
  status)
    if (( wrapped )); then echo "installed; inner command: ${inner:-<none>}"
    else echo "not installed; current command: ${current:-<none>}"; fi
    ;;
  install)
    if (( wrapped )); then echo "already installed (inner: ${inner:-<none>})"; exit 0; fi
    backup=$(mktemp "$SETTINGS.bak-$(date +%Y%m%d-%H%M%S).XXXXXX")
    cp "$SETTINGS" "$backup"
    command="HERDR_PACER_STATUSLINE='$current' '$WRAPPER'"
    jq --arg cmd "$command" \
      '.statusLine = ((.statusLine // {}) + { type: "command", command: $cmd })' \
      "$SETTINGS" > "$SETTINGS.tmp" && mv -f "$SETTINGS.tmp" "$SETTINGS"
    echo "installed (backup: $backup)"
    echo "  statusLine still runs: ${current:-<nothing; a minimal ctx line is printed>}"
    ;;
  remove)
    if (( ! wrapped )); then echo "not installed, nothing to remove"; exit 0; fi
    backup=$(mktemp "$SETTINGS.bak-$(date +%Y%m%d-%H%M%S).XXXXXX")
    cp "$SETTINGS" "$backup"
    if [[ -n $inner ]]; then
      jq --arg cmd "$inner" '.statusLine.command = $cmd' "$SETTINGS" > "$SETTINGS.tmp"
    else
      jq 'del(.statusLine)' "$SETTINGS" > "$SETTINGS.tmp"
    fi
    mv -f "$SETTINGS.tmp" "$SETTINGS"
    echo "removed (backup: $backup); statusLine: ${inner:-<none>}"
    ;;
  *) echo "usage: ${0##*/} install | remove | status" >&2; exit 2 ;;
esac
