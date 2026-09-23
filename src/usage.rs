//! The 5h and weekly windows: collectors, the shared cache, and the dotted bar.
//!
//! Know-how reused (with thanks):
//!   - Kamyil/herdr-usage-popup: Codex over `codex app-server`, OpenCode over
//!     `omp usage --json`.
//!   - amali01/cc-pacer: the Claude OAuth usage endpoint, its cache, and the
//!     back-off around it. cc-pacer's own cache is preferred when it is fresh,
//!     so a running Claude session pays for the request and we just read it.
//!
//! The 5h, weekly and monthly windows are collected; the settings pick which
//! of them the dock shows. Billing and credit balances are left out — this is
//! about pace, not spend.

use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ── the bar, shared by every surface ──

/// How a bar is drawn. Each has a size, 1–3, whose meaning is its own:
/// dot rows for Dots, height for Bar, fill shade for Blocks; Slants has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Dots,
    Bar,
    Blocks,
    Slants,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Dots, Style::Bar, Style::Blocks, Style::Slants];

    pub fn name(self) -> &'static str {
        match self {
            Style::Dots => "dots",
            Style::Bar => "bar",
            Style::Blocks => "blocks",
            Style::Slants => "slants",
        }
    }

    pub fn from_name(name: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.name() == name)
    }

    /// (full cell, half cell, empty cell). The empty cell differs from the
    /// full one in shape as well as color, so a bar still reads where color
    /// is stripped (the tab bar) — except one dot row, which is the rail.
    fn glyphs(self, size: u8) -> (char, Option<char>, char) {
        match (self, size) {
            (Style::Dots, 1) => ('⣀', Some('⡀'), '⣀'),
            (Style::Dots, 3) => ('⣶', Some('⡆'), '⣀'),
            (Style::Dots, _) => ('⣤', Some('⡄'), '⣀'),
            (Style::Bar, 1) => ('▂', None, '▁'),
            (Style::Bar, 3) => ('▆', None, '▁'),
            (Style::Bar, _) => ('▄', None, '▁'),
            (Style::Blocks, 1) => ('▒', None, '░'),
            (Style::Blocks, 2) => ('▓', None, '░'),
            (Style::Blocks, _) => ('█', None, '░'),
            (Style::Slants, _) => ('▰', None, '▱'),
        }
    }
}

/// (used, rail) for `pct` over `cells` cells. Dots count half cells (two dot
/// columns a cell); the others fill whole cells.
pub fn bar(pct: u32, cells: usize, style: Style, size: u8) -> (String, String) {
    let (full, half, empty) = style.glyphs(size);
    let steps = if half.is_some() { 2 } else { 1 };
    let filled = (pct.min(100) as usize * cells * steps + 50) / 100;
    let (mut used, mut rail) = (String::new(), String::new());
    for i in 0..cells {
        match filled.saturating_sub(i * steps) {
            d if d >= steps => used.push(full),
            1 => used.extend(half),
            _ => rail.push(empty),
        }
    }
    (used, rail)
}

/// cc-pacer's thresholds, so both pacers read the same.
pub fn bucket(pct: u32) -> &'static str {
    match pct {
        90.. => "crit",
        70.. => "hot",
        50.. => "warn",
        _ => "ok",
    }
}

pub fn duration(seconds: i64) -> String {
    if seconds <= 0 {
        return "now".into();
    }
    let (d, h, m) = (seconds / 86400, seconds % 86400 / 3600, seconds % 3600 / 60);
    match (d, h) {
        (d, h) if d > 0 => format!("{d}d{h:02}h"),
        (_, h) if h > 0 => format!("{h}h{m:02}m"),
        _ => format!("{m}m"),
    }
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Epoch seconds from a number or an RFC 3339 timestamp, as providers report
/// reset times. `None` when there is none.
pub fn epoch(value: &Value) -> Option<i64> {
    let secs = match value {
        Value::Number(n) => n.as_f64()? as i64,
        Value::String(s) => s.parse::<i64>().ok().or_else(|| rfc3339(s))?,
        _ => return None,
    };
    (secs > 0).then_some(secs)
}

fn rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |from: usize, to: usize| s.get(from..to)?.parse::<i64>().ok();
    if b.len() < 19 || b[4] != b'-' || b[10] != b'T' && b[10] != b' ' {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        tz => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let hh: i64 = tz.get(1..3)?.parse().ok()?;
            let mm: i64 = tz.get(4..6).and_then(|m| m.parse().ok()).unwrap_or(0);
            sign * (hh * 3600 + mm * 60)
        }
    };
    // days from civil (Howard Hinnant)
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + sec - offset)
}

