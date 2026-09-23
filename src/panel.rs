//! The settings popup, opened from the dock's ⚙: what the dock, the agent
//! sidebar and the tab bar show. Mouse or keys; every change is saved and
//! applied at once.
//!
//!   Usage dock
//!     Agents    [x] Codex    [x] Claude    [x] OpenCode
//!     Windows   [x] 5h       [x] Weekly    [ ] Monthly
//!     Dots      ( ) 1 row    (•) 2 rows    ( ) 3 rows
//!
//!   Agent sidebar
//!     Show      [x] Context  [ ] 5h        [ ] Weekly
//!     Layout    (•) a row each             ( ) one line
//!     Dots      ( ) 1 row    (•) 2 rows    ( ) 3 rows
//!
//!   Tab bar
//!     Show      [ ] Codex    [ ] Claude    [ ] OpenCode
//!     Windows   [x] 5h       [x] Weekly    [ ] Monthly
//!     Bar       ( ) numbers  ( ) 1 row     (•) 2 rows    ( ) 3 rows

use crate::settings::{self, Settings};
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::{cursor, execute, terminal};
use std::io::Write;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const FOCUS: &str = "\x1b[7m";
const SUBTLE: &str = "\x1b[38;2;150;140;160m";

#[derive(Clone, Copy, PartialEq)]
enum Setting {
    Agent(usize),
    Window(usize),
    DockDots(u8),
    Metric(usize),
    OneLine(bool),
    SidebarDots(u8),
    TabAgent(usize),
    TabWindow(usize),
    TabDots(u8),
}

