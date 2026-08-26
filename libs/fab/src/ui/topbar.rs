//! Lane D. Top bar: app mark, the menu bar (`menubar.rs` — File / Edit /
//! View / Window / Help, a real one), workspace tabs (Layout / Walkthrough /
//! Sections / Sheets / Sun Study / Render), scene name. Workspace tabs emit
//! `ShellAction::SetWorkspace`; everything menu-shaped belongs to
//! `FabMenuBar`, which raises menus through the shared `FabMenuLayer`.

use crate::api::*;
use crate::ui::menubar::{
    action_for as menubar_action_for, edit_menu, file_menu, help_menu, view_menu, window_menu,
};
use crate::ui::popover::{dropdown_clicked, menu_picked, open_menu, MenuItem, MenuPlace};
use crate::ui::widgets::{
    clamp_header_pan, clipped_header_controls, HeaderControlSpan,
};
use makepad_widgets::*;

const OVERFLOW_OWNER: LiveId = live_id!(fab_top_overflow);
const OVERFLOW_PARENT_BASE: u64 = 0x6269_6d78_0011_0100;
const OVERFLOW_WORKSPACE_BASE: u64 = 0x6269_6d78_0011_0200;

const TOP_CONTROL_COUNT: usize = 13;

fn top_control_path(index: usize) -> &'static [LiveId] {
    match index {
        0 => ids!(scroller.content.mark),
        1 => ids!(scroller.content.menubar.m_file),
        2 => ids!(scroller.content.menubar.m_edit),
        3 => ids!(scroller.content.menubar.m_view),
        4 => ids!(scroller.content.menubar.m_window),
        5 => ids!(scroller.content.menubar.m_help),
        6 => ids!(scroller.content.workspaces.ws_quad),
        7 => ids!(scroller.content.workspaces.ws_walk),
        8 => ids!(scroller.content.workspaces.ws_sections),
        9 => ids!(scroller.content.workspaces.ws_sheets),
        10 => ids!(scroller.content.workspaces.ws_sun),
        11 => ids!(scroller.content.workspaces.ws_render),
        _ => ids!(scroller.content.scene_name),
    }
}

fn overflow_workspace_index(id: LiveId) -> Option<usize> {
    (id.0 >= OVERFLOW_WORKSPACE_BASE
        && id.0 < OVERFLOW_WORKSPACE_BASE + Workspace::ALL.len() as u64)
        .then(|| (id.0 - OVERFLOW_WORKSPACE_BASE) as usize)
}

