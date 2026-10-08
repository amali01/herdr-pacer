//! The 5h and weekly windows: collectors, the shared cache, and the dotted bar.
//!
//! Know-how reused (with thanks):
//!   - Kamyil/herdr-usage-popup: Codex over `codex app-server`, OpenCode over
//!     `omp usage --json`.
//!   - amali01/cc-pacer: the Claude OAuth usage endpoint and the back-off
//!     around it.
//!
//! Claude can be signed in more than once: each CLAUDE_CONFIG_DIR is a login
//! of its own, with its own windows. Every one known is collected and shown
//! on its own (see `claude_accounts`).
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
    /// Which login of the provider: a Claude config dir, "" for the default.
    pub account: String,
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
            account: String::new(),
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
            account: String::new(),
            plan: String::new(),
            window: String::new(),
            pct: 0,
            resets: None,
            error: Some(message.into()),
        }
    }

    fn of(self, account: &str) -> Row {
        Row { account: account.into(), ..self }
    }
}

/// One account of a provider as the surfaces draw it: 5h over weekly over
/// monthly.
pub struct Agent {
    pub key: &'static str,
    pub account: String,
    pub title: String,
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

/// An agent per account, in `AGENTS` order; a provider's accounts in the
/// order their rows came, which puts the default first.
pub fn agents(rows: &[Row]) -> Vec<Agent> {
    let mut out = vec![];
    for (key, title) in crate::settings::AGENTS {
        let mut accounts: Vec<&str> = vec![];
        for r in rows.iter().filter(|r| r.provider == key) {
            if !accounts.contains(&r.account.as_str()) {
                accounts.push(&r.account);
            }
        }
        for account in accounts {
            let mine = || rows.iter().filter(move |r| r.provider == key && r.account == account);
            // the first row wins: Codex lists its main limit before per-model ones
            let find = |prefixes: &[&str]| {
                mine().find(|r| r.error.is_none() && prefixes.iter().any(|p| r.window.starts_with(p))).cloned()
            };
            let agent = Agent {
                key,
                account: account.into(),
                title: if account.is_empty() { title.into() } else { account_title(account) },
                plan: mine().find(|r| !r.plan.is_empty()).map(|r| r.plan.clone()).unwrap_or_default(),
                five: find(&["5h"]),
                week: find(&["7d", "weekly"]),
                month: find(&["30d", "monthly", "1mo"]),
                error: mine().find_map(|r| r.error.clone()),
            };
            if agent.windows().iter().any(|w| w.is_some()) || agent.error.is_some() {
                out.push(agent);
            }
        }
    }
    // two dirs with one name (~/.claude-2, ~/work/.claude-2) go by their paths
    let titles: Vec<String> = out.iter().map(|a| a.title.clone()).collect();
    for a in out.iter_mut().filter(|a| !a.account.is_empty()) {
        if titles.iter().filter(|t| **t == a.title).count() > 1 {
            a.title = tilde(&a.account);
        }
    }
    out
}

/// A path as it reads under the home directory.
fn tilde(dir: &str) -> String {
    let home = home();
    match Path::new(dir).strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => dir.to_string(),
    }
}

/// A second account's name, from its config dir: `~/.claude-2` is Claude-2,
/// `~/.claude-work` Claude-work, `~/work` Claude work. A dir named like the
/// default shows its path instead, and so do two dirs with one name.
pub fn account_title(dir: &str) -> String {
    let name = Path::new(dir).file_name().map_or(String::new(), |n| n.to_string_lossy().trim_start_matches('.').to_string());
    match name.get(..6) {
        _ if name.is_empty() || name.eq_ignore_ascii_case("claude") => tilde(dir),
        Some(head) if head.eq_ignore_ascii_case("claude") => format!("Claude{}", &name[6..]),
        _ => format!("Claude {name}"),
    }
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

// ── claude: one login per config dir ──

/// The config dir of a Claude account: CLAUDE_CONFIG_DIR as the account's
/// sessions see it, or ~/.claude for the default ("").
fn claude_dir(account: &str) -> PathBuf {
    if account.is_empty() { home().join(".claude") } else { PathBuf::from(account) }
}

/// The keychain entry Claude Code keeps an account's login under: its own
/// name for the default, and with CLAUDE_CONFIG_DIR set, that name and the
/// first 8 hex digits of the dir's sha256.
// ponytail: Claude NFC-normalizes the dir first; a non-ASCII path in a
// decomposed form would hash differently here
fn keychain_service(account: &str) -> String {
    const NAME: &str = "Claude Code-credentials";
    if account.is_empty() {
        return NAME.into();
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, account.as_bytes());
    let hex: String = digest.as_ref()[..4].iter().map(|b| format!("{b:02x}")).collect();
    format!("{NAME}-{hex}")
}

fn claude_token(account: &str) -> Option<String> {
    if account.is_empty() {
        if let Ok(token) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
            return Some(token);
        }
    }
    let from = |blob: &str| {
        serde_json::from_str::<Value>(blob).ok()?["claudeAiOauth"]["accessToken"].as_str().map(String::from)
    };
    let file = claude_dir(account).join(".credentials.json");
    if let Some(token) = fs::read_to_string(file).ok().and_then(|b| from(&b)) {
        return Some(token);
    }
    let service = keychain_service(account);
    let keyring: [(&str, &[&str]); 2] = [
        ("secret-tool", &["lookup", "service", &service]),
        ("security", &["find-generic-password", "-s", &service, "-w"]),
    ];
    keyring.iter().find_map(|(bin, args)| {
        let out = detached(&on_path(bin)?).args(*args).output().ok()?;
        from(String::from_utf8_lossy(&out.stdout).trim())
    })
}

