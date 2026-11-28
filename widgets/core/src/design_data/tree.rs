//! Paged, window-local views of the same widget paths returned by design pick.
use super::{arg, json_str, pick, window_arg, Args};
use crate::{makepad_draw::*, widget::WidgetUid, widget_tree::CxWidgetExt, window::Window};
use std::{collections::VecDeque, fmt::Write, ops::Range};

pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 500;
pub const DEFAULT_ROOT_DEPTH: usize = 3;
pub const MAX_DEPTH: usize = 4;

struct Tree {
    index: pick::IndexedTree,
    roots: Vec<usize>,
    scope: Range<usize>,
    included: Vec<bool>,
    total: usize,
    window: usize,
}

impl Tree {
    fn new(cx: &Cx, requested: Option<usize>) -> Result<Self, String> {
        let mut index = pick::indexed_flat_tree(cx);
        let mut included = vec![false; index.rows.len()];
        let mut windows = Vec::new();
        for (i, row) in index.rows.iter().enumerate() {
            included[i] = !row.inspector && index.parent[i].is_none_or(|p| included[p]);
            if included[i] && row.ty == "Window" {
                let widget = cx.widget_tree().widget(WidgetUid(row.uid));
                let id = widget.borrow::<Window>().map(|window| window.window_index());
                if let Some(id) = id {
                    windows.push((i, id));
                }
            }
        }

        let mut scope = 0..index.rows.len();
        let window = if let Some(w) = requested {
            let &(i, _) = windows.iter().find(|(_, id)| *id == w).ok_or_else(|| format!("no window {w}"))?;
            let depth = index.rows[i].depth;
            scope = i..(i + 1..scope.end).find(|&j| index.rows[j].depth <= depth).unwrap_or(scope.end);
            index.parent[i] = None;
            w
        } else {
            match windows.as_slice() { [(_, w)] => *w, _ => 0 }
        };
        let mut roots = Vec::new();
        let mut total = 0;
        for i in scope.clone().filter(|&i| included[i]) {
            total += 1;
            index.children[i].retain(|&child| included[child]);
            if index.parent[i].is_none() { roots.push(i); }
        }
        Ok(Self { index, roots, scope, included, total, window })
    }

    fn resolve(&self, cx: &Cx, path: &str, window: Option<usize>) -> Result<usize, String> {
        if path.is_empty() || path.chars().any(char::is_control) { return Err("invalid widget path".into()); }
        let wanted = format!("/{}", path.trim().trim_start_matches('/'));
        let scoped = self.scope.clone().filter(|&i| self.included[i]);
        let paths = &self.index.paths;
        if let Some(i) = scoped.clone().find(|&i| paths[i] == wanted)
            .or_else(|| scoped.clone().find(|&i| paths[i].ends_with(&wanted))) {
            return Ok(i);
        }
        let uid = pick::resolve_target(cx, path, window)?.widget_uid().0;
        scoped.into_iter().find(|&i| self.index.rows[i].uid == uid)
            .ok_or_else(|| format!("no widget at path {path:?} in this tree"))
    }

    fn children(&self, parent: Option<usize>) -> &[usize] {
        parent.map_or(&self.roots, |p| &self.index.children[p])
    }

