//! Who started whom, drawn in the Agents sidebar. An orchestrator names
//! itself as the parent on a worker's pane before starting it:
//!
//!   herdr pane report-metadata <pane> --source orchestrator --token pacer_parent=$HERDR_PANE_ID
//!
//! and the Agents view, sorted so each child sits under its parent and each
//! repo's agents (its worktrees too) sit together, gives each agent one line:
//! its branch, Herdr's own status circle, its name, its repo and, in
//! superscript, its account. An orchestrator goes by its repo, numbered when
//! the repo has more than one; a worker leaves out the repo its parent shows:
//!
//!   ● gh-dash orch 1 · ᶜˡᵃᵘᵈᵉ⁻³
//!     ├ · ○ ui · ᶜˡᵃᵘᵈᵉ
//!     │   └ · ● ui-table · ᶜᵒᵈᵉˣ
//!     └ · ○ docs · herdr-pacer ᶜˡᵃᵘᵈᵉ
//!   ○ claude · gh-dash ᶜˡᵃᵘᵈᵉ
//!
//! Herdr puts " · " between two tokens and only a space after its circle, so
//! the branch meets the circle across a dot in the tree's own gray; that way
//! the circle is Herdr's, in the theme's status colors, and the name is not.
//!
//! An agent no one started is a root; a link to a pane that holds no agent
//! counts as none, and a loop is cut at one of its agents. The sort lives in
//! Herdr until its server exits, so the startup hook puts it back.

use crate::herdr::{self, call};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// The rows an agent's subagents take; past the last, a count.
pub const SUBS: [&str; 3] = ["pacer_sub1", "pacer_sub2", "pacer_sub3"];

struct Node {
    pane: String,
    label: String,
    parent: Option<String>,
    /// the git repo it runs in, the main one for a worktree, and what tells
    /// two repos of the same name apart
    repo: Option<String>,
    repo_key: Option<String>,
    account: Option<String>,
    /// Claude Code's own subagents running inside it, oldest first
    subs: Vec<String>,
}

/// An agent's place: the parent it is drawn under, what leads its circle (the
/// rails and its branch, none for a root), the tree's lines that lead the
/// rows below it (`rail`: the rails running on), and its sort key.
#[derive(Clone, Debug, Default, PartialEq)]
struct Place {
    up: Option<usize>,
    head: String,
    /// what leads a branch under it: its children's and its subagents'
    cont: String,
    /// whether agents of its own hang under it, after its subagents
    children: bool,
    rail: String,
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
            // a child's branch sits under its parent's circle: " · " and the
            // circle make four cells, so each level steps in four
            let (branch, cont) = match last {
                None => ("", String::new()),
                Some(true) => ("└", format!("{prefix}    ")),
                Some(false) => ("├", format!("{prefix}│   ")),
            };
            let kids: Vec<usize> = children[n].iter().copied().filter(|&c| !seen[c]).collect();
            for (k, &c) in kids.iter().enumerate().rev() {
                stack.push((c, Some(n), cont.clone(), Some(k + 1 == kids.len()), format!("{key}.{c:04}")));
            }
            // under the name: a root's starts where Herdr starts the row
            // its own line runs down from its circle when anything hangs under it
            let rail = match kids.is_empty() && nodes[n].subs.is_empty() {
                true => cont.clone(),
                false => format!("{cont}│"),
            };
            out[n] = Place { up, head: format!("{prefix}{branch}"), cont, children: !kids.is_empty(), rail, key };
        }
    }
    out
}

/// Herdr trims leading spaces from a token, not a braille blank.
pub fn lead(line: String) -> String {
    match line.strip_prefix(' ') {
        Some(rest) => format!("⠀{rest}"),
        None => line,
    }
}