// ── the rows every surface reads ──

/// One window of one provider, or (with `error`) a line saying why there is none.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub provider: String,
    pub plan: String,
    pub window: String,
    pub pct: u32,
    pub resets: Option<i64>,
    pub error: Option<String>,
}

impl Row {
    fn window(provider: &str, plan: &str, window: String, pct: f64, resets: Option<i64>) -> Row {
        Row {
            provider: provider.into(),
            plan: plan.into(),
            window,
            pct: pct.max(0.0) as u32,
            resets,
            error: None,
        }
    }

    fn error(provider: &str, message: &str) -> Row {
        Row {
            provider: provider.into(),
            plan: String::new(),
            window: String::new(),
            pct: 0,
            resets: None,
            error: Some(message.into()),
        }
    }
}

/// A provider as the surfaces draw it: 5h over weekly over monthly.
pub struct Agent {
    pub key: &'static str,
    pub title: &'static str,
    pub plan: String,
    pub five: Option<Row>,
    pub week: Option<Row>,
    pub month: Option<Row>,
    pub error: Option<String>,
}

impl Agent {
    /// The windows in settings order: 5h, weekly, monthly.
    pub fn windows(&self) -> [&Option<Row>; 3] {
        [&self.five, &self.week, &self.month]
    }
}

pub fn agents(rows: &[Row]) -> Vec<Agent> {
    crate::settings::AGENTS
        .into_iter()
        .filter_map(|(key, title)| {
            let mine = || rows.iter().filter(move |r| r.provider == key);
            // the first row wins: Codex lists its main limit before per-model ones
            let find = |prefixes: &[&str]| {
                mine().find(|r| r.error.is_none() && prefixes.iter().any(|p| r.window.starts_with(p))).cloned()
            };
            let agent = Agent {
                key,
                title,
                plan: mine().find(|r| !r.plan.is_empty()).map(|r| r.plan.clone()).unwrap_or_default(),
                five: find(&["5h"]),
                week: find(&["7d", "weekly"]),
                month: find(&["30d", "monthly", "1mo"]),
                error: mine().find_map(|r| r.error.clone()),
            };
            (agent.windows().iter().any(|w| w.is_some()) || agent.error.is_some()).then_some(agent)
        })
        .collect()
}

// ── collectors ──

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn on_path(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        return Path::new(bin).is_file().then(|| bin.into());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(bin))
        .find(|p| p.is_file())
}

fn env_or(names: &[&str], default: &str) -> String {
    names.iter().find_map(|n| std::env::var(n).ok()).unwrap_or_else(|| default.into())
}

/// A provider CLI started away from the caller's terminal. Herdr names a
/// pane's agent from its foreground process group, so a dock that ran
/// `codex` in its own group would flicker into the agent list as a Codex
/// session for the second the fetch takes.
fn detached(bin: &Path) -> Command {
    let mut command = Command::new(bin);
    command.process_group(0).stderr(Stdio::null());
    command
}

