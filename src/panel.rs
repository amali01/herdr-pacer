//! The settings popup, opened from the dock's ⚙: what the dock, the agent
//! sidebar and the tab bar show, and how their bars are drawn. Mouse or keys;
//! every change is saved and applied at once.
//!
//!   Usage dock
//!     Agents     [x] Codex    [x] Claude   [x] OpenCode
//!     Windows    [x] 5h       [x] Weekly   [ ] Monthly
//!     Style      (•) Dots   ( ) Bar    ( ) Blocks  ( ) Slants
//!     Rows       ( ) 1 row    (•) 2 rows   ( ) 3 rows       ⣤⣤⣤⣤⣤⡄⣀⣀ 64%
//!
//! The row under Style is named for what it changes — dot rows for Dots,
//! thickness for Bar, shade for Blocks — and a preview beside it draws the
//! bar as it will look. Slants come in one size, and so does "numbers" in the
//! tab bar, which has no bar at all.
//!
//! The agents are listed in the order the dock, the popup and the tab bar show
//! them; `<` and `>` (or shift ←→) move the focused one.

use crate::settings::{self, Settings, AGENTS};
use crate::usage::{self, Style};
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::{cursor, execute, terminal};
use std::io::Write;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const FOCUS: &str = "\x1b[7m";
const SUBTLE: &str = "\x1b[38;2;150;140;160m";
const RAIL: &str = "\x1b[38;2;70;66;78m";
const WARN: &str = "\x1b[38;2;255;176;85m";
const SAMPLE: u32 = 64; // the preview's percentage

#[derive(Clone, Copy, PartialEq)]
enum Setting {
    Agent(usize),
    Window(usize),
    DockStyle(Style),
    DockSize(u8),
    Metric(usize),
    OneLine(bool),
    SidebarStyle(Style),
    SidebarSize(u8),
    TabAgent(usize),
    TabWindow(usize),
    TabStyle(Option<Style>),
    TabSize(u8),
    /// Shown, not chosen: the one size a style comes in.
    Fixed,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    Styles,
    Size(Option<Style>, u8), // the style it sizes, and the size, for the preview
}

