//! The top menu bar: File / Edit / View / Window / Help.
//!
//! Fab behaviour, not a row of buttons that happen to be labelled like
//! menus: press File and the menu opens under it; slide sideways with the
//! button still down or up and the menu *tracks* — Edit opens the moment the
//! pointer crosses it; arrows walk the rows and step into flyouts; Left/Right
//! at the root hop to the neighbouring menu; Enter fires; Esc and a click
//! anywhere else close.
//!
//! **Every row here fires a `ShellAction` that visibly changes the app.** The
//! menus are built from the action inventory, not from Fab's layout: an
//! entry exists only when the action behind it has a reader somewhere that
//! moves pixels. `ui.toolbar_open`, `ui.area_maximized`, `ui.quad_view`,
//! `ui.properties_tab` has no reader in the app today, so Toolbar / Maximize
//! Area / Quad View / Preferences are *not* here. There
//! is no undo stack in `api.rs`, so there is no Undo. When those land, the
//! rows follow — one `MenuItem` each.
//!
//! The bar owns nothing else: the drawing, hover tracking, flyouts and
//! keyboard live in `menu.rs`, which every other dropdown in the app shares.

use crate::api::*;
use crate::ui::menu::menu_cycle;
use crate::ui::popover::{open_menu, ui_actions, FabUiAction, MenuItem, MenuPlace};
use makepad_widgets::*;

/// One owner id per top-level menu, so a pick can be traced back to the menu
/// it came from without a second lookup.
fn owner(i: usize) -> LiveId {
    LiveId(0x6269_6d78_0002_0000 | i as u64)
}

fn owner_index(id: LiveId) -> Option<usize> {
    let base = 0x6269_6d78_0002_0000u64;
    (id.0 >= base && id.0 < base + MENU_COUNT as u64).then(|| (id.0 - base) as usize)
}

/// Recent-file rows carry their index in the id.
const RECENT_BASE: u64 = 0x6269_6d78_0003_0000;
fn recent_id(i: usize) -> LiveId {
    LiveId(RECENT_BASE | i as u64)
}
fn recent_index(id: LiveId) -> Option<usize> {
    (id.0 >= RECENT_BASE && id.0 < RECENT_BASE + 64).then(|| (id.0 - RECENT_BASE) as usize)
}

const MENU_COUNT: usize = 5;

fn item_path(i: usize) -> &'static [LiveId] {
    match i {
        0 => ids!(m_file),
        1 => ids!(m_edit),
        2 => ids!(m_view),
        3 => ids!(m_window),
        _ => ids!(m_help),
    }
}

// ===========================================================================
// The menus, built from live state
// ===========================================================================

pub(crate) fn file_menu(state: &AppState) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::new(live_id!(file_open), "Open…").key("Cmd+O"),
        MenuItem::new(live_id!(file_demo), "Open Demo House").key("Cmd+Shift+O"),
    ];
    if !state.recent.is_empty() {
        let rows: Vec<MenuItem> = state
            .recent
            .iter()
            .enumerate()
            .take(10)
            .map(|(i, p)| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.display().to_string());
                MenuItem::new(recent_id(i), &name)
            })
            .collect();
        items.push(MenuItem::new(live_id!(file_recent), "Open Recent").flyout(rows));
    }
    let path = current_path(state);
    items.push(
        MenuItem::new(live_id!(file_reload), "Reload").enabled_if(path.is_some()),
    );
    items.push(MenuItem::sep());
    items.push(MenuItem::new(live_id!(file_quit), "Quit").key("Cmd+Q"));
    items
}

pub(crate) fn edit_menu(state: &AppState) -> Vec<MenuItem> {
    let has_sel = !state.scene_state.selection.set.is_empty();
    let has_scene = !state.scene.elements.is_empty();
    vec![
        MenuItem::new(live_id!(edit_select_all), "Select All").enabled_if(has_scene),
        MenuItem::new(live_id!(edit_deselect), "Deselect All").enabled_if(has_sel),
        MenuItem::sep(),
        MenuItem::new(live_id!(edit_hide), "Hide Selected")
            .key("H")
            .enabled_if(has_sel),
        MenuItem::new(live_id!(edit_isolate), "Isolate Selected")
            .key("Shift+H")
            .enabled_if(has_sel),
        MenuItem::new(live_id!(edit_unhide), "Unhide All").key("Alt+H"),
    ]
}