/// A terminal has one font size; superscript is the smallest text it draws.
/// There is no superscript q, so it stays as it is.
pub fn tiny(s: &str) -> String {
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

/// The tokens that draw one agent: its branch, its name, its repo and
/// account, the rails of its usage line, and its sort key.
fn tokens(p: &Place, n: &Node, name: &str, repo: Option<&str>) -> Map<String, Value> {
    let mut map = Map::new();
    // a branch lines up under the name of a root, whose circle Herdr draws
    // two cells left of where a row's later lines start
    let branch = (!p.head.is_empty()).then(|| lead(format!("  {}", p.head)));
    map.insert("pacer_branch".into(), branch.map_or(Value::Null, Value::from));
    map.insert("pacer_name".into(), name.into());
    let with: Vec<String> = repo.map(String::from).into_iter().chain(n.account.as_deref().map(tiny)).collect();
    map.insert("pacer_repo".into(), if with.is_empty() { Value::Null } else { with.join(" ").into() });
    // the usage rows lead with them in a token of their own, so the tree stays
    // gray whatever color the usage takes; Herdr trims a token's spaces at
    // both ends, braille blanks it keeps
    let lines = (!p.rail.is_empty()).then(|| p.rail.replace(' ', "⠀"));
    map.insert("pacer_lines".into(), lines.map_or(Value::Null, Value::from));
    map.insert("pacer_tree_key".into(), p.key.clone().into());
    // its subagents branch off under its usage, before its own children's entries
    let mut subs: Vec<String> = n.subs.iter().map(|s| format!("⁺ {s}")).collect();
    if subs.len() > SUBS.len() {
        let more = subs.len() - (SUBS.len() - 1);
        subs.truncate(SUBS.len() - 1);
        subs.push(format!("⁺ {more} more"));
    }
    for (i, token) in SUBS.iter().enumerate() {
        let line = subs.get(i).map(|s| {
            let last = i + 1 == subs.len() && !p.children;
            lead(format!("{}{} {s}", p.cont, if last { "└" } else { "├" }))
        });
        map.insert(token.to_string(), line.map_or(Value::Null, Value::from));
    }
    map
}

/// The cells a sidebar row has past Herdr's margins and border.
pub fn width() -> usize {
    (ui("sidebar_width").and_then(|w| w.as_integer()).unwrap_or(26) as usize).saturating_sub(4)
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
                repo,
                repo_key,
                account: text(a, "display_agent").or_else(|| text(a, "agent")),
                subs: running(&text(a, "pane_id").unwrap_or_default()),
            }
        })
        .collect();
    forget_closed(&nodes);
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
        if let Err(e) = herdr::report_tokens(&node.pane, Value::Object(changed.clone()), None) {
            failed = Some(e);
        } else if changed.contains_key("pacer_lines") {
            // the room the usage rows have changed: a bar may fit, or not
            crate::context::redraw(&node.pane, a["agent"].as_str().unwrap_or_default());
        }
    }
    failed.map_or(Ok(()), Err)
}

// ── Claude Code's subagents ──
//
// They run inside Claude's own process, so Herdr never lists them. Claude's
// SubagentStart and SubagentStop hooks (`herdr-pacer subagent start|stop`,
// which `claude-hook install` adds) keep each pane's running ones in a file,
// and the tree lists them under their agent.

/// How long a subagent whose stop never came stays listed.
const SUB_TTL: i64 = 6 * 3600;

fn subs_file(pane: &str) -> std::path::PathBuf {
    crate::usage::state_dir().join(format!("subagents-{pane}.json"))
}

