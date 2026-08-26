//! Lane D. The application shell: top bar, the area Dock, status bar, and the
//! overlay layers (menus, pie, tooltips, modals, perf graph). It also owns the
//! two things only a screen-wide widget can own — the **workspaces** and the
//! **area tree** — because both are edits to the Dock's node map.
//!
//! Six areas exist (`Area0`…`Area5`), each a `FabArea` that can show any
//! editor; a workspace is one arrangement of them. Switching a workspace tab
//! rebuilds only the Dock's layout nodes: stable tab bodies stay owned by the
//! Dock above every workspace, so viewport GPU caches and interaction state
//! survive re-parenting. Ctrl+Space swaps the whole tree for the area under
//! the pointer and swaps it back. The corner grip splits its area, or joins it
//! away on an inward drag.

use crate::api::*;
use crate::ui::area::{area_actions, AreaAction};
use crate::ui::viewport_area::open_shading_pie;
use makepad_widgets::*;
use std::collections::HashMap;
use std::time::Instant;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    mod.widgets.FabShellBase = #(FabShell::register_widget(vm))
    mod.widgets.FabShell = set_type_default() do mod.widgets.FabShellBase{
        width: Fill
        height: Fill
        flow: Overlay
        show_bg: true
        draw_bg +: {
            color: fab.color_area
        }
        main := View{
            width: Fill
            height: Fill
            flow: Down
            topbar := FabTopBar{}
            dock := Dock{
                width: Fill
                height: Fill
                splitter +: {
                    size: fab.splitter_size
                    draw_bg +: {
                        color: fab.color_border
                        color_hover: fab.color_accent
                        color_drag: fab.color_accent_hover
                    }
                }

                root := DockSplitter{
                    axis: SplitterAxis.Horizontal
                    align: SplitterAlign.FromB(360.0)
                    a: @center_split
                    b: @right_split
                }
                center_split := DockSplitter{
                    axis: SplitterAxis.Vertical
                    align: SplitterAlign.FromB(150.0)
                    a: @viewport_split
                    b: @bottom_split
                }
                bottom_split := DockSplitter{
                    axis: SplitterAxis.Horizontal
                    align: SplitterAlign.Weighted(0.5)
                    a: @tabs_4
                    b: @tabs_5
                }
                viewport_split := DockSplitter{
                    axis: SplitterAxis.Horizontal
                    align: SplitterAlign.Weighted(0.5)
                    a: @tabs_0
                    b: @tabs_1
                }
                right_split := DockSplitter{
                    axis: SplitterAxis.Vertical
                    align: SplitterAlign.FromA(300.0)
                    a: @tabs_2
                    b: @tabs_3
                }
                tabs_0 := DockTabs{ tabs: [@area_0] selected: 0 closable: false hide_tab_bar: true }
                tabs_1 := DockTabs{ tabs: [@area_1] selected: 0 closable: false hide_tab_bar: true }
                tabs_2 := DockTabs{ tabs: [@area_2] selected: 0 closable: false hide_tab_bar: true }
                tabs_3 := DockTabs{ tabs: [@area_3] selected: 0 closable: false hide_tab_bar: true }
                tabs_4 := DockTabs{ tabs: [@area_4] selected: 0 closable: false hide_tab_bar: true }
                tabs_5 := DockTabs{ tabs: [@area_5] selected: 0 closable: false hide_tab_bar: true }

                area_0 := DockTab{ name: "3D Viewport" template: @PermanentTab kind: @Area0 }
                area_1 := DockTab{ name: "Raytraced" template: @PermanentTab kind: @Area1 }
                area_2 := DockTab{ name: "Outliner" template: @PermanentTab kind: @Area2 }
                area_3 := DockTab{ name: "Properties" template: @PermanentTab kind: @Area3 }
                area_4 := DockTab{ name: "Sheets" template: @PermanentTab kind: @Area4 }
                area_5 := DockTab{ name: "Tours" template: @PermanentTab kind: @Area5 }

                Area0 := FabArea{ slot: 0 view_index: 0 editor: @Viewport }
                Area1 := FabArea{ slot: 1 view_index: 1 editor: @Viewport }
                Area2 := FabArea{ slot: 2 view_index: 0 editor: @Outliner }
                Area3 := FabArea{ slot: 3 view_index: 0 editor: @Properties }
                Area4 := FabArea{ slot: 4 view_index: 0 editor: @Sheets }
                Area5 := FabArea{ slot: 5 view_index: 0 editor: @Tours }
            }
            statusbar := FabStatusBar{}
        }
        file_browser := FabFileBrowser{}
        keymap_help := FabKeymapHelp{}
        command_palette := FabCommandPalette{}
        perf_box := View{
            visible: false
            width: Fill
            height: Fill
            perf_graph := PerfGraph{}
        }
        // Menus and the pie sit under the tooltip layer and over everything
        // else: an overlay draw list floats over every area and splitter, and
        // being last in the tree is what makes them win hover.
        // The colour-picker popover lives here for the same reason: children
        // handle events in reverse order, so only a top-of-shell modal wins
        // the press race against the dock's areas.
        color_layer := FabColorPickerLayer{}
        menu_layer := FabMenuLayer{}
        pie_layer := FabPieLayer{}
        tip_layer := TipLayer{}
    }
}

