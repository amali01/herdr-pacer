//! Who started whom, drawn in the Agents sidebar. An orchestrator names
//! itself as the parent on a worker's pane before starting it:
//!
//!   herdr pane report-metadata <pane> --source orchestrator --token pacer_parent=$HERDR_PANE_ID
//!
//! and the Agents view, sorted so each child sits under its parent and each
//! repo's agents (its worktrees too) sit together, gives each agent one line:
//! the tree, its status circle and name, its repo and, in superscript, its
//! account. An orchestrator goes by its repo, numbered when the repo has
//! more than one; a worker leaves out the repo its parent shows:
//!
//!   ● gh-dash orch 1 · ᶜˡᵃᵘᵈᵉ⁻³
//!   ├─ ○ ui · ᶜˡᵃᵘᵈᵉ
//!   │  └─ ● ui-table · ᶜᵒᵈᵉˣ
//!   └─ ○ docs · herdr-pacer ᶜˡᵃᵘᵈᵉ
//!   ● gh-dash orch 2 · ᶜˡᵃᵘᵈᵉ
//!   └─ ○ api · ᶜˡᵃᵘᵈᵉ
//!   ○ claude · gh-dash ᶜˡᵃᵘᵈᵉ
//!
//! An agent no one started is a root; a link to a pane that holds no agent
//! counts as none, and a loop is cut at one of its agents. The sort lives in
//! Herdr until its server exits, so the startup hook puts it back.

use crate::herdr::{self, call};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// Each status's circle and color: Herdr's dots, so the circle can sit
/// beside the name rather than in Herdr's own column at the far left. A
/// token has one color, so the line takes it; only the states that want
/// attention have one, so a list of idle agents stays plain to read.
pub const STATES: [(&str, &str, Option<&str>); 5] = [
    ("idle", "○", None),
    ("working", "●", Some("#e6c800")),
    ("blocked", "●", Some("#ff5555")),
    ("done", "●", Some("#4a9eff")),
    ("unknown", "○", None),
];

struct Node {
    pane: String,
    label: String,
    parent: Option<String>,
    status: String,
    /// the git repo it runs in, the main one for a worktree, and what tells
    /// two repos of the same name apart
    repo: Option<String>,
    repo_key: Option<String>,
    account: Option<String>,
}

/// An agent's place: the parent it is drawn under, what leads its line (the
/// rails and its branch), and its sort key.
#[derive(Clone, Debug, Default, PartialEq)]
struct Place {
    up: Option<usize>,
    head: String,
    key: String,
}

