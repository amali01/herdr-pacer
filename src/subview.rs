//! A Claude subagent's live view: what the pane tree.rs opens for it runs
//! (`herdr-pacer subview`). It follows the log Claude keeps of the subagent,
//! printing its task, its tool calls and what it says; lists the pane as an
//! agent, named in the tree after the task once Claude writes it down; keeps
//! its context bar; and ends when the subagent finishes or the Claude that
//! runs it is gone.

use crate::herdr::call;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::Duration;

pub fn run() -> Result<(), String> {
    let var = |k: &str| std::env::var(k).map_err(|_| format!("{k} is not set: a subagent's pane sets it"));
    let (log, meta, parent, me) = (var("PACER_SUBAGENT_LOG")?, var("PACER_SUBAGENT_META")?, var("PACER_SUBAGENT_PARENT")?, var("HERDR_PANE_ID")?);
    let (mut read, mut named, mut shown, mut tasked, mut placed) = (0u64, None::<String>, None, false, String::new());
    // an agent in Herdr's list
    call("pane.report_agent", json!({ "pane_id": me, "source": "herdr-pacer", "agent": "claude", "state": "working" })).map_err(|e| e.to_string())?;
    loop {
        let summary: Value = std::fs::read_to_string(&meta).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // named by its task, its type until Claude writes the task down; a
        // token, since Herdr's agent names cannot hold a sentence
        let task = summary["description"].as_str().or(summary["agentType"].as_str());
        if let Some(task) = task.filter(|t| named.as_deref() != Some(*t)) {
            if crate::herdr::report_tokens(&me, json!({ "pacer_task": task }), None).is_ok() {
                named = Some(task.to_string());
                let _ = crate::tree::sync();
            }
        }
        // a subagent started by another sits under that one's view while it
        // is open, under the agent running them all otherwise
        let up = summary["parentAgentId"].as_str().and_then(|id| crate::tree::view_of(&parent, id));
        let up = up.filter(|v| !crate::herdr::gone("pane.get", json!({ "pane_id": v }))).unwrap_or_else(|| parent.clone());
        if up != placed && crate::herdr::report_tokens(&me, json!({ "pacer_parent": up }), None).is_ok() {
            placed = up;
            let _ = crate::tree::sync();
        }
        if let Ok(mut file) = std::fs::File::open(&log) {
            let _ = file.seek(SeekFrom::Start(read));
            let mut lines = BufReader::new(file);
            let mut line = String::new();
            // a line Claude is still writing waits for its newline
            while lines.read_line(&mut line).is_ok_and(|n| n > 0) && line.ends_with('\n') {
                read += line.len() as u64;
                let entry: Value = serde_json::from_str(&line).unwrap_or_default();
                // the first message is the task; later ones are notes Claude adds to it
                let note = entry["type"] == "user" && std::mem::replace(&mut tasked, true);
                if let Some(text) = render(&entry).filter(|_| !note) {
                    println!("{text}");
                }
                let pct = context(&entry, summary["model"].as_str().unwrap_or_default());
                if pct.is_some() && pct != shown {
                    let _ = crate::context::report(&me, pct, 21_600_000);
                    shown = pct;
                }
                line.clear();
            }
        }
        // done, or its Claude gone without a stop (killed, crashed) while
        // its pane may stay open
        if summary["finished"] == true || crate::herdr::gone("agent.get", json!({ "target": parent })) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// One entry of the log as the view prints it: the task, then each tool call
/// and what the subagent says. Tool results and Claude's own notes are left
/// out.
fn render(entry: &Value) -> Option<String> {
    let first = |text: &str, n: usize| text.trim().lines().take(n).collect::<Vec<_>>().join("\n  ");
    let content = &entry["message"]["content"];
    match entry["type"].as_str()? {
        // Claude adds notes after the task, each a <tag> block of its own
        "user" => content.as_str().map(|task| task.split("\n<").next().unwrap_or(task)).map(|task| format!("▸ {}", first(task, 4))),
        "assistant" => {
            let said: Vec<String> = content
                .as_array()?
                .iter()
                .filter_map(|item| match item["type"].as_str()? {
                    "text" => Some(format!("  {}", first(item["text"].as_str()?, 6))).filter(|t| !t.trim().is_empty()),
                    "tool_use" => {
                        let input = &item["input"];
                        let what = ["description", "command", "file_path", "pattern", "url", "prompt"].iter().find_map(|k| input[k].as_str());
                        let what: String = what.unwrap_or_default().lines().next().unwrap_or_default().chars().take(70).collect();
                        Some(format!("⚙ {} {what}", item["name"].as_str()?))
                    }
                    _ => None,
                })
                .collect();
            (!said.is_empty()).then(|| said.join("\n"))
        }
        _ => None,
    }
}

/// How full the subagent's context window is, from a reply's token counts.
/// shortcut: a window of 200k, or 1M for a `[1m]` model; read the model's real
/// size once Claude records it.
fn context(entry: &Value, model: &str) -> Option<u32> {
    let usage = &entry["message"]["usage"];
    let used: u64 = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"].iter().filter_map(|k| usage[k].as_u64()).sum();
    let model = entry["message"]["model"].as_str().unwrap_or(model);
    let window: u64 = if model.contains("[1m]") { 1_000_000 } else { 200_000 };
    (entry["type"] == "assistant" && used > 0).then(|| (used * 100 / window).min(100) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_prints_the_task_the_tools_and_what_is_said() {
        let task = json!({ "type": "user", "message": { "content": "Review the grant terms\nand report back\n<context_window_protection>\n  rules" } });
        assert_eq!(render(&task).unwrap(), "▸ Review the grant terms\n  and report back");
        let turn = json!({ "type": "assistant", "message": { "content": [
            { "type": "text", "text": "Looking at the terms." },
            { "type": "tool_use", "name": "Bash", "input": { "command": "ls src\nmore" } },
        ] } });
        assert_eq!(render(&turn).unwrap(), "  Looking at the terms.\n⚙ Bash ls src");
        let result = json!({ "type": "user", "message": { "content": [{ "type": "tool_result" }] } });
        assert!(render(&result).is_none() && render(&json!({ "type": "attachment" })).is_none());
    }

    #[test]
    fn a_reply_tells_how_full_the_context_is() {
        let reply = |model: &str| json!({ "type": "assistant", "message": { "model": model, "usage": {
            "input_tokens": 10_000, "cache_read_input_tokens": 30_000, "cache_creation_input_tokens": 10_000, "output_tokens": 999 } } });
        assert_eq!(context(&reply("claude-haiku"), ""), Some(25));
        assert_eq!(context(&reply("claude-opus[1m]"), ""), Some(5));
        assert_eq!(context(&json!({ "type": "user" }), ""), None);
    }
}
