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
//! stream encoder. There are two picture textures, drawn in turn: a picture is never drawn into
//! a texture the encoder still reads (`CaptureState::in_flight`; one texture drawn over while it
//! was encoded sent torn pictures, the game window half of one frame and half of the next).
//! While a viewer is behind (`CaptureState::hold`) or the encoder still reads the texture that
//! is next, the frame draws C alone and the picture is owed, as when it is too soon.
//! A cursor move repaints only the window pass: no picture. Colour is BT.709
//! limited range.

use crate::dock_warp::WindowFrame;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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

/// How a picture is cut into tiles: `cols` columns by `rows` bands (1 x 1: whole pictures).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilePlan {
    pub cols: u32,
    pub rows: u32,
}

impl TilePlan {
    pub const WHOLE: TilePlan = TilePlan { cols: 1, rows: 1 };
    /// Macroblocks a second one stream may carry: H.264 level 5.2's, which hardware decoders
    /// (VideoToolbox among them) take; a faster stream is cut further.
    pub const MAX_MB_RATE: u64 = 2_073_600;
    /// Pixels a second one H.265 stream may carry for a viewer's hardware decoder (a decoder
    /// thread per stream, one picture at a time). Apple M4 VideoToolbox, 4:4:4 RExt, idle:
    /// 7680x2160 alone in 7.9 ms (2.1 Gpx/s, 94% of a picture at 120: no margin), the same in two
    /// bands decoded at once in 6.7 ms; 3840x2160 alone in 4.6 ms. So 8K wide at 120 is two
    /// bands, 3840x2160 at 120 one.
    pub const DECODE_PX_RATE: u64 = 1_300_000_000;
    /// The fewest H.265 bands of a `size` picture at `fps` that each fit [`Self::DECODE_PX_RATE`].
    pub fn decode_bands(size: [u32; 2], fps: u32) -> u32 {
        (size[0] as u64 * size[1] as u64 * fps as u64).div_ceil(Self::DECODE_PX_RATE).clamp(1, 16) as u32
    }
    pub fn tiles(&self) -> u32 {
        self.cols * self.rows
    }
    /// The plan for `size` pictures at `fps` with `split` on `engines` engines whose streams take
    /// at most `max` pixels (None: not known, no limit). Tiles: the fewest tiles that are a
    /// multiple of the engines (every engine gets the same share) and that a viewer's decoder
    /// takes at `fps` (`MAX_MB_RATE` per stream), fewest columns first; never fewer than the size
    /// limit needs. Interleave / Off: the size limit's tiles only.
    ///
    /// H.265 (`hevc`): one stream as large as the encoder takes (no seams, one decoder), or more
    /// bands: as many as a viewer's decoder needs at `fps` ([`Self::decode_bands`], at least
    /// `bands`: what the viewers' measured decode asked for), and with `spread` at least a band
    /// per engine (the engines encode a picture at once). Bands are at least 64 rows.
    pub fn new(size: [u32; 2], fps: u32, split: super::settings::Split, engines: u32, max: Option<[u32; 2]>, hevc: bool, spread: bool, bands: u32) -> TilePlan {
        let [mw, mh] = max.map_or([u32::MAX, u32::MAX], |[w, h]| [(w / 16 * 16).max(16), (h / 16 * 16).max(16)]);
        let min_cols = size[0].div_ceil(mw).max(1);
        let min_rows = size[1].div_ceil(mh).max(1);
        let least = TilePlan { cols: min_cols, rows: min_rows };
        if split != super::settings::Split::Tiles {
            return least;
        }
        if hevc {
            let want = Self::decode_bands(size, fps).max(bands).max(if spread { engines } else { 1 });
            let rows = want.min(size[1] / 64).max(least.rows);
            return TilePlan { cols: least.cols, rows };
        }
        if engines <= 1 {
            return least;
        }
        // tiles at least 64 pixels each way, each a stream the decoder takes at this rate
        let mb_rate = |c: u32, r: u32| (size[0] / c).div_ceil(16) as u64 * (size[1] / r + 16).div_ceil(16) as u64 * fps as u64;
        let fits = |c: u32, r: u32| c >= min_cols && r >= min_rows && size[0] / c >= 64 && size[1] / r >= 64 && mb_rate(c, r) <= Self::MAX_MB_RATE;
        for k in 1..=4 {
            let n = engines * k;
            if let Some(c) = (1..=n).find(|c| n % c == 0 && fits(*c, n / c)) {
                return TilePlan { cols: c, rows: n / c };
            }
        }
        least
    }
    /// The (start, length) of each of `parts` cuts of `len` pixels, every cut but the last a
    /// whole number of `unit`s (the last takes the rest).
    fn edges(len: u32, parts: u32, unit: u32) -> Vec<(u32, u32)> {
        let parts = parts.max(1);
        let step = (len / parts) / unit * unit;
        (0..parts).map(|i| if i + 1 == parts { (step * i, len - step * i) } else { (step * i, step) }).collect()
    }
    /// The columns of `len` pixels: 64-pixel multiples but the last (which takes the rest).
    pub fn columns(len: u32, parts: u32) -> Vec<(u32, u32)> {
        Self::edges(len, parts, 64)
    }
    /// The bands of `len` rows: whole macroblock rows but the last.
    pub fn bands(len: u32, parts: u32) -> Vec<(u32, u32)> {
        Self::edges(len, parts, 16)
    }
}