pub(crate) fn view_menu(state: &AppState) -> Vec<MenuItem> {
    let v = state.view();
    let has_sel = !state.scene_state.selection.set.is_empty();
    let viewpoints: Vec<MenuItem> = [
        (live_id!(vp_front), PresetView::Front, "Numpad 1"),
        (live_id!(vp_back), PresetView::Back, "Ctrl+Numpad 1"),
        (live_id!(vp_right), PresetView::Right, "Numpad 3"),
        (live_id!(vp_left), PresetView::Left, "Ctrl+Numpad 3"),
        (live_id!(vp_top), PresetView::Top, "Numpad 7"),
        (live_id!(vp_bottom), PresetView::Bottom, "Ctrl+Numpad 7"),
        (live_id!(vp_iso), PresetView::Isometric, "Numpad 9"),
    ]
    .iter()
    .map(|(id, p, key)| {
        MenuItem::new(*id, p.label())
            .key(key)
            .radio(v.preset == Some(*p))
    })
    .collect();

    let shading: Vec<MenuItem> = [
        (live_id!(sh_wire), Shading::Wireframe),
        (live_id!(sh_solid), Shading::Solid),
        (live_id!(sh_material), Shading::Material),
        (live_id!(sh_realtime), Shading::Realtime),
        (live_id!(sh_rendered), Shading::Rendered),
        (live_id!(sh_ink), Shading::HiddenLine),
    ]
    .iter()
    .map(|(id, s)| MenuItem::new(*id, s.label()).radio(v.shading == *s))
    .collect();

    // Only the flags something actually reads. `Overlays::statistics`,
    // `shadows`, `dof`, `section_caps` and `pivot` have no reader in the app
    // today, so they are not rows here — a checkbox that toggles a field
    // nobody looks at is the fake menu this app was told not to have.
    let o = v.overlays;
    let overlays = vec![
        MenuItem::new(live_id!(ov_grid), "Grid").checked(o.grid),
        MenuItem::new(live_id!(ov_axes), "Axes").checked(o.axes),
        MenuItem::new(live_id!(ov_outlines), "Selection Outline").checked(o.outlines),
        MenuItem::new(live_id!(ov_wire), "Wireframe on Shaded").checked(o.wire_on_shaded),
        MenuItem::sep(),
        MenuItem::new(live_id!(ov_cavity), "Cavity").checked(o.cavity),
        MenuItem::new(live_id!(ov_ssao), "Ambient Occlusion").checked(o.ssao),
        MenuItem::sep(),
        // Genuinely context-dependent: nothing to draw until the tool ran.
        MenuItem::new(live_id!(ov_measure), "Measurements")
            .checked(o.measurements)
            .enabled_if(!state.measurements.is_empty()),
        MenuItem::new(live_id!(ov_sections), "Section Planes")
            .checked(o.section_planes)
            .enabled_if(state.scene_state.section.enabled),
        MenuItem::sep(),
        MenuItem::new(live_id!(ov_text), "Text Info").checked(o.text_info),
        MenuItem::new(live_id!(ov_gizmo), "Navigation Gizmo").checked(o.nav_gizmo),
    ];

    vec![
        MenuItem::new(live_id!(view_frame_all), "Frame All").key("Home"),
        MenuItem::new(live_id!(view_frame_sel), "Frame Selected")
            .key(".")
            .enabled_if(has_sel),
        MenuItem::sep(),
        MenuItem::new(live_id!(view_viewpoint), "Viewpoint").flyout(viewpoints),
        MenuItem::new(live_id!(view_ortho), "Orthographic")
            .key("Numpad 5")
            .checked(v.camera.ortho),
        MenuItem::sep(),
        MenuItem::new(live_id!(view_shading), "Shading").flyout(shading),
        MenuItem::new(live_id!(view_xray), "X-Ray")
            .key("Alt+Z")
            .checked(v.xray),
        MenuItem::new(live_id!(view_overlays), "Overlays").flyout(overlays),
        MenuItem::sep(),
        MenuItem::new(live_id!(view_sidebar), "Sidebar")
            .key("N")
            .checked(state.ui.sidebar_open),
        MenuItem::new(live_id!(view_perf), "Performance Graph")
            .key("Shift+F3")
            .checked(state.ui.show_perf),
    ]
}

