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

/// The segment for `key` (an AGENTS key), or None when it has none. A
/// provider signed in more than once shows each account, and drops the bars
/// for the numbers when they would not fit Herdr's 80 characters, then the
/// accounts that still do not.
pub fn segment(rows: &[Row], s: &Settings, key: &str) -> Option<String> {
    let i = AGENTS.iter().position(|(k, _)| *k == key)?;
    if !s.tab_agents[i] {
        return None;
    }
    let agents: Vec<_> = usage::agents(rows).into_iter().filter(|a| a.key == key).collect();
    let line = |style: Option<usage::Style>| {
        let parts: Vec<String> = agents
            .iter()
            .filter_map(|agent| {
                let windows: Vec<String> = (0..3)
                    .filter(|&w| s.tab_windows[w])
                    .filter_map(|w| {
                        let r = agent.windows()[w].as_ref()?;
                        Some(match style {
                            None => format!("{} {}%", LABELS[w], r.pct),
                            Some(style) => {
                                let (used, rail) = usage::bar(r.pct, CELLS, style, s.tab_size);
                                format!("{} {used}{rail} {}%", LABELS[w], r.pct)
                            }
                        })
                    })
                    .collect();
                (!windows.is_empty()).then(|| format!("{} {}", agent.title, windows.join(" · ")))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("   "))
    };
    let fits = |l: &String| l.chars().count() <= 80;
    line(s.tab_style).filter(fits).or_else(|| {
        // still too long: the accounts that fit, since Herdr drops a longer line whole
        let numbers = line(None)?;
        let mut kept = String::new();
        for part in numbers.split("   ") {
            let next = if kept.is_empty() { part.to_string() } else { format!("{kept}   {part}") };
            if !fits(&next) {
                break;
            }
            kept = next;
        }
        (!kept.is_empty()).then_some(kept)
    })
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
        Row { provider: provider.into(), account: String::new(), plan: String::new(), window: window.into(), pct, resets: None, error: None }
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

    #[test]
    fn each_claude_account_in_one_segment() {
        let second = |r: Row| Row { account: "/h/.claude-2".into(), ..r };
        let mut rows = vec![row("claude", "5h", 98), second(row("claude", "5h", 12))];
        let s = Settings { tab_agents: [false, true, false], tab_windows: [true, false, false], ..Settings::default() };
        assert_eq!(segment(&rows, &s, "claude").as_deref(), Some("Claude 5h ⣤⣤⣤⣤⣤⣤ 98%   Claude-2 5h ⡄⣀⣀⣀⣀⣀ 12%"));
        rows.extend([row("claude", "7d", 34), row("claude", "30d", 5), second(row("claude", "7d", 40))]);
        let all = Settings { tab_windows: [true; 3], ..s };
        assert_eq!(
            segment(&rows, &all, "claude").as_deref(),
            Some("Claude 5h 98% · wk 34% · mo 5%   Claude-2 5h 12% · wk 40%"),
            "the numbers alone when the bars would pass 80"
        );
        let many: Vec<Row> = (2..9).map(|n| Row { account: format!("/h/.claude-{n}"), ..row("claude", "5h", 50) }).collect();
        let line = segment(&many, &all, "claude").unwrap();
        assert!(line.chars().count() <= 80 && line.ends_with('%'), "whole accounts, as many as fit: {line}");
    }
}
