//! The desktop as a hardware-encodable picture, drawn by the WM itself while a viewer watches.
//!
//! `DeskCapture` is the main window's body. With no capture wanted it draws its children as any
//! view does. While one is wanted (`CaptureState::want`) its frame is two passes chained so
//! that their parent links are also their execution order and their repaint propagation:
//!
//! ```text
//! window pass   C as one quad, then the window's cursor list (LAST tier)
//!  └ picture    w/4 x 3h/2 shared BGRA8: NV12 packed (makepad_video's nv12_picture layout):
//!     │         rows 0..h four luma samples per texel (DrawNv12Luma), rows h..3h/2
//!     │         U0 V0 U1 V1 per texel, 2x2 averages of C (DrawNv12Chroma); for 4:4:4
//!     │         (`Want::full_chroma`) w/4 x 3h: each chroma row the U V pairs of every pixel
//!     │         of one source row, over two texture rows
//!     └ C       the whole desktop, every overlay and popup of it (nested), no cursor
//! ```
//!
//! C repaints when the desktop changes, which repaints the picture (`CaptureState::painted`):
//! the WM then hands the picture texture, with the fence of the frame that rendered it, to its
//! stream encoder. A cursor move repaints only the window pass: no picture. Colour is BT.709
//! limited range.

use crate::dock_warp::WindowFrame;
use makepad_widgets::makepad_draw::render_target::{RenderTarget, RenderTargetFormat};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*

    // Four luma samples of row j, columns 4i..4i+3 of the source rect, in memory order B G R A.
    mod.widgets.DrawNv12Luma = set_type_default() do #(DrawNv12Luma::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        source: vec4(0.0, 0.0, 1.0, 1.0)
        extent: vec2(1.0, 1.0)
        pixel: fn() {
            let t = floor(self.pos * self.rect_size)
            let inv = vec2(1.0 / self.extent.x, 1.0 / self.extent.y)
            let row = self.source.y + t.y + 0.5
            let x = self.source.x + t.x * 4.0 + 0.5
            let w = vec3(0.2126, 0.7152, 0.0722)
            let y0 = (16.0 + 219.0 * dot(self.image.sample(vec2(x, row) * inv).xyz, w)) / 255.0
            let y1 = (16.0 + 219.0 * dot(self.image.sample(vec2(x + 1.0, row) * inv).xyz, w)) / 255.0
            let y2 = (16.0 + 219.0 * dot(self.image.sample(vec2(x + 2.0, row) * inv).xyz, w)) / 255.0
            let y3 = (16.0 + 219.0 * dot(self.image.sample(vec2(x + 3.0, row) * inv).xyz, w)) / 255.0
            return vec4(y2, y1, y0, y3)
        }
    }

    // Two chroma samples, memory order U0 V0 U1 V1: 4:2:0 each the average of a 2x2 block (one
    // bilinear tap at its centre); 4:4:4 (`full`) two neighbouring pixels of one row as they
    // are, a source row over two texture rows (its left half, then its right half).
    mod.widgets.DrawNv12Chroma = set_type_default() do #(DrawNv12Chroma::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        source: vec4(0.0, 0.0, 1.0, 1.0)
        extent: vec2(1.0, 1.0)
        full: 0.0
        pixel: fn() {
            let t = floor(self.pos * self.rect_size)
            let inv = vec2(1.0 / self.extent.x, 1.0 / self.extent.y)
            // which half of its source row this texture row holds (Splash shaders have no `mod`)
            let half = t.y - floor(t.y * 0.5) * 2.0
            let full_p = vec2(self.source.x + half * self.rect_size.x * 2.0 + t.x * 2.0 + 0.5, self.source.y + floor(t.y * 0.5) + 0.5)
            let sub_p = vec2(self.source.x + t.x * 4.0 + 1.0, self.source.y + t.y * 2.0 + 1.0)
            let p0 = mix(sub_p, full_p, self.full)
            let step = mix(2.0, 1.0, self.full)
            let c0 = self.image.sample(p0 * inv).xyz
            let c1 = self.image.sample((p0 + vec2(step, 0.0)) * inv).xyz
            let cu = vec3(-0.1146, -0.3854, 0.5)
            let cv = vec3(0.5, -0.4542, -0.0458)
            let u0 = (128.0 + 224.0 * dot(c0, cu)) / 255.0
            let v0 = (128.0 + 224.0 * dot(c0, cv)) / 255.0
            let u1 = (128.0 + 224.0 * dot(c1, cu)) / 255.0
            let v1 = (128.0 + 224.0 * dot(c1, cv)) / 255.0
            return vec4(u1, v0, u0, v1)
        }
    }

    mod.widgets.DeskCaptureBase = #(DeskCapture::register_widget(vm))
    mod.widgets.DeskCapture = set_type_default() do mod.widgets.DeskCaptureBase {
        width: Fill height: Fill flow: Overlay
        draw_frame +: {
            image: texture_2d(float)
            pixel: fn() { return self.image.sample(self.pos) }
        }
        draw_luma +: {}
        draw_chroma +: {}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNv12Luma {
    #[deref]
    draw_super: DrawQuad,
    /// The streamed rect of C, in C's pixels.
    #[live]
    source: Vec4f,
    /// C's size in pixels.
    #[live]
    extent: Vec2f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNv12Chroma {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    source: Vec4f,
    #[live]
    extent: Vec2f,
    /// 1: 4:4:4 (every pixel's chroma), 0: 4:2:0.
    #[live]
    full: f32,
}

/// What the WM wants captured: the streamed screen's rect in desktop pixels and the picture
/// size (that rect's size, w % 4 == 0, h % 2 == 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Want {
    pub screen_px: [u32; 4],
    pub stream: [u32; 2],
    /// 4:4:4: chroma at every pixel (the picture is w/4 x 3h).
    pub full_chroma: bool,
}

/// Shared between the WM's Desk Link (writes `want`) and the `DeskCapture` widget (draws it):
/// a Cx global.
#[derive(Default)]
pub struct CaptureState {
    pub want: Option<Want>,
    frame: Option<WindowFrame>,
    picture: Option<RenderTarget>,
    /// Draws made with the current target.
    pub drawn: u64,
    /// Frames whose picture was held back (too soon after the last one), for the timeline.
    pub held: u64,
    /// The picture was drawn in the frame being made: once it is submitted, it is encoded.
    pub painted: bool,
    /// Raised with `painted`, so the WM looks right after the frame is submitted.
    pub signal: Option<SignalToUI>,
    /// The desktop as the window draws it: its size in device pixels and the dpi factor.
    pub desktop_px: [u32; 2],
    pub dpi: f64,
    /// Pacing: at most one picture per `interval` s. A desktop change drawn sooner draws C
    /// alone (no picture) and leaves the picture `owed`; the WM draws again when it is due.
    pub interval: f64,
    pub last_picture: f64,
    pub owed: bool,
}

impl CaptureState {
    /// The picture texture (made at the first capture draw; shared, for the encoder).
    pub fn picture(&self) -> Option<Texture> {
        Some(self.picture.as_ref()?.texture.clone())
    }

    /// No capture: give the targets' memory back.
    pub fn release(&mut self, cx: &mut Cx) {
        if let Some(frame) = self.frame.take() {
            frame.forget(cx);
        }
        if let Some(t) = self.picture.as_mut() {
            t.release(cx);
        }
        self.picture = None;
        self.drawn = 0;
        self.painted = false;
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DeskCapture {
    #[deref]
    view: View,
    #[live]
    draw_frame: DrawQuad,
    #[live]
    draw_luma: DrawNv12Luma,
    #[live]
    draw_chroma: DrawNv12Chroma,
}

impl Widget for DeskCapture {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let dpi = cx.current_dpi_factor();
        let root = cx.owning_window_or_root_pass_size();
        let mut st = std::mem::take(cx.global::<CaptureState>());
        st.dpi = dpi;
        st.desktop_px = [(root.x * dpi).round() as u32, (root.y * dpi).round() as u32];
        let Some(want) = st.want else {
            *cx.global::<CaptureState>() = st;
            return self.view.draw_walk(cx, scope, walk);
        };
        let rect = cx.walk_turtle(walk);
        let [w, h] = want.stream;
        let clear = vec4(0.0, 0.0, 0.0, 0.0);
        let mut picture = st.picture.take().unwrap_or_else(|| RenderTarget::new(cx, "wm_desk_link_picture"));
        let mut frame = st.frame.take().unwrap_or_else(|| WindowFrame::new_with_name(cx, "wm_desk_link"));
        // shared (another GPU user reads it by its handle) and kept between pictures
        let chroma_rows = if want.full_chroma { h * 2 } else { h / 2 };
        let resized = picture.ensure(cx, (w / 4, h + chroma_rows), RenderTargetFormat::SharedBgra8);
        if resized {
            st.drawn = 0;
        }
        let extent = vec2(st.desktop_px[0].max(1) as f32, st.desktop_px[1].max(1) as f32);
        let source = vec4(want.screen_px[0] as f32, want.screen_px[1] as f32, w as f32, h as f32);

        let now = cx.seconds_since_app_start();
        let hold = !resized && st.drawn > 0 && now - st.last_picture < st.interval * 0.9;
        if hold {
            // too soon for a picture: the desktop alone, C a child of the window pass
            frame.begin(cx, Rect { pos: dvec2(0.0, 0.0), size: root });
            while self.view.draw_walk(cx, scope, Walk::fill()).is_step() {}
            frame.end(cx);
            self.draw_frame.draw_vars.set_texture(0, frame.texture());
            self.draw_frame.draw_abs(cx, rect);
            st.frame = Some(frame);
            st.picture = Some(picture);
            st.owed = true;
            st.held += 1;
            *cx.global::<CaptureState>() = st;
            return DrawStep::done();
        }
        st.last_picture = now;
        st.owed = false;
        picture.begin(cx, clear);
        frame.begin(cx, Rect { pos: dvec2(0.0, 0.0), size: root });
        while self.view.draw_walk(cx, scope, Walk::fill()).is_step() {}
        frame.end(cx);
        self.draw_luma.draw_vars.set_texture(0, frame.texture());
        self.draw_luma.source = source;
        self.draw_luma.extent = extent;
        self.draw_luma.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2((w / 4) as f64, h as f64) });
        self.draw_chroma.draw_vars.set_texture(0, frame.texture());
        self.draw_chroma.source = source;
        self.draw_chroma.extent = extent;
        self.draw_chroma.full = if want.full_chroma { 1.0 } else { 0.0 };
        self.draw_chroma.draw_abs(cx, Rect { pos: dvec2(0.0, h as f64), size: dvec2((w / 4) as f64, chroma_rows as f64) });
        picture.end(cx);

        self.draw_frame.draw_vars.set_texture(0, frame.texture());
        self.draw_frame.draw_abs(cx, rect);
        st.frame = Some(frame);
        st.picture = Some(picture);
        st.drawn += 1;
        st.painted = true;
        if let Some(signal) = &st.signal {
            signal.set();
        }
        *cx.global::<CaptureState>() = st;
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use makepad_widgets::makepad_draw::makepad_platform::makepad_script::script_eval;
    use makepad_widgets::*;

    /// The capture shaders compile on every backend: a shader that does not draws nothing, and
    /// the stream carries zero chroma (a green picture) with no error anywhere else.
    #[test]
    fn capture_shaders_compile_on_every_backend() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            super::script_mod(vm);
            for (name, backend, value) in [
                ("luma", "metal", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.DrawNv12Luma, "metal", false)})),
                ("luma", "hlsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.DrawNv12Luma, "hlsl", false)})),
                ("luma", "glsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.DrawNv12Luma, "glsl", false)})),
                ("luma", "wgsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.DrawNv12Luma, "wgsl", false)})),
                ("chroma", "metal", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.DrawNv12Chroma, "metal", false)})),
                ("chroma", "hlsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.DrawNv12Chroma, "hlsl", false)})),
                ("chroma", "glsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.DrawNv12Chroma, "glsl", false)})),
                ("chroma", "wgsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.DrawNv12Chroma, "wgsl", false)})),
            ] {
                let text = vm.heap().string_with(value, |_heap, text| text.to_string()).expect("the compiler answers with source");
                assert!(!text.starts_with("ERRORS"), "{name} on {backend} did not compile: {text}");
            }
        });
    }
}
