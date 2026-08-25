//! Lane D. Status bar: mouse-button hints for the *current* context on the
//! left, the transient message in the middle, then what is under the pointer
//! and the frame stats on the right.
//!
//! The hint is derived here from the live tool and navigation mode rather than
//! read from `ui.status_hint`: a field only one place writes drifts out of
//! date the moment a mode changes, and Fab's hint line never lies about
//! what the buttons do right now. `StatusMessage` still shows through, because
//! the loader and the bake genuinely have something to say.

use crate::api::*;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    mod.widgets.FabStatusBarBase = #(FabStatusBar::register_widget(vm))
    mod.widgets.FabStatusBar = set_type_default() do mod.widgets.FabStatusBarBase{
        width: Fill
        height: fab.statusbar_height
        flow: Right
        align: Align{x: 0.0 y: 0.0}
        padding: Inset{left: 10 right: 10 top: 2 bottom: 2}
        spacing: 10
        show_bg: true
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                sdf.fill(fab.color_statusbar)
                sdf.rect(0.0, 0.0, self.rect_size.x, 1.0)
                sdf.fill(fab.color_border)
                return sdf.result
            }
        }
        hint := mod.widgets.FabLabelSmall{ height: Fill text: "" }
        Filler{}
        // `Icon` has no `visible` field — only View/Button do — so the
        // warning glyph rides in a View that can be hidden. No y-align:
        // DrawVector.
        warn := View{
            visible: false
            width: 12
            height: Fill
            padding: Inset{top: 4 bottom: 4 left: 0 right: 0}
            mod.widgets.FabIconSmall{
                width: 12
                height: 12
                icon_walk: Walk{ width: 12 height: 12 }
                draw_icon +: {
                    color: fab.color_warning
                    svg: crate_resource("self://resources/icons/warning.svg")
                }
            }
        }
        message := mod.widgets.FabLabelSmall{
            height: Fill
            text: ""
            draw_text +: { color: fab.color_text }
        }
        Filler{}
        picked := mod.widgets.FabLabelSmall{ height: Fill text: "" }
        stats := mod.widgets.FabLabelSmall{ height: Fill text: "" }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabStatusBar {
    #[deref]
    view: View,
}

/// The mouse-button hints for the context the app is in right now.
/// Bindings match `nav/mod.rs` (emulated three-button mouse): bare LMB drag
/// orbits, RMB pans, wheel zooms; W is WASD-forward once walk is entered
/// (`Shift+`` / the Walk tool).
fn hint_for(state: &AppState) -> String {
    let nav = state.view().nav_mode;
    match (nav, state.tool) {
        (NavMode::Walk, _) if state.ui.status_hint.starts_with("Walk ·") => {
            state.ui.status_hint.clone()
        }
        (NavMode::Walk, _) => format!(
            "Walk · {:.1} m · Esc to release",
            crate::nav::walk::EYE_HEIGHT
        ),
        (NavMode::Fly, _) if state.ui.status_hint.starts_with("Fly ·") => {
            state.ui.status_hint.clone()
        }
        (NavMode::Fly, _) => "Fly · Mouse look · Wheel speed · Esc to release".into(),
        (_, Tool::Measure(_)) => {
            "LMB Set point · Snap: vertex/edge/face · RMB Pan · Wheel Zoom · Esc Cancel".into()
        }
        (_, Tool::Section) => "LMB Cut on face · RMB Pan · Wheel Zoom · Esc Cancel".into(),
        (_, Tool::Walk) => "W Walk · LMB Orbit · RMB Pan · Wheel Zoom".into(),
        _ => "LMB Orbit · RMB Pan · Wheel Zoom · W Walk".into(),
    }
}

impl Widget for FabStatusBar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(state) = scope.data.get_mut::<AppState>() {
            let state: &AppState = state;
            self.view
                .label(cx, ids!(hint))
                .set_text(cx, &hint_for(state));
            self.view
                .label(cx, ids!(message))
                .set_text(cx, &state.ui.status_message);
            let failed = matches!(state.load, LoadStatus::Failed { .. });
            self.view.widget(cx, ids!(warn)).set_visible(cx, failed);

            let picked = match state.view().hover {
                Some(h) => match state.scene.element(h.element) {
                    Some(e) => {
                        let story = state
                            .scene
                            .story_of(e.id)
                            .map(|s| format!("{} › ", s.name))
                            .unwrap_or_default();
                        format!("{story}{}", e.name)
                    }
                    None => String::new(),
                },
                None => match state.scene_state.selection.active {
                    Some(id) => state
                        .scene
                        .element(id)
                        .map(|e| format!("[{}] {}", state.scene_state.selection.len(), e.name))
                        .unwrap_or_default(),
                    None => String::new(),
                },
            };
            self.view.label(cx, ids!(picked)).set_text(cx, &picked);
            let s = &state.stats;
            let stats = format!(
                "Elements {}  ·  Tris {}  ·  {:.0} fps  ·  {:.1} MB",
                state.scene.stats.elements,
                fmt_count(s.triangles_drawn),
                s.fps,
                s.gpu_bytes as f64 / 1e6
            );
            self.view.label(cx, ids!(stats)).set_text(cx, &stats);
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hint_is_real_bindings() {
        let state = AppState::default();
        let h = hint_for(&state);
        assert!(h.contains("LMB Orbit"), "hint={h}");
        assert!(h.contains("RMB Pan"), "hint={h}");
        assert!(h.contains("Wheel Zoom"), "hint={h}");
        assert!(h.contains("W Walk"), "hint={h}");
        assert!(!h.contains("MMB"), "stale Fab binding leaked: {h}");
    }
}

fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}
