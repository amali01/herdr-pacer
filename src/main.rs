//! herdr-pacer — pacing for the agents Herdr runs: bars under each agent row
//! in the sidebar (its context window, its account's 5h and weekly windows),
//! and every agent's windows in a dock along the bottom of every tab.

mod context;
mod dock;
mod herdr;
mod panel;
mod settings;
mod setup;
mod tui;
mod upgrade;
mod usage;

const USAGE: &str = "usage: herdr-pacer <command>

  dock                      the usage strip docked along the bottom of a tab
  popup                     the usage popup
  settings                  the settings popup (the dock's ⚙)
  ensure | toggle           dock the current tab (hooks) | show or hide every dock
  statusline                Claude Code statusLine wrapper (reads the payload on stdin)
  claude-hook install|remove|status
  panes sweep | pane <pane_id> [agent] | event
                            context bars for Codex and OpenCode
  fetch                     refresh the usage cache now
  setup                     wrap the statusLine, patch config.toml, reload, dock
  update                    install the published version if it is newer
  migrate                   bring an earlier install up to this build (hooks do this)
  version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fail = |e: String| {
        eprintln!("herdr-pacer: {e}");
        std::process::exit(1)
    };
    match args.first().map(String::as_str) {
        Some("dock") => tui::run(true).unwrap_or_else(|e| fail(e.to_string())),
        Some("popup") => tui::run(false).unwrap_or_else(|e| fail(e.to_string())),
        Some("settings") => panel::run().unwrap_or_else(|e| fail(e.to_string())),
        Some("ensure") => {
            upgrade::migrate(false);
            dock::run("ensure").unwrap_or_else(|e| fail(e.to_string()))
        }
        Some("toggle") => dock::run("toggle").unwrap_or_else(|e| fail(e.to_string())),
        Some("statusline") => std::process::exit(context::statusline()),
        Some("claude-hook") => match context::claude_hook(args.get(1).map_or("status", String::as_str)) {
            Ok(out) => println!("{out}"),
            Err(e) => fail(e),
        },
        Some("panes") => {
            upgrade::migrate(false);
            context::panes(&args[1..]).unwrap_or_else(fail)
        }
        Some("fetch") => usage::fetch(true),
        Some("setup") => setup::run().unwrap_or_else(fail),
        Some("update") => match upgrade::update() {
            Ok(out) => println!("{out}"),
            Err(e) => fail(e),
        },
        Some("migrate") => upgrade::migrate(true),
        Some("version") => println!("herdr-pacer {}", upgrade::VERSION),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2)
        }
    }
}
