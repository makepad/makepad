//! Remote consumers run on their own threads. Native and hosted apps use the same grammar,
//! resolution, snapshots and answers; only transport and waiting differ.
use super::*;
use makepad_a11y::{query::Query, render, ActionArg, Actions, Node, NodeId, States};
use makepad_remote::{json_str, Out, Req};

pub type Trees = Vec<(u64, Tree)>;
pub type Snapshots = Mutex<HashMap<String, Trees>>;

pub trait Backend {
    fn activate(&self);
    fn trees(&self) -> Trees;
    fn wait_change(&self, versions: &[(u64, u64)], timeout: Duration);
    fn action(&self, window: u64, request: ActionRequest, timeout: Duration) -> Result<ActionResult, String>;
    fn present(&self, window: u64, timeout: Duration) -> Result<(), String>;
    fn type_text(&self, window: u64, text: &str) -> Result<(), String>;
    fn user_seq(&self, window: u64) -> Result<u64, String>;
}

pub fn versions(trees: &Trees) -> Vec<(u64, u64)> {
    trees.iter().map(|(k, t)| (*k, t.updates)).collect()
}

fn selected(req: &Req, trees: Trees) -> Result<Trees, String> {
    let pick = req.get("w").map(|w| -> Result<(Option<u32>, u32), String> {
        if let Some((s, w)) = w.split_once(':') {
            Ok((Some(s.parse::<u32>().map_err(|_| "w wants SESSION:INDEX")?), w.parse::<u32>().map_err(|_| "w wants SESSION:INDEX")?))
        } else {
            Ok((None, w.parse::<u32>().map_err(|_| "w wants INDEX or SESSION:INDEX")?))
        }
    }).transpose()?;
    let mut trees: Trees = trees.into_iter().filter(|(k, _)| pick.is_none_or(|(s, w)| *k as u32 == w && s.is_none_or(|s| *k >> 32 == s as u64))).collect();
    trees.sort_by_key(|(key, _)| *key);
    Ok(trees)
}

fn query(req: &Req, verb: &str) -> Result<Query, String> {
    let mut terms = req.get("q").unwrap_or("").to_string();
    if let Some(id) = req.get("node").filter(|_| !verb.is_empty()) {
        NodeId::parse_short(id).ok_or("invalid node id")?;
        terms.push_str(&format!(" id={id}"));
    }
    // Accept both q='role=button name="Save as"' and ui find role=button name='Save as'.
    for (k, v) in &req.kv {
        if matches!(k.as_str(), "role" | "class" | "path" | "state" | "states" | "text" | "text~" | "value" | "value~" | "name~" | "nth" | "id" | "actions") || k == "action" && verb != "act" || k == "name" && !matches!(verb, "act" | "snap") {
            terms.push_str(&format!(" {k}={}", makepad_a11y::query::quote(v)));
        }
    }
    if verb == "find" {
        for word in req.pos.iter().skip(1) {
            terms.push(' ');
            terms.push_str(&makepad_a11y::query::quote(word));
        }
    }
    Query::parse(&terms)
}

pub fn find(trees: &Trees, q: &Query) -> Vec<(u64, Node)> {
    let mut all = q.clone();
    all.nth = None;
    let rows: Vec<_> = trees.iter().flat_map(|(k, t)| all.find(t).into_iter().map(|n| (*k, n.clone()))).collect();
    match q.nth {
        Some(n) => rows.into_iter().nth(n).into_iter().collect(),
        None => rows,
    }
}

pub fn resolve(trees: &Trees, q: &Query) -> Result<(u64, Node), String> {
    let rows = find(trees, q);
    if rows.is_empty() { return Err("no such node (ui find lists matches)".into()); }
    if rows.len() != 1 { return Err(format!("ambiguous: {} nodes match; use nth=, node= or w=", rows.len())); }
    let (key, node) = rows.into_iter().next().unwrap();
    let tree = &trees.iter().find(|(k, _)| *k == key).unwrap().1;
    for id in tree.ancestors(node.id).into_iter().chain(std::iter::once(node.id)) {
        if let Some(n) = tree.node(id) {
            if n.states.contains(States::HIDDEN) { return Err(format!("node #{} is hidden", node.id.short())); }
            if n.states.contains(States::DISABLED) { return Err(format!("node #{} is disabled", node.id.short())); }
        }
    }
    Ok((key, node))
}

