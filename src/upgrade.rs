//! Updating in place.
//!
//! `herdr plugin install` swaps the checkout and rebuilds, but runs nothing of
//! ours afterwards, so the first hook to run a new build migrates whatever the
//! last one left: the statusLine wrapper, the rows and key in config.toml,
//! state files, and docks still running the old build. A stamp of the
//! version it last migrated to makes that a one-time step.
//!
//! `update` is the version-aware install: it compares this build with the
//! version published on GitHub and reinstalls only when that one is newer.

use crate::{context, dock, herdr, settings, setup, usage};
use std::fs;
use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "amali01/herdr-pacer";

fn stamp() -> PathBuf {
    usage::state_dir().join("version")
}

/// Files older builds kept that nothing reads any more.
fn leftovers(state: &Path) -> Vec<PathBuf> {
    let mut old: Vec<PathBuf> = ["usage-rows", "usage-rows.tmp", "sidebar-space"].iter().map(|f| state.join(f)).collect();
    old.push(state.join("usage-rows.lock")); // a directory, when it survived a crash
    old
}

/// Where builds before 0.7.1 kept state outside Herdr.
fn old_state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state"), PathBuf::from)
        .join("herdr-pacer")
}

/// Deletes the old files and the old state directory; dock.json moves into
/// settings.json first. Returns how many went.
fn clean(state: &Path, old_dir: &Path, config: &Path) -> usize {
    let mut removed = 0;
    for path in leftovers(state) {
        let gone = if path.is_dir() { fs::remove_dir(&path) } else { fs::remove_file(&path) };
        removed += gone.is_ok() as usize;
    }
    if old_dir != state && old_dir.is_dir() && fs::remove_dir_all(old_dir).is_ok() {
        removed += 1;
    }
    let dock_json = config.join("dock.json");
    if dock_json.exists() {
        if !config.join("settings.json").exists() {
            settings::save(&settings::load()); // carries `hidden` over
        }
        removed += fs::remove_file(dock_json).is_ok() as usize;
    }
    removed
}

/// Once per new build: bring an existing install up to this one. Changes
/// only what an earlier setup put in place, so a fresh install stays for
/// `setup` to do.
pub fn migrate(force: bool) {
    if !force && fs::read_to_string(stamp()).is_ok_and(|s| s.trim() == VERSION) {
        return;
    }
    let Ok(lock) = fs::File::create(usage::state_dir().join("upgrade.lock")) else { return };
    if lock.try_lock().is_err() {
        return; // another hook is on it
    }
    clean(&usage::state_dir(), &old_state_dir(), &settings::dir());
    let _ = context::claude_hook("refresh");
    if setup::configured() {
        let _ = setup::apply_config();
    }
    let _ = dock::restart();
    let _ = fs::write(stamp(), VERSION);
}

// ── update: install only a newer version ──

fn parse(version: &str) -> Vec<u64> {
    version.trim().trim_start_matches('v').split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect()
}

/// Newer, by numeric dotted parts: 0.10.0 is newer than 0.9.9.
pub fn newer(candidate: &str, than: &str) -> bool {
    parse(candidate) > parse(than)
}

fn published() -> Result<String, String> {
    let url = format!("https://raw.githubusercontent.com/{REPO}/main/herdr-plugin.toml");
    let body = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| format!("could not reach GitHub: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let manifest: toml_edit::DocumentMut = body.parse().map_err(|e| format!("published manifest: {e}"))?;
    manifest["version"].as_str().map(String::from).ok_or_else(|| "published manifest has no version".into())
}

pub fn update() -> Result<String, String> {
    let root = dock::root();
    if !root.components().any(|c| c.as_os_str() == "github") {
        return Ok(format!(
            "herdr-pacer {VERSION} is linked from {}; pull and `cargo build --release` there instead",
            root.display()
        ));
    }
    let latest = published()?;
    if !newer(&latest, VERSION) {
        let note = if newer(VERSION, &latest) { " (newer than the published one)" } else { "" };
        return Ok(format!("herdr-pacer {VERSION} is up to date{note}; GitHub has {latest}"));
    }
    let herdr = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
    let status = std::process::Command::new(&herdr)
        .args(["plugin", "install", REPO, "--yes"])
        .status()
        .map_err(|e| format!("{herdr}: {e}"))?;
    if !status.success() {
        return Err(format!("herdr plugin install failed; {VERSION} is still installed"));
    }
    // the new build migrates now rather than on the next hook
    let _ = std::process::Command::new(root.join("target/release/herdr-pacer")).arg("migrate").status();
    let _ = herdr::call("server.reload_config", serde_json::json!({}));
    Ok(format!("updated herdr-pacer {VERSION} → {latest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("0.7.1", "0.7.0") && newer("0.10.0", "0.9.9") && newer("1.0.0", "0.99"));
        assert!(!newer("0.7.0", "0.7.0") && !newer("0.6.9", "0.7.0") && !newer("v0.7.0", "0.7.0"));
        assert!(newer("0.8.0-rc1", "0.7.9"));
    }

    #[test]
    fn cleaning_removes_only_what_older_builds_left() {
        let state = std::env::temp_dir().join(format!("herdr-pacer-clean-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(state.join("usage-rows.lock")).unwrap();
        for f in ["usage-rows", "sidebar-space", "usage.json", "context.json"] {
            fs::write(state.join(f), "x").unwrap();
        }
        fs::create_dir_all(state.join("xdg/herdr-pacer")).unwrap();
        assert_eq!(clean(&state, &state.join("xdg/herdr-pacer"), &state.join("config")), 4);
        let left: Vec<String> = fs::read_dir(&state).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert!(left.contains(&"usage.json".into()) && left.contains(&"context.json".into()));
        assert!(!state.join("xdg/herdr-pacer").exists() && !state.join("usage-rows").exists());
        let _ = fs::remove_dir_all(&state);
    }
}
