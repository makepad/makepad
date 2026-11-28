use crate::{makepad_draw::*, view::View, widget::*, widget_hooks::WidgetHooks, widget_tree::{CxWidgetExt, live_id_token, widget_type_names}};
use std::collections::{HashMap, HashSet};

/// One resolved widget under the pointer (or pinned by a click).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WidgetPick {
    pub uid: u64,
    /// Dotted id path from the ui root, the same tokens `/d` prints.
    pub path: String,
    pub ty: String,
    /// Window-local rect.
    pub rect: Rect,
    pub window_id: usize,
    /// `Some("padding")` / `Some("margin:<child>")` when the pointer sits in
    /// the spacing band rather than on content — the gap between rects is a
    /// first-class target.
    pub band: Option<String>,
    /// The plane the widget renders on in the exploded view (its component
    /// nesting depth); 0 when it has not drawn while the mode was up.
    pub level: usize,
}

pub fn pick_candidate(
    cx: &Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    attached: &HashSet<DrawListId>,
) -> Option<Rect> {
    if !widget.visible() {
        return None;
    }
    // Only what is on screen: a hidden Dock page keeps its retained draw
    // list and every rect in it, and clicking "through" to those was the
    // stale-tab bug.
    if !widget.area().is_attached(cx, attached) {
        return None;
    }
    // All instances, not the first: a text run's area is its glyph run,
    // and the first glyph alone is not a paragraph.
    let rect = widget.area().clipped_rect_union(cx);
    if rect.size.x <= 0.0 || rect.size.y <= 0.0 || !rect.contains(abs) {
        return None;
    }
    Some(rect)
}

pub fn is_design_transparent(widget: &WidgetRef) -> bool {
    widget
        .borrow::<View>()
        .map_or(false, |view| view.design_mode())
}

fn walk_pick(
    cx: &Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    depth: usize,
    max_level: Option<usize>,
    attached: &HashSet<DrawListId>,
    best: &mut Option<(WidgetRef, Rect, usize)>,
) {
    if !widget.visible() {
        return;
    }
    // The inspector's own panel is never a pick, wherever an app has put it.
    if WidgetHooks::is_inspector(cx, widget.widget_type_id()) {
        return;
    }
    // Exploded view: the cursor is on ONE plane. A widget nested deeper than
    // that plane sits on another sheet, however its 2D rect overlaps the
    // un-projected point — that is what makes a covered parent selectable.
    let on_deeper_plane = max_level.is_some_and(|max| {
        cx.sploded_depth_of(widget.widget_uid().0)
            .is_some_and(|level| level > max)
    });
    if !on_deeper_plane && !is_design_transparent(widget) {
        if let Some(rect) = pick_candidate(cx, widget, abs, attached) {
            let take = match best {
                // Deeper wins; equal depth: later in draw order wins.
                Some((_, _, best_depth)) => depth >= *best_depth,
                None => true,
            };
            if take {
                *best = Some((widget.clone(), rect, depth));
            }
        }
    }
    widget.children(&mut |_id, child| {
        walk_pick(cx, &child, abs, depth + 1, max_level, attached, best);
    });
}