pub(crate) fn window_menu(state: &AppState) -> Vec<MenuItem> {
    let workspaces: Vec<MenuItem> = Workspace::ALL
        .iter()
        .enumerate()
        .map(|(i, w)| {
            MenuItem::new(LiveId(0x6269_6d78_0004_0000 | i as u64), w.label())
                .radio(state.ui.workspace == *w)
        })
        .collect();
    vec![
        MenuItem::new(live_id!(win_workspace), "Workspace").flyout(workspaces),
        MenuItem::sep(),
        MenuItem::new(live_id!(win_lock), "Lock Views").checked(state.ui.lock_views),
    ]
}

fn workspace_index(id: LiveId) -> Option<usize> {
    let base = 0x6269_6d78_0004_0000u64;
    (id.0 >= base && id.0 < base + Workspace::ALL.len() as u64).then(|| (id.0 - base) as usize)
}

pub(crate) fn help_menu() -> Vec<MenuItem> {
    vec![
        MenuItem::new(live_id!(help_keymap), "Keymap Reference").key("F1"),
        MenuItem::new(live_id!(help_palette), "Command Palette").key("F3"),
        MenuItem::sep(),
        MenuItem::new(live_id!(help_about), "About Fab"),
    ]
}

fn current_path(state: &AppState) -> Option<std::path::PathBuf> {
    match &state.load {
        LoadStatus::Loaded { path: Some(p) } => Some(p.clone()),
        _ => state.scene.source_path.clone(),
    }
}

