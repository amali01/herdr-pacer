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

struct Node {
    pane: String,
    label: String,
    parent: Option<String>,
    /// the git repo it runs in, the main one for a worktree, and what tells
    /// two repos of the same name apart
    repo: Option<String>,
    repo_key: Option<String>,
    account: Option<String>,
}

/// An agent's place: the parent it is drawn under, what leads its circle (the
/// rails and its branch, none for a root), the tree's lines that lead the
/// rows below it (`rail`: the rails running on), and its sort key.
#[derive(Clone, Debug, Default, PartialEq)]
struct Place {
    up: Option<usize>,
    head: String,
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
            let rail = match kids.is_empty() {
                true => cont.clone(),
                false => format!("{cont}│"),
            };
            out[n] = Place { up, head: format!("{prefix}{branch}"), rail, key };
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
                // a subagent's view goes by its task (subview.rs)
                label: text(&a["tokens"], "pacer_task").or_else(|| text(a, "name")).or_else(|| text(a, "agent")).unwrap_or_else(|| "agent".into()),
                parent: text(&a["tokens"], "pacer_parent"),
                repo,
                repo_key,
                account: text(a, "display_agent").or_else(|| text(a, "agent")),
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
// which `claude-hook install` adds) give each one a pane of its own in its
// workspace's "subagents" tab, a live view of what it does (subview.rs),
// that Herdr lists as an agent under its parent; it closes when the
// subagent stops. A file per parent pane keeps which pane is whose.

/// How long a subagent whose stop never came stays listed.
const SUB_TTL: i64 = 6 * 3600;
/// How long a stop that came before its start is kept, to turn the start away.
const STOP_TTL: i64 = 600;

fn subs_file(pane: &str) -> std::path::PathBuf {
    crate::usage::state_dir().join(format!("subagents-{pane}.json"))
}

fn subs_of(pane: &str) -> Map<String, Value> {
    std::fs::read_to_string(subs_file(pane)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// A subagent started (with its type) or stopped, in a pane's list. Claude
/// also starts agents of its own with no type; those are not listed. The
/// hooks are async, so a short one's stop can come first: it is kept a while
/// and turns the late start away.
fn note(subs: &mut Map<String, Value>, id: &str, kind: Option<&str>, now: i64) {
    let stopped = subs.get(id).is_some_and(|v| v.get("stopped").is_some());
    match kind {
        Some("") => {}
        Some(_) if stopped => drop(subs.remove(id)),
        // Claude starts a subagent again when it resumes: it keeps its pane
        Some(kind) => drop(subs.entry(id).or_insert_with(|| json!({ "type": kind, "at": now }))),
        None if subs.remove(id).is_some_and(|v| v.get("type").is_some()) => {}
        None => drop(subs.insert(id.into(), json!({ "stopped": now }))),
    }
    subs.retain(|_, v| match (v["at"].as_i64(), v["stopped"].as_i64()) {
        (Some(at), _) => at > now - SUB_TTL,
        (_, Some(at)) => at > now - STOP_TTL,
        _ => false,
    });
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
    let Some(id) = hook["agent_id"].as_str().filter(|i| !i.is_empty()) else { return Ok(()) };
    {
        // parallel subagents start together: one at a time, so they share
        // one subagents tab and one file
        let lock = std::fs::File::create(crate::usage::state_dir().join("subagents.lock")).ok();
        let _held = lock.as_ref().map(|f| f.lock());
        let mut subs = subs_of(&pane);
        let had = subs.get(id).and_then(|v| v["view"].as_str()).map(String::from);
        note(&mut subs, id, if start { hook["agent_type"].as_str() } else { None }, crate::usage::now());
        // a resumed one whose view was closed meanwhile gets a new one
        let open = |e: &Value| e["view"].as_str().is_none_or(|v| herdr::gone("pane.get", json!({ "pane_id": v })));
        if let Some(entry) = subs.get_mut(id).filter(|e| start && e.get("type").is_some() && open(e)) {
            if let Some(view) = open_view(&pane, id, &hook) {
                entry["view"] = json!(view);
            }
        }
        // whole, so the tree never reads one half written
        let _ = match subs.is_empty() {
            true => std::fs::remove_file(subs_file(&pane)),
            false => crate::usage::write_atomic(&subs_file(&pane), Value::Object(subs).to_string().as_bytes()),
        };
        // still under the lock: a start must not pick the pane, or the tab,
        // this takes away
        if let Some(view) = had.filter(|_| !start) {
            let _ = call("pane.close", json!({ "pane_id": view }));
        }
    };
    sync().map_err(|e| e.to_string())
}

/// The pane of a subagent that a pane's Claude runs, while it has one.
pub fn view_of(pane: &str, id: &str) -> Option<String> {
    subs_of(pane).get(id)?["view"].as_str().map(String::from)
}

/// The tab of a workspace that holds its subagents' panes, as it is named;
/// it is known by its id, kept in a file, since a plugin may rename it.
const TAB: &str = "subagents";

fn tab_file(workspace: &str) -> std::path::PathBuf {
    crate::usage::state_dir().join(format!("subagents-tab-{workspace}"))
}

/// Whether a tab is a workspace's subagents tab (the dock stays out of it).
pub fn is_subagents_tab(tab: &str) -> bool {
    let workspace = tab.split(':').next().unwrap_or_default();
    std::fs::read_to_string(tab_file(workspace)).is_ok_and(|t| t.trim() == tab)
}

/// Opens a subagent's live view in its workspace's subagents tab, the tab
/// made on its first, on its parent's Claude account. The view lists itself
/// as an agent, names itself, and says whose it is. Its pane, or None when
/// Herdr would not open one.
fn open_view(parent: &str, id: &str, hook: &Value) -> Option<String> {
    let ws = call("pane.get", json!({ "pane_id": parent })).ok()?["pane"]["workspace_id"].as_str()?.to_string();
    // Claude keeps a subagent's log and summary beside its session's transcript
    let dir = std::path::Path::new(hook["transcript_path"].as_str()?).with_extension("").join("subagents");
    let env = json!({
        "PACER_SUBAGENT_LOG": dir.join(format!("agent-{id}.jsonl")),
        "PACER_SUBAGENT_META": dir.join(format!("agent-{id}.meta.json")),
        "PACER_SUBAGENT_PARENT": parent,
    });
    let kept = std::fs::read_to_string(tab_file(&ws)).unwrap_or_default();
    let tab = Some(kept.trim()).filter(|t| !t.is_empty() && call("tab.get", json!({ "tab_id": t })).is_ok());
    let open = json!({ "plugin_id": "herdr-pacer", "entrypoint": "subagent", "focus": false, "env": env });
    let opened = match tab {
        Some(tab) => {
            let panes = call("pane.list", json!({ "workspace_id": ws })).ok()?;
            let beside = panes["panes"].as_array()?.iter().find(|p| p["tab_id"] == tab)?["pane_id"].clone();
            let at = json!({ "placement": "split", "target_pane_id": beside, "direction": "down" });
            call("plugin.pane.open", merge(open, at)).ok()?
        }
        None => {
            let opened = call("plugin.pane.open", merge(open, json!({ "placement": "tab", "workspace_id": ws }))).ok()?;
            let tab = opened["plugin_pane"]["pane"]["tab_id"].as_str().unwrap_or_default();
            let _ = call("tab.rename", json!({ "tab_id": tab, "label": TAB }));
            let _ = std::fs::write(tab_file(&ws), tab);
            let _ = crate::dock::keep_out(tab);
            opened
        }
    };
    let view = opened["plugin_pane"]["pane"]["pane_id"].as_str()?.to_string();
    crate::context::share_account(parent, &view);
    Some(view)
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

/// A closed pane's list goes with it; a minute's grace, for a pane Herdr has
/// not listed as an agent yet when its first subagent starts.
fn forget_closed(nodes: &[Node]) {
    for file in std::fs::read_dir(crate::usage::state_dir()).into_iter().flatten().flatten() {
        let name = file.file_name().to_string_lossy().into_owned();
        let pane = name.strip_prefix("subagents-").and_then(|n| n.strip_suffix(".json"));
        let settled = file.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e.as_secs() > 60));
        if settled && pane.is_some_and(|p| !nodes.iter().any(|n| n.pane == p)) {
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
        Node { pane: pane.into(), label: label.into(), parent: parent.map(String::from), repo, repo_key, account }
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
    fn a_subagent_is_listed_from_its_start_to_its_stop() {
        let listed = |subs: &Map<String, Value>| subs.iter().filter(|e| e.1.get("type").is_some()).map(|e| e.0.clone()).collect::<Vec<_>>();
        let mut subs = Map::new();
        note(&mut subs, "a1", Some("Explore"), 1000);
        note(&mut subs, "a2", Some("Plan"), 1001);
        note(&mut subs, "x", Some(""), 1002);
        assert_eq!(listed(&subs), ["a1", "a2"], "Claude's own untyped agents are not listed");
        note(&mut subs, "a1", None, 1003);
        note(&mut subs, "never-started", None, 1004);
        assert_eq!(listed(&subs), ["a2"]);
        note(&mut subs, "a3", Some("Explore"), 1001 + SUB_TTL);
        assert_eq!(subs.keys().collect::<Vec<_>>(), ["a3"], "one whose stop never came goes in time");
        // async hooks: a short one's stop can come before its start
        let mut subs = Map::new();
        note(&mut subs, "quick", None, 2000);
        note(&mut subs, "quick", Some("Explore"), 2001);
        assert!(subs.is_empty(), "the late start is turned away");
        note(&mut subs, "early", None, 2000);
        note(&mut subs, "later", Some("Plan"), 2000 + STOP_TTL);
        assert_eq!(subs.keys().collect::<Vec<_>>(), ["later"], "an unmatched stop is not kept for long");
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