struct Item {
    label: &'static str,
    kind: Kind,
    options: Vec<(Setting, &'static str)>,
}

fn style_options(to: fn(Style) -> Setting) -> Vec<(Setting, &'static str)> {
    vec![(to(Style::Dots), "Dots"), (to(Style::Bar), "Bar"), (to(Style::Blocks), "Blocks"), (to(Style::Slants), "Slants")]
}

/// The row under Style: named, and its choices worded, for that style.
fn size_item(style: Option<Style>, size: u8, to: fn(u8) -> Setting) -> Item {
    let three = |label, names: [&'static str; 3]| Item {
        label,
        kind: Kind::Size(style, size),
        options: (1..=3).map(to).zip(names).collect(),
    };
    match style {
        Some(Style::Dots) => three("Rows", ["1 row", "2 rows", "3 rows"]),
        Some(Style::Bar) => three("Thickness", ["thin", "medium", "thick"]),
        Some(Style::Blocks) => three("Shade", ["light", "medium", "solid"]),
        Some(Style::Slants) => Item { label: "Size", kind: Kind::Size(style, size), options: vec![(Setting::Fixed, "one size")] },
        None => Item { label: "Size", kind: Kind::Size(None, size), options: vec![(Setting::Fixed, "no bar")] },
    }
}

fn items(s: &Settings) -> Vec<Item> {
    let plain = |label, options| Item { label, kind: Kind::Plain, options };
    // the same columns as the other style rows, and the numbers alone last
    let mut tab_styles = style_options(|st| Setting::TabStyle(Some(st)));
    tab_styles.push((Setting::TabStyle(None), "numbers"));
    let agents = |to: fn(usize) -> Setting| s.order.iter().map(|&i| (to(i), AGENTS[i].1)).collect();
    vec![
        plain("Agents", agents(Setting::Agent)),
        plain("Windows", vec![(Setting::Window(0), "5h"), (Setting::Window(1), "Weekly"), (Setting::Window(2), "Monthly")]),
        Item { label: "Style", kind: Kind::Styles, options: style_options(Setting::DockStyle) },
        size_item(Some(s.dock_style), s.dock_size, Setting::DockSize),
        plain("Show", vec![(Setting::Metric(0), "Context"), (Setting::Metric(1), "5h"), (Setting::Metric(2), "Weekly")]),
        plain("Layout", vec![(Setting::OneLine(false), "a row each"), (Setting::OneLine(true), "one line")]),
        Item { label: "Style", kind: Kind::Styles, options: style_options(Setting::SidebarStyle) },
        size_item(Some(s.sidebar_style), s.sidebar_size, Setting::SidebarSize),
        plain("Show", agents(Setting::TabAgent)),
        plain("Windows", vec![(Setting::TabWindow(0), "5h"), (Setting::TabWindow(1), "Weekly"), (Setting::TabWindow(2), "Monthly")]),
        Item { label: "Style", kind: Kind::Styles, options: tab_styles },
        size_item(s.tab_style, s.tab_size, Setting::TabSize),
    ]
}

fn is_on(s: &Settings, what: Setting) -> bool {
    match what {
        Setting::Agent(i) => s.agents[i],
        Setting::Window(i) => s.windows[i],
        Setting::DockStyle(st) => s.dock_style == st,
        Setting::DockSize(d) => s.dock_size == d,
        Setting::Metric(i) => s.metrics[i],
        Setting::OneLine(b) => s.one_line == b,
        Setting::SidebarStyle(st) => s.sidebar_style == st,
        Setting::SidebarSize(d) => s.sidebar_size == d,
        Setting::TabAgent(i) => s.tab_agents[i],
        Setting::TabWindow(i) => s.tab_windows[i],
        Setting::TabStyle(st) => s.tab_style == st,
        Setting::TabSize(d) => s.tab_size == d,
        Setting::Fixed => true,
    }
}

fn toggle_window(windows: &mut [bool; 3], i: usize) {
    windows[i] = !windows[i];
    if !windows.contains(&true) {
        windows[i] = true; // a view keeps at least one window
    }
}

/// Toggles a check box or picks a choice.
fn apply(s: &Settings, what: Setting) -> Settings {
    let mut s = s.clone();
    match what {
        Setting::Agent(i) => s.agents[i] = !s.agents[i],
        Setting::Window(i) => toggle_window(&mut s.windows, i),
        Setting::DockStyle(st) => s.dock_style = st,
        Setting::DockSize(d) => s.dock_size = d,
        Setting::Metric(i) => s.metrics[i] = !s.metrics[i],
        Setting::OneLine(b) => s.one_line = b,
        Setting::SidebarStyle(st) => s.sidebar_style = st,
        Setting::SidebarSize(d) => s.sidebar_size = d,
        Setting::TabAgent(i) => s.tab_agents[i] = !s.tab_agents[i],
        Setting::TabWindow(i) => toggle_window(&mut s.tab_windows, i),
        Setting::TabStyle(st) => s.tab_style = st,
        Setting::TabSize(d) => s.tab_size = d,
        Setting::Fixed => {}
    }
    s
}

/// Moves an agent `step` places along the order: the new settings and the
/// place it lands on, or None for anything but an agent or at either end.
fn reorder(s: &Settings, what: Setting, step: isize) -> Option<(Settings, usize)> {
    let (Setting::Agent(i) | Setting::TabAgent(i)) = what else { return None };
    let from = s.order.iter().position(|&o| o == i)?;
    let to = from.checked_add_signed(step).filter(|&to| to < s.order.len())?;
    let mut s = s.clone();
    s.order.swap(from, to);
    Some((s, to))
}

/// Saves, then brings the dock, the sidebar and the tab bar in line with what
/// changed. The dock follows on its own: it watches the settings file.
fn commit(before: &Settings, after: &Settings) {
    settings::save(after);
    if before.windows != after.windows {
        let _ = crate::dock::resize_all(); // a row per window
    }
    if before.one_line != after.one_line {
        let _ = crate::setup::apply_rows();
    }
    let sidebar = |s: &Settings| (s.metrics, s.one_line, s.sidebar_style, s.sidebar_size);
    if sidebar(before) != sidebar(after) {
        crate::context::report_all();
    }
    // the tab bar commands exist while any agent is on, and read the rest of
    // the tab bar settings as they run; a reload runs them at once rather
    // than at their next interval
    let tab = |s: &Settings| (s.tab_agents, s.tab_windows, s.tab_style, s.tab_size);
    let on = |s: &Settings| s.tab_agents.contains(&true);
    if on(before) != on(after) || (on(after) && before.order != after.order) {
        let _ = crate::setup::apply_tabbar();
    } else if on(after) && tab(before) != tab(after) {
        let _ = crate::herdr::call("server.reload_config", serde_json::json!({}));
    }
}

// ── layout ──

const COL: [usize; 3] = [15, 28, 41]; // where options start, three to a row
const STYLE_COL: [usize; 5] = [15, 26, 36, 48, 60]; // the style rows'
const PREVIEW: usize = 58;
const HEADINGS: [(usize, &str); 3] = [(2, "Usage dock"), (8, "Agent sidebar"), (14, "Tab bar")];
const FIRST_ROW: [usize; 12] = [3, 4, 5, 6, 9, 10, 11, 12, 15, 16, 17, 18]; // each item's screen row
const FOOTER: usize = 20;

fn starts(item: &Item) -> Vec<usize> {
    match (item.kind, item.options.len()) {
        (Kind::Styles, n) => STYLE_COL[..n].to_vec(),
        (_, 2) => vec![COL[0], COL[2]],
        (_, n) => COL[..n].to_vec(),
    }
}

/// (item, option) at a 0-based screen cell, for the mouse. Fixed options,
/// shown only, are not hit.
fn hit(items: &[Item], x: usize, y: usize) -> Option<(usize, usize)> {
    let i = FIRST_ROW.iter().position(|&r| r == y)?;
    let opt = starts(&items[i]).iter().rposition(|&c| x + 1 >= c)?;
    (items[i].options.get(opt)?.0 != Setting::Fixed).then_some((i, opt))
}

fn preview(style: Option<Style>, size: u8) -> String {
    match style {
        Some(style) => {
            let (used, rail) = usage::bar(SAMPLE, 8, style, size);
            format!("{WARN}{used}{RESET}{RAIL}{rail}{RESET} {WARN}{SAMPLE}%{RESET}")
        }
        None => format!("{WARN}{SAMPLE}%{RESET}"),
    }
}

fn render(s: &Settings, focus: (usize, usize)) -> String {
    let items = items(s);
    let mut rows = vec![String::new(); FOOTER + 1];
    rows[0] = format!(" {BOLD}herdr-pacer settings{RESET}");
    for (row, heading) in HEADINGS {
        rows[row] = format!(" {SUBTLE}{heading}{RESET}");
    }
    rows[FOOTER] = format!(" {DIM}↑↓ ←→ move · space picks · < > reorders agents · click · q closes{RESET}");
    for (i, item) in items.iter().enumerate() {
        let mut line = format!("   {:<10}", item.label);
        let mut width = 13;
        for (j, ((what, name), start)) in item.options.iter().zip(starts(item)).enumerate() {
            line.extend(std::iter::repeat_n(' ', (start - 1).saturating_sub(width)));
            width = width.max(start - 1);
            let text = match (item.kind, what) {
                (_, Setting::Fixed) => name.to_string(),
                (Kind::Plain, _) if !matches!(what, Setting::OneLine(_)) => {
                    format!("{} {name}", if is_on(s, *what) { "[x]" } else { "[ ]" })
                }
                _ => format!("{} {name}", if is_on(s, *what) { "(•)" } else { "( )" }),
            };
            width += text.chars().count();
            let style = match (focus == (i, j), what) {
                (_, Setting::Fixed) => DIM,
                (true, _) => FOCUS,
                _ => "",
            };
            line.push_str(&format!("{style}{text}{RESET}"));
        }
        if let Kind::Size(style, size) = item.kind {
            line.extend(std::iter::repeat_n(' ', (PREVIEW - 1).saturating_sub(width)));
            line.push_str(&preview(style, size));
        }
        rows[FIRST_ROW[i]] = line;
    }
    format!("\x1b[H{}\x1b[K\x1b[J", rows.join("\x1b[K\r\n"))
}

pub fn run() -> std::io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide, event::EnableMouseCapture)?;
    let result = run_loop();
    let _ = execute!(out, event::DisableMouseCapture, cursor::Show, terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    result
}