fn codex() -> Vec<Row> {
    const KEY: &str = "openai-codex";
    let Some(bin) = on_path(&env_or(&["HERDR_USAGE_CODEX_BIN", "HERDR_PACER_CODEX_BIN"], "codex")) else {
        return vec![];
    };
    let child = detached(&bin)
        .args(["-s", "read-only", "-a", "never", "app-server"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return vec![Row::error(KEY, "codex app-server did not start")];
    };
    // held open until the answer is in: closing it makes app-server quit first
    let mut stdin = child.stdin.take();
    if let Some(stdin) = stdin.as_mut() {
        let _ = stdin.write_all(
            concat!(
                r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"herdr-pacer","version":"1"}}}"#, "\n",
                r#"{"method":"initialized","params":{}}"#, "\n",
                r#"{"id":2,"method":"account/read","params":{}}"#, "\n",
                r#"{"id":3,"method":"account/rateLimits/read","params":{}}"#, "\n",
            )
            .as_bytes(),
        );
    }
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().expect("piped");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });
    let (mut account, mut limits) = (Value::Null, Value::Null);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while limits.is_null() {
        let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else { break };
        match rx.recv_timeout(left) {
            Ok(v) if v["id"] == 2 => account = v["result"]["account"].clone(),
            Ok(v) if v["id"] == 3 => limits = v["result"].clone(),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    if limits["rateLimits"].is_null() {
        return vec![Row::error(KEY, "codex app-server returned no limits")];
    }
    codex_rows(&account, &limits)
}

fn codex_rows(account: &Value, result: &Value) -> Vec<Row> {
    const KEY: &str = "openai-codex";
    let plan = account["planType"].as_str().or(result["rateLimits"]["planType"].as_str()).unwrap_or("");
    let mut limits: Vec<(String, Value)> = match result["rateLimitsByLimitId"].as_object() {
        Some(by_id) if !by_id.is_empty() => by_id.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => vec![("codex".into(), result["rateLimits"].clone())],
    };
    limits.sort_by_key(|(id, _)| id != "codex");
    let mut rows = vec![];
    for (id, limit) in &limits {
        let quota = match id.as_str() {
            "codex" => String::new(),
            _ => format!("/{}", limit["limitName"].as_str().unwrap_or(id)),
        };
        for w in [&limit["primary"], &limit["secondary"]] {
            let (Some(pct), mins) = (w["usedPercent"].as_f64(), w["windowDurationMins"].as_i64().unwrap_or(0)) else {
                continue;
            };
            let label = match mins {
                0 => "?".to_string(),
                m if m % 1440 == 0 => format!("{}d", m / 1440),
                m if m % 60 == 0 => format!("{}h", m / 60),
                m => format!("{m}m"),
            };
            rows.push(Row::window(KEY, plan, label + &quota, pct.floor(), epoch(&w["resetsAt"])));
        }
    }
    rows
}

fn claude_token() -> Option<String> {
    if let Ok(token) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        return Some(token);
    }
    let from = |blob: &str| {
        serde_json::from_str::<Value>(blob).ok()?["claudeAiOauth"]["accessToken"].as_str().map(String::from)
    };
    if let Some(token) = fs::read_to_string(home().join(".claude/.credentials.json")).ok().and_then(|b| from(&b)) {
        return Some(token);
    }
    let keyring: [(&str, &[&str]); 2] = [
        ("secret-tool", &["lookup", "service", "Claude Code-credentials"]),
        ("security", &["find-generic-password", "-s", "Claude Code-credentials", "-w"]),
    ];
    keyring.iter().find_map(|(bin, args)| {
        let out = detached(&on_path(bin)?).args(*args).output().ok()?;
        from(String::from_utf8_lossy(&out.stdout).trim())
    })
}