fn scene_label(state: &AppState) -> String {
    if state.scene.name.is_empty() {
        "No model".to_string()
    } else {
        format!(
            "{} — {} elements, {} tris",
            state.scene.name, state.scene.stats.elements, state.scene.stats.triangles
        )
    }
}

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    mod.widgets.FabTopBarBase = #(FabTopBar::register_widget(vm))
    mod.widgets.FabTopBar = set_type_default() do mod.widgets.FabTopBarBase{
        width: Fill
        height: fab.topbar_height
        flow: Overlay
        // 3 px pad + one shared row height. Fit-height labels are centred;
        // the logo stays in its Fill-height, symmetrically padded slot.
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 8 right: 8 top: 3 bottom: 3}
        show_bg: true
        draw_bg +: {
            color: fab.color_topbar
        }
        scroller := View{
            width: Fill
            height: Fill
            flow: Right
            clip_x: true
            clip_y: true
            margin: Inset{right: 24}
            content := View{
                width: Fill
                height: Fill
                flow: Right
                clip_x: false
                align: Align{x: 0.0 y: 0.5}
                spacing: 2
                mark := View{
                    width: Fit
                    height: Fill
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    spacing: 6
                    margin: Inset{right: 8}
                    logo_slot := View{
                        width: fab.icon_size
                        height: Fill
                        padding: Inset{top: 2 bottom: 2 left: 0 right: 0}
                        logo := FabIcon{
                            draw_icon +: {
                                svg: crate_resource("self://resources/icons/fab.svg")
                                color: fab.color_accent_hover
                            }
                        }
                    }
                    FabHeaderLabel{ height: Fit text: "Fab" }
                }
                menubar := FabMenuBar{}
                FabVr{ height: Fill margin: Inset{left: 8 right: 8} }
                workspaces := View{
                    width: Fit
                    height: Fill
                    flow: Right
                    spacing: 1
                    FabTip{ text: "Open Quad workspace"
                        ws_quad := FabSegmentTab{ text: "Quad" }
                    }
                    FabTip{ text: "Open Walkthrough workspace"
                        ws_walk := FabSegmentTab{ text: "Walkthrough" }
                    }
                    FabTip{ text: "Open Sections workspace"
                        ws_sections := FabSegmentTab{ text: "Sections" }
                    }
                    FabTip{ text: "Open Sheets workspace"
                        ws_sheets := FabSegmentTab{ text: "Sheets" }
                    }
                    FabTip{ text: "Open Sun Study workspace"
                        ws_sun := FabSegmentTab{ text: "Sun Study" }
                    }
                    FabTip{ text: "Open Render workspace"
                        ws_render := FabSegmentTab{ text: "Render" }
                    }
                }
                Filler{}
                scene_name := FabLabelDim{ height: Fit text: "No model" }
            }
        }
        overflow_pin := View{
            width: Fill
            height: Fill
            flow: Right
            align: Align{x: 0.0 y: 0.5}
            Filler{}
            FabTip{ text: "Show controls outside the visible header"
                overflow := FabMenuButton{
                    visible: false
                    width: 22
                    tag: @top_overflow
                    owner: @fab_top_overflow
                    label +: { text: "⋯" }
                    padding: Inset{left: 4 right: 4 top: 0 bottom: 0}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabTopBar {
    #[deref]
    view: View,
    #[rust]
    synced_workspace: Option<Workspace>,
    #[rust]
    header_pan: f64,
    #[rust]
    header_content_width: f64,
    #[rust]
    header_visible_width: f64,
    #[rust]
    pan_drag: Option<(f64, f64)>,
    #[rust]
    clipped_controls: Vec<usize>,
}

impl FabTopBar {
    fn set_header_pan(&mut self, cx: &mut Cx, pan: f64) -> bool {
        let pan = clamp_header_pan(
            pan,
            self.header_content_width,
            self.header_visible_width,
        );
        if (pan - self.header_pan).abs() <= f64::EPSILON {
            return false;
        }
        self.header_pan = pan;
        self.view
            .view(cx, ids!(scroller))
            .set_scroll_pos(cx, dvec2(pan, 0.0));
        self.view.redraw(cx);
        true
    }

    fn update_header_metrics(&mut self, cx: &mut Cx) {
        let viewport = self.view.view(cx, ids!(scroller)).area().rect(cx);
        if viewport.size.x <= 0.0 {
            return;
        }
        let mut controls = Vec::new();
        for index in 0..TOP_CONTROL_COUNT {
            let rect = self.view.widget(cx, top_control_path(index)).area().rect(cx);
            if rect.size.x > 0.1 {
                controls.push((
                    index,
                    HeaderControlSpan {
                        start: rect.pos.x - viewport.pos.x + self.header_pan,
                        end: rect.pos.x + rect.size.x - viewport.pos.x + self.header_pan,
                    },
                ));
            }
        }
        self.header_visible_width = viewport.size.x;
        self.header_content_width = controls
            .iter()
            .map(|(_, span)| span.end)
            .fold(viewport.size.x, f64::max);

        let clamped = clamp_header_pan(
            self.header_pan,
            self.header_content_width,
            self.header_visible_width,
        );
        if (clamped - self.header_pan).abs() > f64::EPSILON {
            self.header_pan = clamped;
            self.view
                .view(cx, ids!(scroller))
                .set_scroll_pos(cx, dvec2(clamped, 0.0));
            self.view.redraw(cx);
        }

        let spans: Vec<HeaderControlSpan> = controls.iter().map(|(_, span)| *span).collect();
        let clipped = clipped_header_controls(
            &spans,
            self.header_visible_width,
            self.header_pan,
        )
        .into_iter()
        .map(|index| controls[index].0)
        .collect::<Vec<_>>();
        if clipped != self.clipped_controls {
            self.clipped_controls = clipped;
            self.view
                .widget(cx, ids!(overflow_pin.overflow))
                .set_visible(cx, !self.clipped_controls.is_empty());
            self.view.redraw(cx);
        }
    }

    fn overflow_menu(&self, state: &AppState) -> Vec<MenuItem> {
        self.clipped_controls
            .iter()
            .map(|index| match *index {
                0 => MenuItem::new(LiveId(OVERFLOW_PARENT_BASE), "Fab").disabled(),
                1 => MenuItem::new(LiveId(OVERFLOW_PARENT_BASE + 1), "File")
                    .flyout(file_menu(state)),
                2 => MenuItem::new(LiveId(OVERFLOW_PARENT_BASE + 2), "Edit")
                    .flyout(edit_menu(state)),
                3 => MenuItem::new(LiveId(OVERFLOW_PARENT_BASE + 3), "View")
                    .flyout(view_menu(state)),
                4 => MenuItem::new(LiveId(OVERFLOW_PARENT_BASE + 4), "Window")
                    .flyout(window_menu(state)),
                5 => MenuItem::new(LiveId(OVERFLOW_PARENT_BASE + 5), "Help")
                    .flyout(help_menu()),
                6..=11 => {
                    let workspace = Workspace::ALL[*index - 6];
                    MenuItem::new(
                        LiveId(OVERFLOW_WORKSPACE_BASE + (*index - 6) as u64),
                        workspace.label(),
                    )
                    .radio(state.ui.workspace == workspace)
                }
                _ => MenuItem::new(
                    LiveId(OVERFLOW_PARENT_BASE + 12),
                    &scene_label(state),
                )
                .disabled(),
            })
            .collect()
    }
}

impl Widget for FabTopBar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let viewport = self.view.view(cx, ids!(scroller)).area().rect(cx);
        match event {
            Event::Scroll(e) if viewport.contains(e.abs) => {
                let delta = e.scroll.x + e.scroll.y;
                if delta.abs() > f64::EPSILON
                    && self.set_header_pan(cx, self.header_pan + delta)
                {
                    e.handled_x.set(true);
                    e.handled_y.set(true);
                }
            }
            Event::MouseDown(e) if e.button.is_middle() && viewport.contains(e.abs) => {
                self.pan_drag = Some((e.abs.x, self.header_pan));
            }
            Event::MouseMove(e) => {
                if let Some((start_x, start_pan)) = self.pan_drag {
                    self.set_header_pan(cx, start_pan - (e.abs.x - start_x));
                }
            }
            Event::MouseUp(e) if e.button.is_middle() => self.pan_drag = None,
            _ => {}
        }
        if let Event::Actions(actions) = event {
            let set = self.view.radio_button_set(
                cx,
                ids_array!(
                    scroller.content.workspaces.ws_quad,
                    scroller.content.workspaces.ws_walk,
                    scroller.content.workspaces.ws_sections,
                    scroller.content.workspaces.ws_sheets,
                    scroller.content.workspaces.ws_sun,
                    scroller.content.workspaces.ws_render,
                ),
            );
            if let Some(i) = set.selected(cx, actions) {
                cx.action(ShellAction::SetWorkspace(Workspace::ALL[i.min(5)]));
            }
            if let Some(anchor) = dropdown_clicked(actions, live_id!(top_overflow)) {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    open_menu(
                        cx,
                        OVERFLOW_OWNER,
                        self.overflow_menu(state),
                        anchor,
                        MenuPlace::BelowRight,
                    );
                }
            }
            if let Some(pick) = menu_picked(actions, OVERFLOW_OWNER) {
                if let Some(index) = overflow_workspace_index(pick) {
                    cx.action(ShellAction::SetWorkspace(Workspace::ALL[index]));
                } else if let Some(action) = scope
                    .data
                    .get_mut::<AppState>()
                    .and_then(|state| menubar_action_for(state, pick))
                {
                    cx.action(action);
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(state) = scope.data.get_mut::<AppState>() {
            self.view
                .label(cx, ids!(scroller.content.scene_name))
                .set_text(cx, &scene_label(state));
            if self.synced_workspace != Some(state.ui.workspace) {
                self.synced_workspace = Some(state.ui.workspace);
                let ids = [
                    ids!(scroller.content.workspaces.ws_quad),
                    ids!(scroller.content.workspaces.ws_walk),
                    ids!(scroller.content.workspaces.ws_sections),
                    ids!(scroller.content.workspaces.ws_sheets),
                    ids!(scroller.content.workspaces.ws_sun),
                    ids!(scroller.content.workspaces.ws_render),
                ];
                for (i, id) in ids.iter().enumerate() {
                    let on = Workspace::ALL[i] == state.ui.workspace;
                    self.view.radio_button(cx, *id).set_active(cx, on, Animate::No);
                }
            }
        }
        self.view
            .view(cx, ids!(scroller))
            .set_scroll_pos(cx, dvec2(self.header_pan, 0.0));
        let step = self.view.draw_walk(cx, scope, walk);
        if step.is_ok() {
            self.update_header_metrics(cx);
        }
        step
    }
}
