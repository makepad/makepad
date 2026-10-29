//! The desktop's view of what a remote headset sees: [`XrMirrorView`], a widget an app places
//! where it likes (a dock tab beside its other panels), or [`DrawXrMirror::draw_headset`] in an
//! app's own view (Sandbox draws it in its game view while a headset streams). It shows the left
//! eye of the app's XR pass. A window showing it repaints from the headset's frames a few times
//! a second (the user's mirror switch on), else only with the window's own UI
//! (`Cx::xr_hold_windows`); a window without it never repaints for the headset.

use makepad_widgets::*;

/// The mirror's shader (its own script module: the draw prelude).
pub(crate) mod shader {
    use makepad_widgets::*;

    script_mod! {
        use mod.pod.*
        use mod.math.*
        use mod.shader.*
        use mod.draw

        // The desktop's mirror of what streams: the left eye's half of the side-by-side colour
        // (premultiplied on black: the room shows black where nothing was drawn).
        mod.draw.DrawXrMirror = mod.std.set_type_default() do #(DrawXrMirror::script_shader(vm)){
            ..mod.draw.DrawQuad
            image: texture_2d(float)
            pixel: fn() {
                let c = self.image.sample(vec2(self.pos.x * 0.5, self.pos.y))
                return vec4(c.xyz + vec3(0.04, 0.045, 0.05) * (1.0 - c.w), 1.0)
            }
        }
    }

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct DrawXrMirror {
        #[deref]
        pub draw_super: DrawQuad,
    }
}

pub use shader::DrawXrMirror;

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
/// size, set by the server's pack each XR frame (None: no headset streams); `drawn`: a mirror
/// was drawn since.
#[derive(Default)]
pub(crate) struct XrMirrorSource {
    pub(crate) color: Option<Texture>,
    pub(crate) eye: [u32; 2],
    pub(crate) live: bool,
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

impl DrawXrMirror {
    /// The headset's left eye fitted into `rect`; false (nothing drawn) while no headset
    /// streams.
    pub fn draw_headset(&mut self, cx: &mut Cx2d, rect: Rect) -> bool {
        let source = cx.cx.global::<XrMirrorSource>();
        let Some(color) = source.color.clone().filter(|_| source.live) else { return false };
        source.drawn = true;
        let eye = if source.eye == [0, 0] { [1680, 1760] } else { source.eye };
        self.draw_vars.set_texture(0, &color);
        self.draw_abs(cx, fit(rect, eye));
        true
    }
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
        if !self.draw_mirror.draw_headset(cx, rect) {
            self.draw_text.draw_abs(cx, rect.pos + dvec2(16.0, 16.0), "No headset is streaming");
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