fn mtime(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

fn pct(window: &Value) -> Option<f64> {
    window["utilization"].as_f64().or(window["used_percentage"].as_f64())
}

/// Whether a usage payload has a 5h or weekly window the dock can draw.
fn has_windows(body: &str) -> bool {
    serde_json::from_str::<Value>(body).is_ok_and(|v| pct(&v["five_hour"]).or(pct(&v["seven_day"])).is_some())
}

/// A cache's mtime, or 0 when it has nothing to draw, so a real one always
/// replaces it and it never holds off a refresh.
fn usable_mtime(path: &Path) -> i64 {
    if fs::read_to_string(path).is_ok_and(|b| has_windows(&b)) { mtime(path) } else { 0 }
}

/// The Claude usage payload, refreshed when older than HERDR_PACER_CLAUDE_REFRESH.
fn claude_usage(state: &Path) -> Option<Value> {
    let cache = state.join("claude-usage.json");
    let backoff = state.join("claude-backoff");
    // cc-pacer keeps the same payload warm while any Claude session is open
    // our uid is the owner of our own state directory
    let uid = fs::metadata(state).ok().map(|m| std::os::unix::fs::MetadataExt::uid(&m));
    if let Some(dirs) = uid.and_then(|uid| fs::read_dir(format!("/tmp/cc-pacer-{uid}")).ok()) {
        // not every cache there is real: cc-pacer's install preview leaves a
        // sample one with no windows in it, so only a payload with them counts
        let ours = usable_mtime(&cache);
        let newest = dirs
            .filter_map(|d| Some(d.ok()?.path().join("usage-cache.json")))
            .filter(|p| p.is_file() && mtime(p) > ours)
            .filter_map(|p| Some((mtime(&p), fs::read_to_string(&p).ok()?)))
            .filter(|(_, body)| has_windows(body))
            .max_by_key(|(t, _)| *t);
        if let Some((_, body)) = newest {
            let _ = write_atomic(&cache, body.as_bytes());
        }
    }
    let refresh: i64 = env_or(&["HERDR_PACER_CLAUDE_REFRESH"], "300").parse().unwrap_or(300);
    let backoff_until: i64 = fs::read_to_string(&backoff).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    if now() - usable_mtime(&cache) > refresh && now() > backoff_until {
        if let Some(token) = claude_token() {
            let fresh = ureq::get("https://api.anthropic.com/api/oauth/usage")
                .timeout(Duration::from_secs(5))
                .set("Authorization", &format!("Bearer {token}"))
                .set("Accept", "application/json")
                .set("anthropic-beta", "oauth-2025-04-20")
                .set("User-Agent", "claude-code/2.1.34")
                .call()
                .ok()
                .and_then(|r| r.into_string().ok())
                .filter(|body| has_windows(body));
            match fresh {
                Some(body) => {
                    let _ = write_atomic(&cache, body.as_bytes());
                    let _ = fs::remove_file(&backoff);
                }
                // ponytail: one flat back-off; split per status code if auth errors get noisy
                None => {
                    let _ = fs::write(&backoff, (now() + 900).to_string());
                }
            }
        }
    }
    serde_json::from_str(&fs::read_to_string(&cache).ok()?).ok()
}

fn claude(state: &Path) -> Vec<Row> {
    const KEY: &str = "claude";
    let usage = claude_usage(state).filter(|u| pct(&u["five_hour"]).or(pct(&u["seven_day"])).is_some());
    let Some(usage) = usage else {
        return match on_path("claude") {
            Some(_) => vec![Row::error(KEY, "no usage yet — sign in to Claude Code")],
            None => vec![],
        };
    };
    let mut rows = vec![];
    let age = now() - mtime(&state.join("claude-usage.json"));
    let max_age: i64 = env_or(&["HERDR_USAGE_CLAUDE_MAX_AGE"], "900").parse().unwrap_or(900);
    if age > max_age {
        rows.push(Row::error(KEY, &format!("usage {} old", duration(age))));
    }
    for (label, key) in [("5h", "five_hour"), ("7d", "seven_day")] {
        let d = &usage[key];
        if let Some(pct) = pct(d) {
            rows.push(Row::window(KEY, "", label.into(), pct.round(), epoch(&d["resets_at"])));
        }
    }
    rows
}

fn opencode_go() -> Vec<Row> {
    const KEY: &str = "opencode-go";
    let Some(bin) = on_path(&env_or(&["HERDR_USAGE_OMP_BIN", "HERDR_PACER_OMP_BIN"], "omp")) else {
        return vec![];
    };
    let out = detached(&bin).args(["usage", "--json", "--provider", KEY]).stdin(Stdio::null()).output();
    let Some(json) = out.ok().filter(|o| o.status.success()).and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
    else {
        return vec![Row::error(KEY, "request failed")];
    };
    let mut rows = vec![];
    for report in json["reports"].as_array().into_iter().flatten().filter(|r| r["provider"] == KEY) {
        let plan = report["metadata"]["planType"].as_str().unwrap_or("");
        for limit in report["limits"].as_array().into_iter().flatten() {
            let window = limit["scope"]["windowId"].as_str().unwrap_or("?");
            let pct = limit["amount"]["usedFraction"].as_f64().unwrap_or(0.0) * 100.0;
            let resets = limit["window"]["resetsAt"].as_f64().map(|ms| json!((ms / 1000.0).floor()));
            rows.push(Row::window(KEY, plan, window.into(), pct.floor(), resets.as_ref().and_then(epoch)));
        }
    }
    rows
}

// ── the shared cache: whichever copy finds it stale fetches, the rest read ──

/// Herdr's state directory for the plugin. Outside Herdr — the Claude
/// statusLine runs under Claude, not Herdr — the same path is worked out,
/// so every copy shares one cache.
pub fn state_dir() -> PathBuf {
    let dir = std::env::var_os("HERDR_PACER_STATE_DIR")
        .or_else(|| std::env::var_os("HERDR_PLUGIN_STATE_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_STATE_HOME")
                .map_or_else(|| home().join(".local/state"), PathBuf::from)
                .join("herdr/plugins/herdr-pacer")
        });
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn cache_path() -> PathBuf {
    state_dir().join("usage.json")
}

pub fn refresh_seconds() -> i64 {
    env_or(&["HERDR_USAGE_REFRESH_SECONDS", "HERDR_PACER_REFRESH_SECONDS"], "60").parse().unwrap_or(60)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}

pub fn load() -> Vec<Row> {
    let Ok(text) = fs::read_to_string(cache_path()) else { return vec![] };
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(&text) else { return vec![] };
    rows.iter()
        .map(|r| Row {
            provider: r["provider"].as_str().unwrap_or_default().into(),
            plan: r["plan"].as_str().unwrap_or_default().into(),
            window: r["window"].as_str().unwrap_or_default().into(),
            pct: r["pct"].as_u64().unwrap_or(0) as u32,
            resets: r["resets"].as_i64(),
            error: r["error"].as_str().map(String::from),
        })
        .collect()
}

pub fn cache_age() -> i64 {
    now() - mtime(&cache_path())
}

pub fn cache_mtime() -> i64 {
    mtime(&cache_path())
}

/// Fetches into the cache unless another copy already is (the lock) or the
/// cache is younger than the refresh interval. The lock is the OS's, so a
/// copy that dies mid-fetch releases it.
pub fn fetch(force: bool) {
    if !force && cache_age() < refresh_seconds() {
        return;
    }
    let state = state_dir();
    let Ok(lock) = File::create(state.join("usage.lock")) else { return };
    if lock.try_lock().is_err() {
        return;
    }
    let rows: Vec<Row> = [codex(), claude(&state), opencode_go()].concat();
    let json: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({ "provider": r.provider, "plan": r.plan, "window": r.window,
                    "pct": r.pct, "resets": r.resets, "error": r.error })
        })
        .collect();
    let _ = write_atomic(&cache_path(), Value::Array(json).to_string().as_bytes());
    drop(lock);
    crate::context::report_all(); // every agent's 5h and weekly bars
}

