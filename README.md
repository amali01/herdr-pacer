<p align="center">
  <img src="assets/logo.svg" alt="herdr-pacer" width="640">
</p>

<p align="center">
  <a href="https://herdr.dev/plugins/"><img alt="Listed in the Herdr plugin marketplace" src="https://img.shields.io/badge/herdr-marketplace-4a9eff?style=flat-square&labelColor=15181d"></a>
  <a href="https://github.com/amali01/herdr-pacer/stargazers"><img alt="Stars" src="https://img.shields.io/github/stars/amali01/herdr-pacer?style=flat-square&labelColor=15181d&color=4a9eff"></a>
</p>

Pacing for the agents [Herdr](https://herdr.dev) runs, in two places:

- **Per session** — a dotted bar under each agent row in the sidebar showing
  that session's context window, colored by how full it is. Claude Code, Codex,
  and OpenCode.
- **Globally** — a dock along the bottom of every tab with the **5h** and
  **weekly** windows for Codex, Claude, and OpenCode, same bars, same colors.
  The prefix key then `u` or `U` (`ctrl+b u`) shows or hides it; its `⟳` and
  `✕` buttons refresh and hide it with the mouse.

<p align="center">
  <img src="assets/demo.svg" width="960"
       alt="An animated mock of a Herdr session: the sidebar lists spaces and agents and a colored bar fills in under each agent showing that session's context window; then ctrl+b u brings up the usage dock along the bottom of the tab with the 5h and weekly windows for Codex, Claude and OpenCode, and a click on its ✕ hides it again.">
</p>

One number means the same thing wherever it appears: green below 50%, orange
50-69, yellow 70-89, red from 90 — the thresholds [cc-pacer](https://github.com/amali01/cc-pacer)
uses, so the two read alike if you run both. Monthly and billing windows are
deliberately left out; this is about pace.

## How it works

Two paths, no polling. Context bars are pushed when a session says something
new — Claude reports itself from its status line, and for Codex and OpenCode
Herdr pokes us on `pane.agent_status_changed`, which is exactly when a turn
ends. The 5h and weekly numbers are fetched on demand, cached, and refreshed
every minute while a dock or the popup is open. Every dock and the popup share
one cache, so a dock per tab still costs one fetch a minute.

### The sidebar row

Herdr has no plugin API for drawing in the sidebar, but agent rows accept custom
`$tokens` reported per pane, and a row whose tokens are all unreported
disappears. So the bar rides on pane metadata, and shows up only for sessions
that report one.

Two details made it work:

- **`herdr-glue.patch`** adds a `glue = true` token option to Herdr. Stock Herdr
  always inserts `" · "` between row tokens, which would run straight through
  the middle of every bar. With glue, the used half and the rail are two
  differently styled tokens rendered flush.
- **The bar carries its color in its token name.** Herdr's `rules` can recolor a
  token by value, but only when the value parses as a number — and a bar is
  braille. So the status line reports exactly one of `pacer_ctx_{ok,warn,hot,crit}_u`
  and clears the other three; `config.toml` gives each its color.

Herdr does not accept unsolicited pull requests (see its `CONTRIBUTING.md`), so
the patch lives here and is re-applied per release with `./herdr-build.sh`.

### The usage dock

Herdr gives plugins no room of their own outside the tab, so the dock is an
ordinary pane that `dock.py` keeps along the bottom with stock API calls — no
patch. The approach is [herdr-sidebar](https://github.com/alexarthurs/herdr-sidebar)'s.

- **Where it goes.** On `tab.created`, `tab.focused` and `workspace.focused`
  the hook docks the current tab if it has no dock yet: it splits the
  bottom-left pane down, then widens the dock to the full tab by bouncing each
  pane beside it through a temporary tab and splitting it back next to the pane
  above. `pane.move` keeps their processes; the layout around them does change.
  Your focus stays where it was.
- **Size.** Five rows — three of usage inside the pane border. Drag the border
  to resize it; a dock that is already there is never touched again.
- **Show and hide** apply to every tab: `prefix+u` or `prefix+U`, or `✕` on
  the dock. Nothing else is bound, so `ctrl+u` still reaches your shell. Hidden
  means gone, not shrunk. Shown again, it comes back in the current tab and in
  the others as you visit them.
- **No scroll, no flicker.** The dock draws on the terminal's alternate screen,
  which has no scrollback, and paints each frame over the last instead of
  clearing first. Numbers are fetched in the background into a cache every dock
  shares, so a dock per tab costs one fetch a minute.
- **Restarts.** Herdr restores panes without their command, so a restored dock
  is a shell that kept its label and the plugin's directory; the hook replaces
  it with a live one.

The full popup is still there as the `herdr-pacer.open` action.

## Install

Needs `bash`, `jq`, `curl` and `python3`, plus Herdr itself; `sqlite3` only if
you want OpenCode's context bar. Nothing else — herdr-pacer talks to each
provider on its own.

```sh
herdr plugin install amali01/herdr-pacer
herdr plugin action invoke herdr-pacer.setup
```

**The sidebar bars need a Herdr built with `herdr-glue.patch`.** Stock Herdr
inserts `" · "` between row tokens, straight through the middle of every bar, and
the option that suppresses it is not upstream (checked through v0.9.1). Without
the patch nothing breaks — the usage dock is plain plugin code and unaffected,
and the sidebar bar just renders as `used · rail`. To build one, run `./herdr-build.sh` from the plugin
directory (`herdr plugin list` prints it), install the binary it leaves in
`target/release/herdr`, then invoke `setup` again.

Or work from a clone, which is also how you'd hack on it:

```sh
git clone https://github.com/amali01/herdr-pacer.git
cd herdr-pacer
./herdr-build.sh            # builds a patched herdr; prints how to install it
herdr plugin link .
./setup.sh
```

`setup.sh` is idempotent, backs up every file it touches, and:

1. wraps the Claude Code statusLine with `claude-statusline.sh`,
2. checks that the running Herdr understands `glue`,
3. adds the context-bar row and the keybindings to `config.toml`, and
4. reloads Herdr and docks usage in the current tab.

The wrapper is a pass-through: **whatever already drew your status line keeps
drawing it**, and the wrapper only reads the context percentage out of the
payload on its way past. If nothing was there before, it prints a minimal
`ctx N%` line of its own. `./claude-hook.sh remove` puts the original command
back.

## Where the numbers come from

| Reading | Source | Needs |
|---|---|---|
| Claude context | the statusLine payload (`context_window.used_percentage`) | the wrapper installed |
| Codex context | the composer footer — `herdr pane read` picks up the `Context N% left` the TUI already prints | nothing |
| OpenCode context | its SQLite: the last turn's `tokens.total` for the session in that pane's directory, over the model's context limit from opencode's models cache | `sqlite3` |
| Codex 5h / weekly | `codex app-server` → `account/rateLimits/read` | signed-in `codex` CLI |
| Claude 5h / weekly | `api.anthropic.com/api/oauth/usage` — cc-pacer's cache is reused when it happens to be warm | signed-in Claude Code |
| OpenCode 5h / weekly | `omp usage --json` | `omp` on `PATH` |

A startup sweep covers sessions that were already running when Herdr started.

No credential file is written; the Claude bearer token reaches `curl` through
stdin so it never shows up in the process table. The OpenCode database is opened
read-only.

## Files

| File | Purpose |
|---|---|
| `collect.sh` | Provider collectors, the dotted-bar renderer, and the thresholds |
| `claude-statusline.sh` | statusLine wrapper: passes the line through, reports the context window |
| `claude-hook.sh` | `install` / `remove` / `status` for that wrapper |
| `pacer-panes.sh` | Context bars for Codex and OpenCode: `sweep` / `pane` / `event` |
| `usage.sh` | The dock (`usage.sh dock`) and the popup: 5h and weekly per agent |
| `dock.py` | Keeps the dock along the bottom of every tab: `ensure` / `toggle` |
| `sidebar-rows.toml` | The agent rows and colors `setup.sh` applies |
| `herdr-glue.patch` | The Herdr change the bars need |
| `herdr-build.sh` | Clones Herdr, applies the patch, builds it |
| `herdr-plugin.toml` | Plugin manifest: hooks, actions, the popup and dock panes |
| `setup.sh` | One-command setup |

## Knobs

| Env var | Default | Meaning |
|---|---|---|
| `HERDR_PACER_CTX_CELLS` | `10` | Width of the sidebar context bar, in cells |
| `HERDR_PACER_BAR_CELLS` | `20` | Width of the bars in the popup (docks fit theirs to the pane) |
| `HERDR_PACER_CTX_TTL_MS` | `300000` status line, 6h sweeps | How long a session's bar outlives its last refresh |
| `HERDR_PACER_EVENT_SETTLE` | `1.5` | Seconds to let a TUI repaint before reading it |
| `HERDR_PACER_OPENCODE_DB` | `~/.local/share/opencode/opencode.db` | OpenCode's database |
| `HERDR_PACER_OPENCODE_MODELS` | `~/.cache/opencode/models.json` | Where context limits come from |
| `HERDR_PACER_REFRESH_SECONDS` | `60` | Dock and popup auto-refresh |
| `HERDR_PACER_CLAUDE_REFRESH` | `300` | Seconds before we fetch Claude usage ourselves |
| `HERDR_PACER_STATE_DIR` | `$XDG_STATE_HOME/herdr-pacer` | Usage cache location |

Colors live in `config.toml`, not in the reporter — Herdr styles the tokens.

## Credits

- [Kamyil/herdr-usage-popup](https://github.com/Kamyil/herdr-usage-popup) — provider collectors.
- [amali01/cc-pacer](https://github.com/amali01/cc-pacer) — where the Claude usage
  endpoint and the color thresholds came from. **Not a dependency:** herdr-pacer
  fetches usage itself and only borrows cc-pacer's cache when it is already warm,
  and if cc-pacer is drawing your status line the wrapper leaves it drawing it.

MIT, except `herdr-glue.patch`, which is a change to Herdr and carries Herdr's
Apache-2.0 license.
