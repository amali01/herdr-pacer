//! One-command setup, idempotent, with a timestamped backup of each file it
//! touches: wraps the Claude statusLine, puts the sidebar bar rows and the dock
//! key in config.toml, reloads Herdr, and restarts the docks.

use crate::{context, dock, herdr, settings};
use serde_json::json;
use std::path::PathBuf;
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

const TOGGLE: &str = "herdr-pacer.dock-toggle";

fn config_path() -> PathBuf {
    std::env::var_os("HERDR_CONFIG_PATH").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME")
            .map_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"), PathBuf::from)
            .join("herdr/config.toml")
    })
}

fn table<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> &'a mut Table {
    let mut t = doc.as_table_mut();
    for key in path {
        let entry = t.entry(key).or_insert_with(|| {
            let mut new = Table::new();
            new.set_implicit(true);
            Item::Table(new)
        });
        t = entry.as_table_mut().expect("a table");
    }
    t
}

fn keys_of(item: &Item) -> Vec<String> {
    match item {
        i if i.is_str() => vec![i.as_str().unwrap_or_default().into()],
        i => i.as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect(),
    }
}

/// Everything setup changes in config.toml, reported line by line.
fn patch(doc: &mut DocumentMut) -> Vec<String> {
    let mut notes: Vec<String> = vec![];

    // the bar rows under each agent, and the tab bar commands
    notes.push(format!("sidebar bar rows: {}", put_rows(doc)));
    let (keys, exe) = tabbar_wanted();
    if put_tabbar(doc, &keys, exe.as_deref()) {
        notes.push("tab bar: updated".into());
    }

    // the Space gauges and the wider sidebar an early version asked for
    let ui = table(doc, &["ui"]);
    if ui.get("sidebar").and_then(|s| s.get("spaces")).is_some_and(|s| s.to_string().contains("pc_")) {
        ui["sidebar"].as_table_mut().map(|s| s.remove("spaces"));
        notes.push("removed the Space gauge rows".into());
    }
    if ui.get("sidebar_width").and_then(Item::as_integer) == Some(34) {
        ui.remove("sidebar_width");
        notes.push("removed sidebar_width = 34".into());
    }

    // prefix+u (or U) shows or hides the dock. Older setups bound it to the
    // popup with a bare ctrl+u, which swallowed the line delete in shells and
    // Claude Code; the dock once had two more keys of its own.
    let keys = table(doc, &["keys"]);
    if !keys.contains_key("command") {
        keys.insert("command", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let commands = keys["command"].as_array_of_tables_mut().expect("[[keys.command]]");
    let old_stock = |k: &[String]| k == ["prefix+u", "ctrl+u"] || k == ["prefix+u"];
    let before = commands.len();
    commands.retain(|t| {
        let command = t.get("command").and_then(Item::as_str).unwrap_or("");
        let key = t.get("key").map(keys_of).unwrap_or_default();
        !(command == "herdr-pacer.dock-collapse" || command == TOGGLE && key == ["prefix+shift+u"])
    });
    if commands.len() < before {
        notes.push("removed old dock bindings".into());
    }
    let mut bound = false;
    for t in commands.iter_mut() {
        let command = t.get("command").and_then(Item::as_str).unwrap_or("").to_string();
        if command != TOGGLE && command != "herdr-pacer.open" {
            continue;
        }
        if command == "herdr-pacer.open" {
            t["command"] = value(TOGGLE);
            t["description"] = value("show or hide the usage dock");
            notes.push("repointed the usage key to the dock".into());
        }
        if old_stock(&t.get("key").map(keys_of).unwrap_or_default()) {
            t["key"] = value(Array::from_iter(["prefix+u", "prefix+shift+u"]));
            notes.push("dock key is now prefix+u / prefix+U (ctrl+u is free again)".into());
        }
        bound = true;
    }
    if !bound {
        let mut t = Table::new();
        t["key"] = value(Array::from_iter(["prefix+u", "prefix+shift+u"]));
        t["type"] = value("plugin_action");
        t["command"] = value(TOGGLE);
        t["description"] = value("show or hide the usage dock");
        commands.push(t);
        notes.push("bound prefix+u / prefix+U to show or hide the dock".into());
    } else if !notes.iter().any(|n| n.contains("dock key") || n.contains("repointed")) {
        notes.push("dock key already bound".into());
    }
    notes
}

/// Sets `[ui.sidebar.agents] rows` for the saved layout: added, updated or
/// unchanged.
fn put_rows(doc: &mut DocumentMut) -> &'static str {
    let snippet: DocumentMut = context::rows_toml(&settings::load()).parse().expect("generated rows are TOML");
    let rows = snippet["ui"]["sidebar"]["agents"]["rows"].clone();
    let agents = table(doc, &["ui", "sidebar", "agents"]);
    match agents.get("rows") {
        Some(r) if r.to_string().trim() == rows.to_string().trim() => "unchanged",
        had => {
            let verb = if had.is_some() { "updated" } else { "added" };
            agents.insert("rows", rows);
            verb
        }
    }
}

/// A tab bar command of ours, told apart from the user's own entries.
fn is_ours(entry: &toml_edit::Value) -> bool {
    entry.as_inline_table().and_then(|t| t.get("command")).and_then(|c| c.as_str()).is_some_and(|c| c.contains("herdr-pacer") && c.contains(" tabbar "))
}

/// `[ui] tab_bar_right`: a command per agent in `keys`, in the chosen order,
/// while any agent is on in the tab bar settings, none while all are off. Entries of the user's own (zoom, a
/// clock, other widgets) stay as they are, in their order; ours go last.
/// Whether anything changed.
fn put_tabbar(doc: &mut DocumentMut, keys: &[&str], exe: Option<&std::path::Path>) -> bool {
    let ui = table(doc, &["ui"]);
    let before = ui.get("tab_bar_right").map(|i| i.to_string());
    let mut entries = ui.get("tab_bar_right").and_then(Item::as_array).cloned().unwrap_or_default();
    entries.retain(|e| !is_ours(e));
    if let Some(exe) = exe.filter(|_| !keys.is_empty()) {
        let bin = format!("'{}'", exe.to_string_lossy().replace('\'', r"'\''"));
        for key in keys {
            let mut entry = toml_edit::InlineTable::new();
            entry.insert("type", "command".into());
            entry.insert("command", format!("{bin} tabbar {key}").into());
            entry.insert("interval_seconds", 30.into());
            entry.insert("timeout_seconds", 3.into());
            entries.push(entry);
        }
    }
    if entries.is_empty() {
        ui.remove("tab_bar_right");
    } else {
        ui.insert("tab_bar_right", value(entries));
    }
    ui.get("tab_bar_right").map(|i| i.to_string()) != before
}

/// The agent keys for the tab bar commands, in order; none while the tab bar
/// view is off.
fn tabbar_wanted() -> (Vec<&'static str>, Option<PathBuf>) {
    let s = settings::load();
    let keys = match s.tab_agents.contains(&true) {
        true => s.agents_in_order().map(|a| a.0).to_vec(),
        false => vec![],
    };
    (keys, std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()))
}