/// The spacing bands: when the picked widget is a container and the point
/// missed all of its children, say whether the point sits in the container's
/// padding ring or in a child's margin ring.
pub fn resolve_band(cx: &mut Cx, widget: &WidgetRef, rect: Rect, abs: Vec2d) -> Option<String> {
    let mut child_hit = false;
    let mut margin_of = None;
    widget.children(&mut |id, child| {
        if !child.visible() {
            return;
        }
        let child_rect = child.area().clipped_rect_union(cx);
        if child_rect.size.x <= 0.0 || child_rect.size.y <= 0.0 {
            return;
        }
        if child_rect.contains(abs) {
            child_hit = true;
            return;
        }
        let margin = child.walk(cx).margin;
        let expanded = Rect {
            pos: dvec2(
                child_rect.pos.x - margin.left,
                child_rect.pos.y - margin.top,
            ),
            size: dvec2(
                child_rect.size.x + margin.left + margin.right,
                child_rect.size.y + margin.top + margin.bottom,
            ),
        };
        if margin_of.is_none() && expanded.contains(abs) {
            margin_of = Some(format!("margin:{}", live_id_token(id)));
        }
    });
    if child_hit {
        return None;
    }
    if let Some(margin) = margin_of {
        return Some(margin);
    }
    // The gap between two adjacent siblings (flow spacing) is a first-class
    // target too: name both neighbours.
    let mut above: Option<(f64, LiveId)> = None;
    let mut below: Option<(f64, LiveId)> = None;
    let mut left: Option<(f64, LiveId)> = None;
    let mut right: Option<(f64, LiveId)> = None;
    widget.children(&mut |id, child| {
        if !child.visible() {
            return;
        }
        let r = child.area().clipped_rect_union(cx);
        if r.size.x <= 0.0 || r.size.y <= 0.0 {
            return;
        }
        let x_overlaps = abs.x >= r.pos.x && abs.x <= r.pos.x + r.size.x;
        let y_overlaps = abs.y >= r.pos.y && abs.y <= r.pos.y + r.size.y;
        if x_overlaps {
            let bottom = r.pos.y + r.size.y;
            if bottom <= abs.y && above.as_ref().map_or(true, |(edge, _)| bottom > *edge) {
                above = Some((bottom, id));
            }
            if r.pos.y >= abs.y && below.as_ref().map_or(true, |(edge, _)| r.pos.y < *edge) {
                below = Some((r.pos.y, id));
            }
        }
        if y_overlaps {
            let edge = r.pos.x + r.size.x;
            if edge <= abs.x && left.as_ref().map_or(true, |(e, _)| edge > *e) {
                left = Some((edge, id));
            }
            if r.pos.x >= abs.x && right.as_ref().map_or(true, |(e, _)| r.pos.x < *e) {
                right = Some((r.pos.x, id));
            }
        }
    });
    if let (Some((_, a)), Some((_, b))) = (&above, &below) {
        return Some(format!("gap:{}~{}", live_id_token(*a), live_id_token(*b)));
    }
    if let (Some((_, a)), Some((_, b))) = (&left, &right) {
        return Some(format!("gap:{}~{}", live_id_token(*a), live_id_token(*b)));
    }
    if let Some(view) = widget.borrow::<View>() {
        let padding = view.layout.padding;
        let content = Rect {
            pos: dvec2(rect.pos.x + padding.left, rect.pos.y + padding.top),
            size: dvec2(
                rect.size.x - padding.left - padding.right,
                rect.size.y - padding.top - padding.bottom,
            ),
        };
        if (padding.left != 0.0
            || padding.right != 0.0
            || padding.top != 0.0
            || padding.bottom != 0.0)
            && !content.contains(abs)
        {
            return Some("padding".to_string());
        }
    }
    None
}

pub fn resolve_pick(
    cx: &mut Cx,
    root: &WidgetRef,
    abs: Vec2d,
    window_id: usize,
    attached: &HashSet<DrawListId>,
) -> Option<WidgetPick> {
    let mut best = None;
    // Exploded: the router already re-addressed `abs` onto the plane the ray
    // hit; a miss (off the deck) picks nothing.
    let max_level = if cx.sploded_active() {
        Some(cx.sploded_hit_level()?)
    } else {
        None
    };
    // Only descendants are pick targets. The remote supplies the Window as the root.
    root.children(&mut |_id, child| {
        walk_pick(cx, &child, abs, 0, max_level, attached, &mut best);
    });
    let (widget, rect, _depth) = best?;
    let uid = widget.widget_uid();
    let level = cx.sploded_depth_of(uid.0).unwrap_or(0);
    let path_ids = cx.widget_tree().path_to(uid);
    let path = if path_ids.is_empty() {
        format!("uid:{}", uid.0)
    } else {
        path_ids
            .iter()
            .map(|id| live_id_token(*id))
            .collect::<Vec<_>>()
            .join(".")
    };
    let ty = widget
        .widget_type_id()
        .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
        .map(live_id_token)
        .unwrap_or_else(|| "-".to_string());
    let band = resolve_band(cx, &widget, rect, abs);
    Some(WidgetPick {
        uid: uid.0,
        path,
        ty,
        rect,
        window_id,
        band,
        level,
    })
}

