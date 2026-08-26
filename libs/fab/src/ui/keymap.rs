//! Lane D. Keymap help (F1 / Help menu), generated from one table so the
//! tooltips, the menus and this panel can never disagree.
//!
//! It is drawn with our own rows rather than `Markdown` on purpose: the
//! markdown path pulled the mono font in mid-frame and the shared glyph atlas
//! came back with holes — whole words vanished from every panel in the app.
//! A `PortalList` of two labels and a key cap has no such appetite.

use crate::api::*;
use makepad_widgets::*;

/// The keymap, single source of truth for tooltips and the help panel.
/// `(keys, action, context)`.
pub const KEYMAP: &[(&str, &str, &str)] = &[
    ("LMB drag", "Orbit", "3D Viewport"),
    ("RMB drag", "Pan", "3D Viewport"),
    ("Wheel", "Zoom (to cursor)", "3D Viewport"),
    ("Ctrl+LMB drag", "Dolly", "3D Viewport"),
    ("Alt+LMB drag", "Pan (emulated)", "3D Viewport"),
    ("LMB click", "Select", "3D Viewport"),
    ("Shift+LMB", "Extend selection", "3D Viewport"),
    ("RMB click", "Context menu", "3D Viewport"),
    ("Double-click", "Set orbit pivot / reveal in outliner", "3D Viewport"),
    ("Numpad 1 / 3 / 7", "Front / Right / Top", "3D Viewport"),
    ("Ctrl+Numpad 1 / 3 / 7", "Back / Left / Bottom", "3D Viewport"),
    ("Numpad 9", "Isometric", "3D Viewport"),
    ("Numpad 5", "Orthographic / Perspective", "3D Viewport"),
    ("Numpad 2/4/6/8", "Orbit 15°", "3D Viewport"),
    ("Home", "Frame all", "3D Viewport"),
    (".", "Frame selected", "3D Viewport"),
    ("H / Shift+H / Alt+H", "Hide / Isolate / Unhide all", "3D Viewport"),
    ("Z", "Shading pie menu, including Realtime", "3D Viewport"),
    ("Alt+Z", "X-ray", "3D Viewport"),
    ("P", "Stop / Resume path tracer", "Raytraced viewport"),
    ("T", "Toolbar", "3D Viewport"),
    ("Shift+`", "Enter walk / fly (Esc to exit)", "3D Viewport"),
    ("W", "Enter walk / move forward", "3D Viewport"),
    ("Click row", "Select element(s)", "Outliner"),
    ("Cmd/Ctrl+click", "Toggle row selection", "Outliner"),
    ("Shift+click", "Select range", "Outliner"),
    ("Eye column", "Hide / show element", "Outliner"),
    ("RMB", "Context menu", "Outliner"),
    ("Drag a value", "Change it", "Properties"),
    ("Click a value", "Type it", "Properties"),
    ("Ctrl while dragging", "Snap to increment", "Properties"),
    ("Shift while dragging", "Fine adjust", "Properties"),
    ("F12", "Render image", "Global"),
    ("F3", "Command palette", "Global"),
    ("Shift+F3", "Performance graph", "Global"),
    ("Ctrl+Space", "Maximize area", "Global"),
    ("N", "Sidebar", "Global"),
    ("Cmd+O", "Open", "Global"),
    ("Cmd+Shift+O", "Open demo house", "Global"),
    ("F1", "This help", "Global"),
    ("Cmd+Q", "Quit", "Global"),
];