/// Adds or removes the tab bar commands after the tab bar settings change.
pub fn apply_tabbar() -> Result<(), String> {
    let (keys, exe) = tabbar_wanted();
    rewrite(|doc| put_tabbar(doc, &keys, exe.as_deref()))
}

/// Rewrites the sidebar rows after the layout changes, and reloads Herdr.
pub fn apply_rows() -> Result<(), String> {
    rewrite(|doc| put_rows(doc) != "unchanged")
}

/// Brings an earlier setup's rows and dock key up to this build (upgrades).
pub fn apply_config() -> Result<(), String> {
    rewrite(|doc| {
        let before = doc.to_string();
        patch(doc);
        doc.to_string() != before
    })
}

/// Whether setup has run here before: config.toml carries our rows or key.
pub fn configured() -> bool {
    std::fs::read_to_string(config_path()).is_ok_and(|t| t.contains("$pacer_") || t.contains("herdr-pacer."))
}

fn rewrite(change: impl FnOnce(&mut DocumentMut) -> bool) -> Result<(), String> {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    if change(&mut doc) {
        context::backup(&path);
        std::fs::write(&path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))?;
        herdr::call("server.reload_config", json!({})).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn glue_supported() -> bool {
    let herdr = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
    std::process::Command::new(herdr)
        .arg("--default-config")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("glue = true"))
}