fn subs_of(pane: &str) -> Map<String, Value> {
    std::fs::read_to_string(subs_file(pane)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// A pane's running subagents, by type, oldest first.
fn running(pane: &str) -> Vec<String> {
    let mut subs: Vec<(i64, String)> =
        subs_of(pane).values().filter_map(|v| Some((v["at"].as_i64()?, v["type"].as_str()?.to_string()))).collect();
    subs.sort();
    subs.into_iter().filter(|s| s.0 > crate::usage::now() - SUB_TTL).map(|s| s.1).collect()
}

/// A subagent started (with its type) or stopped, in a pane's list. Claude
/// also stops agents of its own that never started under a type; those, and
/// ones past their time, are dropped.
fn note(subs: &mut Map<String, Value>, id: &str, kind: Option<&str>, now: i64) {
    match kind.filter(|k| !k.is_empty()) {
        Some(kind) => drop(subs.insert(id.into(), json!({ "type": kind, "at": now }))),
        None => drop(subs.remove(id)),
    }
    subs.retain(|_, v| v["at"].as_i64().is_some_and(|at| at > now - SUB_TTL));
}

/// `subagent start|stop`, a Claude hook: the payload on stdin.
pub fn subagent(event: &str) -> Result<(), String> {
    let start = match event {
        "start" => true,
        "stop" => false,
        _ => return Err("usage: herdr-pacer subagent start | stop".into()),
    };
    let Ok(pane) = std::env::var("HERDR_PANE_ID") else { return Ok(()) }; // Claude outside Herdr
    let mut payload = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut payload);
    let hook: Value = serde_json::from_str(&payload).unwrap_or_default();
    let Some(id) = hook["agent_id"].as_str() else { return Ok(()) };
    {
        // parallel subagents start together: one read-modify-write at a time
        let lock = std::fs::File::create(crate::usage::state_dir().join("subagents.lock")).ok();
        let _held = lock.as_ref().map(|f| f.lock());
        let mut subs = subs_of(&pane);
        note(&mut subs, id, if start { hook["agent_type"].as_str() } else { None }, crate::usage::now());
        let _ = match subs.is_empty() {
            true => std::fs::remove_file(subs_file(&pane)),
            false => std::fs::write(subs_file(&pane), Value::Object(subs).to_string()),
        };
    }
    sync().map_err(|e| e.to_string())
}

/// A closed pane's list goes with it.
fn forget_closed(nodes: &[Node]) {
    for file in std::fs::read_dir(crate::usage::state_dir()).into_iter().flatten().flatten() {
        let name = file.file_name().to_string_lossy().into_owned();
        let pane = name.strip_prefix("subagents-").and_then(|n| n.strip_suffix(".json"));
        if pane.is_some_and(|p| !nodes.iter().any(|n| n.pane == p)) {
            let _ = std::fs::remove_file(file.path());
        }
    }
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
        Node { pane: pane.into(), label: label.into(), parent: parent.map(String::from), repo, repo_key, account, subs: vec![] }
    }

    fn in_repo(mut n: Node, repo: &str) -> Node {
        (n.repo, n.repo_key) = (Some(repo.into()), Some(format!("/{repo}/.git")));
        n
    }

    /// Every agent's line, in sort order, as Herdr draws it: " · " between
    /// tokens, a space after its circle (○ here).
    fn drawn(nodes: &[Node]) -> Vec<String> {
        let places = layout(nodes);
        let mut lines: Vec<(String, String)> = draw(nodes, &places)
            .iter()
            .map(|m| {
                let branch = m["pacer_branch"].as_str().map(|b| format!("{b} · ")).unwrap_or_default();
                let repo = m["pacer_repo"].as_str().map(|r| format!(" · {r}")).unwrap_or_default();
                (m["pacer_tree_key"].as_str().unwrap().into(), format!("{branch}○ {}{repo}", m["pacer_name"].as_str().unwrap()))
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
                "○ gh-dash orch · ᶜˡᵃᵘᵈᵉ⁻³",
                "⠀ ├ · ○ ui · ᶜˡᵃᵘᵈᵉ",
                "⠀ │   └ · ○ ui-table · ᶜᵒᵈᵉˣ",
                "⠀ └ · ○ docs · herdr-pacer ᶜˡᵃᵘᵈᵉ",
                "○ lone · herdr-pacer ᶜˡᵃᵘᵈᵉ",
            ]
        );
        // the usage line under each runs the rails on, its text under the name
        let rails: Vec<String> = layout(&nodes).into_iter().map(|p| p.rail).collect();
        assert_eq!(rails, ["│", "│       ", "│   │", "", "    "]);
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
                "○ gh-dash orch 1 · ᶜˡᵃᵘᵈᵉ",
                "⠀ └ · ○ worker-a · ᶜˡᵃᵘᵈᵉ",
                "○ lone · gh-dash ᶜˡᵃᵘᵈᵉ",
                "○ gh-dash orch 2 · ᶜˡᵃᵘᵈᵉ",
                "⠀ └ · ○ worker-b · ᶜˡᵃᵘᵈᵉ",
                "○ other · herdr-pacer ᶜˡᵃᵘᵈᵉ",
            ]
        );
    }

    #[test]
    fn subagents_branch_off_under_their_agent_before_its_children() {
        let mut nodes = [node("p1", "orch", None), node("p2", "ui", Some("p1")), node("p3", "lone", None)];
        nodes[1].subs = vec!["Explore".into(), "goldfish-reviewer".into()];
        nodes[2].subs = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let places = layout(&nodes);
        let maps = draw(&nodes, &places);
        let subs = |i: usize| SUBS.iter().filter_map(|t| maps[i][*t].as_str().map(String::from)).collect::<Vec<_>>();
        assert_eq!(subs(1), ["⠀   ├ ⁺ Explore", "⠀   └ ⁺ goldfish-reviewer"], "under ui's circle");
        assert_eq!(places[1].rail, "    │", "its line runs down to them");
        assert_eq!(subs(2), ["├ ⁺ a", "├ ⁺ b", "└ ⁺ 2 more"], "past three, a count");
        assert!(subs(0).is_empty() && places[0].rail == "│");
    }

    #[test]
    fn a_subagent_is_listed_from_its_start_to_its_stop() {
        let mut subs = Map::new();
        note(&mut subs, "a1", Some("Explore"), 1000);
        note(&mut subs, "a2", Some("Plan"), 1001);
        note(&mut subs, "x", Some(""), 1002);
        assert_eq!(subs.len(), 2, "Claude's own untyped agents are not listed");
        note(&mut subs, "a1", None, 1003);
        note(&mut subs, "never-started", None, 1004);
        assert_eq!(subs.keys().collect::<Vec<_>>(), ["a2"]);
        note(&mut subs, "a3", Some("Explore"), 1001 + SUB_TTL);
        assert_eq!(subs.keys().collect::<Vec<_>>(), ["a3"], "one whose stop never came goes in time");
    }

    #[test]
    fn a_root_has_no_branch_and_nothing_said_has_no_token() {
        let mut n = node("p1", "orch", None);
        n.account = None;
        let m = tokens(&layout(std::slice::from_ref(&n))[0], &n, "orch", None);
        assert!(m["pacer_branch"].is_null() && m["pacer_repo"].is_null());
        assert_eq!(m["pacer_name"], "orch");
    }

    #[test]
    fn a_dead_or_looping_parent_makes_a_root() {
        let nodes = [node("p1", "a", Some("p2")), node("p2", "b", Some("p1")), node("p3", "c", Some("gone")), node("p4", "d", Some("p4"))];
        let out = layout(&nodes);
        assert_eq!((out[0].head.as_str(), out[0].key.as_str()), ("", "0000.0000"), "the loop is cut at its first agent");
        assert_eq!((out[1].head.as_str(), out[1].key.as_str()), ("└", "0000.0000.0001"));
        assert_eq!(out[2].head, "");
        assert_eq!(out[3].head, "");
    }

    #[test]
    fn a_loop_keeps_the_agents_hanging_off_it() {
        // child → a, a ⇄ b, with the child listed first: it stays a's child
        let nodes = [node("c", "child", Some("a")), node("a", "a", Some("b")), node("b", "b", Some("a"))];
        let out = layout(&nodes);
        assert!(out[0].key.starts_with(&out[1].key) && out[0].head.ends_with('└'), "{out:?}");
    }
}
