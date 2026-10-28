//! The desktop's view of what a remote headset sees: [`XrMirrorView`], a widget an app places
//! where it likes (a dock tab beside its other panels; Sandbox draws the same mirror in its game
//! view, `XrServe::draw_mirror`). It shows the left eye of the app's XR pass while a headset
//! streams. A window showing it repaints from the headset's frames a few times a second (the
//! user's mirror switch on), else only with the window's own UI (`Cx::xr_hold_windows`); a
//! window without it never repaints for the headset.

use crate::serve::DrawXrMirror;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.XrMirrorViewBase = #(XrMirrorView::register_widget(vm))
    mod.widgets.XrMirrorView = set_type_default() do mod.widgets.XrMirrorViewBase {
        width: Fill height: Fill
        draw_bg +: { color: #x101214 }
        draw_text +: { color: theme.color_text_meta }
    }
}

/// The app's XR pass as the desktop mirrors it: its side-by-side colour and the streamed eye
/// size, set by `XrServe::draw` each XR frame; `drawn`: a mirror was drawn since.
#[derive(Default)]
pub(crate) struct XrMirrorSource {
    pub(crate) color: Option<Texture>,
    pub(crate) eye: [u32; 2],
    pub(crate) drawn: bool,
}

/// `rect`'s largest part of the eye's shape, centred.
pub(crate) fn fit(rect: Rect, eye: [u32; 2]) -> Rect {
    let aspect = eye[0].max(1) as f64 / eye[1].max(1) as f64;
    let (mut w, mut h) = (rect.size.x, rect.size.x / aspect);
    if h > rect.size.y {
        h = rect.size.y;
        w = h * aspect;
    }
    Rect { pos: rect.pos + (rect.size - dvec2(w, h)) * 0.5, size: dvec2(w, h) }
}

/// The headset's view on the desktop (see the module).
#[derive(Script, ScriptHook, Widget)]
pub struct XrMirrorView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_mirror: DrawXrMirror,
    #[live]
    draw_text: DrawText,
}

impl Widget for XrMirrorView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        #[cfg(not(target_arch = "wasm32"))]
        let live = makepad_desk_link::xr_host::session_eye().is_some();
        #[cfg(target_arch = "wasm32")]
        let live = false;
        let source = cx.cx.global::<XrMirrorSource>();
        match source.color.clone().filter(|_| live) {
            Some(color) => {
                source.drawn = true;
                let eye = source.eye;
                self.draw_mirror.draw_vars.set_texture(0, &color);
                self.draw_mirror.draw_abs(cx, fit(rect, eye));
            }
            None => {
                self.draw_text.draw_abs(cx, rect.pos + dvec2(16.0, 16.0), "No headset is streaming");
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // the headset's frames (at the mirror's rate on the desktop)
        if let Event::XrUpdate(_) = event {
            self.draw_bg.redraw(cx);
        }
    }
}
