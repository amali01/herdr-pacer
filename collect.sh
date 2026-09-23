#!/usr/bin/env bash
# herdr-pacer — collect.sh
# Provider collectors and the dotted bar, shared by the usage dock and popup
# (usage.sh) and the context bars (claude-statusline.sh, pacer-panes.sh).
# Sourced, never executed.
#
# Row contract (unit-separated):
#   provider <US> plan <US> account <US> window <US> percent <US> resets_at
#   (resets_at is whatever the provider reports: epoch seconds or ISO-8601)
#   A `window` starting with `!` is a status line, not data.
#
# Only the 5h and weekly windows are collected. Monthly windows are dropped
# on purpose — the gauges show pace, not billing.
#
# Know-how reused (with thanks):
#   - Kamyil/herdr-usage-popup: Codex over `codex app-server`, OpenCode over
#     `omp usage --json`.
#   - amali01/cc-pacer: the Claude OAuth usage endpoint, its cache, and the
#     back-off around it. cc-pacer's own cache is preferred when it is fresh,
#     so a running Claude session pays for the request and we just read it.

OMP_BIN="${HERDR_USAGE_OMP_BIN:-${HERDR_PACER_OMP_BIN:-omp}}"
CODEX_BIN="${HERDR_USAGE_CODEX_BIN:-${HERDR_PACER_CODEX_BIN:-codex}}"
STATE_DIR_OVERRIDE="${HERDR_USAGE_STATE_DIR:-${HERDR_PACER_STATE_DIR:-}}"
CLAUDE_REFRESH="${HERDR_PACER_CLAUDE_REFRESH:-300}"   # seconds before we fetch ourselves
CLAUDE_MAX_AGE="${HERDR_USAGE_CLAUDE_MAX_AGE:-900}"   # seconds before data is flagged stale

US=$(printf '\037')
COLLECTORS=(collect_codex collect_claude collect_opencode_go)

state_dir() {
  if [[ -n $STATE_DIR_OVERRIDE ]]; then printf '%s' "$STATE_DIR_OVERRIDE"
  elif [[ -n ${HERDR_PLUGIN_STATE_DIR:-} ]]; then printf '%s' "$HERDR_PLUGIN_STATE_DIR"
  else printf '%s' "${XDG_STATE_HOME:-$HOME/.local/state}/herdr-pacer"
  fi
}

# the scratch dir is created on first use, so sourcing this file for `bar` alone
# (the status line does) costs nothing
WORK_DIR=""
ROWS_TMP=""
ensure_workdir() {
  [[ -n $WORK_DIR ]] && return 0
  WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/herdr-pacer.XXXXXX") || return 1
  ROWS_TMP="$WORK_DIR/rows"
}
cleanup() { [[ -n $WORK_DIR ]] && rm -rf "$WORK_DIR"; }

duration() { # seconds -> 3d19h / 46m / now
  local s=$1 d h m
  if (( s <= 0 )); then printf 'now'; return; fi
  d=$(( s / 86400 )); h=$(( (s % 86400) / 3600 )); m=$(( (s % 3600) / 60 ))
  if (( d > 0 )); then printf '%dd%02dh' "$d" "$h"
  elif (( h > 0 )); then printf '%dh%02dm' "$h" "$m"
  else printf '%dm' "$m"; fi
}

# ── dotted bars (braille) ──
# Both surfaces draw the same shape: the used part in dense dots, the rest as a
# thin rail. `bar` prints "used<TAB>rail" (split it with ${out%%$'\t'*}, not read,
# which would swallow an empty half) so the sidebar can color the halves as
# two tokens and usage.sh can color them inline.
BAR_FULL=⣤    # two dot rows: a thin bar, not a full-height block
BAR_HALF=⡄
BAR_RAIL=⣀    # one row, so the track still reads without color