// ===========================================================================
// Workspaces — one Dock node map each
// ===========================================================================

fn tabs_id(slot: usize) -> LiveId {
    match slot {
        0 => live_id!(tabs_0),
        1 => live_id!(tabs_1),
        2 => live_id!(tabs_2),
        3 => live_id!(tabs_3),
        4 => live_id!(tabs_4),
        _ => live_id!(tabs_5),
    }
}

fn area_tab_id(slot: usize) -> LiveId {
    match slot {
        0 => live_id!(area_0),
        1 => live_id!(area_1),
        2 => live_id!(area_2),
        3 => live_id!(area_3),
        4 => live_id!(area_4),
        _ => live_id!(area_5),
    }
}

fn area_kind(slot: usize) -> LiveId {
    match slot {
        0 => live_id!(Area0),
        1 => live_id!(Area1),
        2 => live_id!(Area2),
        3 => live_id!(Area3),
        4 => live_id!(Area4),
        _ => live_id!(Area5),
    }
}

fn area_name(slot: usize) -> &'static str {
    match slot {
        0 => "3D Viewport",
        1 => "Raytraced",
        2 => "Outliner",
        3 => "Properties",
        4 => "Sheets",
        _ => "Tours",
    }
}

/// Every layout is built from the same six leaves, so the tab nodes are
/// emitted once here and only the tree above them changes.
fn with_leaves(mut map: HashMap<LiveId, DockItem>, slots: &[usize]) -> HashMap<LiveId, DockItem> {
    for &s in slots {
        map.insert(
            tabs_id(s),
            DockItem::Tabs {
                tabs: vec![area_tab_id(s)],
                selected: 0,
                closable: false,
                hide_tab_bar: true,
            },
        );
        map.insert(
            area_tab_id(s),
            DockItem::Tab {
                name: area_name(s).to_string(),
                template: live_id!(PermanentTab),
                kind: area_kind(s),
            },
        );
    }
    map
}

fn split(
    map: &mut HashMap<LiveId, DockItem>,
    id: LiveId,
    axis: SplitterAxis,
    align: SplitterAlign,
    a: LiveId,
    b: LiveId,
) {
    map.insert(id, DockItem::Splitter { axis, align, a, b });
}

