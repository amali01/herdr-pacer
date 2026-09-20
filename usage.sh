#!/usr/bin/env bash
# herdr-pacer — usage.sh
# Global usage for every agent Herdr runs, as a popup: one section per agent,
# a dotted bar for the 5h window and one for the week underneath it. Both bars
# take their color from the usage itself, on cc-pacer's thresholds.
#
# Opens with the prefix key, then `u` (ctrl+b u by default).
#
#   HERDR usage
#
#     Codex                                              team
#     5h      ⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀   78%   ⟳ 2h14m
#     weekly  ⣿⣿⣿⣿⣿⣿⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀   46%   ⟳ 3d19h
set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
QUIT=0
REFRESH_SECONDS="${HERDR_USAGE_REFRESH_SECONDS:-${HERDR_PACER_REFRESH_SECONDS:-60}}"
BAR_CELLS="${HERDR_PACER_BAR_CELLS:-20}"

# shellcheck source=collect.sh
source ./collect.sh
trap 'cleanup; printf "\033[H\033[2J"; exit 0' INT TERM HUP
trap cleanup EXIT

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

bar_line() { # label pct resets_at
  local label=$1 pct=$2 resets=$3 out used rail color epoch left=""
  out=$(bar "$pct" "$BAR_CELLS")          # a tab-split read would eat an empty half
  used=${out%%$'\t'*}; rail=${out#*$'\t'}
  color=$(pct_color "$pct")
  epoch=$(reset_epoch "$resets")
  if [[ -n $epoch ]]; then
    left=$(( epoch - $(date +%s) ))
    (( left > 0 )) && left="   ${C_DIM}⟳ $(duration "$left")${C_RESET}" || left=""
  fi
  printf '    %s%-7s%s %s%s%s%s%s  %s%3d%%%s%s\n' \
    "$C_SUBTLE" "$label" "$C_RESET" \
    "$color" "$used" "$C_RESET" "$C_RAIL" "$rail" \
    "$color" "$pct" "$C_RESET" "$left"
}

section() { # title plan 5h_pct 5h_reset weekly_pct weekly_reset error
  local title=$1 plan=$2 p5=$3 r5=$4 p7=$5 r7=$6 err=$7
  printf '  %s%s%s' "$C_WHITE" "$title" "$C_RESET"
  [[ -n $plan ]] && printf '  %s%s%s' "$C_DIM" "$plan" "$C_RESET"
  printf '\n'
  [[ -n $p5 ]] && bar_line "5h" "$p5" "$r5"
  [[ -n $p7 ]] && bar_line "weekly" "$p7" "$r7"
  [[ -n $err ]] && printf '    %s%s%s\n' "$C_DIM" "$err" "$C_RESET"
  printf '\n'
}

render() {
  printf '\033[H\033[2J'

  if ! command -v jq > /dev/null 2>&1; then
    printf '  jq not found on PATH\n'
    return
  fi

  run_collectors
  (( QUIT )) && return

  local rows
  rows=$(< "$ROWS_TMP")
  printf '%sHERDR usage%s\n\n' "$C_SUBTLE" "$C_RESET"
  if [[ -z $rows ]]; then
    printf '  %sno usage available — no provider CLI found%s\n' "$C_DIM" "$C_RESET"
    printf '  %sinstall codex · claude · omp%s\n' "$C_DIM" "$C_RESET"
    return
  fi

  local -A five=() five_at=() week=() week_at=() plan=() err=()
  local provider p account window pct resets
  while IFS="$US" read -r provider p account window pct resets; do
    [[ -n $provider ]] || continue
    if [[ $window == '!'* ]]; then err[$provider]="${window#!}"; continue; fi
    [[ -n $p ]] && plan[$provider]=$p
    case "$window" in
      *eserve*)    : ;;                       # codex reserve: not a pace window
      5h*)         five[$provider]=$pct; five_at[$provider]=$resets ;;
      7d*|weekly*) week[$provider]=$pct; week_at[$provider]=$resets ;;
    esac
  done <<< "$rows"

  local key title
  for key in openai-codex claude opencode-go; do
    case "$key" in
      openai-codex) title=Codex ;;
      claude) title=Claude ;;
      *) title=OpenCode ;;
    esac
    [[ -n ${five[$key]:-}${week[$key]:-}${err[$key]:-} ]] || continue
    section "$title" "${plan[$key]:-}" \
      "${five[$key]:-}" "${five_at[$key]:-}" \
      "${week[$key]:-}" "${week_at[$key]:-}" \
      "${err[$key]:-}"
  done
}

print_footer() { printf '%sr refresh · q close%s\n' "$C_DIM" "$C_RESET"; }

render
if (( QUIT )); then printf '\033[H\033[2J'; exit 0; fi
print_footer

[[ -t 0 ]] || exit 0

while true; do
  if IFS= read -rsn1 -t "$REFRESH_SECONDS" key; then
    case "$key" in
      q | Q | $'\e' | $'\r' | $'\n' | $'\x03') break ;;
      r | R) : ;;
    esac
  fi
  render
  (( QUIT )) && break
  print_footer
done

printf '\033[H\033[2J'
