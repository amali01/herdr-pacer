//! Bars under each agent row in the sidebar: its context window, and the 5h
//! and weekly windows of the account it runs on — whichever the settings pick,
//! one row each or all on one line.
//!
//! A bar rides on pane metadata tokens. Herdr's `rules` can only recolor a
//! token whose value parses as a number, and a bar is braille, so the color
//! travels in the token name: of pacer_<m>_{ok,warn,hot,crit} exactly one is
//! reported, carrying the label, the percentage and the used dots, and
//! config.toml colors each; pacer_<m> is the gray rail. Five tokens a metric
//! keeps three metrics inside Herdr's 16 tokens a row.
//!
//!   ctx 58% ⣤⣤⣤⣤⣤⣤⣀⣀⣀⣀        claude reports its own through `statusline`
//!   5h   5% ⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀        codex from its composer footer, opencode from
//!   wk  48% ⣤⣤⣤⣤⣤⣀⣀⣀⣀⣀        its SQLite; 5h and weekly from the usage cache

use crate::herdr::{self, call};
use crate::settings::{self, Settings, METRICS};
use crate::usage;
use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

const BUCKETS: [(&str, &str); 4] = [("ok", "#00af50"), ("warn", "#ffb055"), ("hot", "#e6c800"), ("crit", "#ff5555")];
const RAIL_FG: &str = "#46424e";

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// The tokens of metric `m` (an index into METRICS) at `pct`, or the ones
/// that clear it (`None`).
fn tokens(m: usize, pct: Option<u32>, s: &Settings) -> Map<String, Value> {
    let prefix = METRICS[m].1;
    let shown = s.metrics.iter().filter(|&&on| on).count();
    let rows_cells = env_u64("HERDR_PACER_CTX_CELLS", 10) as usize;
    // one line has to fit the sidebar: a bar for one metric, a stub for two,
    // just the numbers for three — glued to the metric before with a braille
    // blank, which Herdr does not trim the way it trims a space
    let cells = match (s.one_line, shown) {
        (false, _) | (true, 0..=1) => rows_cells,
        (true, 2) => 2,
        _ => 0,
    };
    let lead = if s.one_line && s.metrics[..m].contains(&true) { "⠀" } else { "" };
    let text = |p: u32, used: &str| {
        let label = if shown > 1 { METRICS[m].2 } else { "" };
        let pct = if s.one_line { format!("{p}%") } else { format!("{p:>3}%") };
        let head = match (label, s.one_line) {
            ("", _) => pct,
            (l, true) => format!("{l} {pct}"),
            (l, false) => format!("{l:<3} {pct}"),
        };
        if cells == 0 { format!("{lead}{head}") } else { format!("{lead}{head} {used}") }
    };
    let mut map = Map::new();
    let bar = pct.map(|p| usage::bar(p, cells, s.sidebar_dots));
    for (b, _) in BUCKETS {
        let mine = pct.is_some_and(|p| usage::bucket(p) == b);
        let value = match (&bar, mine) {
            (Some((used, _)), true) => json!(text(pct.unwrap_or(0), used)),
            _ => Value::Null,
        };
        map.insert(format!("pacer_{prefix}_{b}"), value);
    }
    let rail = bar.map(|(_, rail)| rail).filter(|r| !r.is_empty());
    map.insert(format!("pacer_{prefix}"), rail.map_or(Value::Null, Value::from));
    map
}

/// The tokens the bars used before the settings popup, cleared on sight.
fn legacy() -> Map<String, Value> {
    let mut map: Map<String, Value> = BUCKETS
        .iter()
        .flat_map(|(b, _)| [format!("pacer_ctx_{b}_u"), format!("pacer_ctx_{b}_t")])
        .map(|k| (k, Value::Null))
        .collect();
    map.insert("pacer_ctx".into(), Value::Null);
    map
}

// ── the rows config.toml carries ──

/// A metric's five tokens. After the first metric on one line they are glued
/// on, and the value brings its own leading blank.
fn group(prefix: &str, glued: bool) -> String {
    let glue = if glued { ", glue = true" } else { "" };
    let used: Vec<String> = BUCKETS
        .iter()
        .map(|(b, fg)| format!("{{ token = \"$pacer_{prefix}_{b}\", fg = \"{fg}\"{glue} }}"))
        .collect();
    format!("[{}, {{ token = \"$pacer_{prefix}\", fg = \"{RAIL_FG}\", glue = true }}]", used.join(", "))
}

