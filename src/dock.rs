//! Keeps a usage pane docked along the bottom of every tab.
//!
//! Herdr has no pane placement for this, so it is built from stock calls, the
//! way herdr-sidebar builds its side column: split the bottom-left pane down,
//! then widen the dock by bouncing each pane beside it through a temporary tab
//! and splitting it back next to the pane above — pane.move keeps their
//! processes. Whether it is hidden lives in settings.json.

use crate::herdr::{self, call, Result};
use crate::settings;
use serde_json::{json, Value};
use std::fs::File;
use std::path::PathBuf;

const MARK: &str = "herdr-pacer\" dock"; // how a live dock is told from other panes
const LABEL: &str = "usage"; // the manifest pane's title
const FLOOR: f64 = 0.1; // herdr clamps every split ratio to 0.1..0.9

fn hidden() -> bool {
    settings::load().hidden
}

fn set_hidden(value: bool) {
    settings::save(&settings::Settings { hidden: value, ..settings::load() });
}

/// The dock's height: its border, a title row, and a row per window shown.
fn rows() -> f64 {
    let windows = settings::load().windows.iter().filter(|&&on| on).count().max(1);
    (3 + windows) as f64
}

/// The plugin checkout: $HERDR_PLUGIN_ROOT, or three levels above
/// target/release/herdr-pacer.
pub fn root() -> PathBuf {
    std::env::var_os("HERDR_PLUGIN_ROOT").map(PathBuf::from).unwrap_or_else(|| {
        let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).unwrap_or_default();
        exe.ancestors().nth(3).map(PathBuf::from).unwrap_or_default()
    })
}

// ── layout math (pure; the tests cover it) ──

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}

fn rect(v: &Value) -> Rect {
    let n = |k: &str| v[k].as_i64().unwrap_or(0);
    Rect { x: n("x"), y: n("y"), w: n("width"), h: n("height") }
}

fn walk(node: &Value) -> Vec<&Value> {
    match node["type"].as_str() {
        Some("pane") => vec![node],
        _ => [walk(&node["first"]), walk(&node["second"])].concat(),
    }
}

/// A live dock runs `herdr-pacer dock`. Herdr restores panes without their
/// command, so a restored one is a shell that kept the label and our cwd.
fn is_dock(pane: &Value, root: &str) -> bool {
    match pane["command"].as_array() {
        Some(argv) => {
            let line = argv.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ");
            line.contains(MARK) || line.contains("usage.sh\" dock") // a dock from before the port
        }
        None => pane["label"] == LABEL && pane["cwd"] == root,
    }
}

fn bottom_left(panes: &[(String, Rect)], area: Rect) -> Option<String> {
    panes.iter().find(|(_, r)| r.x == 0 && r.y + r.h == area.h).map(|(id, _)| id.clone())
}

fn full_width(panes: &[(String, Rect)], area: Rect, dock: &str) -> bool {
    panes.iter().any(|(id, r)| id == dock && r.w >= area.w)
}

/// (pane beside the dock, pane above the dock), or None once the dock spans
/// the tab. Moving the first next to the second widens the dock.
fn widen_step(panes: &[(String, Rect)], area: Rect, dock: &str) -> Option<(String, String)> {
    let me = panes.iter().find(|(id, _)| id == dock)?.1;
    if me.w >= area.w {
        return None;
    }
    let beside = panes.iter().find(|(id, r)| id != dock && r.x == me.x + me.w && r.y <= me.y && me.y < r.y + r.h)?;
    let above = panes.iter().find(|(id, r)| id != dock && r.x == me.x && r.y + r.h == me.y)?;
    Some((beside.0.clone(), above.0.clone()))
}

/// The share the panes above keep, so the dock gets `rows` rows.
fn dock_ratio(rows: f64, height: i64) -> f64 {
    1.0 - (rows / height.max(1) as f64).clamp(FLOOR, 1.0 - FLOOR)
}

// ── herdr ──

fn tree(tab: &str) -> Result<Value> {
    Ok(call("layout.export", json!({ "tab_id": tab }))?["layout"].take())
}

