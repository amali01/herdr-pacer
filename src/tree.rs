//! Who started whom, drawn in the Agents sidebar. An orchestrator names
//! itself as the parent on a worker's pane before starting it:
//!
//!   herdr pane report-metadata <pane> --source orchestrator --token pacer_parent=$HERDR_PANE_ID
//!
//! and the Agents view, sorted so each child sits under its parent, draws
//! the tree with each agent's status circle beside its name and its
//! workspace, tab and agent wrapped below, the lines unbroken:
//!
//!   ● orch
//!   │ herdr-pacer · 1
//!   ├─ ○ ui
//!   │  │ herdr-pacer · 1
//!   │  └─ ● ui-table
//!   │       herdr-pacer · 1
//!   └─ ○ api
//!        herdr-pacer · 1
//!
//! An agent no one started is a root; a link to a pane that holds no agent
//! counts as none, and a loop is cut at one of its agents. The sort lives in
//! Herdr until its server exits, so the startup hook puts it back.

use crate::herdr::{self, call};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// Each status's circle and color: Herdr's dots, so the circle can sit
/// beside the name rather than in Herdr's own column at the far left.
pub const STATES: [(&str, &str, &str); 5] = [
    ("idle", "○", "#00af50"),
    ("working", "●", "#e6c800"),
    ("blocked", "●", "#ff5555"),
    ("done", "●", "#4a9eff"),
    ("unknown", "○", "#8a8494"),
];

/// The rows an agent's details wrap onto; past the last, the rest is cut.
pub const WHERE: [&str; 3] = ["pacer_where", "pacer_where2", "pacer_where3"];

struct Node {
    pane: String,
    label: String,
    parent: Option<String>,
    status: String,
    /// where it runs: workspace and tab, one piece per name between ` · `
    place: Vec<String>,
    agent: Option<String>,
}

/// An agent's place: what leads its own line (the rails and its branch),
/// what leads its children's (`cont`), and its sort key.
#[derive(Clone, Debug, Default, PartialEq)]
struct Place {
    /// the parent it is drawn under
    up: Option<usize>,
    head: String,
    cont: String,
    kids: bool,
    key: String,
}

/// Each agent's place, in agent.list order. Keys are fixed-width positions
/// joined by '.', so a child sorts after its parent and before the parent's
/// next sibling.
fn layout(nodes: &[Node]) -> Vec<Place> {
    let at = |pane: &str| nodes.iter().position(|n| n.pane == pane);
    let parent = |i: usize| nodes[i].parent.as_deref().and_then(at).filter(|&p| p != i);
    let children: Vec<Vec<usize>> = (0..nodes.len()).map(|p| (0..nodes.len()).filter(|&c| parent(c) == Some(p)).collect()).collect();
    let mut out = vec![Place::default(); nodes.len()];
    let mut seen = vec![false; nodes.len()];
    let roots: Vec<usize> = (0..nodes.len()).filter(|&i| parent(i).is_none()).collect();
    for i in roots.into_iter().chain(0..nodes.len()) {
        if seen[i] {
            continue;
        }
        // left unreached, i hangs off a loop: climb into it and cut it there,
        // so the loop's other agents and everything below keep their parents
        let mut top = i;
        let mut climbed = vec![false; nodes.len()];
        while let Some(p) = parent(top).filter(|&p| !std::mem::replace(&mut climbed[p], true)) {
            top = p;
        }
        let mut stack = vec![(top, None, String::new(), None::<bool>, format!("{top:04}"))];
        while let Some((n, up, prefix, last, key)) = stack.pop() {
            if std::mem::replace(&mut seen[n], true) {
                continue;
            }
            let (branch, cont) = match last {
                None => ("", String::new()),
                Some(true) => ("└─ ", format!("{prefix}   ")),
                Some(false) => ("├─ ", format!("{prefix}│  ")),
            };
            let kids: Vec<usize> = children[n].iter().copied().filter(|&c| !seen[c]).collect();
            for (k, &c) in kids.iter().enumerate().rev() {
                stack.push((c, Some(n), cont.clone(), Some(k + 1 == kids.len()), format!("{key}.{c:04}")));
            }
            out[n] = Place { up, head: format!("{prefix}{branch}"), cont, kids: !kids.is_empty(), key };
        }
    }
    out
}

/// Herdr trims leading spaces from a token, not a braille blank.
fn lead(line: String) -> String {
    match line.strip_prefix(' ') {
        Some(rest) => format!("⠀{rest}"),
        None => line,
    }
}

/// Pieces joined by ` · ` onto at most `rows` lines of `width` cells. Only a
/// piece too long for a line of its own, or what overflows the last line, is
/// cut. shortcut: counts chars as cells, so wide emoji may still get cut by Herdr.
fn wrap(pieces: &[String], width: usize, rows: usize) -> Vec<String> {
    let len = |s: &str| s.chars().count();
    let fit = |s: &str| if len(s) <= width { s.to_string() } else { s.chars().take(width.saturating_sub(1)).collect::<String>() + "…" };
    let mut lines: Vec<String> = vec![];
    for p in pieces {
        match lines.last_mut() {
            Some(l) if len(l) + 3 + len(p) <= width => *l += &format!(" · {p}"),
            _ => lines.push(fit(p)),
        }
    }
    if lines.len() > rows {
        let rest = lines.split_off(rows - 1).join(" · ");
        lines.push(fit(&rest));
    }
    lines
}