pub fn run() -> Result<(), String> {
    println!("==> 1/4 Claude Code statusLine wrapper");
    for line in context::claude_hook("install")?.lines() {
        println!("    {line}");
    }

    println!("==> 2/4 Herdr with the glue patch");
    if glue_supported() {
        println!("    glue supported");
    } else {
        println!("    WARNING: this herdr has no `glue` token option, so the context bar");
        println!("    renders as \"used · rail\". Build a patched herdr with");
        println!("    {}/herdr-build.sh, install it, then rerun.", dock::root().display());
    }

    let path = config_path();
    println!("==> 3/4 Herdr config: {}", path.display());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "# herdr configuration\n".into());
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(copy) = path.exists().then(|| context::backup(&path)).flatten() {
        println!("    backup: {}", copy.display());
    }
    for note in patch(&mut doc) {
        println!("    {note}");
    }
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))?;

    println!("==> 4/4 reload Herdr config and dock usage");
    match herdr::call("server.reload_config", json!({})) {
        Ok(_) => {
            println!("    reloaded");
            match dock::restart() {
                Ok(true) => println!("    usage docked in this tab; other tabs dock as you visit them"),
                Ok(false) => println!("    the dock is hidden; ctrl+b u brings it back"),
                Err(e) => println!("    could not dock: {e}"),
            }
        }
        Err(_) => println!("    no running server (start herdr once; config applies then)"),
    }

    let _ = std::fs::write(crate::usage::state_dir().join("version"), crate::upgrade::VERSION);
    println!();
    println!("done — context bars appear under each Claude session as its status line");
    println!("refreshes, and the 5h and weekly windows sit in a dock along the bottom of");
    println!("every tab: the prefix key then u or U (ctrl+b u) shows or hides it, and");
    println!("its ✕ button hides it.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_an_old_config_and_stays_put() {
        let old = r#"# mine
[ui]
sidebar_width = 34

[[keys.command]]
key = ["prefix+u", "ctrl+u"]
type = "plugin_action"
command = "herdr-pacer.open"
description = "open usage"

[[keys.command]]
key = "prefix+shift+m"
type = "plugin_action"
command = "herdr-pacer.dock-collapse"
description = "y"

[[keys.command]]
key = "prefix+g"
type = "shell"
command = "lazygit"
"#;
        let mut doc: DocumentMut = old.parse().unwrap();
        patch(&mut doc);
        let once = doc.to_string();
        assert!(once.starts_with("# mine"));
        assert!(!once.contains("sidebar_width") && !once.contains("dock-collapse") && !once.contains("ctrl+u"));
        assert!(once.contains(r#"key = ["prefix+u", "prefix+shift+u"]"#) && once.contains(TOGGLE));
        assert!(once.contains("lazygit") && once.contains("$pacer_ctx_ok"));
        let mut again: DocumentMut = once.parse().unwrap();
        let notes = patch(&mut again);
        assert_eq!(again.to_string(), once);
        assert_eq!(notes, ["sidebar bar rows: unchanged", "dock key already bound"]);
    }

    #[test]
    fn tab_bar_commands_come_and_go_beside_the_users_own() {
        let mut doc: DocumentMut = "[ui]\ntab_bar_right = [{ type = \"zoom\" }, { type = \"datetime\" }]\n".parse().unwrap();
        let exe = std::path::Path::new("/p/it's/herdr-pacer");
        let keys = ["claude", "openai-codex", "opencode-go"];
        assert!(put_tabbar(&mut doc, &keys, Some(exe)));
        let on = doc.to_string();
        assert!(on.contains(r#"{ type = "zoom" }, { type = "datetime" }"#), "the user's own come first: {on}");
        assert_eq!(on.matches(" tabbar ").count(), 3);
        assert!(on.contains(r#"'/p/it'\''s/herdr-pacer' tabbar claude"#));
        assert!(on.find("tabbar claude") < on.find("tabbar openai-codex"), "in the chosen order");
        assert!(!put_tabbar(&mut doc, &keys, Some(exe)), "already there");
        assert!(put_tabbar(&mut doc, &[keys[1], keys[0], keys[2]], Some(exe)), "reordered");
        assert!(put_tabbar(&mut doc, &[], Some(exe)));
        assert_eq!(doc.to_string().matches("tabbar").count(), 0);
        assert!(doc.to_string().contains("zoom"));
        let mut empty: DocumentMut = "[ui]\n".parse().unwrap();
        assert!(!put_tabbar(&mut empty, &[], Some(exe)), "off, and nothing to remove");
    }

    #[test]
    fn a_custom_dock_key_is_kept() {
        let mut doc: DocumentMut =
            "[[keys.command]]\nkey = \"alt+u\"\ntype = \"plugin_action\"\ncommand = \"herdr-pacer.dock-toggle\"\n"
                .parse()
                .unwrap();
        patch(&mut doc);
        assert!(doc.to_string().contains(r#"key = "alt+u""#));
    }
}