/// What the WM wants captured: the streamed screen's rect in desktop pixels and the picture
/// size (that rect's size, w % 4 == 0, h % 2 == 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Want {
    pub screen_px: [u32; 4],
    pub stream: [u32; 2],
    /// 4:4:4: chroma at every pixel (the picture is w/4 x 3h).
    pub full_chroma: bool,
    /// The encoder's tiles: each column's chroma rows are packed within the column (4:4:4: a
    /// column's chroma row is its own two texture rows), so every tile is one texel rectangle.
    pub plan: TilePlan,
    /// H.265 (else H.264), and the encoder's best pictures per bit (else its fastest): the
    /// encoder's, the capture is the same.
    pub hevc: bool,
    pub sharp: bool,
}

/// Shared between the WM's Desk Link (writes `want`) and the `DeskCapture` widget (draws it):
/// a Cx global.
#[derive(Default)]
pub struct CaptureState {
    pub want: Option<Want>,
    frame: Option<WindowFrame>,
    /// The two picture textures, drawn in turn: `next` is drawn next, the other holds the last
    /// picture.
    pictures: [Option<RenderTarget>; 2],
    next: usize,
    /// Draws made with the current target.
    pub drawn: u64,
    /// Frames whose picture was held back (too soon after the last one), for the timeline.
    pub held: u64,
    /// The picture drawn in the frame being made (its texture's encoder hold): once the frame
    /// is submitted, it goes to the encoder with it.
    pub painted: Option<InFlight>,
    /// Raised with `painted`, so the WM looks right after the frame is submitted.
    pub signal: Option<SignalToUI>,
    /// The desktop as the window draws it: its size in device pixels and the dpi factor.
    pub desktop_px: [u32; 2],
    pub dpi: f64,
    /// Pacing: at most one picture per `interval` s. A desktop change drawn sooner draws C
    /// alone (no picture) and leaves the picture `owed`; the WM draws again when it is due.
    pub interval: f64,
    /// The streamed screen's frames per picture (its refresh over the stream rate, at least
    /// 1): a picture may follow the last one after this many frames whatever the clock says
    /// (frame times jitter around the interval; at a stream rate equal to the refresh every
    /// frame is a picture).
    pub every: u32,
    /// Frames drawn since the last picture.
    pub since: u32,
    pub last_picture: f64,
    pub owed: bool,
    /// A viewer is behind on pictures (its socket backlog): pictures are held back, the
    /// capture itself stays (its texture, the encoder's registration, no new key frame).
    pub hold: bool,
    /// Per picture texture: pictures of it not encoded yet (each an `InFlight`, which the encoder
    /// drops once its GPU reads are done). A texture is drawn only at 0, so the encoder never
    /// reads a picture being drawn over, and an encoder slower than the capture never queues
    /// pictures (each one it takes shows the desktop as it is now).
    pub in_flight: [Arc<AtomicUsize>; 2],
    /// Frames held back for `hold` or `in_flight` (a subset of `held`), for the timeline.
    pub held_behind: u64,
    /// Of those, the ones held only because the encoder still read the next texture (no viewer
    /// behind): the adaptive rate's encoder-bound count.
    pub held_encoder: u64,
}

