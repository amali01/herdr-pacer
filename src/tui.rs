//! The dock (a strip along the bottom of every tab) and the popup, drawn from
//! the shared usage cache.
//!
//!   Codex  team                    Claude  max                    ⚙   ⟳   ✕
//!   5h ⣤⣤⣤⣤⣤⣤⣤⣤⡄⣀  97%  ⟳ 2h14m    5h ⣤⣤⣀⣀⣀⣀⣀⣀⣀⣀  13%  ⟳ 4h02m
//!   wk ⣤⣤⣤⣤⣀⣀⣀⣀⣀⣀  45%  ⟳ 3d19h    wk ⣤⣤⣤⣤⡄⣀⣀⣀⣀⣀  47%  ⟳ 5d01h
//!
//! Each column gives up detail as it narrows — the reset time first, then bar
//! cells, then the bar itself (`5h 97%`) — and when even the numbers do not
//! fit, it shows the agents that do and a `+N` for the rest.
//!
//! A frame is painted over the previous one, never cleared first, and the dock
//! lives on the alternate screen, which has no scrollback — so it neither
//! blinks on a refresh nor scrolls under the mouse wheel.

use crate::settings::{self, Settings, AGENTS};
use crate::usage::{self, Agent, Row};
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::{cursor, execute, terminal};
use std::io::Write;
use std::time::Duration;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const WHITE: &str = "\x1b[38;2;235;225;235m";
const SUBTLE: &str = "\x1b[38;2;150;140;160m";
const RAIL: &str = "\x1b[38;2;70;66;78m";

/// cc-pacer's palette, so a window looks the same in both pacers.
fn color(pct: u32) -> &'static str {
    match usage::bucket(pct) {
        "crit" => "\x1b[38;2;255;85;85m",
        "hot" => "\x1b[38;2;230;200;0m",
        "warn" => "\x1b[38;2;255;176;85m",
        _ => "\x1b[38;2;0;175;80m",
    }
}

/// A line built from colored spans, measured in cells as it grows.
#[derive(Default)]
struct Line {
    text: String,
    width: usize,
}

impl Line {
    fn push(&mut self, color: &str, s: &str) -> &mut Self {
        self.text.push_str(color);
        self.text.push_str(s);
        self.text.push_str(RESET);
        self.width += s.chars().count();
        self
    }

    fn pad(&mut self, to: usize) -> &mut Self {
        let n = to.saturating_sub(self.width);
        self.text.extend(std::iter::repeat_n(' ', n));
        self.width += n;
        self
    }
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn reset_left(row: &Row) -> Option<String> {
    let left = row.resets? - usage::now();
    (left > 0).then(|| usage::duration(left))
}

fn status(rows: &[Row]) -> Option<&'static str> {
    match (rows.is_empty(), usage::fetching()) {
        (true, true) => Some("fetching usage…"),
        (true, false) => Some("no usage available — install codex · claude · omp"),
        _ => None,
    }
}

// ── the dock: a column per agent, buttons at the end of the first row ──

/// Where the buttons sit: herdr's scrollbar gutter covers the last column.
fn settings_x(cols: usize) -> usize {
    cols.saturating_sub(10)
}

fn refresh_x(cols: usize) -> usize {
    cols.saturating_sub(6)
}

fn hide_x(cols: usize) -> usize {
    cols.saturating_sub(2)
}

const WINDOW_LABELS: [&str; 3] = ["5h", "wk", "mo"];

fn shown(rows: &[Row], s: &Settings) -> Vec<Agent> {
    usage::agents(rows)
        .into_iter()
        .filter(|a| AGENTS.iter().position(|(k, _)| *k == a.key).is_none_or(|i| s.agents[i]))
        .collect()
}

/// One window of one column, in `content` cells: bar, percentage and reset
/// time while they fit, then less.
fn cell(line: &mut Line, label: &str, r: &Row, content: usize, dots: u8) {
    let with_reset = content >= 6 + 17;
    let cells = match content {
        c if with_reset => (c - 17).min(20),
        c if c >= 3 + 8 => (c - 8).min(20),
        _ => 0,
    };
    line.push(SUBTLE, label).push("", " ");
    if cells > 0 {
        let (used, rail) = usage::bar(r.pct, cells, dots);
        line.push(color(r.pct), &used).push(RAIL, &rail).push("", " ");
    }
    line.push(color(r.pct), &format!("{:>3}%", r.pct));
    if with_reset {
        let left = reset_left(r).map_or(String::new(), |l| format!("⟳ {l}"));
        line.push("", "  ").push(DIM, &left);
    }
}