/// `[ui.sidebar.agents] rows` for the chosen layout. Rows of metrics that are
/// switched off stay: Herdr hides a row whose tokens are all unreported.
pub fn rows_toml(s: &Settings) -> String {
    let mut rows = vec![r#"["state_icon", "machine", "workspace", "tab"]"#.to_string(), r#"["agent"]"#.to_string()];
    let groups: Vec<String> = METRICS.iter().enumerate().map(|(i, m)| group(m.1, s.one_line && i > 0)).collect();
    match s.one_line {
        true => rows.push(format!("[{}]", groups.iter().map(|g| &g[1..g.len() - 1]).collect::<Vec<_>>().join(", "))),
        false => rows.extend(groups),
    }
    format!("[ui.sidebar.agents]\nrows = [\n{}\n]\n", rows.iter().map(|r| format!("  {r},")).collect::<Vec<_>>().join("\n"))
}

// ── reporting ──

fn ctx_cache() -> PathBuf {
    usage::state_dir().join("context.json")
}

/// The last context percentage each pane reported, so a settings change can
/// redraw it without waiting for the agent's next turn.
fn remember(pane: &str, pct: Option<u32>) {
    let path = ctx_cache();
    let mut all: Map<String, Value> =
        std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let cutoff = usage::now() - 6 * 3600;
    all.retain(|_, v| v["at"].as_i64().unwrap_or(0) > cutoff);
    match pct {
        Some(p) => drop(all.insert(pane.into(), json!({ "pct": p, "at": usage::now() }))),
        None => drop(all.remove(pane)),
    }
    let _ = std::fs::write(&path, Value::Object(all).to_string());
}

fn recalled(pane: &str) -> Option<u32> {
    let all: Value = serde_json::from_str(&std::fs::read_to_string(ctx_cache()).ok()?).ok()?;
    (all[pane]["at"].as_i64()? > usage::now() - 6 * 3600).then(|| all[pane]["pct"].as_u64())?.map(|p| p as u32)
}

/// This pane's context bar, as the settings want it (cleared when off).
pub fn report(pane: &str, pct: Option<u32>, ttl_ms: u64) -> herdr::Result<()> {
    remember(pane, pct);
    let s = settings::load();
    let pct = pct.filter(|_| s.metrics[0]);
    herdr::report_tokens(pane, Value::Object(tokens(0, pct, &s)), pct.map(|_| ttl_ms))
}

fn provider(agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("claude"),
        "codex" => Some("openai-codex"),
        "opencode" => Some("opencode-go"),
        _ => None,
    }
}

/// This pane's 5h and weekly bars, from the usage cache for its agent's account.
pub fn report_usage(pane: &str, agent: &str, s: &Settings, rows: &[usage::Row]) -> herdr::Result<()> {
    let found = provider(agent).and_then(|key| usage::agents(rows).into_iter().find(|a| a.key == key));
    let mut map = Map::new();
    for (m, window) in [(1, found.as_ref().and_then(|a| a.five.as_ref())), (2, found.as_ref().and_then(|a| a.week.as_ref()))] {
        map.extend(tokens(m, window.map(|w| w.pct).filter(|_| s.metrics[m]), s));
    }
    herdr::report_tokens(pane, Value::Object(map), Some(env_u64("HERDR_PACER_CTX_TTL_MS", 21_600_000)))
}

/// Redraws every agent's bars: after a fetch, and when the settings change.
pub fn report_all() {
    let s = settings::load();
    let rows = usage::load();
    let Ok(list) = call("agent.list", json!({})) else { return };
    for a in list["agents"].as_array().into_iter().flatten() {
        let (Some(pane), Some(agent)) = (a["pane_id"].as_str(), a["agent"].as_str()) else { continue };
        let _ = herdr::report_tokens(pane, Value::Object(legacy()), None);
        let pct = recalled(pane);
        let _ = herdr::report_tokens(pane, Value::Object(tokens(0, pct.filter(|_| s.metrics[0]), &s)), Some(21_600_000));
        let _ = report_usage(pane, agent, &s, &rows);
    }
}

// ── claude: a pass-through statusLine ──

