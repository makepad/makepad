//! The desktop's view of what a remote headset sees: [`XrMirrorView`], a widget an app places
//! where it likes (a dock tab beside its other panels), or [`DrawXrMirror::draw_headset`] in an
//! app's own view (Sandbox draws it in its game view while a headset streams). It shows the left
//! eye of the app's XR pass, repainted on its own clock ([`XrMirrorClock`], a few times a second)
//! while it is shown, a headset streams and the user's mirror switch is on. Nothing else on the
//! desktop renders for the headset: a mirror that is not shown mirrors nothing.

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
/// size, set by the server's pack each XR frame; `live`: a headset's frames flow; `mirror`: the
/// user's mirror switch.
#[derive(Default)]
pub(crate) struct XrMirrorSource {
    pub(crate) color: Option<Texture>,
    pub(crate) eye: [u32; 2],
    pub(crate) live: bool,
    pub(crate) mirror: bool,
}

/// A desktop mirror's own clock: it repaints at [`XrMirrorClock::RATE_HZ`] on a timer (never on
/// the headset's frames), only while it is shown (drawn lately), a headset streams and the
/// user's mirror switch is on. A widget drawing a mirror calls [`Self::drawn`] from its draw
/// and redraws when [`Self::tick`] says so.
#[derive(Default)]
pub struct XrMirrorClock {
    timer: Timer,
    drawn_at: f64,
}

impl XrMirrorClock {
    /// The mirror's repaints a second.
    pub const RATE_HZ: f64 = 8.0;

    fn wanted(cx: &mut Cx) -> bool {
        let s = cx.global::<XrMirrorSource>();
        s.live && s.mirror
    }

    /// The mirror was drawn (shown) now: its clock runs while it is wanted.
    pub fn drawn(&mut self, cx: &mut Cx) {
        self.drawn_at = cx.seconds_since_app_start();
        if self.timer.is_empty() && Self::wanted(cx) {
            self.timer = cx.start_interval(1.0 / Self::RATE_HZ);
        }
    }

    /// Whether the mirror should redraw for this event (its clock's tick while it is shown and
    /// wanted; otherwise the clock stops).
    pub fn tick(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if self.timer.is_event(event).is_none() {
            return false;
        }
        let shown = cx.seconds_since_app_start() - self.drawn_at < 3.0 / Self::RATE_HZ;
        if shown && Self::wanted(cx) {
            return true;
        }
        cx.stop_timer(self.timer);
        self.timer = Timer::empty();
        false
    }
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
        let Some(color) = source.color.clone().filter(|_| source.live && source.mirror) else { return false };
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
    #[rust]
    clock: XrMirrorClock,
}

impl Widget for XrMirrorView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        if !self.draw_mirror.draw_headset(cx, rect) {
            self.draw_text.draw_abs(cx, rect.pos + dvec2(16.0, 16.0), "No headset is streaming (or the mirror is off)");
        }
        self.clock.drawn(cx.cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // the headset's view, on the mirror's own clock while shown
        if self.clock.tick(cx, event) {
            self.draw_bg.redraw(cx);
        }
    }
}
