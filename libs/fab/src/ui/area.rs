//! Lane D. The Fab **area**: one rectangle of the screen that can show any
//! editor, with the editor-type dropdown in its own header, and a corner grip
//! that splits or joins it.
//!
//! An area is a `PageFlip` with `lazy_init` — only the editor you are actually
//! looking at is instantiated, which is what makes it safe to let any of the
//! six areas become a 3D viewport without paying for six 3D passes.
//!
//! The header dropdown (`ui/dropdown.rs`) only reports that it was pressed and
//! where it is; the area catches that report on its way out (`Cx::map_actions`)
//! and raises the editor menu stamped with its own slot. That is why lane G's
//! Tours panel — whose Rust this lane may not touch — gets a working
//! editor-type dropdown for free, just by having put a `FabDropdownButton`
//! in its header.

use crate::ui::dropdown::*;
use crate::ui::popover::{open_menu, ui_actions, FabUiAction, MenuIcon, MenuItem, MenuPlace};
use makepad_widgets::*;

pub fn area_owner(slot: usize) -> LiveId {
    LiveId(0x6269_6d78_0001_0000 | slot as u64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EditorKind {
    #[default]
    Viewport,
    Outliner,
    Properties,
    Sheets,
    Tours,
    Info,
    Render,
}

impl EditorKind {
    pub const ALL: [EditorKind; 7] = [
        EditorKind::Viewport,
        EditorKind::Outliner,
        EditorKind::Properties,
        EditorKind::Sheets,
        EditorKind::Tours,
        EditorKind::Info,
        EditorKind::Render,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EditorKind::Viewport => "3D Viewport",
            EditorKind::Outliner => "Outliner",
            EditorKind::Properties => "Properties",
            EditorKind::Sheets => "Sheets",
            EditorKind::Tours => "Tours",
            EditorKind::Info => "Info",
            EditorKind::Render => "Render",
        }
    }

    pub fn page(self) -> LiveId {
        match self {
            EditorKind::Viewport => live_id!(Viewport),
            EditorKind::Outliner => live_id!(Outliner),
            EditorKind::Properties => live_id!(Properties),
            EditorKind::Sheets => live_id!(Sheets),
            EditorKind::Tours => live_id!(Tours),
            EditorKind::Info => live_id!(Info),
            EditorKind::Render => live_id!(Render),
        }
    }

    pub fn from_page(id: LiveId) -> EditorKind {
        EditorKind::ALL
            .iter()
            .copied()
            .find(|k| k.page() == id)
            .unwrap_or(EditorKind::Viewport)
    }

    fn menu_icon(self) -> MenuIcon {
        match self {
            EditorKind::Viewport => MenuIcon::Viewport,
            EditorKind::Outliner => MenuIcon::Outliner,
            EditorKind::Properties => MenuIcon::Properties,
            EditorKind::Sheets => MenuIcon::Sheets,
            EditorKind::Tours => MenuIcon::Tours,
            EditorKind::Info => MenuIcon::Info,
            EditorKind::Render => MenuIcon::Render,
        }
    }
}

/// The editor-type menu, with the current one ticked.
pub fn editor_menu(current: EditorKind) -> Vec<MenuItem> {
    EditorKind::ALL
        .iter()
        .map(|k| {
            let mut it = MenuItem::new(k.page(), k.label()).icon(k.menu_icon());
            if *k == current {
                it = it.icon(MenuIcon::Check);
            }
            it
        })
        .collect()
}

// ===========================================================================
// The area
// ===========================================================================

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    let SheetsEditor = View{
        width: Fill
        height: Fill
        flow: Down
        show_bg: true
        draw_bg +: { color: fab.color_editor }
        header := mod.widgets.FabAreaHeader{
            FabTip{ text: "Choose editor"
                editor_type := mod.widgets.FabDropdownButton{ label +: { text: "Sheets" } }
            }
            Filler{}
        }
        sheets := FabSheetView{}
    }

    let RenderEditor = View{
        width: Fill
        height: Fill
        flow: Down
        show_bg: true
        draw_bg +: { color: fab.color_editor }
        header := mod.widgets.FabAreaHeader{
            FabTip{ text: "Choose editor"
                editor_type := mod.widgets.FabDropdownButton{ label +: { text: "Render" } }
            }
            Filler{}
        }
        render := FabRenderView{}
    }

    mod.widgets.FabAreaBase = #(FabArea::register_widget(vm))
    mod.widgets.FabArea = set_type_default() do mod.widgets.FabAreaBase{
        width: Fill
        height: Fill
        flow: Overlay
        slot: 0
        view_index: 0
        editor: @Viewport
        pages := PageFlip{
            width: Fill
            height: Fill
            lazy_init: true
            active_page: @Viewport
            Viewport := FabViewportArea{}
            Outliner := FabOutliner{}
            Properties := FabProperties{}
            Sheets := SheetsEditor{}
            Tours := FabToursPanel{}
            Info := FabInfoPanel{}
            Render := RenderEditor{}
        }
        // Corner split/join grip. Pushed with fillers, never far-edge align.
        grip_col := View{
            width: Fill
            height: Fill
            flow: Down
            Filler{}
            grip_row := View{
                width: Fill
                height: Fit
                flow: Right
                Filler{}
                grip := View{
                    width: fab.corner_zone
                    height: fab.corner_zone
                    cursor: MouseCursor.Crosshair
                    show_bg: true
                    draw_bg +: {
                        hover: instance(0.0)
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            let w = self.rect_size.x
                            let h = self.rect_size.y
                            let c = fab.color_border_light.mix(fab.color_accent_hover, self.hover)
                            sdf.move_to(w - 1.5, h - 8.5)
                            sdf.line_to(w - 8.5, h - 1.5)
                            sdf.stroke(c, 1.0)
                            sdf.move_to(w - 1.5, h - 4.5)
                            sdf.line_to(w - 4.5, h - 1.5)
                            sdf.stroke(c, 1.0)
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
                    }
                }
            }
        }
    }
}

