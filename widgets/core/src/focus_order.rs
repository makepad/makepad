//! Named focus groups reuse the window's navigation tree and its draw order.

use crate::{makepad_draw::*, nav_control::NavControl};
use std::collections::HashMap;

#[derive(Default)]
struct FocusList {
    stamp: Option<(DrawListId, u64)>,
    groups: HashMap<Area, LiveId>,
}

#[derive(Default)]
struct FocusOrder {
    lists: Vec<FocusList>,
    requested: Option<Area>,
}

/// Register during drawing. Entries expire with their draw list, including
/// hidden and removed controls; cached draw lists keep their existing order.
pub fn register(cx: &mut Cx, area: Area, group: LiveId) {
    let Some(list) = area.draw_list_id() else { return };
    let stamp = (list, cx.draw_lists[list].recording_gen);
    let order = cx.global::<FocusOrder>();
    if order.lists.len() <= list.index() {
        order.lists.resize_with(list.index() + 1, FocusList::default);
    }
    let entry = &mut order.lists[list.index()];
    if entry.stamp != Some(stamp) {
        entry.stamp = Some(stamp);
        entry.groups.clear();
    }
    entry.groups.insert(area, group);
}

/// Move within one group and the current window/modal. The destination
/// consumes the request to open its editor, even if it precedes us in draw order.
pub fn advance(cx: &mut Cx, area: Area, group: LiveId, backwards: bool) {
    let Some(mut root) = area.draw_list_id() else { return };
    while let Some(parent) = cx.draw_lists[root].codeflow_parent_id {
        root = parent;
    }
    let stops = CxDraw::nav_scope_stops(cx, root);
    let order = cx.global::<FocusOrder>();
    let stops: Vec<_> = stops.into_iter().filter(|stop| {
        stop.draw_list_id()
            .and_then(|list| order.lists.get(list.index()))
            .and_then(|entry| entry.groups.get(stop)) == Some(&group)
    }).collect();
    let Some(at) = stops.iter().position(|stop| *stop == area) else { return };
    let count = stops.len();
    let next = stops[(at + if backwards { count - 1 } else { 1 }) % count];
    order.requested = Some(next);
    if let Some((_, scroll_stack)) =
        CxDraw::iterate_nav_stops(cx, root, |_, stop| (stop.area == next).then_some(next))
    {
        NavControl::send_trigger_to_scroll_stack(cx, scroll_stack);
    }
    cx.set_key_focus(next);
    cx.redraw_area(next);
}

pub fn take_request(cx: &mut Cx, area: Area) -> bool {
    let order = cx.global::<FocusOrder>();
    if !area.is_empty() && order.requested == Some(area) {
        order.requested = None;
        true
    } else {
        false
    }
}