/// Runs the statusLine command it wraps — whatever drew the line keeps
/// drawing it — and reports the context percentage from the payload on its
/// way past. With nothing to wrap it prints a minimal `ctx N%`.
pub fn statusline() -> i32 {
    let mut payload = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload);
    let pct = serde_json::from_str::<Value>(&payload)
        .ok()
        .and_then(|v| v["context_window"]["used_percentage"].as_f64())
        .map(|p| p.round().max(0.0) as u32);
    let inner = std::env::var("HERDR_PACER_STATUSLINE").unwrap_or_default();
    let code = if inner.is_empty() {
        println!("ctx {}%", pct.unwrap_or(0));
        0
    } else {
        let child = Command::new("sh").args(["-c", &inner]).stdin(Stdio::piped()).spawn();
        match child {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(payload.as_bytes());
                }
                child.wait().ok().and_then(|s| s.code()).unwrap_or(1)
            }
            Err(_) => 1,
        }
    };
    // after the line is out, so Claude never waits on the Herdr socket for it
    if let Ok(pane) = std::env::var("HERDR_PANE_ID") {
        if pct.is_some() {
            let _ = report(&pane, pct, env_u64("HERDR_PACER_CTX_TTL_MS", 300_000)); // the line refreshes every minute
        }
        let _ = report_usage(&pane, "claude", &settings::load(), &usage::load());
    }
    code
}

// ── claude: installing the wrapper ──

fn settings_path() -> PathBuf {
    std::env::var_os("CLAUDE_SETTINGS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude/settings.json"))
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The command a wrapped statusLine carries after HERDR_PACER_STATUSLINE=,
/// read back from its shell quoting. `None` when it is not ours.
fn wrapped_inner(command: &str) -> Option<String> {
    if !(command.contains("claude-statusline.sh") || command.contains("herdr-pacer")) {
        return None;
    }
    let rest = command.strip_prefix("HERDR_PACER_STATUSLINE=")?;
    let mut inner = String::new();
    let mut chars = rest.chars().peekable();
    loop {
        match chars.next()? {
            '\'' => loop {
                match chars.next()? {
                    '\'' => break,
                    c => inner.push(c),
                }
            },
            '\\' => inner.push(chars.next()?),
            ' ' => return Some(inner),
            c => inner.push(c),
        }
    }
}

fn backup(path: &std::path::Path) -> Option<PathBuf> {
    let stamp = usage::now();
    let copy = path.with_extension(format!("json.bak-{stamp}"));
    std::fs::copy(path, &copy).ok().map(|_| copy)
}

/// install | remove | status of the statusLine wrapper in Claude's settings.
pub fn claude_hook(action: &str) -> Result<String, String> {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let mut settings: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let current = settings["statusLine"]["command"].as_str().unwrap_or("").to_string();
    let inner = wrapped_inner(&current);
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| e.to_string())?;
    let ours = format!("{} statusline", quote(&exe.to_string_lossy()));
    let write = |settings: &Value| -> Result<Option<PathBuf>, String> {
        let saved = path.exists().then(|| backup(&path)).flatten();
        let body = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())? + "\n";
        std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(saved)
    };
    let show = |s: &str| if s.is_empty() { "<none>".to_string() } else { s.to_string() };
    match action {
        "status" => Ok(match &inner {
            Some(i) => format!("installed; inner command: {}", show(i)),
            None => format!("not installed; current command: {}", show(&current)),
        }),
        "install" => {
            let inner_cmd = inner.clone().unwrap_or(current.clone());
            let command = format!("HERDR_PACER_STATUSLINE={} {ours}", quote(&inner_cmd));
            if command == current {
                return Ok(format!("already installed (inner: {})", show(&inner_cmd)));
            }
            if !settings["statusLine"].is_object() {
                settings["statusLine"] = json!({});
            }
            settings["statusLine"]["type"] = json!("command");
            settings["statusLine"]["command"] = json!(command);
            let saved = write(&settings)?;
            let verb = if inner.is_some() { "updated to this build" } else { "installed" };
            Ok(format!(
                "{verb} (backup: {})\n  statusLine still runs: {}",
                saved.map_or("none".into(), |p| p.display().to_string()),
                if inner_cmd.is_empty() { "<nothing; a minimal ctx line is printed>".to_string() } else { inner_cmd }
            ))
        }
        "remove" => {
            let Some(inner) = inner else { return Ok("not installed, nothing to remove".into()) };
            match inner.is_empty() {
                true => drop(settings.as_object_mut().map(|o| o.remove("statusLine"))),
                false => settings["statusLine"]["command"] = json!(inner),
            }
            let saved = write(&settings)?;
            Ok(format!(
                "removed (backup: {}); statusLine: {}",
                saved.map_or("none".into(), |p| p.display().to_string()),
                show(&inner)
            ))
        }
        _ => Err("usage: herdr-pacer claude-hook install | remove | status".into()),
    }
}

