#!/usr/bin/env bash
# herdr-pacer — setup.sh
# One-command setup: wraps the Claude statusLine so each session reports its
# context window, adds the context-bar row and the popup keybinding to
# config.toml, and reloads Herdr.
#
# Usage: ./setup.sh
# Idempotent. A timestamped backup is kept next to each file it touches.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
CONFIG="${HERDR_CONFIG_PATH:-$HOME/.config/herdr/config.toml}"
HERDR="${HERDR_BIN_PATH:-herdr}"

echo "==> 1/4 Claude Code statusLine wrapper"
"$ROOT/claude-hook.sh" install | sed 's/^/    /'

echo "==> 2/4 Herdr with the glue patch"
if "$HERDR" --default-config 2>/dev/null | grep -q 'glue = true'; then
  echo "    glue supported"
else
  echo "    WARNING: this herdr has no \`glue\` token option, so the context bar"
  echo "    renders as \"used · rail\". Build a patched herdr with"
  echo "    $ROOT/herdr-build.sh, install it, then rerun."
fi

echo "==> 3/4 Herdr config: $CONFIG"
mkdir -p "$(dirname "$CONFIG")"
[[ -f $CONFIG ]] || printf '# herdr configuration\n' > "$CONFIG"
backup=$(mktemp "$CONFIG.bak-$(date +%Y%m%d-%H%M%S).XXXXXX")
cp "$CONFIG" "$backup"
echo "    backup: $backup"

python3 - "$CONFIG" "$ROOT/sidebar-rows.toml" <<'PY'
import re, sys
config_path, snippet_path = sys.argv[1], sys.argv[2]
text = open(config_path).read()

snippet = open(snippet_path).read()
agents = snippet[snippet.index("[ui.sidebar.agents]"):].rstrip() + "\n"
rows_block = agents[agents.index("rows = ["):]


def section(name):
    """(start, end) of the body of table `name`, or None."""
    match = re.search(r"^\[%s\]\s*$" % re.escape(name), text, re.M)
    if not match:
        return None
    start = match.end()
    nxt = re.search(r"^\[", text[start:], re.M)
    return match.start(), start, start + (nxt.start() if nxt else len(text) - start)


def put_rows(name, body):
    global text
    found = section(name)
    if not found:
        if not text.endswith("\n"):
            text += "\n"
        text += "\n[%s]\n%s" % (name, body)
        return "added"
    _, start, end = found
    chunk = text[start:end]
    rows = re.search(r"^rows = \[.*?(?:^\]\s*$|\]\s*$)", chunk, re.M | re.S)
    if rows and rows.group(0).strip() == body.strip():
        return "unchanged"
    if rows:
        chunk = chunk[: rows.start()] + body + chunk[rows.end():].lstrip("\n")
    else:
        chunk = "\n".join([body.rstrip("\n"), chunk.lstrip("\n")])
    text = text[:start] + chunk + text[end:]
    return "updated"


print("    context-bar row: " + put_rows("ui.sidebar.agents", rows_block))

# drop the sidebar gauges this plugin used to put on Space rows
found = section("ui.sidebar.spaces")
if found and "pc_" in text[found[1]:found[2]]:
    head, start, end = found
    text = text[:head] + text[end:]
    print("    removed the Space gauge rows")
width = re.search(r"^sidebar_width = 34\s*$\n?", text, re.M)   # widened for those gauges
if width:
    text = text[: width.start()] + text[width.end():]
    print("    removed sidebar_width = 34")

# and v0.1.0's context row, whose tokens no longer exist
found = section("ui.sidebar.agents")
if found and "pacer_bar" in text[found[1]:found[2]]:
    _, start, end = found
    chunk = re.sub(r"^rows = \[.*pacer_bar.*\]\s*$\n?", "", text[start:end], flags=re.M)
    text = text[:start] + chunk + text[end:]
    print("    removed the old $pacer_bar row")

if "herdr-pacer.open" not in text:
    if not text.endswith("\n"):
        text += "\n"
    text += (
        '\n[[keys.command]]\n'
        'key = ["prefix+u", "ctrl+u"]\n'
        'type = "plugin_action"\n'
        'command = "herdr-pacer.open"\n'
        'description = "open usage"\n'
    )
    print("    added the prefix+u keybinding")
else:
    print("    keybinding already present")

open(config_path, "w").write(text)

try:
    import tomllib
    tomllib.loads(text)
except ModuleNotFoundError:
    pass
except Exception as exc:
    sys.exit("    config.toml is not valid TOML after patching: %s" % exc)
PY

echo "==> 4/4 reload Herdr config"
if "$HERDR" server reload-config > /dev/null 2>&1; then
  echo "    reloaded"
else
  echo "    no running server (start herdr once; config applies then)"
fi

echo
echo "done — context bars appear under each Claude session as its status line"
echo "refreshes; press the prefix key then u (ctrl+b u) for global usage."