/// A WidgetPick for a KNOWN widget (the climb's steps), same fields as a
/// resolved one.
pub fn pick_of_widget(
    cx: &mut Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    window_id: usize,
) -> Option<WidgetPick> {
    let uid = widget.try_widget_uid()?;
    let rect = widget.area().clipped_rect_union(cx);
    if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
        return None;
    }
    let level = cx.sploded_depth_of(uid.0).unwrap_or(0);
    let path_ids = cx.widget_tree().path_to(uid);
    let path = if path_ids.is_empty() {
        format!("uid:{}", uid.0)
    } else {
        path_ids
            .iter()
            .map(|id| live_id_token(*id))
            .collect::<Vec<_>>()
            .join(".")
    };
    let ty = widget
        .widget_type_id()
        .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
        .map(live_id_token)
        .unwrap_or_else(|| "-".to_string());
    let band = resolve_band(cx, widget, rect, abs);
    Some(WidgetPick { uid: uid.0, path, ty, rect, window_id, band, level })
}

/// Every widget's path, in names a person can read.
///
/// `WidgetTree::path_to` renders an unnamed node as `-` and a list item as a
/// bare index, so a real path came out as `-0.main_window.body.dock.-.-.3` —
/// the same string for every unnamed sibling, and no help to anyone reading
/// it. Here each segment is, in order of preference:
///
/// * the node's own name (`dock`, `tOverview`, `press_demo`), or
/// * its TYPE when it has no name of its own (`View`, `Label`), and
/// * `.1`, `.2`… appended when siblings would otherwise collide — four
///   unnamed Labels under one View become `Label.1`..`Label.4`. The slash is
///   the hierarchy; a dot is only which one of several.
///
/// The head is dropped, because it is on every path in the app and therefore
/// tells nobody anything: the tree root (which has neither a name nor a
/// reliably-registered type), any single-child chain under it, and the
/// `Window`'s own `body` container. What is left starts at the first thing
/// the app itself put on screen — `dock.tOverview.View.View.Label_1`.
///
/// Built from `flat_tree`, whose depth-first order and depth column give both
/// the parent chain and the sibling order. That is a whole-tree walk, so this
/// is for CLICKS (copy, @mention, a selection change) — never for hover.
pub fn readable_paths(cx: &Cx) -> Vec<(u64, String)> {
    let tree = indexed_flat_tree(cx);
    tree.rows.iter().map(|row| row.uid).zip(tree.paths).collect()
}

pub(super) struct IndexedTree {
    pub rows: Vec<crate::widget_tree::FlatTreeRow>,
    pub parent: Vec<Option<usize>>,
    pub children: Vec<Vec<usize>>,
    pub paths: Vec<String>,
}

pub(super) fn indexed_flat_tree(cx: &Cx) -> IndexedTree {
    let rows = cx.widget_tree().flat_tree(cx);
    let n = rows.len();
    // The parent of each row, read straight off the depth-first order.
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut stack: Vec<usize> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        stack.truncate(row.depth as usize);
        parent[i] = stack.last().copied();
        stack.push(i);
    }
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            children[*p].push(i);
        }
    }
    let mut segment: Vec<String> = rows
        .iter()
        .map(|row| readable_segment(&row.name, &row.ty))
        .collect();
    // Siblings that want the same segment get numbered, in child order.
    let roots: Vec<usize> = (0..n).filter(|i| parent[*i].is_none()).collect();
    let sibling_groups = children
        .iter()
        .chain(std::iter::once(&roots))
        .filter(|group| group.len() > 1);
    for group in sibling_groups {
        let mut counts = HashMap::<&str, usize>::new();
        for &i in group {
            *counts.entry(&segment[i]).or_default() += 1;
        }
        // Count down in reverse so the first sibling still gets ordinal 1.
        counts.retain(|_, count| *count > 1);
        let mut numbered = Vec::new();
        for &i in group.iter().rev() {
            if let Some(count) = counts.get_mut(segment[i].as_str()) {
                numbered.push((i, *count));
                *count -= 1;
            }
        }
        for (i, ordinal) in numbered {
            segment[i].push_str(&format!(".{ordinal}"));
        }
    }
    // How many leading segments are the shared, meaningless head: the root
    // itself, then any node that is the only way down, then the Window's
    // `body`. Never so many that a path runs out.
    let mut skip = 1usize; // the root
    if roots.len() == 1 {
        let mut cur = roots[0];
        while children[cur].len() == 1 {
            let next = children[cur][0];
            if children[next].is_empty() {
                break; // the chain ends here: this node IS the path
            }
            cur = next;
            skip += 1;
        }
        // The Window's content container. It is named by the framework, it
        // wraps everything an app draws, and it is on every path.
        for child in children[cur].iter() {
            if segment[*child] == "body" && !children[*child].is_empty() {
                skip += 1;
                break;
            }
        }
    }
    let mut full: Vec<String> = Vec::with_capacity(n);
    for i in 0..n {
        let depth = rows[i].depth as usize;
        let path = match parent[i] {
            Some(_) if depth <= skip => format!("/{}", segment[i]),
            Some(p) => format!("{}/{}", full[p], segment[i]),
            None => format!("/{}", segment[i]),
        };
        full.push(path);
    }
    IndexedTree { rows, parent, children, paths: full }
}