/// The tokens that draw one agent: its line, colored by status, its details
/// below the rails, and its sort key. Unused ones are cleared. Its details
/// leave out where it runs when that is where its parent runs.
fn tokens(p: &Place, n: &Node, up: Option<&Node>, width: usize) -> Map<String, Value> {
    let status = STATES.iter().find(|s| s.0 == n.status).map_or("unknown", |s| s.0);
    let mut map = Map::new();
    for (state, circle, _) in STATES {
        // Herdr starts an agent's first row two cells left of the rest
        let line = (state == status).then(|| lead(format!("  {}{circle} {}", p.head, n.label)));
        map.insert(format!("pacer_tree_{state}"), line.map_or(Value::Null, Value::from));
    }
    let prefix = format!("{}{}", p.cont, if p.kids { "│ " } else { "  " });
    let mut detail: Vec<String> = n.place.iter().filter(|w| !up.is_some_and(|u| u.place.contains(w))).chain(&n.agent).cloned().collect();
    // a tab named after its workspace or agent (auto-title) says it once
    let mut said = std::collections::HashSet::new();
    detail.retain(|w| said.insert(w.clone()));
    let lines = wrap(&detail, width.saturating_sub(prefix.chars().count()).max(1), WHERE.len());
    for (i, token) in WHERE.iter().enumerate() {
        map.insert(token.to_string(), lines.get(i).map_or(Value::Null, |l| lead(format!("{prefix}{l}")).into()));
    }
    map.insert("pacer_tree_key".into(), p.key.clone().into());
    map
}

/// A `[ui]` setting from config.toml.
fn ui(key: &str) -> Option<toml_edit::Item> {
    let config = std::fs::read_to_string(crate::setup::config_path()).ok()?;
    let doc: toml_edit::DocumentMut = config.parse().ok()?;
    doc.get("ui")?.get(key).cloned()
}

/// Redraws every agent's place in the tree, reporting only what changed.
pub fn sync() -> herdr::Result<()> {
    // one run at a time, so an older snapshot never lands after a newer one
    let lock = std::fs::File::create(crate::usage::state_dir().join("tree.lock")).ok();
    let _held = lock.as_ref().map(|f| f.lock());
    let list = |method: &str, key: &str| call(method, json!({})).map(|r| r[key].as_array().cloned().unwrap_or_default());
    let text = |a: &Value, k: &str| a[k].as_str().filter(|s| !s.is_empty()).map(String::from);
    let labels = |items: Vec<Value>, id: &str| -> HashMap<String, String> {
        items.iter().filter_map(|w| Some((text(w, id)?, text(w, "label")?))).collect()
    };
    let agents = list("agent.list", "agents")?;
    let workspaces = labels(list("workspace.list", "workspaces")?, "workspace_id");
    let tabs = labels(list("tab.list", "tabs")?, "tab_id");
    let nodes: Vec<Node> = agents
        .iter()
        .map(|a| {
            let at = |labels: &HashMap<String, String>, id: &str| labels.get(a[id].as_str().unwrap_or_default()).cloned();
            let place = [at(&workspaces, "workspace_id"), at(&tabs, "tab_id")]
                .into_iter()
                .flatten()
                .flat_map(|s| s.split(" · ").map(|w| w.trim().to_string()).collect::<Vec<_>>())
                .filter(|w| !w.is_empty())
                .collect();
            Node {
                pane: text(a, "pane_id").unwrap_or_default(),
                label: text(a, "name").or_else(|| text(a, "agent")).unwrap_or_else(|| "agent".into()),
                parent: text(&a["tokens"], "pacer_parent"),
                status: text(a, "agent_status").unwrap_or_default(),
                place,
                agent: text(a, "display_agent").or_else(|| text(a, "agent")),
            }
        })
        .collect();
    // the content width: the sidebar less its margins and border
    let width = (ui("sidebar_width").and_then(|w| w.as_integer()).unwrap_or(26) as usize).saturating_sub(4);
    let mut failed = None;
    let mut places = layout(&nodes);
    if priority() {
        // Herdr orders by attention, not by the tree: no branches to mislead
        places.iter_mut().for_each(|p| *p = Place { key: std::mem::take(&mut p.key), ..Place::default() });
    }
    for ((place, node), a) in places.iter().zip(&nodes).zip(&agents) {
        let changed: Map<String, Value> = tokens(place, node, place.up.map(|u| &nodes[u]), width).into_iter().filter(|(k, v)| &a["tokens"][k] != v).collect();
        if node.pane.is_empty() || changed.is_empty() {
            continue;
        }
        // a pane that closed since the list fails alone; the rest still update
        if let Err(e) = herdr::report_tokens(&node.pane, Value::Object(changed), None) {
            failed = Some(e);
        }
    }
    failed.map_or(Ok(()), Err)
}