// ===========================================================================
// Widget
// ===========================================================================

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    let MenuBarItem = View{
        width: Fit
        height: fab.row_height
        flow: Right
        align: Align{x: 0.5 y: 0.5}
        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            hover: instance(0.0)
            down: instance(0.0)
            open: instance(0.0)
            focus: instance(0.0)
            disabled: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                let c = vec4(fab.color_button_hover.xyz, (self.hover * 0.9 + self.down * 0.1) * (1.0 - self.disabled))
                sdf.fill_keep(c.mix(vec4(fab.color_accent.xyz, 1.0), self.open))
                sdf.stroke(vec4(fab.color_focus_ring.xyz, self.focus), 1.0)
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
            down: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {down: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {down: 1.0} }
                }
            }
        }
        label := mod.widgets.FabLabel{ height: Fit text: "" }
    }

    mod.widgets.FabMenuBarBase = #(FabMenuBar::register_widget(vm))
    mod.widgets.FabMenuBar = set_type_default() do mod.widgets.FabMenuBarBase{
        width: Fit
        height: Fill
        flow: Right
        align: Align{x: 0.0 y: 0.0}
        spacing: 1
        FabTip{ text: "Open File menu"
            m_file := MenuBarItem{ label +: { text: "File" } }
        }
        FabTip{ text: "Open Edit menu"
            m_edit := MenuBarItem{ label +: { text: "Edit" } }
        }
        FabTip{ text: "Open View menu"
            m_view := MenuBarItem{ label +: { text: "View" } }
        }
        FabTip{ text: "Open Window menu"
            m_window := MenuBarItem{ label +: { text: "Window" } }
        }
        FabTip{ text: "Open Help menu"
            m_help := MenuBarItem{ label +: { text: "Help" } }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabMenuBar {
    #[deref]
    view: View,
    #[rust]
    open: Option<usize>,
    #[rust]
    hovered: Option<usize>,
}

impl FabMenuBar {
    fn item_at(&self, cx: &mut Cx, p: Vec2d) -> Option<usize> {
        (0..MENU_COUNT).find(|i| {
            self.view
                .view(cx, item_path(*i))
                .area()
                .clipped_rect(cx)
                .contains(p)
        })
    }

    /// Push the open highlight into the five item shaders. Hover/press are
    /// the item's own animator (100 ms ease).
    fn sync(&mut self, cx: &mut Cx) {
        for i in 0..MENU_COUNT {
            let open = if self.open == Some(i) { 1.0f64 } else { 0.0 };
            let mut w = self.view.view(cx, item_path(i));
            script_apply_eval!(cx, w, {
                draw_bg +: {
                    open: #(open)
                }
            });
        }
        self.view.redraw(cx);
    }

    fn raise(&mut self, cx: &mut Cx, scope: &mut Scope, i: usize) {
        let Some(state) = scope.data.get_mut::<AppState>() else {
            return;
        };
        let items = self.build(i, state);
        let anchor = self.view.view(cx, item_path(i)).area().rect(cx);
        self.open = Some(i);
        self.sync(cx);
        open_menu(cx, owner(i), items, anchor, MenuPlace::Below);
    }

    fn build(&self, i: usize, state: &AppState) -> Vec<MenuItem> {
        match i {
            0 => file_menu(state),
            1 => edit_menu(state),
            2 => view_menu(state),
            3 => window_menu(state),
            _ => help_menu(),
        }
    }
}

/// One picked row → one action. Everything here has a reader that moves
/// pixels; see the module doc for what is deliberately absent.
pub(crate) fn action_for(state: &AppState, id: LiveId) -> Option<ShellAction> {
    let v = state.active_view;
    if let Some(i) = recent_index(id) {
        return state.recent.get(i).cloned().map(ShellAction::OpenFile);
    }
    if let Some(i) = workspace_index(id) {
        return Some(ShellAction::SetWorkspace(Workspace::ALL[i]));
    }
    {
        let mut overlays = state.view().overlays;
        match id {
            // ---- File ----
            _ if id == live_id!(file_open) => Some(ShellAction::ShowFileBrowser(true)),
            _ if id == live_id!(file_demo) => Some(ShellAction::OpenDemo),
            _ if id == live_id!(file_reload) => current_path(state).map(ShellAction::OpenFile),
            _ if id == live_id!(file_quit) => Some(ShellAction::Quit),
            // ---- Edit ----
            _ if id == live_id!(edit_select_all) => Some(ShellAction::SelectSet(
                state
                    .scene
                    .elements
                    .iter()
                    .map(|e| e.id)
                    .filter(|id| state.is_visible(*id))
                    .collect(),
            )),
            _ if id == live_id!(edit_deselect) => Some(ShellAction::ClearSelection),
            _ if id == live_id!(edit_hide) => Some(ShellAction::HideSelected),
            _ if id == live_id!(edit_isolate) => Some(ShellAction::IsolateSelected),
            _ if id == live_id!(edit_unhide) => Some(ShellAction::UnhideAll),
            // ---- View ----
            _ if id == live_id!(view_frame_all) => Some(ShellAction::FrameAll(v)),
            _ if id == live_id!(view_frame_sel) => Some(ShellAction::FrameSelected(v)),
            _ if id == live_id!(view_ortho) => Some(ShellAction::ToggleOrtho(v)),
            _ if id == live_id!(view_xray) => Some(ShellAction::ToggleXray(v)),
            _ if id == live_id!(view_sidebar) => Some(ShellAction::ToggleSidebar),
            _ if id == live_id!(view_perf) => Some(ShellAction::TogglePerf),
            _ if id == live_id!(vp_front) => Some(ShellAction::PresetView(v, PresetView::Front)),
            _ if id == live_id!(vp_back) => Some(ShellAction::PresetView(v, PresetView::Back)),
            _ if id == live_id!(vp_left) => Some(ShellAction::PresetView(v, PresetView::Left)),
            _ if id == live_id!(vp_right) => Some(ShellAction::PresetView(v, PresetView::Right)),
            _ if id == live_id!(vp_top) => Some(ShellAction::PresetView(v, PresetView::Top)),
            _ if id == live_id!(vp_bottom) => Some(ShellAction::PresetView(v, PresetView::Bottom)),
            _ if id == live_id!(vp_iso) => {
                Some(ShellAction::PresetView(v, PresetView::Isometric))
            }
            _ if id == live_id!(sh_wire) => Some(ShellAction::SetShading(v, Shading::Wireframe)),
            _ if id == live_id!(sh_solid) => Some(ShellAction::SetShading(v, Shading::Solid)),
            _ if id == live_id!(sh_material) => {
                Some(ShellAction::SetShading(v, Shading::Material))
            }
            _ if id == live_id!(sh_realtime) => {
                Some(ShellAction::SetShading(v, Shading::Realtime))
            }
            _ if id == live_id!(sh_rendered) => {
                Some(ShellAction::SetShading(v, Shading::Rendered))
            }
            _ if id == live_id!(sh_ink) => Some(ShellAction::SetShading(v, Shading::HiddenLine)),
            _ if id == live_id!(ov_grid) => {
                overlays.grid = !overlays.grid;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            _ if id == live_id!(ov_axes) => {
                overlays.axes = !overlays.axes;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            _ if id == live_id!(ov_outlines) => {
                overlays.outlines = !overlays.outlines;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            _ if id == live_id!(ov_wire) => {
                overlays.wire_on_shaded = !overlays.wire_on_shaded;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            _ if id == live_id!(ov_cavity) => {
                overlays.cavity = !overlays.cavity;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            _ if id == live_id!(ov_ssao) => {
                overlays.ssao = !overlays.ssao;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            _ if id == live_id!(ov_measure) => {
                overlays.measurements = !overlays.measurements;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            _ if id == live_id!(ov_sections) => {
                overlays.section_planes = !overlays.section_planes;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            _ if id == live_id!(ov_text) => {
                overlays.text_info = !overlays.text_info;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            _ if id == live_id!(ov_gizmo) => {
                overlays.nav_gizmo = !overlays.nav_gizmo;
                Some(ShellAction::SetOverlays(v, overlays))
            }
            // ---- Window ----
            _ if id == live_id!(win_lock) => Some(ShellAction::ToggleLockViews),
            // ---- Help ----
            _ if id == live_id!(help_keymap) => Some(ShellAction::ShowKeymapHelp(true)),
            _ if id == live_id!(help_palette) => Some(ShellAction::ToggleCommandPalette),
            _ if id == live_id!(help_about) => Some(ShellAction::StatusMessage(format!(
                "Fab viewer {} — makepad · {} elements, {} triangles",
                env!("CARGO_PKG_VERSION"),
                state.scene.stats.elements,
                state.scene.stats.triangles
            ))),
            _ => None,
        }
    }
}

impl Widget for FabMenuBar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        match event {
            Event::MouseMove(e) => {
                let over = self.item_at(cx, e.abs);
                if over != self.hovered {
                    self.hovered = over;
                    self.sync(cx);
                }
                // Fab's menu tracking: with one menu open, crossing a
                // sibling opens that one, button down or not.
                if let (Some(i), Some(_)) = (over, self.open) {
                    if self.open != Some(i) {
                        self.raise(cx, scope, i);
                    }
                }
            }
            Event::MouseDown(e) if e.button.is_primary() => {
                if let Some(i) = self.item_at(cx, e.abs) {
                    if self.open == Some(i) {
                        self.open = None;
                        self.sync(cx);
                    } else {
                        self.raise(cx, scope, i);
                    }
                }
            }
            _ => {}
        }
        if let Event::Actions(actions) = event {
            let mut picked = None;
            let mut open = self.open;
            for a in ui_actions(actions) {
                match a {
                    FabUiAction::MenuPicked { owner: o, id } => {
                        if owner_index(*o).is_some() {
                            picked = Some(*id);
                        }
                    }
                    // The bar's `open` mirrors the layer's broadcast — the
                    // layer is the one place that knows a menu went down,
                    // whatever took it down (a pick, Escape, a click outside,
                    // another popup opening over it, focus loss), and which
                    // menu is up instead.
                    FabUiAction::MenuOpened { owner: o } => open = owner_index(*o),
                    FabUiAction::MenuClosed { owner: o } => {
                        if owner_index(*o) == open {
                            open = None;
                        }
                    }
                    _ => {}
                }
            }
            if open != self.open {
                self.open = open;
                self.sync(cx);
            }
            if let Some(id) = picked {
                let action = scope
                    .data
                    .get_mut::<AppState>()
                    .and_then(|state| action_for(state, id));
                if let Some(a) = action {
                    cx.action(a);
                }
            }
            if let Some(i) = self.open {
                if let Some(forward) = menu_cycle(actions, owner(i)) {
                    let next = if forward {
                        (i + 1) % MENU_COUNT
                    } else {
                        (i + MENU_COUNT - 1) % MENU_COUNT
                    };
                    self.raise(cx, scope, next);
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}