    fn block(&self, cx: &Cx, parent: Option<usize>, offset: usize, total: usize, nodes: &[usize], matched: &[bool]) -> String {
        let path = parent.map_or("", |p| &self.index.paths[p]);
        let mut block = format!(r#"{{"path":{},"offset":{offset},"total":{total},"nodes":["#, json_str(path));
        for (n, &i) in nodes.iter().enumerate() {
            if n != 0 { block.push(','); }
            let row = &self.index.rows[i];
            let widget = cx.widget_tree().widget(WidgetUid(row.uid));
            let visible = widget.visible();
            write!(block, r#"{{"path":{},"name":{},"ty":{},"children":{},"visible":{visible}"#,
                json_str(&self.index.paths[i]), json_str(&row.name), json_str(&row.ty), self.index.children[i].len()).unwrap();
            if matched.get(i) == Some(&true) { block.push_str(r#","matched":true"#); }
            if visible && row.ty != "Window" {
                let rect = widget.area().clipped_rect_union(cx);
                let [x, y, w, h] = [rect.pos.x, rect.pos.y, rect.size.x, rect.size.y];
                if w > 0. && h > 0. && [x, y, w, h, x + w, y + h].iter().all(|n| n.is_finite()) {
                    write!(block, r#","rect":[{x},{y},{w},{h}]"#).unwrap();
                }
            }
            block.push('}');
        }
        block.push_str("]}");
        block
    }

    fn pages(&self, cx: &Cx, parent: Option<usize>, depth: usize, offset: usize, mut budget: usize) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut pending = VecDeque::from([(parent, depth, offset)]);
        while let Some((parent, depth, offset)) = pending.pop_front() {
            let children = self.children(parent);
            let start = offset.min(children.len());
            let page = &children[start..start + budget.min(children.len() - start)];
            blocks.push(self.block(cx, parent, start, children.len(), page, &[]));
            budget -= page.len();
            if budget == 0 { break; }
            if depth > 1 {
                pending.extend(page.iter().filter(|&&i| !self.index.children[i].is_empty())
                    .map(|&i| (Some(i), depth - 1, 0)));
            }
        }
        blocks
    }

    fn reveal(&self, cx: &Cx, target: usize, limit: usize) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut child = Some(target);
        while let Some(i) = child {
            let parent = self.index.parent[i];
            let siblings = self.children(parent);
            let pos = siblings.iter().position(|&s| s == i).unwrap();
            let offset = if pos < limit { 0 } else { pos - limit / 2 };
            let page = &siblings[offset..(offset + limit).min(siblings.len())];
            blocks.push(self.block(cx, parent, offset, siblings.len(), page, &[]));
            child = parent;
        }
        blocks.reverse();
        blocks
    }

    fn query(&self, cx: &Cx, query: &str, offset: usize, limit: usize) -> (Vec<String>, usize, bool) {
        let matches = |s: &str| s.as_bytes().windows(query.len()).any(|s| s.eq_ignore_ascii_case(query.as_bytes()));
        let mut total = 0;
        let mut matched = vec![false; self.index.rows.len()];
        let mut keep = matched.clone();
        for i in self.scope.clone().filter(|&i| self.included[i]) {
            let row = &self.index.rows[i];
            let segment = self.index.paths[i].rsplit('/').next().unwrap();
            if matches(&row.name) || matches(&row.ty) || matches(segment) {
                if (offset..offset.saturating_add(limit)).contains(&total) {
                    matched[i] = true;
                    let mut cursor = Some(i);
                    while let Some(j) = cursor {
                        if keep[j] { break; }
                        keep[j] = true;
                        cursor = self.index.parent[j];
                    }
                }
                total += 1;
            }
        }
        let mut blocks = Vec::new();
        for parent in std::iter::once(None).chain(self.scope.clone().filter(|&i| keep[i]).map(Some)) {
            let nodes: Vec<_> = self.children(parent).iter().copied().filter(|&i| keep[i]).collect();
            if parent.is_none() || !nodes.is_empty() {
                blocks.push(self.block(cx, parent, 0, nodes.len(), &nodes, &matched));
            }
        }
        (blocks, total, offset.saturating_add(limit) < total)
    }
}

fn number(args: &Args, key: &str, default: usize) -> Result<usize, String> {
    arg(args, key).map(str::parse).transpose().map(|n| n.unwrap_or(default)).map_err(|_| format!("invalid {key}"))
}

fn bounded(args: &Args, key: &str, default: usize, max: usize) -> Result<usize, String> {
    let value = number(args, key, default)?;
    if value == 0 { return Err(format!("{key} must be at least 1")); }
    if value > max { crate::log!("design tree: clamping {key} from {value} to {max}"); }
    Ok(value.min(max))
}

pub(super) fn remote(cx: &mut Cx, args: &Args) -> Result<String, String> {
    let window = window_arg(args)?;
    let path = arg(args, "path");
    let depth = bounded(args, "depth", if path.is_some() { 1 } else { DEFAULT_ROOT_DEPTH }, MAX_DEPTH)?;
    let limit = bounded(args, "limit", DEFAULT_LIMIT, MAX_LIMIT)?;
    let offset = number(args, "offset", 0)?;
    let tree = Tree::new(cx, window)?;
    let (mode, blocks, total_matches, truncated) = match (path, arg(args, "reveal"), arg(args, "query")) {
        (path, None, None) => {
            let parent = path.map(|p| tree.resolve(cx, p, window)).transpose()?;
            ("children", tree.pages(cx, parent, depth, offset, limit), 0, false)
        }
        (None, Some(path), None) => ("reveal", tree.reveal(cx, tree.resolve(cx, path, window)?, limit), 0, false),
        (None, None, Some(query)) if !query.trim().is_empty() && !query.chars().any(char::is_control) => {
            let (blocks, total, truncated) = tree.query(cx, query, offset, limit);
            ("query", blocks, total, truncated)
        }
        (None, None, Some(_)) => return Err("invalid widget tree query".into()),
        _ => return Err("choose one of path, reveal or query".into()),
    };
    Ok(format!(r#"{{"window":{},"mode":"{mode}","total_nodes":{},"total_matches":{total_matches},"truncated":{truncated},"parents":[{}]}}"#,
        tree.window, tree.total, blocks.join(",")))
}