/// A picture of one texture not encoded yet: the texture is not drawn over while one lives.
pub struct InFlight(Arc<AtomicUsize>);

impl InFlight {
    pub fn new(count: &Arc<AtomicUsize>) -> InFlight {
        count.fetch_add(1, Ordering::AcqRel);
        InFlight(count.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl CaptureState {
    /// The texture of the last picture (shared, for the encoder).
    pub fn picture(&self) -> Option<Texture> {
        Some(self.pictures[1 - self.next].as_ref()?.texture.clone())
    }

    /// A hold of the last picture's texture, for another job that reads it (a refinement).
    pub fn hold_last(&self) -> InFlight {
        InFlight::new(&self.in_flight[1 - self.next])
    }

    /// No capture: give the targets' memory back.
    pub fn release(&mut self, cx: &mut Cx) {
        if let Some(frame) = self.frame.take() {
            frame.forget(cx);
        }
        for slot in &mut self.pictures {
            if let Some(t) = slot.as_mut() {
                t.release(cx);
            }
            *slot = None;
        }
        self.drawn = 0;
        self.painted = None;
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
        let slot = st.next;
        let mut picture = st.pictures[slot].take().unwrap_or_else(|| RenderTarget::new(cx, if slot == 0 { "wm_desk_link_picture_0" } else { "wm_desk_link_picture_1" }));
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
        // the first picture is drawn whatever is behind (nothing is encoded before it)
        let behind = !resized && st.drawn > 0 && (st.hold || st.in_flight[slot].load(Ordering::Acquire) > 0);
        st.since = st.since.saturating_add(1);
        let too_soon = now - st.last_picture < st.interval * 0.9 && st.since < st.every.max(1);
        let hold = behind || !resized && st.drawn > 0 && too_soon;
        if hold {
            // too soon for a picture: the desktop alone, C a child of the window pass
            frame.begin(cx, Rect { pos: dvec2(0.0, 0.0), size: root });
            while self.view.draw_walk(cx, scope, Walk::fill()).is_step() {}
            frame.end(cx);
            self.draw_frame.draw_vars.set_texture(0, frame.texture());
            self.draw_frame.draw_abs(cx, rect);
            st.frame = Some(frame);
            st.pictures[slot] = Some(picture);
            st.owed = true;
            st.held += 1;
            st.held_behind += behind as u64;
            st.held_encoder += (behind && !st.hold) as u64;
            *cx.global::<CaptureState>() = st;
            return DrawStep::done();
        }
        st.last_picture = now;
        st.since = 0;
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
        self.draw_chroma.extent = extent;
        self.draw_chroma.full = if want.full_chroma { 1.0 } else { 0.0 };
        // one draw per tile column: its chroma rows within its own texels
        for (x0, cw) in TilePlan::columns(w, want.plan.cols) {
            self.draw_chroma.source = vec4(source.x + x0 as f32, source.y, cw as f32, h as f32);
            self.draw_chroma.draw_abs(cx, Rect { pos: dvec2((x0 / 4) as f64, h as f64), size: dvec2((cw / 4) as f64, chroma_rows as f64) });
        }
        picture.end(cx);

        self.draw_frame.draw_vars.set_texture(0, frame.texture());
        self.draw_frame.draw_abs(cx, rect);
        st.frame = Some(frame);
        st.pictures[slot] = Some(picture);
        st.next = 1 - slot;
        st.drawn += 1;
        st.painted = Some(InFlight::new(&st.in_flight[slot]));
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

#[cfg(test)]
mod plan_tests {
    use super::*;
    use super::super::settings::Split;
    #[test]
    fn an_8k_wide_picture_spreads_over_every_engine_in_columns_within_the_encoders_width() {
        let max = Some([4096, 4096]);
        assert_eq!(TilePlan::new([7680, 2160], 120, Split::Tiles, 4, max, false, false, 1), TilePlan { cols: 2, rows: 2 });
        assert_eq!(TilePlan::new([7680, 2160], 120, Split::Tiles, 3, max, false, false, 1), TilePlan { cols: 2, rows: 3 }, "3 x 2560x2160 is past the decoder's rate at 120");
        assert_eq!(TilePlan::new([7680, 2160], 60, Split::Tiles, 3, max, false, false, 1), TilePlan { cols: 3, rows: 1 });
        assert_eq!(TilePlan::new([7680, 2160], 120, Split::Tiles, 2, max, false, false, 1), TilePlan { cols: 2, rows: 2 });
        assert_eq!(TilePlan::new([7680, 2160], 120, Split::Off, 4, max, false, false, 1), TilePlan { cols: 2, rows: 1 });
        assert_eq!(TilePlan::new([7680, 2160], 120, Split::Interleave, 4, max, false, false, 1), TilePlan { cols: 2, rows: 1 });
        assert_eq!(TilePlan::new([3840, 2160], 120, Split::Tiles, 2, max, false, false, 1), TilePlan { cols: 1, rows: 2 });
        assert_eq!(TilePlan::new([3840, 2160], 120, Split::Tiles, 1, max, false, false, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([3840, 2160], 120, Split::Interleave, 2, max, false, false, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([1280, 100], 120, Split::Tiles, 4, max, false, false, 1), TilePlan { cols: 4, rows: 1 });
        // H.265 (8192 wide): one stream while a decoder keeps up, else bands; a band per engine
        let hevc = Some([8192, 8192]);
        assert_eq!(TilePlan::new([3840, 2160], 120, Split::Tiles, 3, hevc, true, false, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([7680, 2160], 60, Split::Tiles, 3, hevc, true, false, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([7680, 2160], 120, Split::Tiles, 3, hevc, true, false, 1), TilePlan { cols: 1, rows: 2 }, "8K wide at 120: two bands from the first picture");
        assert_eq!(TilePlan::new([7680, 2160], 120, Split::Tiles, 1, hevc, true, false, 1), TilePlan { cols: 1, rows: 2 }, "one engine bands too");
        assert_eq!(TilePlan::new([7680, 4320], 120, Split::Tiles, 3, hevc, true, false, 1), TilePlan { cols: 1, rows: 4 });
        assert_eq!(TilePlan::new([7680, 2160], 120, Split::Tiles, 3, hevc, true, true, 1), TilePlan { cols: 1, rows: 3 });
        assert_eq!(TilePlan::new([3840, 2160], 120, Split::Tiles, 3, hevc, true, false, 3), TilePlan { cols: 1, rows: 3 }, "a viewer's measured decode asked for 3");
        assert_eq!(TilePlan::new([7680, 4320], 120, Split::Off, 3, hevc, true, true, 1), TilePlan::WHOLE);
        assert_eq!(TilePlan::new([1280, 100], 120, Split::Tiles, 3, hevc, true, true, 1), TilePlan::WHOLE, "bands of at least 64 rows");
        assert_eq!(TilePlan::new([1280, 200], 120, Split::Tiles, 3, hevc, true, true, 1), TilePlan { cols: 1, rows: 3 });
    }
    #[test]
    fn tile_edges_are_whole_macroblocks_and_cover_the_picture_once() {
        assert_eq!(TilePlan::columns(7680, 2), vec![(0, 3840), (3840, 3840)]);
        assert_eq!(TilePlan::columns(7680, 3), vec![(0, 2560), (2560, 2560), (5120, 2560)]);
        assert_eq!(TilePlan::bands(2160, 2), vec![(0, 1072), (1072, 1088)]);
        assert_eq!(TilePlan::columns(1000, 1), vec![(0, 1000)]);
        // a viewer-sized desktop: the first column a 64 multiple, the last takes the rest
        assert_eq!(TilePlan::columns(6800, 2), vec![(0, 3392), (3392, 3408)]);
        assert_eq!(TilePlan::columns(5776, 3), vec![(0, 1920), (1920, 1920), (3840, 1936)]);
    }
}