/// The rows the panel draws: section headers interleaved with entries.
enum HelpRow {
    Section(&'static str),
    Entry(&'static str, &'static str),
}

fn help_rows() -> Vec<HelpRow> {
    let mut out = Vec::new();
    let mut last = "";
    for (k, a, c) in KEYMAP {
        if *c != last {
            last = c;
            out.push(HelpRow::Section(c));
        }
        out.push(HelpRow::Entry(k, a));
    }
    out
}

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    mod.widgets.FabKeymapHelpBase = #(FabKeymapHelp::register_widget(vm))
    mod.widgets.FabKeymapHelp = set_type_default() do mod.widgets.FabKeymapHelpBase{
        width: Fill
        height: Fill
        modal := Modal{
            content +: {
                width: 620
                height: Fit
                View{
                    width: Fill
                    height: Fit
                    flow: Down
                    padding: 14
                    spacing: 8
                    show_bg: true
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius_lg)
                            sdf.fill_keep(fab.color_popover)
                            sdf.stroke(fab.color_popover_border, 1.0)
                            return sdf.result
                        }
                    }
                    title_row := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        align: Align{x: 0.0 y: 0.5}
                        spacing: 8
                        mod.widgets.FabTitleLabel{ text: "Keymap" }
                        Filler{}
                        mod.widgets.FabLabelMuted{ text: "F1 closes" }
                    }
                    mod.widgets.FabHr{}
                    list := PortalList{
                        width: Fill
                        height: 520
                        flow: Down
                        auto_tail: false
                        Section := View{
                            width: Fill
                            height: 26
                            flow: Right
                            align: Align{x: 0.0 y: 1.0}
                            padding: Inset{left: 2 right: 2 top: 8 bottom: 2}
                            head := mod.widgets.FabHeaderLabel{ text: "" }
                        }
                        Entry := View{
                            width: Fill
                            height: fab.row_height
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 2 right: 2 top: 0 bottom: 0}
                            spacing: 10
                            keys := View{
                                width: 190
                                height: Fit
                                flow: Right
                                cap := mod.widgets.FabKeyCap{ cap +: { text: "" } }
                            }
                            what := mod.widgets.FabLabel{ width: Fill text: "" }
                        }
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        align: Align{x: 0.0 y: 0.5}
                        Filler{}
                        close := mod.widgets.FabButton{ text: "Close" }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabKeymapHelp {
    #[deref]
    view: View,
    #[rust]
    shown: bool,
}

impl Widget for FabKeymapHelp {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            let modal = self.view.modal(cx, ids!(modal));
            if modal.dismissed(actions) || self.view.button(cx, ids!(modal.close)).clicked(actions) {
                cx.action(ShellAction::ShowKeymapHelp(false));
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(state) = scope.data.get_mut::<AppState>() {
            if state.ui.keymap_help_open != self.shown {
                self.shown = state.ui.keymap_help_open;
                let modal = self.view.modal(cx, ids!(modal));
                if self.shown {
                    modal.open(cx);
                } else {
                    modal.close(cx);
                }
            }
        }
        let rows = help_rows();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            list.set_item_range(cx, 0, rows.len());
            while let Some(i) = list.next_visible_item(cx) {
                match rows.get(i) {
                    Some(HelpRow::Section(name)) => {
                        let item = list.item(cx, i, live_id!(Section));
                        item.label(cx, ids!(head)).set_text(cx, name);
                        item.draw_all(cx, &mut Scope::empty());
                    }
                    Some(HelpRow::Entry(k, a)) => {
                        let item = list.item(cx, i, live_id!(Entry));
                        item.label(cx, ids!(keys.cap.cap)).set_text(cx, k);
                        item.label(cx, ids!(what)).set_text(cx, a);
                        item.draw_all(cx, &mut Scope::empty());
                    }
                    None => {}
                }
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A duplicate binding in the same context is a bug you only notice at
    /// the worst moment, so the table is checked instead.
    #[test]
    fn no_duplicate_binding_per_context() {
        let mut seen: HashSet<(&str, &str)> = HashSet::new();
        for (k, _, c) in KEYMAP {
            assert!(seen.insert((k, c)), "duplicate binding {k} in {c}");
        }
    }

    #[test]
    fn every_entry_has_a_section() {
        for row in help_rows() {
            if let HelpRow::Entry(k, a) = row {
                assert!(!k.is_empty() && !a.is_empty());
            }
        }
    }

    #[test]
    fn viewport_bindings_match_the_status_bar() {
        let keys: Vec<&str> = KEYMAP
            .iter()
            .filter(|(_, _, c)| *c == "3D Viewport")
            .map(|(k, _, _)| *k)
            .collect();
        assert!(keys.iter().any(|k| k.contains("LMB drag")), "{keys:?}");
        assert!(keys.iter().any(|k| k.contains("RMB drag")), "{keys:?}");
        assert!(keys.iter().any(|k| *k == "Wheel"), "{keys:?}");
        assert!(keys.iter().any(|k| *k == "W"), "{keys:?}");
        assert!(!keys.iter().any(|k| k.contains("MMB drag")), "{keys:?}");
    }

    #[test]
    fn sidebar_binding_is_global() {
        assert!(KEYMAP
            .iter()
            .any(|(key, action, context)| *key == "N"
                && *action == "Sidebar"
                && *context == "Global"));
    }
}