bar() { # pct cells -> used <TAB> rail
  local pct=$1 cells=$2 halves i used="" rail=""
  (( pct < 0 )) && pct=0
  (( pct > 100 )) && pct=100
  halves=$(( (pct * cells * 2 + 50) / 100 ))      # two dot columns per cell
  local d
  for (( i = 0; i < cells; i++ )); do
    d=$(( halves - i * 2 ))
    if   (( d >= 2 )); then used+=$BAR_FULL
    elif (( d == 1 )); then used+=$BAR_HALF
    else                    rail+=$BAR_RAIL
    fi
  done
  printf '%s\t%s' "$used" "$rail"
}

# cc-pacer's thresholds, so both pacers read the same
pct_bucket() { # pct -> ok | warn | hot | crit
  local pct=$1
  if   (( pct >= 90 )); then printf crit
  elif (( pct >= 70 )); then printf hot
  elif (( pct >= 50 )); then printf warn
  else printf ok
  fi
}

reset_epoch() { # epoch seconds or ISO-8601 -> epoch seconds (empty when unknown)
  local value=$1
  [[ -z $value || $value == 0 ]] && return 0
  if [[ $value =~ ^[0-9]+$ ]]; then printf '%s' "$value"; return 0; fi
  date -d "$value" +%s 2>/dev/null || true
}

# ── context-window reporting (shared by the status line and the pane sweeper) ──
# A braille bar cannot be recolored by Herdr's `rules` (they need a numeric
# value), so the color travels in the token name: exactly one bucket is
# reported and the other three are cleared.
CTX_BUCKETS=(ok warn hot crit)