fn run_loop() -> std::io::Result<()> {
    let mut s = settings::load();
    let mut focus = (0, 0);
    let mut out = std::io::stdout();
    loop {
        out.write_all(render(&s, focus).as_bytes())?;
        out.flush()?;
        let items = items(&s);
        let last = items.len() - 1;
        let choose = match event::read()? {
            Event::Key(k) => match k.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char(c @ ('<' | '>')) => move_agent(&mut s, &mut focus, &items, if c == '<' { -1 } else { 1 }),
                KeyCode::Left | KeyCode::Right if k.modifiers.contains(KeyModifiers::SHIFT) => {
                    move_agent(&mut s, &mut focus, &items, if k.code == KeyCode::Left { -1 } else { 1 })
                }
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                KeyCode::Up | KeyCode::Char('k') => {
                    focus.0 = focus.0.saturating_sub(1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    focus.0 = (focus.0 + 1).min(last);
                    None
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    focus.1 = focus.1.saturating_sub(1);
                    None
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    focus.1 += 1;
                    None
                }
                KeyCode::Char(' ') | KeyCode::Enter => Some(focus),
                _ => None,
            },
            Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                hit(&items, m.column as usize, m.row as usize).inspect(|&f| focus = f)
            }
            _ => None,
        };
        focus.1 = focus.1.min(items[focus.0].options.len() - 1);
        if let Some((i, j)) = choose {
            let next = apply(&s, items[i].options[j].0);
            commit(&s, &next);
            s = next;
        }
    }
}

