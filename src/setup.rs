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

    // the bar rows under each agent
    notes.push(format!("sidebar bar rows: {}", put_rows(doc)));

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

/// Rewrites the sidebar rows after the layout changes, and reloads Herdr.
pub fn apply_rows() -> Result<(), String> {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    if put_rows(&mut doc) != "unchanged" {
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
    if path.exists() {
        let copy = path.with_extension(format!("toml.bak-{}", crate::usage::now()));
        std::fs::copy(&path, &copy).map_err(|e| e.to_string())?;
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
    fn a_custom_dock_key_is_kept() {
        let mut doc: DocumentMut =
            "[[keys.command]]\nkey = \"alt+u\"\ntype = \"plugin_action\"\ncommand = \"herdr-pacer.dock-toggle\"\n"
                .parse()
                .unwrap();
        patch(&mut doc);
        assert!(doc.to_string().contains(r#"key = "alt+u""#));
    }
}