/// Each agent's place, in agent.list order. Keys are fixed-width positions
/// joined by '.', so a child sorts after its parent and before the parent's
/// next sibling; they start with the repo's first agent, so a repo's trees
/// sit together.
fn layout(nodes: &[Node]) -> Vec<Place> {
    let at = |pane: &str| nodes.iter().position(|n| n.pane == pane);
    let group = |i: usize| nodes.iter().position(|n| n.repo_key.is_some() && n.repo_key == nodes[i].repo_key).unwrap_or(i);
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
        let mut stack = vec![(top, None, String::new(), None::<bool>, format!("{:04}.{top:04}", group(top)))];
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
            out[n] = Place { up, head: format!("{prefix}{branch}"), key };
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

/// A terminal has one font size; superscript is the smallest text it draws.
/// There is no superscript q, so it stays as it is.
fn tiny(s: &str) -> String {
    let (from, to) = ("abcdefghijklmnoprstuvwxyz0123456789-", "ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ⁰¹²³⁴⁵⁶⁷⁸⁹⁻");
    s.chars().map(|c| c.to_ascii_lowercase()).map(|c| from.chars().zip(to.chars()).find(|f| f.0 == c).map_or(c, |f| f.1)).collect()
}

/// The name an orchestrator (a root with workers) goes by: its repo, and a
/// number when the repo has more than one. None for every other agent.
fn orchestrators(nodes: &[Node], places: &[Place]) -> Vec<Option<String>> {
    let orch = |i: usize| places[i].up.is_none() && nodes[i].repo.is_some() && places.iter().any(|p| p.up == Some(i));
    let mut all: Vec<usize> = (0..nodes.len()).filter(|&i| orch(i)).collect();
    all.sort_by_key(|&i| &places[i].key);
    (0..nodes.len())
        .map(|i| {
            let repo = nodes[i].repo.as_deref().filter(|_| orch(i))?;
            let same: Vec<usize> = all.iter().copied().filter(|&j| nodes[j].repo_key == nodes[i].repo_key).collect();
            Some(match same.iter().position(|&j| j == i) {
                Some(k) if same.len() > 1 => format!("{repo} orch {}", k + 1),
                _ => format!("{repo} orch"),
            })
        })
        .collect()
}

/// Every agent's tokens, in agent.list order. Its repo shows unless its name
/// carries it or its parent shows the same.
fn draw(nodes: &[Node], places: &[Place]) -> Vec<Map<String, Value>> {
    let names = orchestrators(nodes, places);
    nodes
        .iter()
        .zip(places)
        .zip(names)
        .map(|((n, p), name)| {
            let up = p.up.map(|u| &nodes[u]);
            let repo = n.repo.as_deref().filter(|&r| name.is_none() && up.is_none_or(|u| u.repo.as_deref() != Some(r)));
            tokens(p, n, name.as_deref().unwrap_or(&n.label), repo)
        })
        .collect()
}

/// The tokens that draw one agent: its line in its status color, its repo
/// and account, and its sort key.
fn tokens(p: &Place, n: &Node, name: &str, repo: Option<&str>) -> Map<String, Value> {
    let status = STATES.iter().find(|s| s.0 == n.status).map_or("unknown", |s| s.0);
    let mut map = Map::new();
    for (state, circle, _) in STATES {
        // Herdr starts a row that has no state_icon two cells further left
        let line = (state == status).then(|| lead(format!("  {}{circle} {name}", p.head)));
        map.insert(format!("pacer_tree_{state}"), line.map_or(Value::Null, Value::from));
    }
    let with: Vec<String> = repo.map(String::from).into_iter().chain(n.account.as_deref().map(tiny)).collect();
    map.insert("pacer_repo".into(), if with.is_empty() { Value::Null } else { with.join(" ").into() });
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
    let agents = list("agent.list", "agents")?;
    // a workspace in a git repo names it, worktrees the main one; others go by their label
    let repos: HashMap<String, (String, String)> = list("workspace.list", "workspaces")?
        .iter()
        .filter_map(|w| {
            let tree = &w["worktree"];
            let id = text(w, "workspace_id")?;
            // outside git, each workspace is a repo of its own, whatever its label
            let key = text(tree, "repo_key").unwrap_or_else(|| format!("workspace:{id}"));
            Some((id, (text(tree, "repo_name").or_else(|| text(w, "label"))?, key)))
        })
        .collect();
    let nodes: Vec<Node> = agents
        .iter()
        .map(|a| {
            let (repo, repo_key) = repos.get(a["workspace_id"].as_str().unwrap_or_default()).cloned().unzip();
            Node {
                pane: text(a, "pane_id").unwrap_or_default(),
                label: text(a, "name").or_else(|| text(a, "agent")).unwrap_or_else(|| "agent".into()),
                parent: text(&a["tokens"], "pacer_parent"),
                status: text(a, "agent_status").unwrap_or_default(),
                repo,
                repo_key,
                account: text(a, "display_agent").or_else(|| text(a, "agent")),
            }
        })
        .collect();
    let mut places = layout(&nodes);
    if priority() {
        // Herdr orders by attention, not by the tree: no branches to mislead,
        // and no tree sort left over from before the setting changed
        let _ = view();
        places.iter_mut().for_each(|p| *p = Place { key: std::mem::take(&mut p.key), ..Place::default() });
    }
    let mut failed = None;
    for ((want, node), a) in draw(&nodes, &places).into_iter().zip(&nodes).zip(&agents) {
        let changed: Map<String, Value> = want.into_iter().filter(|(k, v)| &a["tokens"][k] != v).collect();
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

/// Sorts the Agents view by the tree, or hands it back to Herdr when the
/// user picked the attention-queue order (`agent_panel_sort = "priority"`).
pub fn view() -> herdr::Result<()> {
    if priority() {
        return call("agent.view.clear", json!({ "source": "plugin:herdr-pacer" })).map(drop);
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
        let (repo, repo_key, account) = (Some("gh-dash".into()), Some("/gh-dash/.git".into()), Some("claude".into()));
        Node { pane: pane.into(), label: label.into(), parent: parent.map(String::from), status: "idle".into(), repo, repo_key, account }
    }

    fn in_repo(mut n: Node, repo: &str) -> Node {
        (n.repo, n.repo_key) = (Some(repo.into()), Some(format!("/{repo}/.git")));
        n
    }

    /// Every agent's line, in sort order, as the sidebar draws it.
    fn drawn(nodes: &[Node]) -> Vec<String> {
        let places = layout(nodes);
        let mut lines: Vec<(String, String)> = draw(nodes, &places)
            .iter()
            .map(|m| {
                let line = STATES.iter().find_map(|s| m[&format!("pacer_tree_{}", s.0)].as_str()).unwrap();
                (m["pacer_tree_key"].as_str().unwrap().into(), format!("{line} · {}", m["pacer_repo"].as_str().unwrap_or_default()))
            })
            .collect();
        lines.sort();
        lines.into_iter().map(|l| l.1).collect()
    }

    #[test]
    fn an_orchestrator_goes_by_its_repo_and_its_workers_sit_under_it() {
        let mut nodes = [
            node("p1", "orch", None),
            node("p2", "ui-table", Some("p3")),
            node("p3", "ui", Some("p1")),
            in_repo(node("p4", "lone", None), "herdr-pacer"),
            in_repo(node("p5", "docs", Some("p1")), "herdr-pacer"),
        ];
        nodes[0].account = Some("Claude-3".into());
        nodes[1].account = Some("codex".into());
        assert_eq!(
            drawn(&nodes),
            [
                "⠀ ○ gh-dash orch · ᶜˡᵃᵘᵈᵉ⁻³",
                "⠀ ├─ ○ ui · ᶜˡᵃᵘᵈᵉ",
                "⠀ │  └─ ○ ui-table · ᶜᵒᵈᵉˣ",
                "⠀ └─ ○ docs · herdr-pacer ᶜˡᵃᵘᵈᵉ",
                "⠀ ○ lone · herdr-pacer ᶜˡᵃᵘᵈᵉ",
            ]
        );
    }

    #[test]
    fn a_repos_agents_sit_together_and_its_orchestrators_are_numbered() {
        let nodes = [
            node("p1", "orch-a", None),
            in_repo(node("p2", "other", None), "herdr-pacer"),
            node("p3", "worker-a", Some("p1")),
            node("p4", "lone", None),
            node("p5", "orch-b", None),
            node("p6", "worker-b", Some("p5")),
        ];
        assert_eq!(
            drawn(&nodes),
            [
                "⠀ ○ gh-dash orch 1 · ᶜˡᵃᵘᵈᵉ",
                "⠀ └─ ○ worker-a · ᶜˡᵃᵘᵈᵉ",
                "⠀ ○ lone · gh-dash ᶜˡᵃᵘᵈᵉ",
                "⠀ ○ gh-dash orch 2 · ᶜˡᵃᵘᵈᵉ",
                "⠀ └─ ○ worker-b · ᶜˡᵃᵘᵈᵉ",
                "⠀ ○ other · herdr-pacer ᶜˡᵃᵘᵈᵉ",
            ]
        );
    }

    #[test]
    fn the_line_takes_its_status_and_clears_the_others() {
        let mut n = node("p1", "orch", None);
        n.status = "blocked".into();
        let m = tokens(&layout(std::slice::from_ref(&n))[0], &n, "orch", None);
        assert_eq!(m["pacer_tree_blocked"], "⠀ ● orch");
        assert!(m["pacer_tree_idle"].is_null());
        n.status = "something-new".into();
        assert_eq!(tokens(&Place::default(), &n, "orch", None)["pacer_tree_unknown"], "⠀ ○ orch");
        n.account = None;
        assert!(tokens(&Place::default(), &n, "orch", None)["pacer_repo"].is_null(), "no repo or account, no token");
    }

    #[test]
    fn a_dead_or_looping_parent_makes_a_root() {
        let nodes = [node("p1", "a", Some("p2")), node("p2", "b", Some("p1")), node("p3", "c", Some("gone")), node("p4", "d", Some("p4"))];
        let out = layout(&nodes);
        assert_eq!((out[0].head.as_str(), out[0].key.as_str()), ("", "0000.0000"), "the loop is cut at its first agent");
        assert_eq!((out[1].head.as_str(), out[1].key.as_str()), ("└─ ", "0000.0000.0001"));
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