// ── codex and opencode: read on each turn ──

fn codex_pct(pane: &str) -> Option<u32> {
    let read = call("pane.read", json!({ "pane_id": pane, "source": "visible", "lines": 40, "strip_ansi": true })).ok()?;
    let text = read["read"]["text"].as_str().or(read["text"].as_str())?.to_string();
    // the last "Context N% left" on screen
    let left = text.match_indices("Context").filter_map(|(i, _)| {
        let rest = text[i + "Context".len()..].trim_start();
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        rest[digits.len()..].starts_with('%').then_some(())?;
        rest[digits.len() + 1..].trim_start().starts_with("left").then(|| digits.parse::<u32>().ok())?
    });
    left.last().map(|l| 100u32.saturating_sub(l))
}

fn sqlite(db: &str, query: &str, json_mode: bool) -> Option<String> {
    let mut command = Command::new("sqlite3");
    if json_mode {
        command.arg("-json");
    }
    let out = command.arg(format!("file:{db}?mode=ro")).arg(query).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn opencode_pct(cwd: &str) -> Option<u32> {
    let xdg = |var: &str, rel: &str| {
        std::env::var(var).unwrap_or_else(|_| format!("{}/{rel}", std::env::var("HOME").unwrap_or_default()))
    };
    let db = std::env::var("HERDR_PACER_OPENCODE_DB")
        .unwrap_or_else(|_| format!("{}/opencode/opencode.db", xdg("XDG_DATA_HOME", ".local/share")));
    let models = std::env::var("HERDR_PACER_OPENCODE_MODELS")
        .unwrap_or_else(|_| format!("{}/opencode/models.json", xdg("XDG_CACHE_HOME", ".cache")));
    let sq = |s: &str| s.replace('\'', "''");
    let session = sqlite(
        &db,
        &format!("select id from session where directory='{}' order by time_updated desc limit 1", sq(cwd)),
        false,
    )
    .filter(|s| !s.is_empty())?;
    let rows = sqlite(
        &db,
        &format!("select data from message where session_id='{}' order by time_created desc limit 8", sq(&session)),
        true,
    )?;
    // the newest turn that actually carries a token total
    let turn = serde_json::from_str::<Value>(&rows)
        .ok()?
        .as_array()?
        .iter()
        .filter_map(|r| serde_json::from_str::<Value>(r["data"].as_str()?).ok())
        .find(|d| d["tokens"]["total"].as_f64().unwrap_or(0.0) > 0.0)?;
    let total = turn["tokens"]["total"].as_f64()? as u64;
    let models: Value = serde_json::from_str(&std::fs::read_to_string(models).ok()?).ok()?;
    let limit = models[turn["providerID"].as_str()?]["models"][turn["modelID"].as_str()?]["limit"]["context"].as_u64()?;
    (limit > 0).then(|| ((total * 100 + limit / 2) / limit) as u32)
}

fn update_pane(pane: &str, agent: Option<&str>) {
    let info = call("pane.get", json!({ "pane_id": pane })).ok();
    let info = info.as_ref().map(|v| &v["pane"]);
    let agent = agent.map(String::from).or_else(|| info?["agent"].as_str().map(String::from));
    let pct = match agent.as_deref() {
        Some("codex") => codex_pct(pane),
        Some("opencode") => info.and_then(|i| i["foreground_cwd"].as_str().or(i["cwd"].as_str())).and_then(opencode_pct),
        _ => return, // claude reports its own; others have none
    };
    let _ = report(pane, pct, env_u64("HERDR_PACER_CTX_TTL_MS", 21_600_000));
    usage::fetch(false); // the 5h and weekly bars stay fresh while the dock is hidden
    let _ = report_usage(pane, agent.as_deref().unwrap_or(""), &settings::load(), &usage::load());
}

/// sweep | pane <id> [agent] | event
pub fn panes(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str).unwrap_or("sweep") {
        "sweep" => {
            let list = call("agent.list", json!({})).map_err(|e| e.to_string())?;
            for a in list["agents"].as_array().into_iter().flatten() {
                if let (Some(pane), Some(agent @ ("codex" | "opencode"))) = (a["pane_id"].as_str(), a["agent"].as_str()) {
                    update_pane(pane, Some(agent));
                }
            }
        }
        "pane" => update_pane(args.get(1).ok_or("pane <pane_id> [agent]")?, args.get(2).map(String::as_str)),
        "event" => {
            let event: Value = std::env::var("HERDR_PLUGIN_EVENT_JSON")
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Null);
            let Some(pane) = event["pane_id"].as_str().or(event["data"]["pane_id"].as_str()) else { return Ok(()) };
            // let the TUI repaint before reading it
            let settle: f64 = std::env::var("HERDR_PACER_EVENT_SETTLE").ok().and_then(|v| v.parse().ok()).unwrap_or(1.5);
            std::thread::sleep(Duration::from_secs_f64(settle));
            update_pane(pane, None);
        }
        _ => return Err("usage: herdr-pacer panes sweep | pane <pane_id> [agent] | event".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bucket_reported_the_rest_cleared() {
        let s = Settings::default();
        let t = tokens(0, Some(72), &s);
        assert_eq!(t["pacer_ctx_hot"], " 72% ⣤⣤⣤⣤⣤⣤⣤");
        assert_eq!(t["pacer_ctx"], "⣀⣀⣀");
        assert!(t["pacer_ctx_ok"].is_null() && t["pacer_ctx_crit"].is_null());
        assert!(tokens(0, None, &s).values().all(Value::is_null));
        let rows = Settings { metrics: [true; 3], sidebar_dots: 3, ..s.clone() };
        assert_eq!(tokens(1, Some(100), &rows)["pacer_5h_crit"], "5h  100% ⣶⣶⣶⣶⣶⣶⣶⣶⣶⣶");
        assert!(tokens(1, Some(100), &rows)["pacer_5h"].is_null(), "no rail left at 100%");
        let line = Settings { one_line: true, ..rows.clone() };
        assert_eq!(tokens(0, Some(58), &line)["pacer_ctx_warn"], "ctx 58%");
        assert_eq!(tokens(2, Some(49), &line)["pacer_wk_ok"], "⠀wk 49%", "glued on after 5h");
        assert!(tokens(2, Some(49), &line)["pacer_wk"].is_null(), "numbers only for three");
        let two = Settings { metrics: [true, false, true], ..line };
        assert_eq!(tokens(2, Some(50), &two)["pacer_wk_warn"], "⠀wk 50% ⣶");
    }

    #[test]
    fn rows_fit_herdr_limits_in_either_layout() {
        for one_line in [false, true] {
            let text = rows_toml(&Settings { one_line, ..Settings::default() });
            let doc: toml_edit::DocumentMut = text.parse().expect("valid TOML");
            let rows = doc["ui"]["sidebar"]["agents"]["rows"].as_array().unwrap();
            assert_eq!(rows.len(), if one_line { 3 } else { 5 });
            assert!(rows.iter().all(|r| r.as_array().unwrap().len() <= 16));
        }
    }

    #[test]
    fn wrapped_statusline_round_trips() {
        let inner = r#"bash "$HOME/.claude/cc-pacer.sh" --it's"#;
        let command = format!("HERDR_PACER_STATUSLINE={} '/x/herdr-pacer' statusline", quote(inner));
        assert_eq!(wrapped_inner(&command).as_deref(), Some(inner));
        let old = r#"HERDR_PACER_STATUSLINE='bash "$HOME/.claude/cc-pacer.sh"' '/p/claude-statusline.sh'"#;
        assert_eq!(wrapped_inner(old).as_deref(), Some(r#"bash "$HOME/.claude/cc-pacer.sh""#));
        assert_eq!(wrapped_inner("HERDR_PACER_STATUSLINE='' '/x/herdr-pacer' statusline").as_deref(), Some(""));
        assert_eq!(wrapped_inner("bash ~/.claude/cc-pacer.sh"), None);
    }
}
