//! `RemoteView`: the remote desktop's picture, aspect-locked and letterboxed in whatever size
//! the window has (a whole-pixel scale where it fits), the remote cursor, the latency details
//! when asked for, and why there is no picture. The window's picture area is told to the
//! desktop it shows (its desktop follows its viewer).

use crate::conn::{Latency, Part, ViewWindow};
use makepad_widgets::makepad_draw::shader::draw_nv12::DrawNv12;
use std::sync::Arc;
use makepad_widgets::makepad_platform::video_file::{DecodedPicture, PicturePlane, VideoColor};
use makepad_widgets::*;
use std::collections::VecDeque;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RemoteViewBase = #(RemoteView::register_widget(vm))
    mod.widgets.RemoteView = set_type_default() do mod.widgets.RemoteViewBase {
        width: Fill height: Fill
        draw_bg +: { color: #x101114 }
        draw_cursor +: {
            // an arrow, tip at the quad's top-left
            pixel: fn() {
                let p = self.pos * self.rect_size
                let inside = p.x >= 0.0 && p.y >= 0.0 && p.x <= p.y * 0.62 && p.y <= 17.0 - p.x * 0.35
                if !inside { discard() }
                let edge = p.x > p.y * 0.62 - 1.6 || p.y > 17.0 - p.x * 0.35 - 1.6 || p.x < 1.2
                if edge { return vec4(0.0, 0.0, 0.0, 1.0) }
                return vec4(1.0, 1.0, 1.0, 1.0)
            }
        }
        draw_label +: {
            color: theme.color_label_outer
            text_style: theme.font_code{font_size: 9}
        }
        note_color: theme.color_bg_container
    }

}

/// How the picture sits in the view: its rect (logical).
#[derive(Clone, Copy, Debug, Default)]
pub struct Placement {
    pub rect: Rect,
}

impl Placement {
    /// A point of the view in stream-normalised coordinates (0..=65535), if over the picture.
    pub fn to_stream(&self, abs: Vec2d) -> Option<(u16, u16)> {
        if self.rect.size.x <= 0.0 || self.rect.size.y <= 0.0 {
            return None;
        }
        let u = (abs.x - self.rect.pos.x) / self.rect.size.x;
        let v = (abs.y - self.rect.pos.y) / self.rect.size.y;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        Some(((u * 65535.0).round() as u16, (v * 65535.0).round() as u16))
    }