/// Readable JSON fields plus the canonical wire payload for lossless typed consumers. IDs are
/// strings (JavaScript numbers cannot represent all NodeIds); no second node decoder is needed.
pub fn tree_json(key: u64, tree: &Tree, nodes: Vec<Node>) -> String {
    let rows: Vec<_> = nodes.iter().map(|n| {
        let b = n.bounds;
        format!("{{\"id\":{},\"role\":{},\"name\":{},\"value\":{},\"text\":{},\"states\":{},\"actions\":{},\"toggle\":{},\"path\":{},\"class\":{},\"bounds\":[{},{},{},{}]}}",
            json_str(&n.id.short()), json_str(n.role.short()), json_str(&n.name), json_str(&n.value), json_str(n.text.as_ref().map_or("", |t| &t.content)), json_str(&n.states.list()), json_str(&n.actions.list()), json_str(n.toggle.short()), json_str(&n.path), json_str(&n.class), b.x, b.y, b.w, b.h)
    }).collect();
    let update = TreeUpdate { full: true, root: tree.root, focus: tree.focus, nodes, ..Default::default() };
    let mut bytes = Vec::new();
    makepad_a11y::wire::encode_update(&update, &mut bytes);
    let wire: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{{\"window\":\"{}:{}\",\"root\":{},\"focus\":{},\"updates\":{},\"since\":{},\"nodes\":[{}],\"wire\":\"{}\"}}", key >> 32, key as u32, json_str(&tree.root.short()), json_str(&tree.focus.short()), tree.updates, tree.notify_seq, rows.join(","), wire)
}

fn rows_json(trees: &Trees, rows: &[(u64, Node)]) -> String {
    let entries: Vec<_> = trees.iter().map(|(k,t)| tree_json(*k, t, rows.iter().filter(|(w,_)| w == k).map(|(_, n)| n.clone()).collect())).collect();
    format!("\"trees\":[{}],\"count\":{}", entries.join(","), rows.len())
}
fn text_result(out: &mut Out, json: bool, field: &str, text: &str) {
    if json { out.line(&format!("{{{}:{}}}", json_str(field), json_str(text))); }
    else { for line in text.lines() { out.line(line); } }
}
fn number<T: std::str::FromStr>(req: &Req, key: &str, default: T) -> Result<T, String> {
    req.get(key).map(|v| v.parse().map_err(|_| format!("{key} wants a number"))).unwrap_or(Ok(default))
}
fn argument(req: &Req, action: Actions, node: &Node) -> Result<ActionArg, String> {
    let selection = node.text.as_ref().map_or((0, 0), |t| t.selection);
    Ok(match action {
        Actions::REPLACE_TEXT => ActionArg::Replace(number(req, "from", selection.0)?, number(req, "to", selection.1)?, req.get("t").ok_or("replace_text needs t=TEXT")?.into()),
        Actions::SET_TEXT_SELECTION => ActionArg::Range(number(req, "from", selection.0)?, number(req, "to", selection.1)?),
        Actions::SCROLL => ActionArg::Scroll(number(req, "dx", 0.0)?, number(req, "dy", 0.0)?),
        Actions::CUSTOM => ActionArg::Custom(req.get("name").ok_or("custom needs name=ACTION")?.into()),
        _ => match req.get("v") {
            Some(v) => v.parse::<f64>().ok().filter(|v| v.is_finite()).map(ActionArg::Number).unwrap_or_else(|| ActionArg::Text(v.into())),
            None => req.get("t").map(|v| ActionArg::Text(v.into())).unwrap_or_default(),
        }
    })
}

/// Shared command interpreter. All waits and action acknowledgements happen on its caller's
/// consumer thread, never in a widget draw or event handler.
pub fn dispatch(req: &Req, out: &mut Out, backend: &dyn Backend, snapshots: &Snapshots) {
    backend.activate();
    out.limit = usize::MAX;
    if let Err(e) = run(req, out, backend, snapshots) { out.err(&e); }
}
fn run(req: &Req, out: &mut Out, backend: &dyn Backend, snapshots: &Snapshots) -> Result<(), String> {
    let verb = req.pos.first().map_or("", String::as_str);
    let json = req.get("json") == Some("1");
    let timeout = Duration::from_millis(number(req, "timeout_ms", 5000u64)?);
    let q = query(req, verb)?;
    let start = Instant::now();
    let mut trees = selected(req, backend.trees())?;
    while trees.iter().all(|(_, t)| t.nodes.is_empty()) && start.elapsed() < timeout && !out.cancelled() {
        backend.activate();
        backend.wait_change(&versions(&backend.trees()), timeout.saturating_sub(start.elapsed()).min(Duration::from_millis(100)));
        trees = selected(req, backend.trees())?;
    }
    match verb {
        "" | "find" => {
            let rows = find(&trees, &q);
            if json {
                let rows = if verb.is_empty() {
                    let start = req.get("node").map(|id| NodeId::parse_short(id).ok_or("invalid node id")).transpose()?;
                    let depth = req.get("depth").map(|_| number::<usize>(req, "depth", 0)).transpose()?;
                    rows.into_iter().filter(|(key, n)| {
                        let tree = &trees.iter().find(|(k, _)| k == key).unwrap().1;
                        let root = start.unwrap_or(tree.root);
                        if n.id == root { return true; }
                        let ancestors = tree.ancestors(n.id);
                        ancestors.iter().position(|id| *id == root).is_some_and(|d| depth.is_none_or(|max| ancestors.len() - d <= max))
                    }).collect()
                } else { rows };
                out.line(&format!("{{{}}}", rows_json(&trees, &rows)));
            }
            else if verb == "find" {
                for (key,n) in &rows {
                    let prefix = if trees.len() > 1 { format!("w={}:{} ", key >> 32, *key as u32) } else { String::new() };
                    out.line(&format!("{prefix}{}", render::node_line(n)));
                }
                out.line(&format!("-- {} matches", rows.len()));
            } else {
                let opt = render::TreeOptions { query: (!q.is_empty()).then_some(q), node: req.get("node").and_then(NodeId::parse_short), depth: req.get("depth").map(|_| number(req, "depth", 0)).transpose()?, max_lines: None };
                if trees.is_empty() { out.line("empty tree (nothing drawn yet)"); }
                for (key,tree) in &trees {
                    if trees.len() > 1 { out.line(&format!("window {}:{}", key >> 32, *key as u32)); }
                    for l in render::tree_text(tree, &opt).lines() { out.line(l); }
                }
            }
        }
        "act" | "type" => {
            if q.is_empty() { return Err("action needs node=ID or q=QUERY".into()); }
            let (key,node) = resolve(&trees, &q)?;
            let seq = backend.user_seq(key)?;
            if req.get("if_user_seq").is_some() && number(req, "if_user_seq", seq)? != seq { return Err("user_intervened: read activity and retry after inspecting the result".into()); }
            let invoke = |action, arg| -> Result<ActionResult, String> {
                if backend.user_seq(key)? != seq { return Err("user_intervened: sequence invalidated".into()); }
                let result = backend.action(key, ActionRequest { req: 0, node: node.id, action, arg }, timeout)?;
                if !matches!(result, ActionResult::Handled | ActionResult::Fallback) { return Err(format!("#{}: {}", node.id.short(), result.short())); }
                Ok(result)
            };
            let result = if verb == "type" {
                let text = req.get("t").ok_or("ui type needs t=TEXT")?;
                invoke(Actions::FOCUS, ActionArg::None)?;
                // Focus may move the caret; read its next semantic update before replacing.
                backend.present(key, timeout)?;
                let fresh = selected(req, backend.trees())?;
                let (_, current) = resolve(&fresh, &Query::parse(&format!("id={}", node.id.short()))?)?;
                if current.actions.contains(Actions::REPLACE_TEXT) {
                    invoke(Actions::REPLACE_TEXT, argument(req, Actions::REPLACE_TEXT, &current)?)?
                } else {
                    if backend.user_seq(key)? != seq { return Err("user_intervened: sequence invalidated".into()); }
                    backend.type_text(key, text)?;
                    ActionResult::Fallback
                }
            } else {
                let name = req.pos.get(1).map(String::as_str).or(req.get("action")).ok_or("ui act needs an action (press, focus, toggle, set_value, ...)")?;
                let action = Actions::from_short(name).ok_or_else(|| format!("unknown action `{name}`"))?;
                invoke(action, argument(req, action, &node)?)?
            };
            if req.get("wait") == Some("1") || verb == "type" { backend.present(key, timeout)?; }
            if backend.user_seq(key)? != seq { return Err("user_intervened: inspect before retrying an applied action".into()); }
            let after = selected(req, backend.trees())?;
            let rows = find(&after, &Query::parse(&format!("id={}", node.id.short()))?).into_iter().filter(|(k,_)| *k == key).collect::<Vec<_>>();
            if json { out.line(&format!("{{\"result\":{},{}}}", json_str(result.short()), rows_json(&after, &rows))); }
            else { out.line(&format!("{}: {}", result.short(), rows.first().map(|(_,n)| render::node_line(n)).unwrap_or_else(|| format!("#{} removed", node.id.short())))); }
        }
        "wait" => {
            let initial = find(&trees, &q);
            let expected = req.get("count").map(|_| number::<usize>(req, "count", 0)).transpose()?;
            loop {
                let rows = find(&trees, &q);
                let ok = if req.get("gone") == Some("1") { rows.is_empty() }
                    else if req.get("changed") == Some("1") { rows != initial }
                    else if let Some(n) = expected { rows.len() == n } else { !rows.is_empty() };
                if ok {
                    if json { out.line(&format!("{{\"elapsed_ms\":{},{}}}", start.elapsed().as_millis(), rows_json(&trees, &rows))); }
                    else {
                        for (_, n) in &rows { out.line(&render::node_line(n)); }
                        out.line(&format!("-- {} matches, {} ms", rows.len(), start.elapsed().as_millis()));
                    }
                    break;
                }
                if out.cancelled() { break; }
                let left = timeout.saturating_sub(start.elapsed());
                if left.is_zero() { return Err(format!("ui wait timed out after {} ms ({} matches)", start.elapsed().as_millis(), rows.len())); }
                // An outstanding wait is still a live remote consumer.
                backend.activate();
                // Use all versions, including windows excluded by w=, to avoid a missed wake.
                let all = backend.trees();
                let current = selected(req, all.clone())?;
                if versions(&trees) != versions(&current) { trees = current; continue; }
                backend.wait_change(&versions(&all), left.min(Duration::from_millis(100)));
                trees = selected(req, backend.trees())?;
            }
        }
        "snap" => {
            let name = req.get("name").ok_or("ui snap needs name=NAME")?;
            let count: usize = trees.iter().map(|(_,t)| t.nodes.len()).sum();
            lock(snapshots).insert(name.to_string(), trees);
            if json { out.line(&format!("{{\"snapshot\":{},\"count\":{count}}}", json_str(name))); }
            else { out.line(&format!("snapshot {name}: {count} nodes")); }
        }
        "diff" => {
            let a = req.get("a").ok_or("ui diff needs a=SNAPSHOT")?;
            let b = req.get("b").unwrap_or("now");
            let (a, b) = {
                let saved = lock(snapshots);
                (saved.get(a).cloned().ok_or_else(|| format!("no snapshot `{a}`"))?, if b == "now" { trees } else { saved.get(b).cloned().ok_or_else(|| format!("no snapshot `{b}`"))? })
            };
            let mut keys: Vec<_> = a.iter().chain(&b).map(|(k,_)| *k).collect(); keys.sort(); keys.dedup();
            let empty = Tree::default();
            let mut text = String::new();
            for k in keys {
                text.push_str(&format!("window {}:{}\n", k >> 32, k as u32));
                text.push_str(&render::diff_text(a.iter().find(|(key,_)| *key == k).map_or(&empty, |(_,t)| t), b.iter().find(|(key,_)| *key == k).map_or(&empty, |(_,t)| t)));
            }
            text_result(out, json, "diff", &text);
        }
        "speak" | "lint" => {
            let mut text = String::new();
            for (_, tree) in &trees {
                if verb == "lint" { text.push_str(&render::lint_text(tree)); }
                else if req.get("node").is_some() {
                    for n in q.find(tree) { text.push_str(&render::speak_node(tree, n)); text.push('\n'); }
                } else { text.push_str(&render::speak_since(tree, number(req, "since", 0)?)); }
            }
            text_result(out, json, verb, &text);
        }
        "oscheck" => {
            let key = trees.first().ok_or("no window with a tree")?.0;
            let check = os_selfcheck().get().ok_or("this backend has no OS accessibility adapter")?;
            text_result(out, json, "oscheck", &check(key, req.get("act") == Some("1")));
        }
        _ => return Err("ui: find, act, type, wait, snap, diff, speak, lint, oscheck (help lists arguments)".into()),
    }
    Ok(())
}
