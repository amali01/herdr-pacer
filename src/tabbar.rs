//! The tab bar view: one segment per agent at the right of Herdr's tab bar.
//!
//!   Codex 5h ⣤⣤⣤⣀⣀⣀ 48% · wk ⣤⣤⣀⣀⣀⣀ 31%   Claude 5h ⣤⣤⣤⣤⣤⣤ 98% · wk ⣤⣤⣀⣀⣀⣀ 34%
//!
//! Herdr runs each `[ui] tab_bar_right` command on an interval and shows the
//! last line it prints — plain text, 80 characters at most: Herdr strips color
//! from the tab bar, so here the dots alone carry how full a window is. An
//! agent switched off, or with nothing to show, prints nothing and Herdr
//! drops its segment.

use crate::settings::{self, Settings, AGENTS};
use crate::usage::{self, Row};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

const CELLS: usize = 6;
const LABELS: [&str; 3] = ["5h", "wk", "mo"];

/// The segment for `key` (an AGENTS key), or None when it has none.
pub fn segment(rows: &[Row], s: &Settings, key: &str) -> Option<String> {
    let i = AGENTS.iter().position(|(k, _)| *k == key)?;
    if !s.tab_agents[i] {
        return None;
    }
    let agent = usage::agents(rows).into_iter().find(|a| a.key == key)?;
    let windows: Vec<String> = (0..3)
        .filter(|&w| s.tab_windows[w])
        .filter_map(|w| {
            let r = agent.windows()[w].as_ref()?;
            Some(match s.tab_style {
                None => format!("{} {}%", LABELS[w], r.pct),
                Some(style) => {
                    let (used, rail) = usage::bar(r.pct, CELLS, style, s.tab_size);
                    format!("{} {used}{rail} {}%", LABELS[w], r.pct)
                }
            })
        })
        .collect();
    (!windows.is_empty()).then(|| format!("{} {}", agent.title, windows.join(" · ")))
}

/// Prints the segment. A stale cache gets a fetch in the background — in its
/// own process group, since Herdr ends this command's group when it exits —
/// so the tab bar never waits on a provider.
pub fn run(key: &str) {
    if usage::cache_age() >= usage::refresh_seconds() && !usage::fetching() {
        if let Ok(exe) = std::env::current_exe() {
            let _ = Command::new(exe)
                .arg("fetch")
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }
    if let Some(line) = segment(&usage::load(), &settings::load(), key) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(provider: &str, window: &str, pct: u32) -> Row {
        Row { provider: provider.into(), plan: String::new(), window: window.into(), pct, resets: None, error: None }
    }

    #[test]
    fn a_segment_per_agent_that_is_on() {
        let rows = [row("claude", "5h", 98), row("claude", "7d", 34), row("openai-codex", "5h", 48)];
        let on = Settings { tab_agents: [true, true, false], ..Settings::default() };
        assert_eq!(segment(&rows, &on, "claude").as_deref(), Some("Claude 5h ⣤⣤⣤⣤⣤⣤ 98% · wk ⣤⣤⣀⣀⣀⣀ 34%"));
        assert_eq!(segment(&rows, &on, "openai-codex").as_deref(), Some("Codex 5h ⣤⣤⣤⣀⣀⣀ 48%"));
        assert_eq!(segment(&rows, &on, "opencode-go"), None, "switched off");
        assert_eq!(segment(&rows, &Settings::default(), "claude"), None, "the view is off by default");
    }

    #[test]
    fn numbers_only_and_windows_follow_the_settings() {
        let rows = [row("claude", "5h", 98), row("claude", "7d", 34)];
        let s = Settings { tab_agents: [true; 3], tab_windows: [false, true, true], tab_style: None, ..Settings::default() };
        assert_eq!(segment(&rows, &s, "claude").as_deref(), Some("Claude wk 34%"));
        let thick = Settings { tab_style: Some(usage::Style::Dots), tab_size: 3, tab_windows: [true, false, false], ..s };
        assert_eq!(segment(&rows, &thick, "claude").as_deref(), Some("Claude 5h ⣶⣶⣶⣶⣶⣶ 98%"));
        let slants = Settings { tab_style: Some(usage::Style::Slants), ..thick.clone() };
        assert_eq!(segment(&rows, &slants, "claude").as_deref(), Some("Claude 5h ▰▰▰▰▰▰ 98%"));
        let three = [row("openai-codex", "5h", 100), row("openai-codex", "7d", 100), row("openai-codex", "30d", 100)];
        let all = Settings { tab_windows: [true; 3], ..thick };
        assert!(segment(&three, &all, "openai-codex").unwrap().chars().count() <= 80, "fits Herdr's 80");
    }
}