    /// The same, clamped to the picture (a drag that leaves it keeps going at the edge).
    pub fn to_stream_clamped(&self, abs: Vec2d) -> (u16, u16) {
        let u = ((abs.x - self.rect.pos.x) / self.rect.size.x.max(1.0)).clamp(0.0, 1.0);
        let v = ((abs.y - self.rect.pos.y) / self.rect.size.y.max(1.0)).clamp(0.0, 1.0);
        ((u * 65535.0).round() as u16, (v * 65535.0).round() as u16)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct RemoteView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_nv12: DrawNv12,
    #[live]
    draw_cursor: DrawQuad,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_note_bg: DrawColor,
    /// The plate under a note or the latency details (drawn translucent).
    #[live]
    note_color: Vec4f,
    /// The newest picture's parts (its tiles, or all of it), as textures.
    #[rust]
    parts: Arc<Vec<ShownPart>>,
    /// GPU parts drawn before them, held until no frame in flight can still sample them.
    #[rust]
    retired: VecDeque<Arc<Vec<ShownPart>>>,
    /// The whole picture's size.
    #[rust]
    size: [u32; 2],
    /// Device pixels per remote pixel as last drawn, and whether that is a whole block (no
    /// resampling).
    #[rust]
    pub scale: f64,
    #[rust]
    pub whole: bool,
    #[rust]
    pub placement: Placement,
    /// Where to draw the remote cursor (stream-normalised), when not under the local pointer.
    #[rust]
    pub remote_cursor: Option<(u16, u16)>,
    #[rust]
    pub lines: Vec<String>,
    #[rust]
    pub note: Option<String>,
    /// The newest picture's draw, for its present time: (frame, decoded_ns, drawn_ns).
    #[rust]
    pub drawn: Option<(u32, u64, u64)>,
    #[rust]
    pending: Option<(u32, u64)>,
    /// Where the picture area's size goes: the shown connection's (its desktop follows it).
    #[rust]
    pub sink: Option<Arc<ViewWindow>>,
}

/// GPU pictures kept alive after they were replaced (frames in flight may sample them).
const RETIRED_PICTURES: usize = 3;

/// A part of the shown picture: its pixels from column `x0`, row `y0` on, `size`, and its
/// texture(s).
pub struct ShownPart {
    x0: u32,
    y0: u32,
    size: [u32; 2],
    color: VideoColor,
    tex: PartTex,
}

enum PartTex {
    /// A decoder's GPU picture: its luma and chroma planes as textures over its memory (the
    /// picture held so the decoder does not reuse it meanwhile).
    Gpu { luma: Texture, chroma: Texture, full_chroma: bool, _picture: DecodedPicture },
    /// CPU NV12 in a texture of its own (replaced in place).
    Cpu(Texture),
}

/// The textures of a decoded picture's parts: GPU parts sampled where the decoder put them
/// (nothing is copied), CPU parts uploaded (into `old`'s texture of the same part and size when
/// there is one). `old` keeps what it had otherwise (the caller retires its GPU parts).
pub fn import_parts(cx: &mut Cx, parts: Vec<Part>, old: &mut Vec<ShownPart>) -> Vec<ShownPart> {
    let mut out = Vec::with_capacity(parts.len());
    for (i, part) in parts.into_iter().enumerate() {
        let tex = match part.gpu {
            Some(picture) => {
                let luma = cx.import_picture_plane(picture.handle, PicturePlane::Luma);
                let chroma = cx.import_picture_plane(picture.handle, PicturePlane::Chroma);
                PartTex::Gpu { luma, chroma, full_chroma: picture.full_chroma, _picture: picture }
            }
            None => {
                // the same part's CPU texture is written again in place
                let reuse = old.get(i).and_then(|o| match &o.tex {
                    PartTex::Cpu(t) if o.size == [part.width, part.height] => Some(t.clone()),
                    _ => None,
                });
                match reuse {
                    Some(t) => {
                        let _ = t.take_vec_u8(cx);
                        t.put_back_vec_u8(cx, part.nv12, None);
                        PartTex::Cpu(t)
                    }
                    None => PartTex::Cpu(Texture::new_nv12(cx, part.width, part.height, part.nv12)),
                }
            }
        };
        out.push(ShownPart { x0: part.x0, y0: part.y0, size: [part.width, part.height], color: part.color, tex });
    }
    out
}

/// Draws a picture (`size`, its `parts`) into `rect`: a draw call per part, each over its own
/// rect of the placed picture (the parts tile it exactly), in the colour its stream says.
pub fn draw_parts(draw: &mut DrawNv12, cx: &mut Cx2d, parts: &[ShownPart], rect: Rect, size: [u32; 2]) {
    let (full_w, full_h) = (size[0].max(1) as f64, size[1].max(1) as f64);
    for part in parts {
        let top = rect.pos.y + rect.size.y * part.y0 as f64 / full_h;
        let bottom = rect.pos.y + rect.size.y * (part.y0 + part.size[1]) as f64 / full_h;
        let left = rect.pos.x + rect.size.x * part.x0 as f64 / full_w;
        let right = rect.pos.x + rect.size.x * (part.x0 + part.size[0]) as f64 / full_w;
        let [pw, ph] = part.size;
        match &part.tex {
            PartTex::Gpu { luma, chroma, full_chroma, .. } => draw.set_planes(luma, chroma, pw, ph, part.color, *full_chroma),
            PartTex::Cpu(t) => draw.set_nv12(t, pw, ph, part.color, None),
        }
        draw.new_draw_call(cx);
        draw.draw_abs(cx, Rect { pos: dvec2(left, top), size: dvec2(right - left, bottom - top) });
    }
}

impl RemoteView {
    /// The newest decoded picture (`width` x `height`), all its parts at once: GPU parts are
    /// sampled where the decoder put them (nothing is copied), CPU parts uploaded.
    pub fn set_picture(&mut self, cx: &mut Cx, width: u32, height: u32, parts: Vec<Part>, frame: u32, decoded_ns: u64) {
        let mut old = std::mem::take(&mut self.parts);
        let parts = match Arc::get_mut(&mut old) {
            Some(old) => import_parts(cx, parts, old),
            None => import_parts(cx, parts, &mut Vec::new()),
        };
        self.show_parts(cx, width, height, Arc::new(parts), frame, decoded_ns, old);
    }

    /// A picture already imported (one decoded picture shown in every window on its desktop:
    /// the same textures, nothing copied).
    pub fn set_shared(&mut self, cx: &mut Cx, width: u32, height: u32, parts: Arc<Vec<ShownPart>>, frame: u32, decoded_ns: u64) {
        let old = std::mem::replace(&mut self.parts, Arc::new(Vec::new()));
        self.show_parts(cx, width, height, parts, frame, decoded_ns, old);
    }

    #[allow(clippy::too_many_arguments)]
    fn show_parts(&mut self, cx: &mut Cx, width: u32, height: u32, parts: Arc<Vec<ShownPart>>, frame: u32, decoded_ns: u64, old: Arc<Vec<ShownPart>>) {
        self.parts = parts;
        // a frame in flight may still sample the last few GPU pictures: the GPU is at most a
        // couple behind
        if old.iter().any(|o| matches!(o.tex, PartTex::Gpu { .. })) {
            self.retired.push_back(old);
        }
        while self.retired.len() > RETIRED_PICTURES {
            self.retired.pop_front();
        }
        self.size = [width, height];
        self.pending = Some((frame, decoded_ns));
        self.draw_bg.redraw(cx);
    }

