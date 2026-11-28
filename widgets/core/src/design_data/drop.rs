//! Pointer placement among a container's visible, drawn children.
use super::{arg, json_str, locate::is_container, pick, window_arg, Args};
use crate::{makepad_draw::*, view::View, widget_tree::CxWidgetExt, WidgetRef, WidgetUid};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct DropHit {
    pub target_path: String,
    pub container_path: String,
    pub place: &'static str,
    pub flow: &'static str,
    pub target_rect: Rect,
    pub container_rect: Rect,
}

fn horizontal(widget: &WidgetRef, children: &[(usize, Rect)]) -> bool {
    match widget.borrow::<View>().map(|v| v.layout.flow) {
        Some(Flow::Right { .. }) => true,
        Some(Flow::Down) => false,
        _ => children.windows(2).any(|pair| {
            let (a, b) = (pair[0].1, pair[1].1);
            a.pos.y < b.pos.y + b.size.y && b.pos.y < a.pos.y + a.size.y
                && (a.pos.x + a.size.x <= b.pos.x + 0.5 || b.pos.x + b.size.x <= a.pos.x + 0.5)
        }),
    }
}

fn gap(children: &[(usize, Rect)], row: bool, pos: DVec2) -> Option<(usize, &'static str)> {
    let at = if row { pos.x } else { pos.y };
    let mut before = None;
    let mut last = None;
    for &(i, r) in children {
        let center = if row { r.center().x } else { r.center().y };
        if center > at && before.is_none_or(|(_, nearest)| center < nearest) { before = Some((i, center)); }
        if last.is_none_or(|(_, furthest)| center >= furthest) { last = Some((i, center)); }
    }
    before.map(|(i, _)| (i, "before")).or_else(|| last.map(|(i, _)| (i, "after")))
}

pub fn resolve_drop_target(cx: &Cx, window: usize, pos: DVec2, exclude_path: Option<&str>) -> Option<DropHit> {
    let excluded = match exclude_path {
        Some(path) => pick::resolve_target(cx, path, Some(window)).ok().map(|w| w.widget_uid().0),
        None => None,
    };
    let mut attached = HashSet::new();
    for pass in cx.passes.id_iter() {
        if cx.get_pass_window_id(pass).is_some_and(|w| w.id() == window) {
            attached.extend(cx.attached_draw_lists(pass));
        }
    }
    let tree = pick::indexed_flat_tree(cx);
    let widget = |i: usize| cx.widget_tree().widget(WidgetUid(tree.rows[i].uid));
    let mut hit = None;
    let mut skip = None;
    for (i, entry) in tree.rows.iter().enumerate() {
        if skip.is_some_and(|depth| entry.depth > depth) { continue; }
        let w = widget(i);
        if Some(entry.uid) == excluded || entry.inspector || !w.visible()
            || w.borrow::<crate::window::Window>().is_some_and(|w| w.window_index() != window) {
            skip = Some(entry.depth);
            continue;
        }
        skip = None;
        if entry.ty != "Window" && !pick::is_design_transparent(&w)
            && pick::pick_candidate(cx, &w, pos, &attached).is_some()
            && hit.is_none_or(|(_, depth)| entry.depth >= depth) {
            hit = Some((i, entry.depth));
        }
    }
    let drawn_children = |i: usize| tree.children[i].iter().copied().filter_map(|child| {
        let w = widget(child);
        if Some(tree.rows[child].uid) == excluded || tree.rows[child].inspector || !w.visible()
            || !w.area().is_attached(cx, &attached) { return None; }
        let rect = w.area().clipped_rect_union(cx);
        (rect.size.x > 0.0 && rect.size.y > 0.0).then_some((child, rect))
    }).collect::<Vec<_>>();
    let answer = |target: usize, container: usize, place, row| DropHit {
        target_path: tree.paths[target].clone(),
        container_path: tree.paths[container].clone(),
        place, flow: if row { "row" } else { "col" },
        target_rect: widget(target).area().clipped_rect_union(cx),
        container_rect: widget(container).area().clipped_rect_union(cx),
    };

    let mut target = hit?.0;
    let picked = widget(target);
    if is_container(cx, &picked) {
        let children = drawn_children(target);
        let row = horizontal(&picked, &children);
        if let Some((child, place)) = gap(&children, row, pos) {
            return Some(answer(child, target, place, row));
        }
        let rect = picked.area().clipped_rect_union(cx);
        let fx = (pos.x - rect.pos.x) / rect.size.x;
        let fy = (pos.y - rect.pos.y) / rect.size.y;
        if tree.parent[target].is_none_or(|p| !is_container(cx, &widget(p)))
            || (0.25..0.75).contains(&fx) && (0.25..0.75).contains(&fy) {
            return Some(answer(target, target, "inside", row));
        }
    }
    // A fixed slot's host cannot take a sibling there. Climb to the first
    // ordered container and offer placement relative to its direct child.
    while let Some(parent) = tree.parent[target] {
        let container = widget(parent);
        if is_container(cx, &container) {
            let row = horizontal(&container, &drawn_children(parent));
            let rect = widget(target).area().clipped_rect_union(cx);
            let before = if row { pos.x < rect.center().x } else { pos.y < rect.center().y };
            return Some(answer(target, parent, if before { "before" } else { "after" }, row));
        }
        target = parent;
    }
    None
}