pub fn fetching() -> bool {
    File::create(state_dir().join("usage.lock")).is_ok_and(|f| f.try_lock().is_err())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_halves_and_rail() {
        let dots = |pct, cells, size| bar(pct, cells, Style::Dots, size);
        assert_eq!(dots(0, 4, 2), ("".into(), "⣀⣀⣀⣀".into()));
        assert_eq!(dots(50, 4, 2), ("⣤⣤".into(), "⣀⣀".into()));
        assert_eq!(dots(63, 4, 2), ("⣤⣤⡄".into(), "⣀".into()));
        assert_eq!(dots(100, 4, 2), ("⣤⣤⣤⣤".into(), "".into()));
        assert_eq!(dots(250, 2, 2), ("⣤⣤".into(), "".into()));
        assert_eq!(dots(63, 4, 1), ("⣀⣀⡀".into(), "⣀".into()));
        assert_eq!(dots(63, 4, 3), ("⣶⣶⡆".into(), "⣀".into()));
        assert_eq!(bar(63, 4, Style::Bar, 1), ("▂▂▂".into(), "▁".into()), "whole cells");
        assert_eq!(bar(50, 4, Style::Bar, 3), ("▆▆".into(), "▁▁".into()));
        assert_eq!(bar(50, 4, Style::Blocks, 1), ("▒▒".into(), "░░".into()));
        assert_eq!(bar(50, 4, Style::Blocks, 3), ("██".into(), "░░".into()));
        assert_eq!(bar(75, 4, Style::Slants, 2), ("▰▰▰".into(), "▱".into()));
        assert!(Style::ALL.iter().all(|&s| Style::from_name(s.name()) == Some(s)));
    }

    #[test]
    fn only_payloads_with_windows_count() {
        assert!(has_windows(r#"{"five_hour":{"utilization":5.0},"seven_day":null}"#));
        assert!(!has_windows(r#"{"extra_usage":{"is_enabled":true,"utilization":80}}"#), "cc-pacer's preview sample");
        assert!(!has_windows(r#"{"five_hour":{},"seven_day":false}"#), "nothing to draw");
        assert!(!has_windows("not json"));
    }

    #[test]
    fn thresholds_and_durations() {
        assert_eq!([bucket(49), bucket(50), bucket(70), bucket(90)], ["ok", "warn", "hot", "crit"]);
        assert_eq!(duration(0), "now");
        assert_eq!(duration(46 * 60), "46m");
        assert_eq!(duration(2 * 3600 + 14 * 60), "2h14m");
        assert_eq!(duration(3 * 86400 + 19 * 3600), "3d19h");
    }

    #[test]
    fn reset_times() {
        assert_eq!(epoch(&json!(1790000000)), Some(1790000000));
        assert_eq!(epoch(&json!("1790000000")), Some(1790000000));
        assert_eq!(epoch(&json!("1970-01-02T00:00:00Z")), Some(86400));
        assert_eq!(epoch(&json!("2026-09-23T10:30:00.123+02:00")), Some(1790152200));
        assert_eq!(epoch(&json!(0)), None);
        assert_eq!(epoch(&json!("soon")), None);
    }

    #[test]
    fn codex_limits() {
        let result = json!({ "rateLimits": { "planType": "team",
            "primary": { "usedPercent": 48.7, "windowDurationMins": 300, "resetsAt": 1790000000 },
            "secondary": { "usedPercent": 31, "windowDurationMins": 10080 } } });
        let rows = codex_rows(&json!({}), &result);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].window.as_str(), rows[0].pct, rows[0].plan.as_str()), ("5h", 48, "team"));
        assert_eq!((rows[1].window.as_str(), rows[1].resets), ("7d", None));
        let by_id = json!({ "rateLimits": {}, "rateLimitsByLimitId": {
            "spark": { "limitName": "spark", "primary": { "usedPercent": 90, "windowDurationMins": 300 } },
            "codex": { "primary": { "usedPercent": 10, "windowDurationMins": 300 },
                       "secondary": { "usedPercent": 5, "windowDurationMins": 43200 } } } });
        let rows = codex_rows(&json!({ "planType": "pro" }), &by_id);
        assert_eq!(rows.iter().map(|r| r.window.as_str()).collect::<Vec<_>>(), ["5h", "30d", "5h/spark"]);
        let agent = &agents(&rows)[0];
        assert_eq!((agent.plan.as_str(), agent.five.as_ref().unwrap().pct), ("pro", 10));
        assert_eq!(agent.month.as_ref().map(|m| m.pct), Some(5));
    }
}