fn workspace_layout(ws: Workspace) -> HashMap<LiveId, DockItem> {
    let mut m: HashMap<LiveId, DockItem> = HashMap::new();
    let root = live_id!(root);
    match ws {
        // Viewport pair, sheets + tours below, outliner over properties right.
        Workspace::Quad => {
            split(
                &mut m,
                root,
                SplitterAxis::Horizontal,
                SplitterAlign::FromB(360.0),
                live_id!(center_split),
                live_id!(right_split),
            );
            split(
                &mut m,
                live_id!(center_split),
                SplitterAxis::Vertical,
                SplitterAlign::FromB(150.0),
                live_id!(viewport_split),
                live_id!(bottom_split),
            );
            split(
                &mut m,
                live_id!(viewport_split),
                SplitterAxis::Horizontal,
                SplitterAlign::Weighted(0.5),
                tabs_id(0),
                tabs_id(1),
            );
            split(
                &mut m,
                live_id!(bottom_split),
                SplitterAxis::Horizontal,
                SplitterAlign::Weighted(0.5),
                tabs_id(4),
                tabs_id(5),
            );
            split(
                &mut m,
                live_id!(right_split),
                SplitterAxis::Vertical,
                SplitterAlign::FromA(300.0),
                tabs_id(2),
                tabs_id(3),
            );
            with_leaves(m, &[0, 1, 2, 3, 4, 5])
        }
        // Realtime walk driver with the locked path-traced follower.
        Workspace::Walkthrough => {
            split(
                &mut m,
                root,
                SplitterAxis::Horizontal,
                SplitterAlign::FromB(420.0),
                tabs_id(0),
                tabs_id(1),
            );
            with_leaves(m, &[0, 1])
        }
        // Two viewports stacked for cut comparison, properties beside them.
        Workspace::Sections => {
            split(
                &mut m,
                root,
                SplitterAxis::Horizontal,
                SplitterAlign::FromB(320.0),
                live_id!(center_split),
                tabs_id(3),
            );
            split(
                &mut m,
                live_id!(center_split),
                SplitterAxis::Vertical,
                SplitterAlign::Weighted(0.5),
                tabs_id(0),
                tabs_id(1),
            );
            with_leaves(m, &[0, 1, 3])
        }
        // Paper first.
        Workspace::Sheets => {
            split(
                &mut m,
                root,
                SplitterAxis::Horizontal,
                SplitterAlign::FromB(300.0),
                tabs_id(4),
                tabs_id(2),
            );
            with_leaves(m, &[2, 4])
        }
        // Sun study: the realtime view plus the tab that drives the sun.
        Workspace::SunStudy => {
            split(
                &mut m,
                root,
                SplitterAxis::Horizontal,
                SplitterAlign::FromB(330.0),
                tabs_id(0),
                tabs_id(3),
            );
            with_leaves(m, &[0, 3])
        }
        // Render: the path-traced view and its settings, tours underneath.
        Workspace::Render => {
            split(
                &mut m,
                root,
                SplitterAxis::Horizontal,
                SplitterAlign::FromB(340.0),
                live_id!(center_split),
                tabs_id(3),
            );
            split(
                &mut m,
                live_id!(center_split),
                SplitterAxis::Vertical,
                SplitterAlign::FromB(160.0),
                tabs_id(1),
                tabs_id(5),
            );
            with_leaves(m, &[1, 3, 5])
        }
    }
}

/// A single-area screen: what Ctrl+Space swaps in.
fn maximized_layout(slot: usize) -> HashMap<LiveId, DockItem> {
    let mut m: HashMap<LiveId, DockItem> = HashMap::new();
    m.insert(
        live_id!(root),
        DockItem::Tabs {
            tabs: vec![area_tab_id(slot)],
            selected: 0,
            closable: false,
            hide_tab_bar: true,
        },
    );
    m.insert(
        area_tab_id(slot),
        DockItem::Tab {
            name: area_name(slot).to_string(),
            template: live_id!(PermanentTab),
            kind: area_kind(slot),
        },
    );
    m
}

/// Which slots a node map actually shows.
fn slots_in(map: &HashMap<LiveId, DockItem>) -> Vec<usize> {
    (0..6)
        .filter(|s| map.contains_key(&area_tab_id(*s)))
        .collect()
}