struct Item {
    label: &'static str,
    options: Vec<(Setting, &'static str)>,
}

fn items() -> Vec<Item> {
    let dots = |f: fn(u8) -> Setting| vec![(f(1), "1 row"), (f(2), "2 rows"), (f(3), "3 rows")];
    vec![
        Item { label: "Agents", options: vec![(Setting::Agent(0), "Codex"), (Setting::Agent(1), "Claude"), (Setting::Agent(2), "OpenCode")] },
        Item { label: "Windows", options: vec![(Setting::Window(0), "5h"), (Setting::Window(1), "Weekly"), (Setting::Window(2), "Monthly")] },
        Item { label: "Dots", options: dots(Setting::DockDots) },
        Item { label: "Show", options: vec![(Setting::Metric(0), "Context"), (Setting::Metric(1), "5h"), (Setting::Metric(2), "Weekly")] },
        Item { label: "Layout", options: vec![(Setting::OneLine(false), "a row each"), (Setting::OneLine(true), "one line")] },
        Item { label: "Dots", options: dots(Setting::SidebarDots) },
        Item { label: "Show", options: vec![(Setting::TabAgent(0), "Codex"), (Setting::TabAgent(1), "Claude"), (Setting::TabAgent(2), "OpenCode")] },
        Item { label: "Windows", options: vec![(Setting::TabWindow(0), "5h"), (Setting::TabWindow(1), "Weekly"), (Setting::TabWindow(2), "Monthly")] },
        Item {
            label: "Bar",
            options: vec![(Setting::TabDots(0), "numbers"), (Setting::TabDots(1), "1 row"), (Setting::TabDots(2), "2 rows"), (Setting::TabDots(3), "3 rows")],
        },
    ]
}

fn is_on(s: &Settings, what: Setting) -> bool {
    match what {
        Setting::Agent(i) => s.agents[i],
        Setting::Window(i) => s.windows[i],
        Setting::DockDots(d) => s.dock_dots == d,
        Setting::Metric(i) => s.metrics[i],
        Setting::OneLine(b) => s.one_line == b,
        Setting::SidebarDots(d) => s.sidebar_dots == d,
        Setting::TabAgent(i) => s.tab_agents[i],
        Setting::TabWindow(i) => s.tab_windows[i],
        Setting::TabDots(d) => s.tab_dots == d,
    }
}

/// Toggles a check box or picks a choice. The dock keeps at least one window.
fn apply(s: &Settings, what: Setting) -> Settings {
    let mut s = s.clone();
    match what {
        Setting::Agent(i) => s.agents[i] = !s.agents[i],
        Setting::Window(i) => {
            s.windows[i] = !s.windows[i];
            if !s.windows.contains(&true) {
                s.windows[i] = true;
            }
        }
        Setting::DockDots(d) => s.dock_dots = d,
        Setting::Metric(i) => s.metrics[i] = !s.metrics[i],
        Setting::OneLine(b) => s.one_line = b,
        Setting::SidebarDots(d) => s.sidebar_dots = d,
        Setting::TabAgent(i) => s.tab_agents[i] = !s.tab_agents[i],
        Setting::TabWindow(i) => {
            s.tab_windows[i] = !s.tab_windows[i];
            if !s.tab_windows.contains(&true) {
                s.tab_windows[i] = true;
            }
        }
        Setting::TabDots(d) => s.tab_dots = d,
    }
    s
}

/// Saves, then brings the dock and the sidebar in line with what changed.
fn commit(before: &Settings, after: &Settings) {
    settings::save(after);
    if before.windows != after.windows {
        let _ = crate::dock::resize_all(); // a row per window
    }
    if before.one_line != after.one_line {
        let _ = crate::setup::apply_rows();
    }
    if (before.metrics, before.one_line, before.sidebar_dots) != (after.metrics, after.one_line, after.sidebar_dots) {
        crate::context::report_all();
    }
    // the tab bar commands exist while any agent is on, and read the rest of
    // the tab bar settings as they run; a reload runs them at once rather
    // than at their next interval
    if before.tab_agents.contains(&true) != after.tab_agents.contains(&true) {
        let _ = crate::setup::apply_tabbar();
    } else if after.tab_agents.contains(&true)
        && (before.tab_agents, before.tab_windows, before.tab_dots) != (after.tab_agents, after.tab_windows, after.tab_dots)
    {
        let _ = crate::herdr::call("server.reload_config", serde_json::json!({}));
    }
}

const COL: [usize; 4] = [14, 27, 40, 53]; // where each option starts on its line
const FIRST_ROW: [usize; 9] = [3, 4, 5, 8, 9, 10, 13, 14, 15]; // the screen row of each item

/// Where the options of an item start: two spread over the span, else a column each.
fn starts(n: usize) -> Vec<usize> {
    if n == 2 { vec![COL[0], COL[2]] } else { COL[..n].to_vec() }
}

/// (item, option) at a 0-based screen cell, for the mouse.
fn hit(x: usize, y: usize) -> Option<(usize, usize)> {
    let item = FIRST_ROW.iter().position(|&r| r == y)?;
    let n = items()[item].options.len();
    let opt = starts(n).iter().rposition(|&c| x + 1 >= c)?;
    (opt < n).then_some((item, opt))
}

fn render(s: &Settings, focus: (usize, usize)) -> String {
    let items = items();
    let mut rows = vec![String::new(); 18];
    rows[0] = format!(" {BOLD}herdr-pacer settings{RESET}");
    rows[2] = format!(" {SUBTLE}Usage dock{RESET}");
    rows[7] = format!(" {SUBTLE}Agent sidebar{RESET}");
    rows[12] = format!(" {SUBTLE}Tab bar{RESET}");
    rows[17] = format!(" {DIM}↑↓ ←→ move · space toggles · click · q closes{RESET}");
    for (i, item) in items.iter().enumerate() {
        let mut line = format!("   {:<9}", item.label);
        let mut width = 12;
        let starts = starts(item.options.len());
        for (j, (what, name)) in item.options.iter().enumerate() {
            line.extend(std::iter::repeat_n(' ', (starts[j] - 1).saturating_sub(width)));
            width = width.max(starts[j] - 1);
            let radio =
                matches!(what, Setting::DockDots(_) | Setting::OneLine(_) | Setting::SidebarDots(_) | Setting::TabDots(_));
            let mark = match (radio, is_on(s, *what)) {
                (true, true) => "(•)",
                (true, false) => "( )",
                (false, true) => "[x]",
                (false, false) => "[ ]",
            };
            let text = format!("{mark} {name}");
            width += text.chars().count();
            match focus == (i, j) {
                true => line.push_str(&format!("{FOCUS}{text}{RESET}")),
                false => line.push_str(&text),
            }
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
    let items = items();
    let mut out = std::io::stdout();
    loop {
        out.write_all(render(&s, focus).as_bytes())?;
        out.flush()?;
        let choose = match event::read()? {
            Event::Key(k) => match k.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                KeyCode::Up | KeyCode::Char('k') => {
                    focus.0 = focus.0.saturating_sub(1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    focus.0 = (focus.0 + 1).min(items.len() - 1);
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
                hit(m.column as usize, m.row as usize).inspect(|&f| focus = f)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_and_choices() {
        let s = Settings::default();
        assert!(!apply(&s, Setting::Agent(1)).agents[1]);
        assert_eq!(apply(&s, Setting::DockDots(3)).dock_dots, 3);
        let one = Settings { windows: [true, false, false], ..s.clone() };
        assert_eq!(apply(&one, Setting::Window(0)).windows, [true, false, false], "the dock keeps a window");
        assert!(apply(&s, Setting::OneLine(true)).one_line);
        assert!(apply(&s, Setting::TabAgent(2)).tab_agents[2]);
        assert_eq!(apply(&s, Setting::TabDots(0)).tab_dots, 0);
    }

    #[test]
    fn clicks_land_on_options() {
        assert_eq!(hit(COL[0] - 1, FIRST_ROW[0]), Some((0, 0)));
        assert_eq!(hit(COL[1] + 3, FIRST_ROW[1]), Some((1, 1)));
        assert_eq!(hit(COL[2] - 1, FIRST_ROW[4]), Some((4, 1)), "one line");
        assert_eq!(hit(2, FIRST_ROW[0]), None, "the label");
        assert_eq!(hit(COL[0], 6), None, "a blank row");
        assert_eq!(hit(COL[3] + 2, FIRST_ROW[8]), Some((8, 3)), "the tab bar's 3 rows");
        assert_eq!(hit(COL[3] + 2, FIRST_ROW[6]), Some((6, 2)), "past the last of three");
    }

    #[test]
    fn options_render_where_clicks_look() {
        let screen = render(&Settings::default(), (0, 0));
        let rows: Vec<String> = screen.split("\r\n").map(|l| {
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
        }).collect();
        let row = &rows[FIRST_ROW[1]];
        for (col, text) in COL.iter().zip(["[x] 5h", "[x] Weekly", "[ ] Monthly"]) {
            assert_eq!(row.chars().skip(col - 1).take(text.chars().count()).collect::<String>(), text);
        }
    }
}
