#!/usr/bin/env bash
# herdr-pacer — usage.sh
# Global usage for every agent Herdr runs: a dotted bar for the 5h window and
# one for the week, per agent. Both bars take their color from the usage itself,
# on cc-pacer's thresholds.
#
#   usage.sh dock   the strip dock.py keeps along the bottom of every tab:
#                   a column per agent, ⟳ refreshes and ✕ hides (mouse or r)
#
#   Codex  team                    Claude  max                        ⟳   ✕
#   5h ⣤⣤⣤⣤⣤⣤⣤⣤⡄⣀  97%  ⟳ 2h14m    5h ⣤⣤⣀⣀⣀⣀⣀⣀⣀⣀  13%  ⟳ 4h02m
#   wk ⣤⣤⣤⣤⣀⣀⣀⣀⣀⣀  45%  ⟳ 3d19h    wk ⣤⣤⣤⣤⡄⣀⣀⣀⣀⣀  47%  ⟳ 5d01h
#
#   usage.sh        the popup: a section per agent, 5h over weekly (q closes)
#
# Every copy shares one cache and fetches into it in the background, so a dock
# per tab costs one fetch a minute, and a frame is only ever painted over the
# previous one — never cleared first.
set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
DOCK=0; [[ ${1:-} == dock ]] && DOCK=1
REFRESH_SECONDS="${HERDR_USAGE_REFRESH_SECONDS:-${HERDR_PACER_REFRESH_SECONDS:-60}}"
BAR_CELLS="${HERDR_PACER_BAR_CELLS:-20}"

# shellcheck source=collect.sh
source ./collect.sh
ROWS_CACHE="$(state_dir)/usage-rows"
FETCH_LOCK="$ROWS_CACHE.lock"
mkdir -p "$(state_dir)"
ensure_workdir || exit 1

if (( DOCK )); then
  # the alternate screen has no scrollback to scroll through; the mouse is ours
  printf '\033[?1049h\033[?7l\033[?25l\033[?1000h\033[?1006h'
  restore() { printf '\033[?1006l\033[?1000l\033[?25h\033[?7h\033[?1049l'; }
else
  restore() { printf '\033[H\033[2J'; }
fi
trap 'cleanup; restore; exit 0' INT TERM HUP
trap 'cleanup; restore' EXIT
RESIZED=0
trap 'RESIZED=1' WINCH   # bash resumes `read` after a trap, so the loop polls this

C_RESET=$'\033[0m'
C_DIM=$'\033[2m'
C_WHITE=$'\033[38;2;235;225;235m'
C_SUBTLE=$'\033[38;2;150;140;160m'
C_RAIL=$'\033[38;2;70;66;78m'

# cc-pacer's palette, so a window looks the same in both pacers
C_OK=$'\033[38;2;0;175;80m'
C_WARN=$'\033[38;2;255;176;85m'
C_HOT=$'\033[38;2;230;200;0m'
C_CRIT=$'\033[38;2;255;85;85m'

pct_color() {
  case "$(pct_bucket "$1")" in
    crit) printf '%s' "$C_CRIT" ;;
    hot)  printf '%s' "$C_HOT" ;;
    warn) printf '%s' "$C_WARN" ;;
    *)    printf '%s' "$C_OK" ;;
  esac
}

mtime() { stat -c %Y "$1" 2>/dev/null || stat -f %m "$1" 2>/dev/null || echo 0; }

# Starts a background fetch when the shared rows are older than the refresh
# interval and no copy is fetching already. A lock older than two minutes
# belongs to a copy that died mid-fetch.
fetch() { # [force]
  local now; now=$(date +%s)
  if [[ -d $FETCH_LOCK ]] && (( now - $(mtime "$FETCH_LOCK") > 120 )); then
    rmdir "$FETCH_LOCK" 2>/dev/null
  fi
  [[ ${1:-} == force ]] || (( now - $(mtime "$ROWS_CACHE") >= REFRESH_SECONDS )) || return 0
  mkdir "$FETCH_LOCK" 2>/dev/null || return 0
  (
    trap - EXIT INT TERM HUP
    run_collectors
    cp "$ROWS_TMP" "$ROWS_CACHE.tmp" && mv -f "$ROWS_CACHE.tmp" "$ROWS_CACHE"
    rmdir "$FETCH_LOCK"
  ) > /dev/null 2>&1 &
}