/// `<` and `>`: moves the focused agent, and the focus with it.
fn move_agent(s: &mut Settings, focus: &mut (usize, usize), items: &[Item], step: isize) -> Option<(usize, usize)> {
    let what = items[focus.0].options.get(focus.1)?.0;
    if let Some((next, to)) = reorder(s, what, step) {
        commit(s, &next);
        *s = next;
        focus.1 = to;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible(screen: &str) -> Vec<String> {
        screen
            .split("\r\n")
            .map(|l| {
                let mut out = String::new();
                let mut chars = l.chars();
                while let Some(c) = chars.next() {
                    if c == '\x1b' {
                        chars.by_ref().find(|c| c.is_ascii_alphabetic());
                    } else {
                        out.push(c);
                    }
                }
                out
            })
            .collect()
    }

    #[test]
    fn toggles_and_choices() {
        let s = Settings::default();
        assert!(!apply(&s, Setting::Agent(1)).agents[1]);
        assert_eq!(apply(&s, Setting::DockStyle(Style::Blocks)).dock_style, Style::Blocks);
        assert_eq!(apply(&s, Setting::DockSize(3)).dock_size, 3);
        let one = Settings { windows: [true, false, false], ..s.clone() };
        assert_eq!(apply(&one, Setting::Window(0)).windows, [true, false, false], "the dock keeps a window");
        assert!(apply(&s, Setting::OneLine(true)).one_line);
        assert!(apply(&s, Setting::TabAgent(2)).tab_agents[2]);
        assert_eq!(apply(&s, Setting::TabStyle(None)).tab_style, None);
        assert_eq!(apply(&s, Setting::Fixed), s);
    }

    #[test]
    fn agents_move_along_the_order() {
        let s = Settings::default();
        let (claude_first, at) = reorder(&s, Setting::Agent(1), -1).unwrap();
        assert_eq!((claude_first.order, at), ([1, 0, 2], 0));
        assert_eq!(items(&claude_first)[0].options[0].1, "Claude", "listed in the order");
        assert_eq!(reorder(&claude_first, Setting::TabAgent(2), 1), None, "already last");
        assert_eq!(reorder(&s, Setting::Window(0), 1), None, "only agents move");
    }

    #[test]
    fn the_row_under_style_is_named_for_it() {
        let names = |style| {
            let item = size_item(style, 2, Setting::DockSize);
            (item.label, item.options.iter().map(|o| o.1).collect::<Vec<_>>())
        };
        assert_eq!(names(Some(Style::Dots)), ("Rows", vec!["1 row", "2 rows", "3 rows"]));
        assert_eq!(names(Some(Style::Bar)), ("Thickness", vec!["thin", "medium", "thick"]));
        assert_eq!(names(Some(Style::Blocks)), ("Shade", vec!["light", "medium", "solid"]));
        assert_eq!(names(Some(Style::Slants)), ("Size", vec!["one size"]));
        assert_eq!(names(None), ("Size", vec!["no bar"]));
    }

    #[test]
    fn clicks_land_on_what_is_drawn() {
        let s = Settings { sidebar_style: Style::Slants, ..Settings::default() };
        let items = items(&s);
        let rows = visible(&render(&s, (0, 0)));
        // every option starts where hit() looks for it
        for (i, item) in items.iter().enumerate() {
            for (j, ((_, name), start)) in item.options.iter().zip(starts(item)).enumerate() {
                let row = &rows[FIRST_ROW[i]];
                let at: String = row.chars().skip(start - 1).take(name.chars().count() + 4).collect();
                assert!(at.contains(name), "item {i} option {j}: {at:?} in {row:?}");
                let expected = (items[i].options[j].0 != Setting::Fixed).then_some((i, j));
                assert_eq!(hit(&items, start, FIRST_ROW[i]), expected);
            }
        }
        assert_eq!(hit(&items, 2, FIRST_ROW[0]), None, "the label");
        assert_eq!(hit(&items, COL[0], 7), None, "a blank row");
        assert!(rows[FIRST_ROW[3]].contains("Rows") && rows[FIRST_ROW[3]].contains("⣤"), "a preview");
        assert!(rows[FIRST_ROW[7]].contains("one size") && rows[FIRST_ROW[7]].contains("▰"));
        assert!(rows.iter().all(|r| r.chars().count() <= 72), "fits the popup");
        // two cells at least between options, so none runs into the next
        for (i, item) in items.iter().enumerate() {
            let starts = starts(item);
            for (j, (_, name)) in item.options.iter().enumerate().take(starts.len() - 1) {
                assert!(starts[j] + name.chars().count() + 4 + 2 <= starts[j + 1] + 1, "item {i}: {name} crowds the next");
            }
        }
    }
}