/// The parent splitter of `child`, and which side it is on.
fn parent_of(map: &HashMap<LiveId, DockItem>, child: LiveId) -> Option<(LiveId, bool)> {
    for (id, item) in map.iter() {
        if let DockItem::Splitter { a, b, .. } = item {
            if *a == child {
                return Some((*id, true));
            }
            if *b == child {
                return Some((*id, false));
            }
        }
    }
    None
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabShell {
    #[deref]
    view: View,
    #[rust]
    perf_shown: bool,
    #[rust]
    workspace: Option<Workspace>,
    #[rust]
    maximized: Option<(usize, HashMap<LiveId, DockItem>)>,
    #[rust]
    mouse: DVec2,
    #[rust]
    split_seq: u64,
    /// Armed by a workspace action and consumed after the replacement layout
    /// has completed its first draw. Enabled only for measurement runs.
    #[rust]
    workspace_switch_started: Option<(Workspace, Workspace, Instant)>,
}

impl FabShell {
    /// The area slot under the pointer, by asking the Dock for each live
    /// item's drawn rect — the same rects the user is looking at.
    fn slot_at(&self, cx: &mut Cx, pos: DVec2) -> Option<usize> {
        let dock = self.view.dock(cx, ids!(main.dock));
        for slot in 0..6 {
            let item = dock.item(area_tab_id(slot));
            if item.is_empty() {
                continue;
            }
            let r = item.area().rect(cx);
            if r.size.x > 0.0 && r.contains(pos) {
                return Some(slot);
            }
        }
        None
    }

    fn apply_layout(&mut self, cx: &mut Cx, map: HashMap<LiveId, DockItem>) {
        let dock = self.view.dock(cx, ids!(main.dock));
        dock.load_state_preserving_items(cx, map);
        self.view.redraw(cx);
    }

    fn workspace_timing_enabled() -> bool {
        std::env::var("FAB_WORKSPACE_SWITCH_TIMING")
            .map(|value| matches!(value.as_str(), "1" | "true" | "on" | "yes"))
            .unwrap_or(false)
    }

    fn set_workspace(&mut self, cx: &mut Cx, ws: Workspace) {
        self.workspace = Some(ws);
        self.maximized = None;
        let map = workspace_layout(ws);
        self.apply_layout(cx, map);
    }

    fn toggle_maximize(&mut self, cx: &mut Cx) {
        if let Some((_, saved)) = self.maximized.take() {
            self.apply_layout(cx, saved);
            return;
        }
        let fallback = {
            let dock = self.view.dock(cx, ids!(main.dock));
            (0..6).find(|s| !dock.item(area_tab_id(*s)).is_empty())
        };
        let Some(slot) = self.slot_at(cx, self.mouse).or(fallback) else {
            return;
        };
        let saved = {
            let dock = self.view.dock(cx, ids!(main.dock));
            dock.clone_state()
        };
        let Some(saved) = saved else { return };
        self.maximized = Some((slot, saved));
        let map = maximized_layout(slot);
        self.apply_layout(cx, map);
    }

    /// Split `slot` in two: the sibling takes the first area that is not
    /// already on screen, so a split never produces two identical panes.
    fn split_area(&mut self, cx: &mut Cx, slot: usize, vertical: bool) {
        let state = {
            let dock = self.view.dock(cx, ids!(main.dock));
            dock.clone_state()
        };
        let Some(mut map) = state else { return };
        let used = slots_in(&map);
        let Some(free) = (0..6).find(|s| !used.contains(s)) else {
            return;
        };
        let leaf = tabs_id(slot);
        self.split_seq = self.split_seq.wrapping_add(1);
        let new_split = LiveId(0x6269_6d78_0004_0000 | self.split_seq);
        let node = DockItem::Splitter {
            axis: if vertical {
                SplitterAxis::Horizontal
            } else {
                SplitterAxis::Vertical
            },
            align: SplitterAlign::Weighted(0.5),
            a: leaf,
            b: tabs_id(free),
        };
        match parent_of(&map, leaf) {
            Some((parent, is_a)) => {
                if let Some(DockItem::Splitter { a: pa, b: pb, .. }) = map.get_mut(&parent) {
                    if is_a {
                        *pa = new_split;
                    } else {
                        *pb = new_split;
                    }
                }
                map.insert(new_split, node);
            }
            None => {
                // `leaf` was the whole screen.
                map.insert(live_id!(root), node);
            }
        }
        let map = with_leaves(map, &[slot, free]);
        self.apply_layout(cx, map);
    }

    /// Join `slot` away: its parent splitter collapses onto the sibling.
    fn join_area(&mut self, cx: &mut Cx, slot: usize) {
        let state = {
            let dock = self.view.dock(cx, ids!(main.dock));
            dock.clone_state()
        };
        let Some(mut map) = state else { return };
        let leaf = tabs_id(slot);
        let Some((parent, is_a)) = parent_of(&map, leaf) else {
            return;
        };
        let sibling = match map.get(&parent) {
            Some(DockItem::Splitter { a, b, .. }) => {
                if is_a {
                    *b
                } else {
                    *a
                }
            }
            _ => return,
        };
        match parent_of(&map, parent) {
            Some((grand, g_is_a)) => {
                if let Some(DockItem::Splitter { a, b, .. }) = map.get_mut(&grand) {
                    if g_is_a {
                        *a = sibling;
                    } else {
                        *b = sibling;
                    }
                }
            }
            None => {
                if let Some(node) = map.get(&sibling).cloned() {
                    map.insert(live_id!(root), node);
                }
            }
        }
        map.remove(&parent);
        map.remove(&leaf);
        map.remove(&area_tab_id(slot));
        self.apply_layout(cx, map);
    }
}

impl Widget for FabShell {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::MouseMove(e) = event {
            self.mouse = e.abs;
        }
        // Z raises the shading pie over the area under the pointer. `main.rs`
        // leaves bare Z alone, so this is the only handler.
        if let Event::KeyDown(ke) = event {
            let m = ke.modifiers;
            if ke.key_code == KeyCode::KeyZ && !m.alt && !m.control && !m.logo && !m.shift {
                let shading = scope
                    .data
                    .get_mut::<AppState>()
                    .map(|s| s.view().shading)
                    .unwrap_or(Shading::Solid);
                let at = self.mouse;
                open_shading_pie(cx, at, shading);
            }
        }

        self.view.handle_event(cx, event, scope);

        if let Event::Actions(actions) = event {
            if Self::workspace_timing_enabled() {
                for action in shell_actions(actions) {
                    if let ShellAction::SetWorkspace(next) = action {
                        if let Some(previous) = self.workspace.filter(|ws| ws != next) {
                            self.workspace_switch_started =
                                Some((previous, *next, Instant::now()));
                        }
                    }
                }
            }
            for a in area_actions(actions) {
                match a {
                    AreaAction::Split { slot, vertical } => self.split_area(cx, *slot, *vertical),
                    AreaAction::Join { slot } => self.join_area(cx, *slot),
                    AreaAction::Focused { .. } => {}
                }
            }
        }

        match event.drag_hits(cx, self.view.area()) {
            DragHit::Drag(drag) => {
                *drag.response.lock().unwrap() = DragResponse::Copy;
            }
            DragHit::Drop(drop) => {
                for item in drop.items.iter() {
                    if let DragItem::FilePath { path, .. } = item {
                        cx.action(ShellAction::OpenFile(std::path::PathBuf::from(path)));
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut want_ws = None;
        let mut want_max = false;
        if let Some(state) = scope.data.get_mut::<AppState>() {
            if self.perf_shown != state.ui.show_perf {
                self.perf_shown = state.ui.show_perf;
                self.view
                    .view(cx, ids!(perf_box))
                    .set_visible(cx, self.perf_shown);
                cx.perf_monitor.set_enabled(self.perf_shown);
            }
            if self.workspace != Some(state.ui.workspace) {
                want_ws = Some(state.ui.workspace);
            }
            if state.ui.area_maximized != self.maximized.is_some() {
                want_max = true;
            }
        }
        if let Some(ws) = want_ws {
            self.set_workspace(cx, ws);
        } else if want_max {
            self.toggle_maximize(cx);
        }
        let step = self.view.draw_walk(cx, scope, walk);
        if step.is_done() {
            if let Some((from, to, started)) = self.workspace_switch_started.take() {
                log!(
                    "fab workspace switch: {} -> {} first presented frame {:.1} ms (target < 100 ms)",
                    from.label(),
                    to.label(),
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_and_walkthrough_reuse_the_same_viewport_tabs() {
        let quad = workspace_layout(Workspace::Quad);
        let walk = workspace_layout(Workspace::Walkthrough);
        for slot in [0, 1] {
            let tab = area_tab_id(slot);
            assert!(matches!(
                (quad.get(&tab), walk.get(&tab)),
                (
                    Some(DockItem::Tab { kind: quad_kind, .. }),
                    Some(DockItem::Tab { kind: walk_kind, .. })
                ) if quad_kind == walk_kind && *quad_kind == area_kind(slot)
            ));
            assert!(matches!(
                (quad.get(&tabs_id(slot)), walk.get(&tabs_id(slot))),
                (
                    Some(DockItem::Tabs { tabs: quad_tabs, .. }),
                    Some(DockItem::Tabs { tabs: walk_tabs, .. })
                ) if quad_tabs == walk_tabs && quad_tabs == &[tab]
            ));
        }
    }

    #[test]
    fn every_workspace_uses_stable_area_ids() {
        for workspace in Workspace::ALL {
            let layout = workspace_layout(workspace);
            for slot in slots_in(&layout) {
                assert!(matches!(
                    layout.get(&area_tab_id(slot)),
                    Some(DockItem::Tab { kind, .. }) if *kind == area_kind(slot)
                ));
            }
        }
    }
}