fn strip(rows: &[Row], cols: usize, s: &Settings) -> Vec<String> {
    let mut agents = shown(rows, s);
    let windows: Vec<usize> = (0..3).filter(|&w| s.windows[w]).collect();
    let room = cols.saturating_sub(12);
    // at narrowest a column is "5h 100%" and two spaces
    let fit = (room / 9).max(1);
    let more = agents.len().saturating_sub(fit);
    agents.truncate(fit);
    let marker = if more > 0 { format!("+{more}") } else { String::new() };
    let width = match agents.len() {
        0 => room,
        n => (room.saturating_sub(marker.chars().count()) / n).min(48),
    };
    let content = width.saturating_sub(2);
    let mut lines: Vec<Line> = (0..=windows.len().max(1)).map(|_| Line::default()).collect();
    for a in &agents {
        let head = match (&a.plan, content >= a.title.len() + a.plan.len() + 2) {
            (plan, true) if !plan.is_empty() => format!("{}  {plan}", a.title),
            _ => a.title.to_string(),
        };
        let edge = lines[0].width + width;
        lines[0].push(WHITE, &truncate(&head, content)).pad(edge);
        for (i, &w) in windows.iter().enumerate() {
            let line = &mut lines[i + 1];
            let edge = line.width + width;
            match (a.windows()[w], &a.error) {
                (Some(r), _) => cell(line, WINDOW_LABELS[w], r, content, s.dock_dots),
                (None, Some(e)) if i == 0 => drop(line.push(DIM, &truncate(e, content))),
                _ => {}
            }
            line.pad(edge);
        }
    }
    lines[0].push(DIM, &marker);
    if let Some(msg) = status(rows) {
        lines[0].push(DIM, &truncate(msg, room));
    }
    lines[0]
        .pad(settings_x(cols))
        .push(SUBTLE, "⚙")
        .pad(refresh_x(cols))
        .push(SUBTLE, "⟳")
        .pad(hide_x(cols))
        .push(SUBTLE, "✕");
    lines.into_iter().map(|l| l.text).collect()
}

// ── the popup: a section per agent ──

fn full(rows: &[Row], s: &Settings) -> Vec<String> {
    let cells: usize = std::env::var("HERDR_PACER_BAR_CELLS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let mut out = vec![];
    let mut line = Line::default();
    line.push(SUBTLE, "HERDR usage");
    out.push(line.text);
    out.push(String::new());
    if let Some(s) = status(rows) {
        let mut line = Line::default();
        line.push(DIM, &format!("  {s}"));
        out.push(line.text);
    }
    let section = |a: &Agent, out: &mut Vec<String>| {
        let mut head = Line::default();
        head.push("", "  ").push(WHITE, a.title);
        if !a.plan.is_empty() {
            head.push(DIM, &format!("  {}", a.plan));
        }
        out.push(head.text);
        for (w, label) in ["5h", "weekly", "monthly"].into_iter().enumerate() {
            let Some(r) = a.windows()[w].as_ref().filter(|_| s.windows[w]) else { continue };
            let (used, rail) = usage::bar(r.pct, cells, s.dock_dots);
            let mut l = Line::default();
            l.push(SUBTLE, &format!("    {label:<7} "))
                .push(color(r.pct), &used)
                .push(RAIL, &rail)
                .push(color(r.pct), &format!("  {:>3}%", r.pct));
            if let Some(left) = reset_left(r) {
                l.push(DIM, &format!("   ⟳ {left}"));
            }
            out.push(l.text);
        }
        if let (true, Some(e)) = (a.windows().iter().all(|w| w.is_none()), &a.error) {
            let mut l = Line::default();
            l.push(DIM, &format!("    {e}"));
            out.push(l.text);
        }
        out.push(String::new());
    };
    for a in shown(rows, s) {
        section(&a, &mut out);
    }
    let mut foot = Line::default();
    foot.push(DIM, "r refresh · q close");
    out.push(foot.text);
    out
}

/// Home, each line then clear-to-end-of-line, then clear below: nothing is
/// erased before the new frame is on screen.
fn paint(lines: &[String]) {
    let mut out = String::from("\x1b[H");
    out.push_str(&lines.join("\x1b[K\r\n"));
    out.push_str("\x1b[K\x1b[J");
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(out.as_bytes());
    let _ = stdout.flush();
}

fn draw(dock: bool) {
    let (cols, _) = terminal::size().unwrap_or((80, 24));
    let rows = usage::load();
    let s = settings::load();
    paint(&if dock { strip(&rows, cols as usize, &s) } else { full(&rows, &s) });
}

/// Fetches on a thread, so painting and clicks never wait on a provider.
fn fetch_in_background(force: bool) {
    if force || usage::cache_age() >= usage::refresh_seconds() {
        std::thread::spawn(move || usage::fetch(force));
    }
}

pub fn run(dock: bool) -> std::io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    if dock {
        execute!(
            stdout,
            terminal::EnterAlternateScreen,
            terminal::DisableLineWrap,
            cursor::Hide,
            event::EnableMouseCapture
        )?;
    } else {
        execute!(stdout, cursor::Hide)?;
    }
    let result = run_loop(dock);
    if dock {
        let _ = execute!(stdout, event::DisableMouseCapture, cursor::Show, terminal::EnableLineWrap, terminal::LeaveAlternateScreen);
    } else {
        let _ = execute!(stdout, cursor::Show, terminal::Clear(terminal::ClearType::All));
    }
    let _ = terminal::disable_raw_mode();
    result
}

