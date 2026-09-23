//! What the dock, the sidebar and the tab bar show, as the settings popup
//! edits it.
//! Stored as settings.json in the plugin's config directory.

use crate::usage::Style;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

pub const AGENTS: [(&str, &str); 3] = [("openai-codex", "Codex"), ("claude", "Claude"), ("opencode-go", "OpenCode")];
pub const WINDOWS: [(&str, &str); 3] = [("5h", "5h"), ("week", "Weekly"), ("month", "Monthly")];
/// Sidebar metrics: settings key, token prefix, row label.
pub const METRICS: [(&str, &str, &str); 3] = [("context", "ctx", "ctx"), ("5h", "5h", "5h"), ("week", "wk", "wk")];

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub hidden: bool,
    /// Dock: agents and windows shown, and how its bars are drawn — a style
    /// and its size (1–3; see `Style`).
    pub agents: [bool; 3],
    pub windows: [bool; 3],
    pub dock_style: Style,
    pub dock_size: u8,
    /// Sidebar: metrics shown under each agent, one row each or all on one.
    pub metrics: [bool; 3],
    pub one_line: bool,
    pub sidebar_style: Style,
    pub sidebar_size: u8,
    /// Tab bar: agents with a segment (none: the view is off), windows shown,
    /// and a style, None for the numbers alone.
    pub tab_agents: [bool; 3],
    pub tab_windows: [bool; 3],
    pub tab_style: Option<Style>,
    pub tab_size: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hidden: false,
            agents: [true; 3],
            windows: [true, true, false],
            dock_style: Style::Dots,
            dock_size: 2,
            metrics: [true, false, false],
            one_line: false,
            sidebar_style: Style::Dots,
            sidebar_size: 2,
            tab_agents: [false; 3],
            tab_windows: [true, true, false],
            tab_style: Some(Style::Dots),
            tab_size: 2,
        }
    }
}

pub fn dir() -> PathBuf {
    std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME")
            .map_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"), PathBuf::from)
            .join("herdr/plugins/config/herdr-pacer")
    })
}

pub fn path() -> PathBuf {
    dir().join("settings.json")
}

fn flags<const N: usize>(v: &Value, keys: [&str; N], default: [bool; N]) -> [bool; N] {
    let mut out = default;
    for (i, k) in keys.iter().enumerate() {
        if let Some(b) = v[k].as_bool() {
            out[i] = b;
        }
    }
    out
}

/// A section's size; saved as "size", or "dots" by 0.8 and before.
fn size(v: &Value, default: u8) -> u8 {
    v["size"].as_u64().or(v["dots"].as_u64()).map_or(default, |d| d.clamp(1, 3) as u8)
}

fn style(v: &Value, default: Style) -> Style {
    v["style"].as_str().and_then(Style::from_name).unwrap_or(default)
}

pub fn load() -> Settings {
    let d = Settings::default();
    let read = |p: PathBuf| fs::read_to_string(p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let Some(v) = read(path()) else {
        // before the settings popup, dock.json held only whether it was hidden
        let hidden = read(dir().join("dock.json")).is_some_and(|v| v["hidden"] == true);
        return Settings { hidden, ..d };
    };
    Settings {
        hidden: v["hidden"].as_bool().unwrap_or(false),
        agents: flags(&v["dock"]["agents"], AGENTS.map(|a| a.0), d.agents),
        windows: flags(&v["dock"]["windows"], WINDOWS.map(|w| w.0), d.windows),
        dock_style: style(&v["dock"], d.dock_style),
        dock_size: size(&v["dock"], d.dock_size),
        metrics: flags(&v["sidebar"]["show"], METRICS.map(|m| m.0), d.metrics),
        one_line: v["sidebar"]["layout"] == "line",
        sidebar_style: style(&v["sidebar"], d.sidebar_style),
        sidebar_size: size(&v["sidebar"], d.sidebar_size),
        tab_agents: flags(&v["tabbar"]["agents"], AGENTS.map(|a| a.0), d.tab_agents),
        tab_windows: flags(&v["tabbar"]["windows"], WINDOWS.map(|w| w.0), d.tab_windows),
        // 0.8 saved the numbers alone as zero dots
        tab_style: match (v["tabbar"]["style"].as_str(), v["tabbar"]["dots"].as_u64()) {
            (Some("numbers"), _) | (None, Some(0)) => None,
            _ => Some(style(&v["tabbar"], Style::Dots)),
        },
        tab_size: size(&v["tabbar"], d.tab_size),
    }
}

fn object<const N: usize>(keys: [&str; N], values: [bool; N]) -> Value {
    Value::Object(keys.iter().zip(values).map(|(k, v)| (k.to_string(), json!(v))).collect())
}

pub fn save(s: &Settings) {
    let v = json!({
        "hidden": s.hidden,
        "dock": {
            "agents": object(AGENTS.map(|a| a.0), s.agents),
            "windows": object(WINDOWS.map(|w| w.0), s.windows),
            "style": s.dock_style.name(),
            "size": s.dock_size,
        },
        "sidebar": {
            "show": object(METRICS.map(|m| m.0), s.metrics),
            "layout": if s.one_line { "line" } else { "rows" },
            "style": s.sidebar_style.name(),
            "size": s.sidebar_size,
        },
        "tabbar": {
            "agents": object(AGENTS.map(|a| a.0), s.tab_agents),
            "windows": object(WINDOWS.map(|w| w.0), s.tab_windows),
            "style": s.tab_style.map_or("numbers", Style::name),
            "size": s.tab_size,
        },
    });
    let _ = fs::create_dir_all(dir());
    let tmp = path().with_extension("tmp");
    let body = serde_json::to_string_pretty(&v).unwrap_or_default() + "\n";
    let _ = fs::write(&tmp, body).and_then(|_| fs::rename(&tmp, path()));
}

/// Changes whenever the settings are saved, so a dock can notice.
pub fn stamp() -> Option<std::time::SystemTime> {
    fs::metadata(path()).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_fills_gaps() {
        let dir = std::env::temp_dir().join(format!("herdr-pacer-settings-{}", std::process::id()));
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", &dir);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load(), Settings::default());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("dock.json"), r#"{"hidden": true}"#).unwrap();
        assert!(load().hidden, "an older dock.json carries over");
        let s = Settings {
            agents: [false, true, true],
            dock_style: Style::Blocks,
            dock_size: 3,
            metrics: [true, true, false],
            one_line: true,
            sidebar_style: Style::Slants,
            tab_agents: [true, false, true],
            tab_style: None,
            ..load()
        };
        save(&s);
        assert_eq!(load(), s);
        fs::write(path(), r#"{"dock": {"dots": 9}}"#).unwrap();
        assert_eq!((load().dock_size, load().windows), (3, Settings::default().windows));
        // what 0.8 saved: sizes as "dots", the tab bar's numbers as zero dots
        fs::write(path(), r#"{"sidebar": {"dots": 1}, "tabbar": {"dots": 0}}"#).unwrap();
        let old = load();
        assert_eq!((old.sidebar_style, old.sidebar_size, old.tab_style), (Style::Dots, 1, None));
        let _ = fs::remove_dir_all(&dir);
    }
}
