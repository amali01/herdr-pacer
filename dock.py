#!/usr/bin/env python3
"""herdr-pacer — dock.py

Keeps a usage pane docked along the bottom of every tab: a split pane running
`usage.sh dock`, grown to the full width of the tab.

  dock.py ensure     dock the current tab (tab and workspace hooks)
  dock.py toggle     hide the dock in every tab, or bring it back
  dock.py demo       self-check of the layout math, no herdr needed

Herdr has no pane placement for this, so it is built from stock calls, the way
herdr-sidebar builds its side column: split the bottom-left pane down, then
widen the dock by bouncing each pane beside it through a temporary tab and
splitting it back next to the pane above — pane.move keeps their processes.
Whether it is hidden lives in dock.json in $HERDR_PLUGIN_CONFIG_DIR.
"""

import fcntl
import json
import os
import socket
import sys

MARK = 'usage.sh" dock'               # how a live dock is told from other panes
LABEL = "usage"                       # the manifest pane's title
ROOT = os.path.dirname(os.path.abspath(__file__))
ROWS = 5                              # three rows of usage inside the pane border
FLOOR = 0.1                           # herdr clamps every split ratio to 0.1..0.9


# ── settings ──

def settings_path():
    base = os.environ.get("HERDR_PLUGIN_CONFIG_DIR") or os.path.join(
        os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config"),
        "herdr", "plugins", "config", "herdr-pacer")
    return os.path.join(base, "dock.json")


def hidden():
    try:
        with open(settings_path()) as f:
            return bool(json.load(f).get("hidden"))
    except (OSError, ValueError, AttributeError):
        return False


def set_hidden(value):
    path = settings_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path + ".tmp", "w") as f:
        json.dump({"hidden": value}, f)
    os.replace(path + ".tmp", path)


# ── herdr socket API: one request per connection ──

class ApiError(Exception):
    pass