/// One path segment, as a person would say it: the node's name, else its
/// type, else a last-resort placeholder. A name that is only digits (a list
/// item's index) is no name at all, so those take the type too.
pub fn readable_segment(name: &str, ty: &str) -> String {
    let named =
        !name.is_empty() && name != "-" && !name.bytes().all(|b| b.is_ascii_digit());
    if named {
        return name.to_string();
    }
    if !ty.is_empty() && ty != "-" {
        return ty.to_string();
    }
    "Widget".to_string()
}

/// One widget's readable path (`/window/body/Button.2`). See
/// [`readable_paths`]. Part of the public reflection API: how an action
/// log names its sender.
pub fn indexed_path(cx: &Cx, uid: u64) -> String {
    readable_paths(cx)
        .into_iter()
        .find(|(u, _)| *u == uid)
        .map(|(_, path)| path)
        .unwrap_or_else(|| format!("uid:{uid}"))
}

/// The widget a path names. An exact match wins; otherwise anything whose
/// path ENDS with it, which is what makes a shortened, hand-written or
/// previously-stored tail still resolve.
fn resolve_indexed(cx: &Cx, path: &str, window: Option<usize>) -> Result<Option<WidgetRef>, String> {
    let path = path.trim();
    if path.is_empty() || path == "/" {
        return Ok(None);
    }
    // A reference written or pasted without its leading slash still means
    // the same thing. Dots are NOT separators here — `Label.2` is one
    // segment, the second Label — so nothing else is normalised.
    let wanted = format!("/{}", path.trim_start_matches('/'));
    let paths = readable_paths(cx);
    let widget = |(uid, _): &(u64, String)| cx.widget_tree().widget(WidgetUid(*uid));
    let exact = paths.iter().filter(|(_, full)| *full == wanted).map(widget);
    if let Some(found) = match_window(cx, path, window, exact)? {
        return Ok(Some(found));
    }
    match_window(cx, path, window, paths.iter().filter(|(_, full)| full.ends_with(&wanted)).map(widget))
}

fn widget_window(cx: &Cx, widget: &WidgetRef) -> Option<usize> {
    let area = widget.area();
    if area.is_valid(cx) {
        if let Some(window) = cx.get_window_id_of(&area) {
            return Some(window.id());
        }
    }
    // Undrawn widgets still belong to the Window that contains them.
    let tree = cx.widget_tree();
    let mut current = widget.clone();
    loop {
        if let Some(window) = current.borrow::<crate::window::Window>() {
            return Some(window.window_index());
        }
        current = tree.widget(tree.parent_of(current.try_widget_uid()?)?);
    }
}