bar_parts() { # pct cells -> sets USED RAIL COLOR
  local out
  out=$(bar "$1" "$2")          # a tab-split read would eat an empty half
  USED=${out%%$'\t'*}; RAIL=${out#*$'\t'}
  COLOR=$(pct_color "$1")
}

reset_left() { # resets_at -> "3d19h" or ""
  local epoch left
  epoch=$(reset_epoch "$1")
  [[ -n $epoch ]] || return 0
  left=$(( epoch - $(date +%s) ))
  (( left > 0 )) && duration "$left"
}

# ── the popup: a section per agent ──

full_line() { # label pct resets_at
  local left
  bar_parts "$2" "$BAR_CELLS"
  left=$(reset_left "$3")
  [[ -n $left ]] && left="   ${C_DIM}⟳ ${left}${C_RESET}"
  printf '    %s%-7s%s %s%s%s%s%s  %s%3d%%%s%s\n' \
    "$C_SUBTLE" "$1" "$C_RESET" \
    "$COLOR" "$USED" "$C_RESET" "$C_RAIL" "$RAIL" \
    "$COLOR" "$2" "$C_RESET" "$left"
}

draw_full() {
  local key
  printf '%sHERDR usage%s\n\n' "$C_SUBTLE" "$C_RESET"
  for key in "${KEYS[@]}"; do
    printf '  %s%s%s' "$C_WHITE" "${TITLE[$key]}" "$C_RESET"
    [[ -n ${PLAN[$key]:-} ]] && printf '  %s%s%s' "$C_DIM" "${PLAN[$key]}" "$C_RESET"
    printf '\n'
    [[ -n ${FIVE[$key]:-} ]] && full_line "5h" "${FIVE[$key]}" "${FIVE_AT[$key]:-}"
    [[ -n ${WEEK[$key]:-} ]] && full_line "weekly" "${WEEK[$key]}" "${WEEK_AT[$key]:-}"
    [[ -n ${ERR[$key]:-} ]] && printf '    %s%s%s\n' "$C_DIM" "${ERR[$key]}" "$C_RESET"
    printf '\n'
  done
  printf '%sr refresh · q close%s' "$C_DIM" "$C_RESET"
}

# ── the dock: a column per agent, buttons on the right ──

BUTTONS=8                          # "  ⟳   ✕ ": herdr's scrollbar gutter covers the last column

strip_cell() { # width cells label pct resets_at -> one padded cell
  local width=$1 cells=$2 left mark='  '
  bar_parts "$4" "$cells"
  left=$(reset_left "$5")
  [[ -n $left ]] && mark='⟳ '   # padded apart: printf counts ⟳ as 3
  printf '%s%-2s%s %s%s%s%s%s %s%3d%%%s  %s%s%-5s%s%*s' \
    "$C_SUBTLE" "$3" "$C_RESET" "$COLOR" "$USED" "$C_RESET" "$C_RAIL" "$RAIL" \
    "$COLOR" "$4" "$C_RESET" "$C_DIM" "$mark" "$left" "$C_RESET" \
    $(( width - cells - 17 )) ''
}

draw_strip() {
  local n=${#KEYS[@]} width cells key head row room gap
  room=$(( COLS - BUTTONS ))
  width=$(( n ? room / n : room )); (( width < 30 )) && width=30
  cells=$(( width - 20 )); (( cells > 20 )) && cells=20; (( cells < 4 )) && cells=4
  for key in "${KEYS[@]}"; do
    head="${TITLE[$key]}${PLAN[$key]:+  ${PLAN[$key]}}"
    printf '%s%-*s%s' "$C_WHITE" "$width" "${head:0:width-1}" "$C_RESET"
  done
  (( n )) || printf '%s%-*s%s' "$C_DIM" "$room" "${STATUS:0:room}" "$C_RESET"
  gap=$(( COLS - (n ? n * width : room) - 6 )); (( gap < 1 )) && gap=1
  printf '%*s%s⟳%s   %s✕%s\n' "$gap" '' "$C_SUBTLE" "$C_RESET" "$C_SUBTLE" "$C_RESET"
  for row in 5h wk; do
    for key in "${KEYS[@]}"; do
      if [[ $row == 5h && -n ${FIVE[$key]:-} ]]; then
        strip_cell "$width" "$cells" 5h "${FIVE[$key]}" "${FIVE_AT[$key]:-}"
      elif [[ $row == wk && -n ${WEEK[$key]:-} ]]; then
        strip_cell "$width" "$cells" wk "${WEEK[$key]}" "${WEEK_AT[$key]:-}"
      elif [[ $row == 5h && -n ${ERR[$key]:-} ]]; then
        printf '%s%-*s%s' "$C_DIM" "$width" "${ERR[$key]:0:width-1}" "$C_RESET"
      else
        printf '%*s' "$width" ''
      fi
    done
    [[ $row == 5h ]] && printf '\n'
  done
}

# Paints a frame over the last one: home, each line then clear-to-eol, then
# clear below. Nothing is erased first, so a redraw never blinks.
paint() {
  printf '\033[H%s\033[K\033[J' "${1//$'\n'/$'\033[K\n'}"
}

render() {
  local provider p account window pct resets key rows frame
  COLS=$(tput cols 2>/dev/null || echo 80)
  rows=$(cat "$ROWS_CACHE" 2>/dev/null)
  STATUS=""
  if ! command -v jq > /dev/null 2>&1; then STATUS="jq not found on PATH"
  elif [[ -z $rows && -d $FETCH_LOCK ]]; then STATUS="fetching usage…"
  elif [[ -z $rows ]]; then STATUS="no usage available — install codex · claude · omp"
  fi

  declare -gA FIVE=() FIVE_AT=() WEEK=() WEEK_AT=() PLAN=() ERR=() TITLE=()
  while IFS="$US" read -r provider p account window pct resets; do
    [[ -n $provider ]] || continue
    if [[ $window == '!'* ]]; then ERR[$provider]="${window#!}"; continue; fi
    [[ -n $p ]] && PLAN[$provider]=$p
    case "$window" in
      *eserve*)    : ;;                       # codex reserve: not a pace window
      5h*)         FIVE[$provider]=$pct; FIVE_AT[$provider]=$resets ;;
      7d*|weekly*) WEEK[$provider]=$pct; WEEK_AT[$provider]=$resets ;;
    esac
  done <<< "$rows"

  KEYS=()
  for key in openai-codex claude opencode-go; do
    case "$key" in
      openai-codex) TITLE[$key]=Codex ;;
      claude) TITLE[$key]=Claude ;;
      *) TITLE[$key]=OpenCode ;;
    esac
    [[ -n ${FIVE[$key]:-}${WEEK[$key]:-}${ERR[$key]:-} ]] && KEYS+=("$key")
  done
  [[ -n $rows && ${#KEYS[@]} -eq 0 ]] && STATUS="no usage available"

  if (( DOCK )); then frame=$(draw_strip)
  elif [[ -n $STATUS ]]; then frame=$(printf '%s%s%s' "$C_DIM" "$STATUS" "$C_RESET")
  else frame=$(draw_full)
  fi
  paint "$frame"
}

# ── input: keys, and SGR mouse reports in the dock ──

read_input() { # sets INPUT to a key, "click X Y", or "" on timeout
  local c rest=""
  INPUT=""
  IFS= read -rsn1 -t 1 c || return 0
  if [[ $c != $'\e' ]]; then INPUT=$c; return 0; fi
  while IFS= read -rsn1 -t 0.05 c; do
    rest+=$c
    [[ ${#rest} -gt 1 && $c == [A-Za-z~] ]] && break
  done
  if [[ $rest =~ ^\[\<([0-9]+)\;([0-9]+)\;([0-9]+)M$ ]]; then
    (( BASH_REMATCH[1] == 0 )) && INPUT="click ${BASH_REMATCH[2]} ${BASH_REMATCH[3]}"
  elif [[ -z $rest ]]; then
    INPUT=$'\e'
  fi
}

on_click() { # x y, 1-based: ⟳ sits at COLS-5 and ✕ at COLS-1 on the first row
  (( $2 == 1 )) || return 0
  if (( $1 >= COLS - 3 )); then
    python3 ./dock.py toggle > /dev/null 2>&1   # hides every dock, this one last
  elif (( $1 >= COLS - 7 )); then
    fetch force
  fi
}

fetch
render
[[ -t 0 ]] || exit 0

seen=$(mtime "$ROWS_CACHE"); drawn=$(date +%s)
while true; do
  read_input
  case "$INPUT" in
    r | R) fetch force ;;
    click*) read -r _ x y <<< "$INPUT"; on_click "$x" "$y" ;;
    q | Q | $'\e' | $'\x03') (( DOCK )) || break ;;
    '') fetch ;;
  esac
  now=$(date +%s); cached=$(mtime "$ROWS_CACHE")
  # repaint when new rows land, the pane is resized, or reset times tick over
  if (( RESIZED || cached != seen || now - drawn >= 60 )); then
    RESIZED=0; seen=$cached; drawn=$now
    render
  fi
done