    /// No picture yet: the area the stream's `size` will take (black until it comes).
    pub fn await_picture(&mut self, cx: &mut Cx, size: [u32; 2]) {
        if self.size == [0, 0] && size[0] > 0 && size[1] > 0 {
            self.size = size;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.parts = Arc::new(Vec::new());
        self.retired.clear();
        self.size = [0, 0];
        self.draw_bg.redraw(cx);
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    /// Where the picture goes, on whole device pixels (a fractional offset would blur every
    /// pixel): fit at a whole-number scale (each remote pixel a whole block of device pixels:
    /// nothing resampled) where the picture fits at 1x or more, else fit (letterboxed, aspect
    /// locked).
    /// Returns the rect and whether every remote pixel is a whole block of device pixels.
    fn place(&self, cx: &Cx2d, view: Rect) -> (Rect, bool) {
        let [w, h] = self.size;
        if w == 0 || h == 0 {
            return (Rect::default(), false);
        }
        let (w, h) = (w as f64, h as f64);
        let dpi = cx.current_dpi_factor();
        // the view in device pixels
        let (vx, vy, vw, vh) = (view.pos.x * dpi, view.pos.y * dpi, view.size.x * dpi, view.size.y * dpi);
        let to_logical = |x: f64, y: f64, sw: f64, sh: f64| Rect { pos: dvec2(x.round() / dpi, y.round() / dpi), size: dvec2(sw / dpi, sh / dpi) };
        let fit = (vw / w).min(vh / h);
        let whole = fit >= 1.0;
        let scale = if whole { fit.floor() } else { fit };
        let (sw, sh) = (w * scale, h * scale);
        (to_logical(vx + (vw - sw) * 0.5, vy + (vh - sh) * 0.5, sw, sh), whole)
    }
}

impl Widget for RemoteView {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, view);
        let (rect, whole) = self.place(cx, view);
        self.placement = Placement { rect };
        self.scale = rect.size.x * cx.current_dpi_factor() / self.size[0].max(1) as f64;
        self.whole = whole;
        // the picture area in device pixels and the scale: a desktop that follows its viewer
        // takes this size (fullscreen = the screen, 1:1)
        let dpi = cx.current_dpi_factor();
        if let Some(sink) = &self.sink {
            let (w, h, scale) = view_window_px(view, dpi);
            sink.set(w, h, scale);
        }
        // whole device-pixel blocks (1:1, 2x, ...): each remote pixel as it is (nearest); scaled
        // otherwise: a sharper (bicubic) resample keeps text crisp
        self.draw_nv12.sharp = if whole { 2.0 } else { 1.0 };
        draw_parts(&mut self.draw_nv12, cx, &self.parts, rect, self.size);
        let drawn = !self.parts.is_empty();
        if drawn {
            if let Some(p) = self.pending.take() {
                // stamped here, in the frame that draws it (the app reads it later, at its next sync)
                self.drawn = Some((p.0, p.1, crate::conn::now_ns()));
            }
        }
        if let (Some((x, y)), true) = (self.remote_cursor, rect.size.x > 0.0) {
            let pos = dvec2(rect.pos.x + x as f64 / 65535.0 * rect.size.x, rect.pos.y + y as f64 / 65535.0 * rect.size.y);
            self.draw_cursor.draw_abs(cx, Rect { pos, size: dvec2(12.0, 18.0) });
        }
        // the latency details (asked for) on a plate of their own, readable over any picture
        let mut y = view.pos.y + 8.0;
        if !self.lines.is_empty() {
            let w = self.lines.iter().map(|l| crate::caption::text_width(&mut self.draw_label, cx, l)).fold(0.0, f64::max);
            self.draw_note_bg.color = crate::caption::fade(self.note_color, 0.88);
            self.draw_note_bg.draw_abs(cx, Rect { pos: dvec2(view.pos.x + 6.0, y - 4.0), size: dvec2(w + 16.0, self.lines.len() as f64 * 15.0 + 6.0) });
        }
        for line in &self.lines {
            self.draw_label.draw_abs(cx, dvec2(view.pos.x + 14.0, y), line);
            y += 15.0;
        }
        if let Some(note) = &self.note {
            let w = crate::caption::text_width(&mut self.draw_label, cx, note);
            let pos = dvec2((view.pos.x + (view.size.x - w) * 0.5).max(view.pos.x + 16.0), view.pos.y + view.size.y * 0.5 - 8.0);
            self.draw_note_bg.color = crate::caption::fade(self.note_color, 0.88);
            self.draw_note_bg.draw_abs(cx, Rect { pos: pos - dvec2(12.0, 8.0), size: dvec2(w + 24.0, 30.0) });
            self.draw_label.draw_abs(cx, pos, note);
        }
        DrawStep::done()
    }
}

/// The picture area as a desktop that follows its viewer takes it: the view's size in whole
/// device pixels (`Rect::device_round`, the one rounding rule) and the scale in hundredths.
pub fn view_window_px(view: Rect, dpi: f64) -> (u32, u32, u32) {
    let d = view.device_round(dpi);
    ((d.size.x * dpi).round() as u32, (d.size.y * dpi).round() as u32, (dpi * 100.0).round() as u32)
}

/// The caption's stats (always shown, next to the window buttons): pictures shown a second,
/// the rate that arrives, the stream's codec and chroma, and what limits it only when something
/// does (dropped pictures only when there are any).
pub fn summary_line(shown: (u32, u32), mbps: f64, stream_rate: Option<&crate::conn::StreamRate>) -> String {
    use makepad_desk_link::limit;
    // padded: the caption shows it in fixed-width digits, so changing numbers keep its width
    let mut line = format!("{:>3} fps · {:>3.0} Mbit/s", shown.0.min(999), mbps.clamp(0.0, 9999.0));
    if let Some(r) = stream_rate {
        line.push_str(&format!(" · {} {}", r.codec_name(), if r.full_chroma { "4:4:4" } else { "4:2:0" }));
        let why = match r.limit {
            limit::LINK => " · link-bound",
            limit::ENCODER => " · encoder-bound",
            limit::CAP => " · capped",
            limit::FIXED => " · fixed rate",
            _ => "",
        };
        line.push_str(why);
    }
    if shown.1 > 0 {
        line.push_str(&format!(" · dropped {}", shown.1.min(999)));
    }
    line
}

/// The rate details ("Latency details"): pictures that arrived a second, what the desktop
/// streams at (`stream`: its `Rate`), this display's refresh (`display`, 0 = not measured yet),
/// the stream's rate target, the encoder's preset and streams, and the desktop's why.
pub fn rate_details_line(fps: f64, stream: Option<u16>, display: u32, stream_rate: Option<&crate::conn::StreamRate>) -> String {
    use makepad_desk_link::pref;
    let stream = stream.map_or("…".to_string(), |s| s.to_string());
    let display = if display == 0 { "…".to_string() } else { display.to_string() };
    let target = match stream_rate {
        Some(r) if r.kbps > 0 => format!("{}", r.kbps / 1000),
        _ => "…".to_string(),
    };
    let mut line = format!("arrived {:>3.0}/s · stream {stream}/s · display {display} Hz · target {target} Mbit/s", fps.clamp(0.0, 999.0));
    if let Some(r) = stream_rate {
        let preset = match r.preset {
            pref::SHARP => "sharp",
            pref::FAST => "fast",
            _ => "auto",
        };
        line.push_str(&format!(" · {preset} · {} stream{}", r.streams, if r.streams == 1 { "" } else { "s" }));
        if !r.why.is_empty() {
            line.push_str(&format!(" · {}", r.why));
        }
    }
    line
}

/// The per-stage latency line ("Latency details"), fixed widths too.
pub fn latency_line(l: &Latency) -> String {
    let f = |v: f64| format!("{:>5.1}", v.clamp(-99.9, 999.9));
    format!("encode {} · network {} · decode {} · present {} · total {} ms", f(l.encode), f(l.network), f(l.decode), f(l.present), f(l.total))
}

/// The flow line ("Latency details"): over the last second, the longest the network thread did
/// not read (a stall there holds the desktop's pictures back) and the most pictures waiting for
/// the decoder.
pub fn flow_line(read_gap_ms: f64, decode_queue: usize) -> String {
    format!("read gap {:>5.1} ms · decode queue {:>3}", read_gap_ms.clamp(0.0, 9999.9), decode_queue.min(999))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_view_window_is_the_views_device_pixels() {
        // a 7680x2160 screen at 2x, maximised with the side panel (250 pt) and the menu bar
        let view = Rect { pos: dvec2(250.0, 25.0), size: dvec2(3590.0, 1030.5) };
        assert_eq!(view_window_px(view, 2.0), (7180, 2061, 200));
        // fractional scales round each side once
        assert_eq!(view_window_px(Rect { pos: dvec2(0.0, 0.0), size: dvec2(1000.8, 600.0) }, 1.25), (1251, 750, 125));
        // fullscreen without the panel: the screen
        assert_eq!(view_window_px(Rect { pos: dvec2(0.0, 0.0), size: dvec2(3840.0, 1080.0) }, 2.0), (7680, 2160, 200));
    }
}
