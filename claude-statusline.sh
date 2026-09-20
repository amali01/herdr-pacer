#!/usr/bin/env bash
# herdr-pacer — claude-statusline.sh
# A pass-through wrapper for the Claude Code statusLine. The inner command still
# draws the line — cc-pacer, unchanged — and on the way past we report this
# pane's context-window usage to Herdr, so the agent's sidebar row can show it.
#
# claude-hook.sh installs it as:
#   HERDR_PACER_STATUSLINE='<the previous command>' /path/to/claude-statusline.sh
set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
# shellcheck source=collect.sh
source ./collect.sh

INNER="${HERDR_PACER_STATUSLINE:-}"
export HERDR_PACER_CTX_TTL_MS="${HERDR_PACER_CTX_TTL_MS:-300000}"   # the line refreshes every minute

payload=$(cat)

if [[ -n ${HERDR_PANE_ID:-} ]] && command -v jq > /dev/null 2>&1; then
  pct=$(jq -r '.context_window.used_percentage // empty' <<< "$payload" 2>/dev/null |
        awk 'NF {printf "%.0f", $1}')
  # never make Claude wait on the Herdr socket for its status line
  [[ -n $pct ]] && ( ctx_report "$HERDR_PANE_ID" "$pct" & ) > /dev/null 2>&1
fi

if [[ -n $INNER ]]; then
  printf '%s' "$payload" | eval "$INNER"
else
  printf '%s' "$payload" | jq -r '"ctx \(.context_window.used_percentage // 0 | round)%"' 2>/dev/null
fi
