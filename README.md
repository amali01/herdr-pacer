# herdr-pacer

Pacing for the agents [Herdr](https://herdr.dev) runs, in two places:

- **Per session** — a dotted bar under each agent row in the sidebar showing
  that session's context window, colored by how full it is. Claude Code, Codex,
  and OpenCode.
- **Globally** — the prefix key then `u` (`ctrl+b u`) opens a popup with the
  **5h** and **weekly** windows for Codex, Claude, and OpenCode, one section per
  agent, same bars, same colors.

```
  HERDR usage

    Codex                                              team
    5h      ⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀   48%   ⟳ 2h14m
    weekly  ⣿⣿⣿⣿⣿⣿⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀   31%   ⟳ 3d19h

    Claude
    5h      ⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇   98%   ⟳ 9m
    weekly  ⣿⣿⣿⣿⣿⣿⣿⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀   34%   ⟳ 4d02h
```

Colors are cc-pacer's thresholds, so a number means the same thing in both:
green below 50%, orange 50-69, yellow 70-89, red from 90. Monthly and billing
windows are deliberately left out; this is about pace.

## How the sidebar bar works

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

## Install

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
3. adds the context-bar row and the keybinding to `config.toml`, and
4. reloads Herdr.

The wrapper is a pass-through: **cc-pacer keeps drawing the status line exactly
as before**, and the wrapper only reads the context percentage out of the
payload on its way past. `./claude-hook.sh remove` puts the original command
back.

## Where the numbers come from

| Reading | Source | Needs |
|---|---|---|
| Claude context | the statusLine payload (`context_window.used_percentage`) | the wrapper installed |
| Codex context | the composer footer — `herdr pane read` picks up the `Context N% left` the TUI already prints | nothing |
| OpenCode context | its SQLite: the last turn's `tokens.total` for the session in that pane's directory, over the model's context limit from opencode's models cache | `sqlite3` |
| Codex 5h / weekly | `codex app-server` → `account/rateLimits/read` | signed-in `codex` CLI |
| Claude 5h / weekly | `api.anthropic.com/api/oauth/usage`, or cc-pacer's cache when it is warm | signed-in Claude Code |
| OpenCode 5h / weekly | `omp usage --json` | `omp` on `PATH` |

Claude reports itself from its status line. Codex and OpenCode have no such
hook, so Herdr's own `[[events]]` do the work: on `pane.agent_status_changed` —
which is exactly when a turn ends — `pacer-panes.sh` refreshes that pane's bar,
and a startup sweep covers sessions that were already running. Nothing polls.

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
| `usage.sh` | The popup: a section per agent, 5h over weekly |
| `sidebar-rows.toml` | The agent rows and colors `setup.sh` applies |
| `herdr-glue.patch` | The Herdr change the bars need |
| `herdr-build.sh` | Clones Herdr, applies the patch, builds it |
| `herdr-plugin.toml` | Plugin manifest: actions and the popup pane |
| `setup.sh` | One-command setup |

## Knobs

| Env var | Default | Meaning |
|---|---|---|
| `HERDR_PACER_CTX_CELLS` | `10` | Width of the sidebar context bar, in cells |
| `HERDR_PACER_BAR_CELLS` | `20` | Width of the bars in the popup |
| `HERDR_PACER_CTX_TTL_MS` | `300000` status line, 6h sweeps | How long a session's bar outlives its last refresh |
| `HERDR_PACER_EVENT_SETTLE` | `1.5` | Seconds to let a TUI repaint before reading it |
| `HERDR_PACER_OPENCODE_DB` | `~/.local/share/opencode/opencode.db` | OpenCode's database |
| `HERDR_PACER_OPENCODE_MODELS` | `~/.cache/opencode/models.json` | Where context limits come from |
| `HERDR_PACER_REFRESH_SECONDS` | `60` | Popup auto-refresh |
| `HERDR_PACER_CLAUDE_REFRESH` | `300` | Seconds before we fetch Claude usage ourselves |
| `HERDR_PACER_STATE_DIR` | `$XDG_STATE_HOME/herdr-pacer` | Usage cache location |

Colors live in `config.toml`, not in the reporter — Herdr styles the tokens.

## Credits

- [Kamyil/herdr-usage-popup](https://github.com/Kamyil/herdr-usage-popup) — provider collectors.
- [amali01/cc-pacer](https://github.com/amali01/cc-pacer) — the Claude usage endpoint, its cache, and the thresholds. cc-pacer still owns the status line; herdr-pacer adds the Herdr-side view.

MIT, except `herdr-glue.patch`, which is a change to Herdr and carries Herdr's
Apache-2.0 license.