ctx_report() { # pane_id pct
  local pane=$1 pct=$2 cells="${HERDR_PACER_CTX_CELLS:-10}" out used rail bucket b args=()
  [[ -n $pane && -n $pct ]] || return 1
  out=$(bar "$pct" "$cells")
  used=${out%%$'\t'*}; rail=${out#*$'\t'}
  bucket=$(pct_bucket "$pct")
  for b in "${CTX_BUCKETS[@]}"; do
    if [[ $b == "$bucket" ]]; then
      args+=(--token "pacer_ctx_${b}_u=$used" --token "pacer_ctx_${b}_t=$rail")
    else
      args+=(--clear-token "pacer_ctx_${b}_u" --clear-token "pacer_ctx_${b}_t")
    fi
  done
  args+=(--token "pacer_ctx=⠀${pct}%")
  "${HERDR_BIN_PATH:-herdr}" pane report-metadata "$pane" --source herdr-pacer \
    --ttl-ms "${HERDR_PACER_CTX_TTL_MS:-21600000}" "${args[@]}" > /dev/null 2>&1
}

ctx_clear() { # pane_id
  local pane=$1 b args=()
  [[ -n $pane ]] || return 1
  for b in "${CTX_BUCKETS[@]}"; do
    args+=(--clear-token "pacer_ctx_${b}_u" --clear-token "pacer_ctx_${b}_t")
  done
  args+=(--clear-token pacer_ctx)
  "${HERDR_BIN_PATH:-herdr}" pane report-metadata "$pane" --source herdr-pacer \
    "${args[@]}" > /dev/null 2>&1
}

error_row() { printf '%s\037%s\037%s\037!%s\037\037\n' "$1" "" "" "$2"; }

collect_codex() {
  local key=openai-codex bin dir fifo out pid i
  bin=$(command -v "$CODEX_BIN") || return 0
  [[ -n $WORK_DIR ]] || return 0
  dir="$WORK_DIR/codex"
  rm -rf "$dir"
  mkdir -p "$dir" || return 0
  fifo="$dir/in"; out="$dir/out"
  mkfifo "$fifo" 2>/dev/null || return 0

  exec 3<> "$fifo"
  "$bin" -s read-only -a never app-server < "$fifo" > "$out" 2>/dev/null &
  pid=$!
  printf '%s\n' \
    '{"id":1,"method":"initialize","params":{"clientInfo":{"name":"herdr-pacer","version":"1"}}}' \
    '{"method":"initialized","params":{}}' \
    '{"id":2,"method":"account/read","params":{}}' \
    '{"id":3,"method":"account/rateLimits/read","params":{}}' >&3

  i=0
  while (( i < 100 )); do
    grep -q '"id":3' "$out" 2>/dev/null && break
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
    i=$(( i + 1 ))
  done

  exec 3>&-
  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null

  if ! jq -e -s 'any(.[]; .id == 3 and .result.rateLimits)' "$out" > /dev/null 2>&1; then
    rm -rf "$dir"
    error_row "$key" 'codex app-server returned no limits'
    return 0
  fi

  jq -rs --arg p "$key" '
    def wlabel($m):
      if $m == null or $m == 0 then "?"
      elif ($m % 1440) == 0 then "\($m / 1440 | floor)d"
      elif ($m % 60) == 0 then "\($m / 60 | floor)h"
      else "\($m)m" end;
    ([.[] | select(.id == 2) | .result.account][0] // {}) as $acct
    | ([.[] | select(.id == 3) | .result][0] // {}) as $res
    | ($acct.planType // $res.rateLimits.planType // "") as $plan
    | ($acct.email // "") as $email
    | (if (($res.rateLimitsByLimitId // {}) | length) > 0
       then ($res.rateLimitsByLimitId
             | to_entries
             | map(.value + { limitId: .key })
             | sort_by(if .limitId == "codex" then 0 else 1 end))
       else [ ($res.rateLimits + { limitId: "codex" }) ] end)
    | .[] as $lim
    | (if $lim.limitId == "codex" then "" else ($lim.limitName // $lim.limitId) end) as $q
    | ([ { w: $lim.primary }, { w: $lim.secondary } ])
    | .[]
    | select(.w != null and .w.usedPercent != null)
    | select((.w.windowDurationMins // 0) < 43200)
    | [ $p, $plan, $email,
        (wlabel(.w.windowDurationMins) + (if $q != "" then "/" + $q else "" end)),
        (.w.usedPercent | floor),
        (.w.resetsAt // 0) ]
    | map(tostring) | join("\u001f")' "$out"

  rm -rf "$dir"
}

claude_token() {
  local blob token=""
  if [[ -n ${CLAUDE_CODE_OAUTH_TOKEN:-} ]]; then printf '%s' "$CLAUDE_CODE_OAUTH_TOKEN"; return; fi
  if [[ -f "$HOME/.claude/.credentials.json" ]]; then
    token=$(jq -r '.claudeAiOauth.accessToken // empty' "$HOME/.claude/.credentials.json" 2>/dev/null)
  fi
  if [[ -z $token ]] && command -v secret-tool > /dev/null 2>&1; then
    blob=$(timeout 2 secret-tool lookup service "Claude Code-credentials" 2>/dev/null)
    [[ -n $blob ]] && token=$(jq -r '.claudeAiOauth.accessToken // empty' <<< "$blob" 2>/dev/null)
  fi
  if [[ -z $token ]] && command -v security > /dev/null 2>&1; then
    blob=$(security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null)
    [[ -n $blob ]] && token=$(jq -r '.claudeAiOauth.accessToken // empty' <<< "$blob" 2>/dev/null)
  fi
  printf '%s' "$token"
}

claude_usage_json() { # prints cached usage JSON, refreshing it when stale
  local state cache backoff newest now age token resp code
  state=$(state_dir); mkdir -p "$state" 2>/dev/null
  cache="$state/claude-usage.json"
  backoff="$state/claude-backoff"
  now=$(date +%s)

  # cc-pacer keeps the same payload warm while any Claude session is open
  newest=$(ls -t "/tmp/cc-pacer-${UID:-$(id -u)}"/*/usage-cache.json 2>/dev/null | head -1)
  [[ -n $newest && $newest -nt $cache ]] && cp -f "$newest" "$cache" 2>/dev/null

  age=$(( now - $(stat -c %Y "$cache" 2>/dev/null || stat -f %m "$cache" 2>/dev/null || echo 0) ))
  if (( age > CLAUDE_REFRESH )) && (( now > $(cat "$backoff" 2>/dev/null || echo 0) )); then
    token=$(claude_token)
    if [[ -n $token ]]; then
      # header via stdin so the bearer token never lands in the process table
      resp=$(printf 'Authorization: Bearer %s\n' "$token" | curl -s --max-time 5 -w '\n%{http_code}' \
        -H 'Accept: application/json' -H @- \
        -H 'anthropic-beta: oauth-2025-04-20' \
        -H 'User-Agent: claude-code/2.1.34' \
        'https://api.anthropic.com/api/oauth/usage' 2>/dev/null)
      code="${resp##*$'\n'}"; resp="${resp%$'\n'*}"
      if [[ $code == 200 ]] && jq -e '.five_hour' <<< "$resp" > /dev/null 2>&1; then
        printf '%s' "$resp" > "$cache.tmp" && mv -f "$cache.tmp" "$cache"
        rm -f "$backoff"
      else
        # ponytail: one flat back-off; split per status code if auth errors get noisy
        printf '%s' "$(( now + 900 ))" > "$backoff"
      fi
    fi
  fi
  [[ -f $cache ]] && cat "$cache"
}

collect_claude() {
  local key=claude json recorded age
  json=$(claude_usage_json)
  if [[ -z $json ]] || ! jq -e '.five_hour // .seven_day' <<< "$json" > /dev/null 2>&1; then
    command -v claude > /dev/null 2>&1 && error_row "$key" 'no usage yet — sign in to Claude Code'
    return 0
  fi

  recorded=$(stat -c %Y "$(state_dir)/claude-usage.json" 2>/dev/null || echo 0)
  age=$(( $(date +%s) - recorded ))
  (( age > CLAUDE_MAX_AGE )) && error_row "$key" "usage $(duration "$age") old"

  jq -r --arg p "$key" '
    [ { w: "5h", d: .five_hour }, { w: "7d", d: .seven_day } ]
    | .[]
    | select(.d != null and (.d.utilization // .d.used_percentage) != null)
    | [ $p, "", "", .w,
        ((.d.utilization // .d.used_percentage) | round),
        (.d.resets_at // 0) ]
    | map(tostring) | join("\u001f")' <<< "$json"
}

collect_opencode_go() {
  local key=opencode-go json
  command -v "$OMP_BIN" > /dev/null 2>&1 || return 0
  if ! json=$("$OMP_BIN" usage --json --provider "$key" 2>/dev/null); then
    error_row "$key" 'request failed'
    return 0
  fi
  printf '%s' "$json" | jq -r --arg p "$key" '
    .reports[]?
    | select(.provider == $p)
    | ((.metadata // {}).planType // "") as $plan
    | .limits[]?
    | select((.scope.windowId // "") != "monthly")
    | [ $p, $plan, "",
        (.scope.windowId // "?"),
        ((.amount.usedFraction // 0) * 100 | floor),
        ((.window.resetsAt // 0) / 1000 | floor) ]
    | map(tostring) | join("\u001f")'
}

run_collectors() { # fills $ROWS_TMP; spins only on a tty
  ensure_workdir || return 1
  local pid key frame i=0
  : > "$ROWS_TMP"
  ( trap - EXIT; for c in "${COLLECTORS[@]}"; do "$c"; done ) >> "$ROWS_TMP" 2>/dev/null &
  pid=$!

  if [[ -t 1 ]]; then
    local frames=(⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏)
    while kill -0 "$pid" 2>/dev/null; do
      frame=${frames[$(( i % ${#frames[@]} ))]}
      printf '\r\033[K  %s fetching usage…' "$frame"
      i=$(( i + 1 ))
      if [[ -t 0 ]]; then
        if IFS= read -rsn1 -t 0.1 key; then
          case "$key" in
            q | Q | $'\e' | $'\r' | $'\n' | $'\x03') QUIT=1; kill "$pid" 2>/dev/null; break ;;
          esac
        fi
      else
        sleep 0.1
      fi
    done
    printf '\r\033[K'
  fi

  wait "$pid" 2>/dev/null
}
