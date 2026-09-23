#!/usr/bin/env bash
# herdr-pacer — setup.sh
# One-command setup: wraps the Claude statusLine so each session reports its
# context window, adds the context-bar row and the keybindings to config.toml,
# reloads Herdr, and docks usage in the current tab.
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

# prefix+u (or U) shows or hides the dock. Older setups bound it to the popup,
# with a bare ctrl+u that swallowed the shell's and Claude Code's line delete;
# repoint that, move a stock key list to the prefix-only one, drop old extras.
DOCK_KEY = 'key = ["prefix+u", "prefix+shift+u"]'
OLD_KEYS = ('key = ["prefix+u", "ctrl+u"]', 'key = "prefix+u"')


def command_blocks():
    return list(re.finditer(r"^\[\[keys\.command\]\]\n(?:(?!\[).*\n?)*", text, re.M))

for block in reversed(command_blocks()):
    body = block.group(0)
    if '"herdr-pacer.dock-collapse"' in body or (
            '"herdr-pacer.dock-toggle"' in body and 'key = "prefix+shift+u"' in body):
        text = text[: block.start()] + text[block.end():]
        print("    removed an old dock binding")
    elif '"herdr-pacer.open"' in body:
        body = body.replace('"herdr-pacer.open"', '"herdr-pacer.dock-toggle"')
        body = re.sub(r'^description = .*$', 'description = "show or hide the usage dock"',
                      body, flags=re.M)
        text = text[: block.start()] + body + text[block.end():]
        print("    repointed the usage key to the dock")
for block in reversed(command_blocks()):
    body = block.group(0)
    if '"herdr-pacer.dock-toggle"' in body and any(old in body for old in OLD_KEYS):
        for old in OLD_KEYS:
            body = body.replace(old, DOCK_KEY)
        text = text[: block.start()] + body + text[block.end():]
        print("    dock key is now prefix+u / prefix+U (ctrl+u is free again)")
if '"herdr-pacer.dock-toggle"' not in text:
    if not text.endswith("\n"):
        text += "\n"
    text += (
        '\n[[keys.command]]\n'
        '%s\n'
        'type = "plugin_action"\n'
        'command = "herdr-pacer.dock-toggle"\n'
        'description = "show or hide the usage dock"\n' % DOCK_KEY
    )
    print("    bound prefix+u / prefix+U to show or hide the dock")
else:
    print("    dock key already bound")

open(config_path, "w").write(text)

try:
    import tomllib
    tomllib.loads(text)
except ModuleNotFoundError:
    pass
except Exception as exc:
    sys.exit("    config.toml is not valid TOML after patching: %s" % exc)
PY

echo "==> 4/4 reload Herdr config and dock usage"
if "$HERDR" server reload-config > /dev/null 2>&1; then
  echo "    reloaded"
  python3 "$ROOT/dock.py" ensure &&
    echo "    usage docked in this tab; other tabs dock as you visit them"
else
  echo "    no running server (start herdr once; config applies then)"
fi

echo
echo "done — context bars appear under each Claude session as its status line"
echo "refreshes, and the 5h and weekly windows sit in a dock along the bottom of"
echo "every tab: the prefix key then u or U (ctrl+b u) shows or hides it, and"
echo "its ✕ button hides it."