/// Whether the user picked the attention-queue order for the Agents view.
fn priority() -> bool {
    ui("agent_panel_sort").is_some_and(|s| s.as_str() == Some("priority"))
}

/// Sorts the Agents view by the tree. Left alone when the user picked the
/// attention-queue order (`agent_panel_sort = "priority"`).
pub fn view() -> herdr::Result<()> {
    if priority() {
        return Ok(());
    }
    let sort = [json!({ "field": { "token": "pacer_tree_key" }, "order": "asc" })]
        .into_iter()
        .chain(["workspace_order", "tab_order", "pane_order"].map(|f| json!({ "field": f, "order": "asc" })))
        .collect::<Vec<_>>();
    call("agent.view.set", json!({ "source": "plugin:herdr-pacer", "label": "tree", "sort": sort })).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(pane: &str, label: &str, parent: Option<&str>) -> Node {
        let place = vec!["herdr-pacer".into(), "1".into()];
        Node { pane: pane.into(), label: label.into(), parent: parent.map(String::from), status: "idle".into(), place, agent: Some("claude".into()) }
    }

    /// Every agent's lines, in sort order, as the sidebar draws them.
    fn drawn(nodes: &[Node], width: usize) -> Vec<String> {
        let places = layout(nodes);
        let mut order: Vec<usize> = (0..nodes.len()).collect();
        order.sort_by_key(|&i| places[i].key.clone());
        let row = |m: &Map<String, Value>, k: &str| m[k].as_str().map(String::from);
        order
            .iter()
            .flat_map(|&i| {
                let m = tokens(&places[i], &nodes[i], places[i].up.map(|u| &nodes[u]), width);
                let head = STATES.iter().find_map(|s| row(&m, &format!("pacer_tree_{}", s.0)));
                head.into_iter().chain(WHERE.iter().filter_map(|w| row(&m, w))).collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn children_sit_under_their_parent_with_unbroken_lines() {
        let nodes = [
            node("p1", "orch", None),
            node("p2", "ui-table", Some("p3")),
            node("p3", "ui", Some("p1")),
            node("p4", "lone", None),
            node("p5", "api", Some("p1")),
        ];
        let mut nodes = nodes;
        nodes[4].place[1] = "2".into(); // api runs in another tab
        assert_eq!(
            drawn(&nodes, 40),
            [
                "⠀ ○ orch",
                "│ herdr-pacer · 1 · claude",
                "⠀ ├─ ○ ui",
                "│  │ claude",
                "⠀ │  └─ ○ ui-table",
                "│       claude",
                "⠀ └─ ○ api",
                "⠀    2 · claude",
                "⠀ ○ lone",
                "⠀ herdr-pacer · 1 · claude",
            ]
        );
    }

    #[test]
    fn details_wrap_rather_than_cut() {
        let mut nodes = [node("p1", "orch", None), node("p2", "ui", Some("p1"))];
        nodes[1].place = vec!["other-repo".into(), "3".into()];
        assert_eq!(drawn(&nodes, 18), ["⠀ ○ orch", "│ herdr-pacer · 1", "│ claude", "⠀ └─ ○ ui", "⠀    other-repo", "⠀    3 · claude"]);
        let pieces: Vec<String> = ["a-very-long-workspace-name", "1", "claude", "x", "y"].map(String::from).to_vec();
        assert_eq!(wrap(&pieces, 10, 2), ["a-very-lo…", "1 · claud…"]);
        assert_eq!(wrap(&pieces[1..], 10, 3), ["1 · claude", "x · y"]);
    }

    #[test]
    fn the_line_takes_its_status_and_clears_the_others() {
        let mut n = node("p1", "orch", None);
        n.status = "blocked".into();
        let m = tokens(&layout(std::slice::from_ref(&n))[0], &n, None, 22);
        assert_eq!(m["pacer_tree_blocked"], "⠀ ● orch");
        assert!(m["pacer_tree_idle"].is_null() && m["pacer_where3"].is_null());
        n.status = "something-new".into();
        assert_eq!(tokens(&Place::default(), &n, None, 22)["pacer_tree_unknown"], "⠀ ○ orch");
    }

    #[test]
    fn a_dead_or_looping_parent_makes_a_root() {
        let nodes = [node("p1", "a", Some("p2")), node("p2", "b", Some("p1")), node("p3", "c", Some("gone")), node("p4", "d", Some("p4"))];
        let out = layout(&nodes);
        assert_eq!((out[0].head.as_str(), out[0].key.as_str()), ("", "0000"), "the loop is cut at its first agent");
        assert_eq!((out[1].head.as_str(), out[1].key.as_str()), ("└─ ", "0000.0001"));
        assert_eq!(out[2].head, "");
        assert_eq!(out[3].head, "");
    }

    #[test]
    fn a_loop_keeps_the_agents_hanging_off_it() {
        // child → a, a ⇄ b, with the child listed first: it stays a's child
        let nodes = [node("c", "child", Some("a")), node("a", "a", Some("b")), node("b", "b", Some("a"))];
        let out = layout(&nodes);
        assert!(out[0].key.starts_with(&out[1].key) && out[0].head.ends_with("└─ "), "{out:?}");
    }
}