fn geometry(tab: &str) -> Result<(Vec<(String, Rect)>, Rect)> {
    let layout = tree(tab)?;
    let first = walk(&layout["root"]).first().and_then(|p| p["pane_id"].as_str()).unwrap_or_default().to_string();
    let g = call("pane.layout", json!({ "pane_id": first }))?;
    let panes = g["layout"]["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| (p["pane_id"].as_str().unwrap_or_default().to_string(), rect(&p["rect"])))
        .collect();
    Ok((panes, rect(&g["layout"]["area"])))
}

fn tabs() -> Result<Vec<Value>> {
    Ok(call("tab.list", json!({}))?["tabs"].as_array().cloned().unwrap_or_default())
}

fn focused_tab() -> Result<Option<String>> {
    Ok(tabs()?.iter().find(|t| t["focused"] == true).and_then(|t| t["tab_id"].as_str()).map(String::from))
}

fn event_tab() -> Result<Option<String>> {
    let event: Value = std::env::var("HERDR_PLUGIN_EVENT_JSON")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let data = if event["data"].is_object() { &event["data"] } else { &event };
    let tab = data["tab_id"].as_str().or(data["tab"]["tab_id"].as_str()).map(String::from);
    match tab.or_else(|| std::env::var("HERDR_TAB_ID").ok()) {
        Some(tab) => Ok(Some(tab)),
        None => focused_tab(),
    }
}

fn close_docks(tab: &str, keep: Option<&str>) -> Result<()> {
    let root = root().to_string_lossy().into_owned();
    let layout = tree(tab)?;
    for pane in walk(&layout["root"]) {
        let id = pane["pane_id"].as_str().unwrap_or_default();
        if is_dock(pane, &root) && Some(id) != keep {
            call("pane.close", json!({ "pane_id": id }))?;
        }
    }
    Ok(())
}

fn open_dock(tab: &str) -> Result<()> {
    let (panes, area) = geometry(tab)?;
    let Some(target) = bottom_left(&panes, area) else { return Ok(()) };
    let opened = call(
        "plugin.pane.open",
        json!({ "plugin_id": "herdr-pacer", "entrypoint": "dock", "placement": "split",
                "target_pane_id": target, "direction": "down", "focus": false }),
    )?;
    let dock = opened["plugin_pane"]["pane"]["pane_id"].as_str().unwrap_or_default().to_string();
    for _ in 0..8 {
        let (panes, area) = geometry(tab)?;
        let Some((beside, above)) = widen_step(&panes, area, &dock) else { break };
        let bounced = call("pane.move", json!({ "pane_id": beside, "destination": { "type": "new_tab" }, "focus": false }))?;
        call(
            "pane.move",
            json!({ "pane_id": bounced["move_result"]["pane"]["pane_id"], "focus": false,
                    "destination": { "type": "tab", "tab_id": tab, "target_pane_id": above, "split": "right" } }),
        )?;
    }
    size(tab)
}

/// Gives the dock its height, when it spans the tab as the root split's bottom.
fn size(tab: &str) -> Result<()> {
    let root_node = tree(tab)?["root"].take();
    let own_root = root().to_string_lossy().into_owned();
    if root_node["type"] == "split" && root_node["direction"] == "down" && is_dock(&root_node["second"], &own_root) {
        let (_, area) = geometry(tab)?;
        call("layout.set_split_ratio", json!({ "tab_id": tab, "path": [], "ratio": dock_ratio(rows(), area.h) }))?;
    }
    Ok(())
}

/// Resizes every dock to the windows now shown, after a settings change.
pub fn resize_all() -> Result<()> {
    let _lock = lock(true);
    for tab in tabs()? {
        let _ = size(tab["tab_id"].as_str().unwrap_or_default());
    }
    Ok(())
}

/// Adds a dock when the tab has none along its bottom. One already there is
/// left alone, so a size dragged by hand stays and nothing redraws.
fn ensure(tab: &str) -> Result<()> {
    let Ok(layout) = tree(tab) else {
        return Ok(()); // gone already, e.g. a bounce tab
    };
    if layout["zoomed"] == true {
        return Ok(());
    }
    let root = root().to_string_lossy().into_owned();
    let panes = walk(&layout["root"]);
    let found: Vec<&str> = panes.iter().filter(|p| is_dock(p, &root)).filter_map(|p| p["pane_id"].as_str()).collect();
    if !found.is_empty() && panes.len() == 1 {
        return Ok(()); // the dock is the tab's only pane
    }
    let (rects, area) = geometry(tab)?;
    let live = |id: &str| panes.iter().any(|p| p["pane_id"] == id && p["command"].is_array());
    let keep = found.iter().copied().find(|d| live(d) && full_width(&rects, area, d));
    if found.len() > keep.is_some() as usize {
        close_docks(tab, keep)?; // restored shells, duplicates
    }
    if keep.is_none() {
        let focused = layout["focused_pane_id"].as_str().unwrap_or_default().to_string();
        open_dock(tab)?;
        if tree(tab)?["focused_pane_id"] != focused.as_str() {
            call("pane.focus", json!({ "pane_id": focused }))?;
        }
    }
    Ok(())
}

/// Hides the dock in every tab, or brings it back in the current one (the
/// others dock as they are visited).
pub fn toggle() -> Result<()> {
    let _lock = lock(true);
    set_hidden(!hidden());
    if !hidden() {
        return focused_tab()?.map_or(Ok(()), |tab| ensure(&tab));
    }
    // the tab we run from goes last: its dock may be the caller (the ✕ button),
    // and closing that pane ends this process
    let here = std::env::var("HERDR_TAB_ID").ok();
    let mut all = tabs()?;
    all.sort_by_key(|t| t["tab_id"].as_str() == here.as_deref());
    for tab in all {
        close_docks(tab["tab_id"].as_str().unwrap_or_default(), None)?;
    }
    Ok(())
}

/// Closes every dock and docks the current tab again, so an upgrade replaces
/// docks still running an older build. False when the dock is hidden.
pub fn restart() -> Result<bool> {
    let _lock = lock(true);
    for tab in tabs()? {
        close_docks(tab["tab_id"].as_str().unwrap_or_default(), None)?;
    }
    if hidden() {
        return Ok(false);
    }
    focused_tab()?.map_or(Ok(()), |tab| ensure(&tab))?;
    Ok(true)
}

/// The tab and workspace hooks. Focus events come in bursts, so a busy lock
/// means another copy is already on it.
pub fn ensure_event() -> Result<()> {
    let Some(_lock) = lock(false) else { return Ok(()) };
    if hidden() {
        return Ok(());
    }
    event_tab()?.map_or(Ok(()), |tab| ensure(&tab))
}

fn lock(blocking: bool) -> Option<File> {
    let file = File::create(crate::usage::state_dir().join("dock.lock")).ok()?;
    match blocking {
        true => file.lock().ok()?,
        false => file.try_lock().ok()?,
    }
    Some(file)
}

pub fn run(command: &str) -> herdr::Result<()> {
    match command {
        "ensure" => ensure_event(),
        "toggle" => toggle(),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, x: i64, y: i64, w: i64, h: i64) -> (String, Rect) {
        (id.into(), Rect { x, y, w, h })
    }

    #[test]
    fn widening_a_bottom_dock() {
        let area = Rect { x: 0, y: 0, w: 100, h: 40 };
        let (left, dock, right) = (pane("l", 0, 0, 50, 35), pane("d", 0, 35, 50, 5), pane("r", 50, 0, 50, 40));
        assert_eq!(bottom_left(&[right.clone(), left.clone(), dock.clone()], area).as_deref(), Some("d"));
        assert_eq!(bottom_left(&[right.clone(), left.clone()], area), None);
        assert_eq!(widen_step(&[left.clone(), dock, right], area, "d"), Some(("r".into(), "l".into())));
        let wide = pane("d", 0, 35, 100, 5);
        assert_eq!(widen_step(&[left, wide.clone()], area, "d"), None);
        assert!(full_width(&[wide], area, "d"));
        assert!((dock_ratio(5.0, 40) - 0.875).abs() < 1e-9 && (dock_ratio(5.0, 80) - 0.9).abs() < 1e-9);
    }

    #[test]
    fn telling_docks_apart() {
        let live = json!({ "command": ["sh", "-c", "exec \"/p/target/release/herdr-pacer\" dock"] });
        let old = json!({ "command": ["sh", "-c", "exec \"/p/usage.sh\" dock"] });
        let restored = json!({ "label": LABEL, "cwd": "/p" });
        assert!(is_dock(&live, "/p") && is_dock(&old, "/p") && is_dock(&restored, "/p"));
        assert!(!is_dock(&json!({ "label": LABEL, "cwd": "/home" }), "/p"));
        assert!(!is_dock(&json!({ "label": LABEL, "command": ["vim"] }), "/p"));
    }
}