def call(method, **params):
    with socket.socket(socket.AF_UNIX) as s:
        s.connect(os.environ["HERDR_SOCKET_PATH"])
        s.sendall((json.dumps({"id": "herdr-pacer", "method": method,
                               "params": params}) + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
    reply = json.loads(buf)
    if "error" in reply:
        raise ApiError("%s: %s" % (method, reply["error"].get("message")))
    return reply["result"]


def tree(tab_id):
    return call("layout.export", tab_id=tab_id)["layout"]


def geometry(tab_id):
    root = tree(tab_id)["root"]
    return call("pane.layout", pane_id=walk(root)[0]["pane_id"])["layout"]


# ── layout math (pure; demo() covers it) ──

def walk(node):
    return [node] if node["type"] == "pane" else walk(node["first"]) + walk(node["second"])


def is_dock(pane):
    """A live dock runs `usage.sh dock`. Herdr restores panes without their
    command, so a restored one is a shell that kept the label and our cwd."""
    command = pane.get("command")
    if command:
        return MARK in " ".join(command)
    return pane.get("label") == LABEL and pane.get("cwd") == ROOT


def bottom_left(panes, area):
    for pane in panes:
        r = pane["rect"]
        if r["x"] == 0 and r["y"] + r["height"] == area["height"]:
            return pane["pane_id"]
    return None


def full_width(panes, area, dock_id):
    return any(p["pane_id"] == dock_id and p["rect"]["width"] >= area["width"] for p in panes)


def widen_step(panes, area, dock_id):
    """(pane beside the dock, pane above the dock), or None once the dock
    spans the tab. Moving the first next to the second widens the dock."""
    me = next((p["rect"] for p in panes if p["pane_id"] == dock_id), None)
    if me is None or me["width"] >= area["width"]:
        return None
    beside = above = None
    for pane in panes:
        r = pane["rect"]
        if pane["pane_id"] == dock_id:
            continue
        if r["x"] == me["x"] + me["width"] and r["y"] <= me["y"] < r["y"] + r["height"]:
            beside = pane["pane_id"]
        if r["x"] == me["x"] and r["y"] + r["height"] == me["y"]:
            above = pane["pane_id"]
    return (beside, above) if beside and above else None


def dock_ratio(height):
    """The share the panes above keep, so the dock gets ROWS rows."""
    return 1 - min(max(ROWS / max(height, 1), FLOOR), 1 - FLOOR)


# ── actions ──

def tabs():
    return call("tab.list")["tabs"]


def focused_tab():
    return next((t["tab_id"] for t in tabs() if t.get("focused")), None)


def event_tab():
    try:
        event = json.loads(os.environ.get("HERDR_PLUGIN_EVENT_JSON") or "{}")
    except ValueError:
        event = {}
    data = event.get("data", event)
    return data.get("tab_id") or (data.get("tab") or {}).get("tab_id") or \
        os.environ.get("HERDR_TAB_ID") or focused_tab()


def close_docks(tab_id, keep=None):
    for pane in walk(tree(tab_id)["root"]):
        if is_dock(pane) and pane["pane_id"] != keep:
            call("pane.close", pane_id=pane["pane_id"])


def open_dock(tab_id):
    layout = geometry(tab_id)
    target = bottom_left(layout["panes"], layout["area"])
    if not target:
        return
    opened = call("plugin.pane.open", plugin_id="herdr-pacer", entrypoint="dock",
                  placement="split", target_pane_id=target, direction="down", focus=False)
    dock_id = opened["plugin_pane"]["pane"]["pane_id"]
    for _ in range(8):
        layout = geometry(tab_id)
        step = widen_step(layout["panes"], layout["area"], dock_id)
        if not step:
            break
        beside, above = step
        bounced = call("pane.move", pane_id=beside, destination={"type": "new_tab"}, focus=False)
        call("pane.move", pane_id=bounced["move_result"]["pane"]["pane_id"], focus=False,
             destination={"type": "tab", "tab_id": tab_id, "target_pane_id": above,
                          "split": "right"})
    root = tree(tab_id)["root"]
    if root["type"] == "split" and root["direction"] == "down" and is_dock(root["second"]):
        call("layout.set_split_ratio", tab_id=tab_id, path=[],
             ratio=dock_ratio(geometry(tab_id)["area"]["height"]))


def ensure(tab_id):
    """Adds a dock when the tab has none along its bottom. One already there is
    left alone, so a size dragged by hand stays and nothing redraws."""
    try:
        layout = tree(tab_id)
    except ApiError:
        return                                  # gone already, e.g. a bounce tab
    if layout.get("zoomed"):
        return
    panes = walk(layout["root"])
    found = [p["pane_id"] for p in panes if is_dock(p)]
    if found and len(panes) == 1:
        return                                  # the dock is the tab's only pane
    rects = geometry(tab_id)
    live = {p["pane_id"] for p in panes if p.get("command")}
    keep = next((d for d in found if d in live and
                 full_width(rects["panes"], rects["area"], d)), None)
    if len(found) > (keep is not None):
        close_docks(tab_id, keep)               # restored shells, duplicates
    if keep is None:
        focused = layout["focused_pane_id"]
        open_dock(tab_id)
        if tree(tab_id)["focused_pane_id"] != focused:
            call("pane.focus", pane_id=focused)


def toggle():
    set_hidden(not hidden())
    if not hidden():
        ensure(focused_tab())
        return
    # the tab we run from goes last: its dock may be the caller (the ✕ button),
    # and closing that pane ends this process
    here = os.environ.get("HERDR_TAB_ID")
    for tab in sorted(tabs(), key=lambda t: t["tab_id"] == here):
        close_docks(tab["tab_id"])


def locked(blocking):
    state = os.environ.get("HERDR_PLUGIN_STATE_DIR") or os.path.join(
        os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state"), "herdr-pacer")
    os.makedirs(state, exist_ok=True)
    handle = open(os.path.join(state, "dock.lock"), "w")
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB))
    except BlockingIOError:
        return None                             # focus events come in bursts
    return handle


def main(argv):
    command = argv[1] if len(argv) > 1 else "ensure"
    if command == "demo":
        return demo()
    if command not in ("ensure", "toggle"):
        print(__doc__, file=sys.stderr)
        return 2
    lock = locked(blocking=command == "toggle")
    if lock is None:
        return 0
    with lock:
        if command == "toggle":
            toggle()
        elif not hidden():
            ensure(event_tab())
    return 0


def demo():
    area = {"x": 0, "y": 0, "width": 100, "height": 40}
    left = {"pane_id": "l", "rect": {"x": 0, "y": 0, "width": 50, "height": 35}}
    dock = {"pane_id": "d", "rect": {"x": 0, "y": 35, "width": 50, "height": 5}}
    right = {"pane_id": "r", "rect": {"x": 50, "y": 0, "width": 50, "height": 40}}
    assert bottom_left([right, left, dock], area) == "d"
    assert bottom_left([right, left], area) is None
    assert widen_step([left, dock, right], area, "d") == ("r", "l")
    wide = dict(dock, rect=dict(dock["rect"], width=100))
    assert widen_step([left, wide], area, "d") is None and full_width([wide], area, "d")
    assert abs(dock_ratio(40) - 0.875) < 1e-9 and abs(dock_ratio(80) - 0.9) < 1e-9
    assert is_dock({"pane_id": "d", "command": ["sh", "-c", 'exec "x/usage.sh" dock']})
    restored = {"pane_id": "x", "label": LABEL, "cwd": ROOT}
    assert is_dock(restored) and not is_dock(dict(restored, cwd="/home"))
    assert not is_dock({"pane_id": "y", "label": LABEL, "command": ["vim"]})
    print("dock.py demo: ok")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except (ApiError, OSError, KeyError) as err:
        print("herdr-pacer dock: %s" % err, file=sys.stderr)
        sys.exit(1)