pub(super) fn remote(cx: &Cx, args: &Args) -> Result<String, String> {
    let (x, y) = match (arg(args, "at"), arg(args, "x"), arg(args, "y")) {
        (Some(at), None, None) => at.split_once(',').ok_or("expected at=x,y")?,
        (None, Some(x), Some(y)) => (x, y),
        _ => return Err("supply at=x,y or x and y".into()),
    };
    let coord = |s: &str| s.trim().parse::<f64>().ok().filter(|n| n.is_finite()).ok_or("invalid drop coordinate");
    let pos = dvec2(coord(x)?, coord(y)?);
    let window = window_arg(args)?.unwrap_or(0);
    let Some(hit) = resolve_drop_target(cx, window, pos, arg(args, "exclude")) else { return Ok("null".into()) };
    let rect = |r: Rect| format!("[{},{},{},{}]", r.pos.x, r.pos.y, r.size.x, r.size.y);
    Ok(format!(r#"{{"target_path":{},"container_path":{},"place":"{}","flow":"{}","target_rect":{},"container_rect":{}}}"#,
        json_str(&hit.target_path), json_str(&hit.container_path), hit.place, hit.flow, rect(hit.target_rect), rect(hit.container_rect)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_target_skips_excluded_subtree_and_picks_flow_gap() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        crate::widget::register_widget_type::<View>(&mut cx, "View");
        let root = cx.with_vm(|vm| {
            let view = |vm: &mut ScriptVm, width, children: &[(&str, WidgetRef)]| {
                let mut v = View::script_new(vm);
                v.walk = Walk::fixed(width, 40.);
                v.layout = Layout::flow_right();
                v.layout.spacing = 10.;
                for &(name, ref child) in children {
                    v.children.push((LiveId::from_str_with_lut(name).unwrap(), child.clone()));
                }
                WidgetRef::new_with_inner(Box::new(v))
            };
            let inner = view(vm, 40., &[]);
            let a = view(vm, 40., &[("inner", inner)]);
            let b = view(vm, 40., &[]);
            let c = view(vm, 40., &[]);
            view(vm, 160., &[("a", a), ("b", b), ("c", c)])
        });
        crate::set_ui_root(&mut cx, &root);
        let window = WindowHandle::new(&mut cx);
        let pass = DrawPass::new(&mut cx);
        window.set_pass(&mut cx, &pass);
        let size = dvec2(160., 40.);
        pass.set_size(&mut cx, size);
        let mut list = DrawList2d::new(&mut cx);
        cx.redraw_all();
        let event = std::mem::take(&mut cx.new_draw_event);
        {
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, Some(1.));
            list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }
        let hit = |x, exclude| resolve_drop_target(&cx, window.window_id().id(), dvec2(x, 20.), exclude)
            .unwrap_or_else(|| panic!("expected a drop target at x={x} with exclude={exclude:?}"));
        let between = hit(45., None);
        assert_eq!((between.target_path.as_str(), between.place, between.flow), ("/b", "before", "row"), "horizontal gap inserts before b");
        let unmounted = hit(45., Some("unmounted"));
        assert_eq!((unmounted.target_path, unmounted.place, unmounted.flow), (between.target_path, between.place, between.flow),
            "an unresolved exclusion preserves the horizontal gap target");
        let inner = hit(20., None);
        assert!(inner.target_path.ends_with("/inner"), "the nested widget is normally picked");
        let lifted = hit(20., Some("a"));
        assert_eq!((lifted.target_path.as_str(), lifted.place), ("/b", "before"), "excluding a prunes its pick subtree and its gap rectangle");
        assert_eq!(lifted.target_rect, root.child(id!(b)).area().clipped_rect_union(&cx), "target geometry belongs to b");
    }
}
