#!/usr/bin/env bash
# herdr-pacer — pacer-panes.sh
# Context-window bars for the agents that cannot report one themselves.
#
#   codex     — its composer footer prints "Context 72% left"; `herdr pane read`
#               gives us that line, which is the same number the TUI shows.
#   opencode  — its SQLite holds the last turn's token total and model; divide
#               by that model's context limit from opencode's models cache.
#
# Claude Code reports its own from claude-statusline.sh. Herdr runs this on
# startup and on every agent status change, so a bar refreshes when a turn ends.
#
# usage: pacer-panes.sh sweep | pane <pane_id> [agent] | event
set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
QUIT=0
# shellcheck source=collect.sh
source ./collect.sh

HERDR="${HERDR_BIN_PATH:-herdr}"
OC_DB="${HERDR_PACER_OPENCODE_DB:-${XDG_DATA_HOME:-$HOME/.local/share}/opencode/opencode.db}"
OC_MODELS="${HERDR_PACER_OPENCODE_MODELS:-${XDG_CACHE_HOME:-$HOME/.cache}/opencode/models.json}"
SETTLE="${HERDR_PACER_EVENT_SETTLE:-1.5}"     # let the TUI repaint before reading

sq() { printf "%s" "${1//\'/\'\'}"; }          # single-quote escape for sqlite

ctx_codex() { # pane_id -> used percent
  local text left
  text=$("$HERDR" pane read "$1" --source visible --lines 40 2>/dev/null) || return 1
  left=$(grep -oE 'Context[[:space:]]+[0-9]+%[[:space:]]+left' <<< "$text" | tail -1 |
         grep -oE '[0-9]+' | head -1)
  [[ -n $left ]] || return 1
  printf '%s' $(( 100 - left ))
}

ctx_opencode() { # cwd -> used percent
  local cwd=$1 session row total model provider limit
  [[ -f $OC_DB && -f $OC_MODELS ]] || return 1
  command -v sqlite3 > /dev/null 2>&1 || return 1

  session=$(sqlite3 "file:$OC_DB?mode=ro" \
    "select id from session where directory='$(sq "$cwd")' order by time_updated desc limit 1" 2>/dev/null)
  [[ -n $session ]] || return 1

  # newest turn that actually carries a token total
  row=$(sqlite3 -json "file:$OC_DB?mode=ro" \
    "select data from message where session_id='$(sq "$session")' order by time_created desc limit 8" 2>/dev/null |
    jq -r '[.[].data | fromjson? | select((.tokens.total // 0) > 0)][0] // empty' 2>/dev/null)
  [[ -n $row ]] || return 1

  total=$(jq -r '.tokens.total | floor' <<< "$row" 2>/dev/null)
  provider=$(jq -r '.providerID // empty' <<< "$row" 2>/dev/null)
  model=$(jq -r '.modelID // empty' <<< "$row" 2>/dev/null)
  limit=$(jq -r --arg p "$provider" --arg m "$model" \
    '.[$p].models[$m].limit.context // empty' "$OC_MODELS" 2>/dev/null)
  [[ -n $total && -n $limit && $limit -gt 0 ]] || return 1

  printf '%s' $(( (total * 100 + limit / 2) / limit ))
}

pane_cwd() {
  "$HERDR" pane get "$1" 2>/dev/null |
    jq -r '(.result.pane // .result) | .foreground_cwd // .cwd // empty' 2>/dev/null
}

update_pane() { # pane_id [agent]
  local pane=$1 agent=${2:-} pct=""
  [[ -n $agent ]] || agent=$("$HERDR" pane get "$pane" 2>/dev/null |
    jq -r '(.result.pane // .result).agent // empty' 2>/dev/null)
  case "$agent" in
    codex)    pct=$(ctx_codex "$pane") ;;
    opencode) pct=$(ctx_opencode "$(pane_cwd "$pane")") ;;
    *) return 0 ;;                       # claude reports its own; others have none
  esac
  if [[ -n $pct ]]; then ctx_report "$pane" "$pct"; else ctx_clear "$pane"; fi
}

sweep() {
  local pane agent
  while IFS=$'\t' read -r pane agent; do
    [[ -n $pane ]] || continue
    update_pane "$pane" "$agent"
  done < <("$HERDR" agent list 2>/dev/null |
    jq -r '.result.agents[]? | select(.agent == "codex" or .agent == "opencode") |
           "\(.pane_id)\t\(.agent)"' 2>/dev/null)
}

case "${1:-sweep}" in
  sweep) sweep ;;
  pane)  update_pane "${2:?pane id}" "${3:-}" ;;
  event)
    pane=$(jq -r '.pane_id // empty' <<< "${HERDR_PLUGIN_EVENT_JSON:-}" 2>/dev/null)
    [[ -n $pane ]] || exit 0
    sleep "$SETTLE"
    update_pane "$pane"
    ;;
  *) printf 'usage: %s [sweep|pane <pane_id> [agent]|event]\n' "${0##*/}" >&2; exit 2 ;;
esac