/// What the corner grip asked the shell to do. The shell owns the Dock tree,
/// so the area only reports the gesture.
#[derive(Debug)]
pub enum AreaAction {
    /// Split this slot; `vertical` = a horizontal drag made two side-by-side
    /// areas, otherwise stacked.
    Split { slot: usize, vertical: bool },
    /// Join this slot away (drag inwards).
    Join { slot: usize },
    /// The pointer entered this area — it becomes the active one.
    Focused { slot: usize, view_index: usize },
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabArea {
    #[deref]
    view: View,
    #[live(0)]
    slot: usize,
    #[live(0)]
    view_index: usize,
    #[live]
    editor: LiveId,
    #[rust]
    applied: Option<LiveId>,
    #[rust]
    grip_from: Option<DVec2>,
}

impl FabArea {
    pub fn kind(&self) -> EditorKind {
        EditorKind::from_page(self.editor)
    }
}

impl Widget for FabArea {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let owner = area_owner(self.slot);
        // An editor-type press that came out of THIS area is this area's: turn
        // it into a menu request stamped with our own slot before anyone else
        // sees the pass.
        let mut anchor = None;
        cx.map_actions(
            |cx| self.view.handle_event(cx, event, scope),
            |_cx, buf| {
                for a in buf.iter() {
                    if let Some(FabUiAction::DropdownClicked { tag, anchor: r }) =
                        a.downcast_ref::<FabUiAction>()
                    {
                        if *tag == live_id!(editor_type) {
                            anchor = Some(*r);
                        }
                    }
                }
                buf
            },
        );
        if let Some(anchor) = anchor {
            let items = editor_menu(self.kind());
            open_menu(cx, owner, items, anchor, MenuPlace::Below);
        }

        if let Event::Actions(actions) = event {
            for a in ui_actions(actions) {
                if let FabUiAction::MenuPicked { owner: o, id } = a {
                    if *o == owner {
                        self.editor = *id;
                        self.applied = None;
                        self.view.redraw(cx);
                    }
                }
            }
            // Corner grip: a short drag out of the corner splits, a drag into
            // the area joins it away.
            let grip = self.view.view(cx, ids!(grip_col.grip_row.grip));
            if let Some(fd) = grip.finger_down(actions) {
                self.grip_from = Some(fd.abs);
            }
            if let Some(fu) = grip.finger_up(actions) {
                if let Some(from) = self.grip_from.take() {
                    let d = fu.abs - from;
                    if d.x.abs() > 12.0 || d.y.abs() > 12.0 {
                        let inward = d.x < 0.0 && d.y < 0.0;
                        if inward {
                            cx.action(AreaAction::Join { slot: self.slot });
                        } else {
                            cx.action(AreaAction::Split {
                                slot: self.slot,
                                vertical: d.x.abs() >= d.y.abs(),
                            });
                        }
                    }
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.applied != Some(self.editor) {
            self.applied = Some(self.editor);
            let page = self.editor;
            let idx = self.view_index;
            let pages = self.view.page_flip(cx, ids!(pages));
            if let Some(w) = pages.set_active_page(cx, page) {
                if page == live_id!(Viewport) {
                    let mut w = w;
                    script_apply_eval!(cx, w, { view_index: #(idx) });
                }
            }
            // Keep the header dropdown honest: its label, and the per-slot
            // menu-owner id it mirrors the open look for. Only the live
            // pages resolve; the rest are empty refs and no-op.
            let label = EditorKind::from_page(page).label();
            let owner = area_owner(self.slot);
            for path in [
                ids!(pages.Viewport.header.editor_type),
                ids!(pages.Outliner.header.editor_type),
                ids!(pages.Properties.header.editor_type),
                ids!(pages.Sheets.header.editor_type),
                ids!(pages.Tours.header.editor_type),
                ids!(pages.Info.header.editor_type),
                ids!(pages.Render.header.editor_type),
            ] {
                self.view.fab_dropdown_button(cx, path).set_owner(owner);
            }
            for path in [
                ids!(pages.Viewport.header.editor_type.label),
                ids!(pages.Outliner.header.editor_type.label),
                ids!(pages.Properties.header.editor_type.label),
                ids!(pages.Sheets.header.editor_type.label),
                ids!(pages.Tours.header.editor_type.label),
                ids!(pages.Info.header.editor_type.label),
                ids!(pages.Render.header.editor_type.label),
            ] {
                self.view.label(cx, path).set_text(cx, label);
            }
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

/// Read the corner-grip gestures out of an actions pass.
pub fn area_actions(actions: &Actions) -> impl Iterator<Item = &AreaAction> {
    actions.iter().filter_map(|a| a.downcast_ref::<AreaAction>())
}