/// `~/` in a dir from the settings, as the shell would have expanded it.
fn expand(dir: &str) -> String {
    match dir.strip_prefix("~/") {
        Some(rest) => home().join(rest).to_string_lossy().into_owned(),
        None => dir.to_string(),
    }
}

fn seen_path() -> PathBuf {
    state_dir().join("claude-accounts.json")
}

fn seen() -> Vec<String> {
    let text = fs::read_to_string(seen_path()).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

/// Notes a config dir a Claude session ran with, so the views show its
/// account from then on.
pub fn saw_claude_account(dir: &str) {
    let mut all = seen();
    if dir.is_empty() || all.iter().any(|d| d == dir) {
        return;
    }
    all.push(dir.into());
    let _ = write_atomic(&seen_path(), json!(all).to_string().as_bytes());
}

/// The default account, then each config dir a session ran with, this
/// process runs with, or the settings list — once each, and only while the
/// dir is there. The first spelling of a dir wins: it is the one a session
/// used, and the keychain entry is named after that spelling.
pub fn claude_accounts() -> Vec<String> {
    let mut found: Vec<String> = seen();
    found.extend(std::env::var("CLAUDE_CONFIG_DIR").ok());
    found.extend(crate::settings::load().claude_dirs.iter().map(|d| expand(d)));
    distinct_dirs(found)
}

fn distinct_dirs(dirs: Vec<String>) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut real: Vec<PathBuf> = vec![];
    for dir in dirs.into_iter().map(|d| d.trim().to_string()).filter(|d| !d.is_empty()) {
        let Ok(canon) = fs::canonicalize(&dir) else { continue };
        if canon.is_dir() && !real.contains(&canon) {
            real.push(canon);
            out.push(dir);
        }
    }
    out
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
fn has_windows(usage: &Value) -> bool {
    pct(&usage["five_hour"]).or(pct(&usage["seven_day"])).is_some()
}

/// An account's cached usage and back-off files; the default keeps the
/// names it had before there were accounts.
fn claude_files(state: &Path, account: &str) -> (PathBuf, PathBuf) {
    let tag = match account {
        "" => String::new(),
        a => format!("-{}", &keychain_service(a)["Claude Code-credentials-".len()..]),
    };
    (state.join(format!("claude-usage{tag}.json")), state.join(format!("claude-backoff{tag}")))
}

fn read_usage(path: &Path) -> Option<Value> {
    serde_json::from_str::<Value>(&fs::read_to_string(path).ok()?).ok().filter(has_windows)
}

/// A cache's mtime, or 0 when it has nothing to draw, so a real one always
/// replaces it and it never holds off a refresh.
fn usable_mtime(path: &Path) -> i64 {
    if read_usage(path).is_some() { mtime(path) } else { 0 }
}

/// The account's usage payload, refreshed when older than
/// HERDR_PACER_CLAUDE_REFRESH. A running session keeps it fresh for free
/// (`claude_from_statusline`), so the request is for accounts no session is on.
fn claude_usage(state: &Path, account: &str) -> Option<Value> {
    let (cache, backoff) = claude_files(state, account);
    let refresh: i64 = env_or(&["HERDR_PACER_CLAUDE_REFRESH"], "300").parse().unwrap_or(300);
    let backoff_until: i64 = fs::read_to_string(&backoff).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    if now() - usable_mtime(&cache) > refresh && now() > backoff_until {
        if let Some(token) = claude_token(account) {
            let fresh = ureq::get("https://api.anthropic.com/api/oauth/usage")
                .timeout(Duration::from_secs(5))
                .set("Authorization", &format!("Bearer {token}"))
                .set("Accept", "application/json")
                .set("anthropic-beta", "oauth-2025-04-20")
                .set("User-Agent", "claude-code/2.1.34")
                .call()
                .ok()
                .and_then(|r| r.into_string().ok())
                .filter(|body| serde_json::from_str::<Value>(body).is_ok_and(|v| has_windows(&v)));
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
    read_usage(&cache)
}

/// An account's windows from its payload, flagged when it is `age` old.
fn claude_rows(account: &str, usage: &Value, age: i64) -> Vec<Row> {
    const KEY: &str = "claude";
    let mut rows = vec![];
    let max_age: i64 = env_or(&["HERDR_USAGE_CLAUDE_MAX_AGE"], "900").parse().unwrap_or(900);
    if age > max_age {
        rows.push(Row::error(KEY, &format!("usage {} old", duration(age))).of(account));
    }
    for (label, key) in [("5h", "five_hour"), ("7d", "seven_day")] {
        let d = &usage[key];
        if let Some(pct) = pct(d) {
            rows.push(Row::window(KEY, "", label.into(), pct.round(), epoch(&d["resets_at"])).of(account));
        }
    }
    rows
}

/// Every account's rows. One that is not signed in has none; the default
/// says so only while no other account has any.
fn claude(state: &Path) -> Vec<Row> {
    let mut rows = vec![];
    for account in claude_accounts() {
        if let Some(usage) = claude_usage(state, &account) {
            let age = now() - mtime(&claude_files(state, &account).0);
            rows.extend(claude_rows(&account, &usage, age));
        }
    }
    if rows.is_empty() && on_path("claude").is_some() {
        rows.push(Row::error("claude", "no usage yet — sign in to Claude Code"));
    }
    rows
}

/// The 5h and weekly windows Claude Code hands its statusLine, kept as the
/// account's cache: the session paid for them, and no other request is
/// needed while one is open. The account's rows, when the payload has any.
pub fn claude_from_statusline(account: &str, rate_limits: &Value) -> Option<Vec<Row>> {
    saw_claude_account(account);
    let cache = claude_files(&state_dir(), account).0;
    let cached = read_usage(&cache).unwrap_or_else(|| json!({}));
    let (usage, newer) = merge_windows(&cached, rate_limits, now());
    if newer {
        let _ = write_atomic(&cache, usage.to_string().as_bytes());
    }
    has_windows(&usage).then(|| claude_rows(account, &usage, now() - if newer { now() } else { mtime(&cache) }))
}

/// The payload's windows laid over the cached ones, and whether any was
/// newer. The payload is the session's last API answer, which an idle session
/// hands over again and again, so a window counts as newer only when it
/// carries news: a later reset, or within the same window more use — use
/// only rises until the window resets. A window that has already reset is
/// dropped, from either side.
fn merge_windows(cached: &Value, payload: &Value, now: i64) -> (Value, bool) {
    let mut usage = cached.clone();
    let mut newer = false;
    for key in ["five_hour", "seven_day"] {
        if epoch(&cached[key]["resets_at"]).is_some_and(|r| r <= now) {
            usage[key] = Value::Null;
        }
        let (old, new) = (&cached[key], &payload[key]);
        let (Some(used), Some(resets)) = (pct(new), epoch(&new["resets_at"])) else { continue };
        let old_resets = epoch(&old["resets_at"]).filter(|&r| r > now);
        let news = match old_resets {
            _ if resets <= now => false,
            Some(r) if r == resets => pct(old).is_none_or(|o| used > o),
            Some(r) => resets > r,
            None => true,
        };
        if news {
            usage[key] = new.clone();
            newer = true;
        }
    }
    (usage, newer)
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

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
            account: r["account"].as_str().unwrap_or_default().into(),
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
            json!({ "provider": r.provider, "account": r.account, "plan": r.plan, "window": r.window,
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
        assert!(has_windows(&json!({"five_hour": {"utilization": 5.0}, "seven_day": null})));
        assert!(has_windows(&json!({"seven_day": {"used_percentage": 40}})), "as the statusLine hands it over");
        assert!(!has_windows(&json!({"extra_usage": {"is_enabled": true, "utilization": 80}})), "cc-pacer's preview sample");
        assert!(!has_windows(&json!({"five_hour": {}, "seven_day": false})), "nothing to draw");
    }

    #[test]
    fn each_account_has_its_own_keychain_entry_and_files() {
        assert_eq!(keychain_service(""), "Claude Code-credentials");
        // what Claude Code names it: sha256 of CLAUDE_CONFIG_DIR, 8 hex digits
        assert_eq!(keychain_service("/u/.claude-2"), "Claude Code-credentials-7570ccf5");
        let state = Path::new("/s");
        assert_eq!(claude_files(state, "").0, state.join("claude-usage.json"), "the default keeps its old name");
        assert_eq!(claude_files(state, "/u/.claude-2").1, state.join("claude-backoff-7570ccf5"));
        assert_eq!(claude_dir("/u/.claude-2"), PathBuf::from("/u/.claude-2"));
    }

    #[test]
    fn a_statusline_snapshot_counts_only_with_news() {
        let (now, at) = (1_000_000, 1_000_000 + 3600);
        let win = |used: u32, resets: i64| json!({ "used_percentage": used, "resets_at": resets });
        let cached = json!({ "five_hour": win(40, at), "seven_day": win(10, at * 2) });
        let merged = |payload: Value| merge_windows(&cached, &payload, now);
        assert!(!merged(json!({ "five_hour": win(40, at) })).1, "the same answer again, from an idle session");
        assert!(!merged(json!({ "five_hour": win(38, at) })).1, "less use in one window is an older answer");
        let (usage, newer) = merged(json!({ "five_hour": win(45, at), "seven_day": win(9, at * 2) }));
        assert!(newer);
        assert_eq!((pct(&usage["five_hour"]), pct(&usage["seven_day"])), (Some(45.0), Some(10.0)));
        assert!(merged(json!({ "five_hour": win(2, at + 18000) })).1, "a new window after a reset");
        assert!(!merged(json!({ "five_hour": win(90, now - 1) })).1, "a window that has already reset");
        let (usage, _) = merge_windows(&cached, &json!({}), at);
        assert!(usage["five_hour"].is_null() && pct(&usage["seven_day"]) == Some(10.0), "nor is a cached one kept");
        let (fresh, newer) = merge_windows(&json!({}), &json!({ "seven_day": win(15, at) }), now);
        assert!(newer && pct(&fresh["seven_day"]) == Some(15.0) && fresh["five_hour"].is_null());
    }

    #[test]
    fn account_titles() {
        assert_eq!(account_title("/u/.claude-2"), "Claude-2");
        assert_eq!(account_title("/u/.config/claude-work/"), "Claude-work");
        assert_eq!(account_title("/u/personal"), "Claude personal");
        assert_eq!(account_title("/srv/other/.claude"), "/srv/other/.claude", "named like the default");
    }

    #[test]
    fn accounts_are_distinct_dirs_that_exist() {
        let dir = std::env::temp_dir().join(format!("herdr-pacer-accounts-{}", std::process::id()));
        let two = dir.join(".claude-2");
        fs::create_dir_all(&two).unwrap();
        let spelled = |p: &Path| p.to_string_lossy().into_owned();
        let found = distinct_dirs(vec![
            spelled(&two),
            format!("{}/", spelled(&two)), // the same dir, spelled otherwise
            spelled(&dir.join("gone")),
            " ".into(),
        ]);
        assert_eq!(found, ["".to_string(), spelled(&two)], "the default, then the first spelling");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn accounts_are_drawn_apart() {
        let usage = json!({ "five_hour": { "used_percentage": 12.4, "resets_at": 1790000000 },
                            "seven_day": { "used_percentage": 40 } });
        let mut rows = claude_rows("", &json!({ "five_hour": { "utilization": 90 } }), 0);
        rows.extend(claude_rows("/u/.claude-2", &usage, 0));
        let all = agents(&rows);
        let shown: Vec<_> = all.iter().map(|a| (a.title.as_str(), a.five.as_ref().map(|r| r.pct))).collect();
        assert_eq!(shown, [("Claude", Some(90)), ("Claude-2", Some(12))]);
        assert_eq!(all[1].five.as_ref().unwrap().resets, Some(1790000000));
        assert_eq!(all[1].week.as_ref().map(|r| r.pct), Some(40));
        assert!(all[0].week.is_none(), "the default's windows are its own");
        let twin = claude_rows("/u/work/.claude-2", &usage, 0);
        let titles: Vec<_> = agents(&[rows.clone(), twin].concat()).into_iter().map(|a| a.title).collect();
        assert_eq!(titles, ["Claude", "/u/.claude-2", "/u/work/.claude-2"], "one name twice: the paths");
        let stale = claude_rows("/u/.claude-2", &usage, 3600);
        assert_eq!(stale[0].error.as_deref(), Some("usage 1h00m old"));
        assert_eq!(agents(&stale)[0].account, "/u/.claude-2");
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