/// Keep the resolver's order within the requested window. A path that already
/// matched in another window must never fall through to a less specific edit.
fn match_window(
    cx: &Cx,
    path: &str,
    window: Option<usize>,
    widgets: impl IntoIterator<Item = WidgetRef>,
) -> Result<Option<WidgetRef>, String> {
    let mut matches = widgets.into_iter().filter(|w| !w.is_empty());
    let Some(first) = matches.next() else { return Ok(None) };
    let Some(window) = window else { return Ok(Some(first)) };
    std::iter::once(first).chain(matches)
        .find(|w| widget_window(cx, w) == Some(window))
        .map(Some).ok_or_else(|| format!("no widget at path {path:?} in window {window}"))
}

/// Is this segment an anonymous node's numbered stand-in (`-`, `-2`)? Those
/// are positions, not names, so the loose finder must not turn them into ids.
pub fn is_anonymous_segment(segment: &str) -> bool {
    segment == "-"
        || (segment.starts_with('-') && segment.len() > 1 && segment[1..].bytes().all(|b| b.is_ascii_digit()))
}

/// The widget a readable path names: an indexed path (`/window/body/Button.2`)
/// taken as written, else a waypoint search over the named segments. Part
/// of the public reflection API: how a story names its subject.
pub fn resolve_widget_by_path(cx: &Cx, path: &str) -> Result<WidgetRef, String> {
    resolve_path(cx, path, None)
}

fn resolve_path(cx: &Cx, path: &str, window: Option<usize>) -> Result<WidgetRef, String> {
    let tree = cx.widget_tree();
    // An indexed path names one widget and nothing else: take it as written
    // before falling back to the waypoint search below, which drops the
    // anonymous segments and can only land on the nearest named ancestor.
    if let Some(found) = resolve_indexed(cx, path, window)? {
        return Ok(found);
    }
    let ids: Vec<LiveId> = path
        .split(['.', '/'])
        .filter(|segment| !segment.is_empty() && !is_anonymous_segment(segment))
        .map(LiveId::from_str)
        .collect();
    if ids.is_empty() {
        return Err("empty path".to_string());
    }
    let root = tree.root_uid();
    let find = |ids: &[LiveId]| {
        if window.is_some() {
            match_window(cx, path, window, tree.find_all_within(root, ids))
        } else {
            let found = tree.find_within(root, ids);
            Ok((!found.is_empty()).then_some(found))
        }
    };
    if let Some(found) = find(&ids)? {
        return Ok(found);
    }
    // The full dotted path from /tweak/state includes ancestors the finder
    // treats as waypoints; a stale head (window renamed) can still resolve
    // from the tail.
    if ids.len() > 1 {
        if let Some(found) = find(&ids[1..])? {
            return Ok(found);
        }
        let tail = [*ids.last().unwrap()];
        if let Some(found) = find(&tail)? {
            return Ok(found);
        }
    }
    Err(format!("no widget at path {path:?}"))
}

pub(super) fn resolve_target(cx: &Cx, path: &str, window: Option<usize>) -> Result<WidgetRef, String> {
    if let Some(uid) = path.strip_prefix("uid:") {
        let uid = WidgetUid(uid.parse().map_err(|_| "invalid widget uid")?);
        let widget = cx.widget_tree().widget(uid);
        return match_window(cx, path, window, [widget])?.ok_or_else(|| format!("no widget at {path}"));
    }
    resolve_path(cx, path, window)
}