fn run_loop(dock: bool) -> std::io::Result<()> {
    fetch_in_background(false);
    draw(dock);
    let (mut seen, mut drawn, mut prefs) = (usage::cache_mtime(), usage::now(), settings::stamp());
    loop {
        let mut repaint = false;
        if event::poll(Duration::from_secs(1))? {
            match event::read()? {
                Event::Resize(..) => repaint = true,
                Event::Key(k) => match k.code {
                    KeyCode::Char('r') | KeyCode::Char('R') => fetch_in_background(true),
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) && !dock => return Ok(()),
                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc | KeyCode::Enter if !dock => {
                        return Ok(())
                    }
                    _ => {}
                },
                Event::Mouse(m) if dock && m.kind == MouseEventKind::Down(MouseButton::Left) && m.row == 0 => {
                    let cols = terminal::size().map_or(80, |(c, _)| c as usize);
                    let x = m.column as usize;
                    if x + 2 >= hide_x(cols) {
                        // hides every dock, this one last: that ends this process
                        let _ = crate::dock::toggle();
                    } else if x + 2 >= refresh_x(cols) {
                        fetch_in_background(true);
                    } else if x + 2 >= settings_x(cols) {
                        let _ = crate::herdr::call(
                            "plugin.pane.open",
                            serde_json::json!({ "plugin_id": "herdr-pacer", "entrypoint": "settings",
                                                "placement": "popup", "focus": true }),
                        );
                    }
                }
                _ => {}
            }
        } else {
            fetch_in_background(false);
        }
        let (cached, stamp) = (usage::cache_mtime(), settings::stamp());
        // repaint when new rows land, the settings or the pane size change, or
        // reset times tick over
        if repaint || cached != seen || stamp != prefs || usage::now() - drawn >= 60 {
            seen = cached;
            prefs = stamp;
            drawn = usage::now();
            draw(dock);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn row(provider: &str, window: &str, pct: u32) -> Row {
        Row { provider: provider.into(), plan: String::new(), window: window.into(), pct, resets: None, error: None }
    }

    fn strip_text(rows: &[Row], cols: usize, s: &Settings) -> Vec<String> {
        strip(rows, cols, s).iter().map(|l| visible(l)).collect()
    }

    #[test]
    fn strip_puts_buttons_before_the_gutter() {
        let rows = [row("openai-codex", "5h", 97), row("openai-codex", "7d", 45), row("claude", "5h", 5)];
        let lines = strip_text(&rows, 100, &Settings::default());
        assert_eq!(lines.len(), 3, "a title row and the 5h and weekly rows");
        assert_eq!(lines[0].chars().count(), 99, "the last column is herdr's gutter");
        for (x, c) in [(settings_x(100), '⚙'), (refresh_x(100), '⟳'), (hide_x(100), '✕')] {
            assert_eq!(lines[0].chars().nth(x), Some(c));
        }
        assert!(lines[0].starts_with("Codex") && lines[0].contains("Claude"));
        assert!(lines[1].starts_with("5h ⣤") && lines[1].contains(" 97%"));
        assert!(lines[2].starts_with("wk "));
    }

    #[test]
    fn strip_columns_line_up() {
        let rows = [row("openai-codex", "5h", 10), row("claude", "5h", 20), row("claude", "7d", 30)];
        let lines = strip_text(&rows, 120, &Settings::default());
        let claude_col = lines[0][..lines[0].find("Claude").unwrap()].chars().count();
        let at = |l: &str| l.chars().skip(claude_col).take(2).collect::<String>();
        assert_eq!((at(&lines[1]), at(&lines[2])), ("5h".into(), "wk".into()));
    }

    #[test]
    fn strip_gives_up_detail_as_it_narrows() {
        let rows = [row("openai-codex", "5h", 97), row("claude", "5h", 5), row("opencode-go", "5h", 50)];
        let s = Settings { windows: [true, false, false], dock_dots: 3, ..Settings::default() };
        let wide = strip_text(&rows, 150, &s);
        assert!(wide[1].contains("⣶") && wide[1].contains(" 97%"));
        let narrow = strip_text(&rows, 50, &s);
        assert!(!narrow[1].contains('⣶') && !narrow[1].contains('⣀'), "numbers only: {:?}", narrow[1]);
        assert!(narrow[1].contains("5h  97%") && narrow[1].contains("5h  50%"));
        let tiny = strip_text(&rows, 30, &s);
        assert!(tiny[0].contains("+1"), "the rest are counted: {:?}", tiny[0]);
        let only_claude = Settings { agents: [false, true, false], ..s };
        assert!(!strip_text(&rows, 150, &only_claude)[0].contains("Codex"));
    }
}