pub(super) fn remote(cx: &mut Cx, args: &super::Args) -> Result<String, String> {
    let window_id = super::window_arg(args)?;
    let (pick, widget) = match (super::arg(args, "at"), super::arg(args, "path")) {
        (Some(at), None) => {
            let window = window_id.unwrap_or(0);
            let (x, y) = at.split_once(',').ok_or("need at=x,y")?;
            let point = dvec2(x.trim().parse().map_err(|_| "invalid x")?, y.trim().parse().map_err(|_| "invalid y")?);
            let mut attached = HashSet::new();
            for pass in cx.passes.id_iter() {
                if cx.get_pass_window_id(pass).is_some_and(|w| w.id() == window) {
                    attached.extend(cx.attached_draw_lists(pass));
                }
            }
            let tree = cx.widget_tree();
            let root = tree.flat_tree(cx).into_iter()
                .map(|row| tree.widget(WidgetUid(row.uid)))
                .find(|widget| widget.borrow::<crate::window::Window>().is_some_and(|w| w.window_index() == window))
                .unwrap_or_else(|| tree.widget(tree.root_uid()));
            let pick = resolve_pick(cx, &root, point, window, &attached).ok_or("no widget at point")?;
            let widget = cx.widget_tree().widget(WidgetUid(pick.uid));
            (pick, widget)
        }
        (None, Some(path)) => {
            let widget = resolve_target(cx, path, window_id)?;
            if widget.borrow::<crate::window::Window>().is_some() {
                return Err("the Window itself is not a pick target".into());
            }
            let window = widget_window(cx, &widget).unwrap_or(0);
            let point = widget.area().clipped_rect_union(cx).center();
            let pick = pick_of_widget(cx, &widget, point, window).ok_or("widget has no drawn rect")?;
            (pick, widget)
        }
        _ => return Err("need one of at=x,y or path=".into()),
    };
    use super::json_str;
    let margin = widget.walk(cx).margin;
    let padding = widget.borrow::<View>().map(|v| v.layout.padding).unwrap_or_default();
    Ok(format!(r#"{{"uid":{},"path":{},"ref":{},"type":{},"rect":[{},{},{},{}],"window":{},"margin":[{},{},{},{}],"padding":[{},{},{},{}],"band":{}}}"#,
        pick.uid, json_str(&pick.path), json_str(&indexed_path(cx, pick.uid)), json_str(&pick.ty),
        pick.rect.pos.x, pick.rect.pos.y, pick.rect.size.x, pick.rect.size.y, pick.window_id,
        margin.left, margin.top, margin.right, margin.bottom, padding.left, padding.top, padding.right, padding.bottom,
        pick.band.as_deref().map(json_str).unwrap_or_else(|| "null".into())))
}

pub(super) fn rect_json(cx: &Cx, args: &super::Args) -> Result<String, String> {
    let expected_window = super::window_arg(args)?;
    let uid: u64 = super::need(args, "uid")?.parse().map_err(|_| "invalid widget uid")?;
    if uid == 0 {
        return Err("invalid widget uid".into());
    }
    let widget = cx.widget_tree().widget(WidgetUid(uid));
    if widget.is_empty() {
        return Err(format!("no widget at uid:{uid}"));
    }
    let window = widget_window(cx, &widget);
    if let Some(expected) = expected_window {
        if window != Some(expected) {
            return Err(format!("no widget at uid:{uid} in window {expected}"));
        }
    }
    let rect = widget.area().clipped_rect_union(cx);
    if ![rect.pos.x, rect.pos.y, rect.size.x, rect.size.y].iter().all(|n| n.is_finite())
        || rect.size.x <= 0.0 || rect.size.y <= 0.0
    {
        return Err("widget has no drawn rect".into());
    }
    Ok(format!(r#"{{"window":{},"rect":[{},{},{},{}]}}"#,
        window.unwrap_or(0), rect.pos.x, rect.pos.y, rect.size.x, rect.size.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_segment_says_what_the_widget_is() {
        // A real name wins.
        assert_eq!(readable_segment("main_window", "Window"), "main_window");
        // No name: the type is what a person would call it.
        assert_eq!(readable_segment("-", "View"), "View");
        // A list item's index is not a name either — the type reads better.
        assert_eq!(readable_segment("3", "Label"), "Label");
        // Neither: something has to be said.
        assert_eq!(readable_segment("-", "-"), "Widget");
    }

    #[test]
    fn an_anonymous_segment_is_a_position_not_a_name() {
        // Legacy `-` / `-2` stand-ins can still arrive from a hand-written
        // path or an older note store; the loose finder must drop them
        // rather than hash them into ids.
        assert!(is_anonymous_segment("-"));
        assert!(is_anonymous_segment("-0"));
        assert!(is_anonymous_segment("-12"));
        assert!(!is_anonymous_segment("-a"));
        assert!(!is_anonymous_segment("main_window"));
        // `Label.2` is a NAME with an ordinal, not a position stand-in: the
        // slash is the hierarchy, the dot is only which one of several.
        assert!(!is_anonymous_segment("Label.2"));
    }
}
