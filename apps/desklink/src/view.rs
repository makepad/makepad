//! The viewer's two widgets: `RemoteView` (the remote desktop's picture, aspect-locked and
//! letterboxed in whatever size the window has, or 1:1 device pixels; the remote cursor; the
//! latency line) and `ConnList` (the side panel's connections, their state and latency).

use crate::conn::{Latency, State};
use makepad_widgets::makepad_draw::shader::draw_nv12::DrawNv12;
use crate::conn::Part;
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
            color: #xe8e8ea
            text_style: theme.font_code{font_size: 9}
        }
        draw_note_bg +: { color: #x000000aa }
    }

    mod.widgets.ConnListBase = #(ConnList::register_widget(vm))
    mod.widgets.ConnList = set_type_default() do mod.widgets.ConnListBase {
        width: Fill height: Fill
        draw_row +: {
            selected: instance(0.0)
            hover: instance(0.0)
            pixel: fn() {
                let base = #x00000000
                let hover = #xffffff14
                let sel = #x3b82f655
                return mix(mix(base, hover, self.hover), sel, self.selected)
            }
        }
        draw_dot +: {
            color: #x888888
            pixel: fn() {
                let d = length(self.pos - vec2(0.5, 0.5))
                if d > 0.5 { discard() }
                return vec4(self.color.xyz * self.color.w, self.color.w)
            }
        }
        draw_name +: { color: #xe8e8ea text_style: theme.font_regular{font_size: 10} }
        draw_detail +: { color: #x9a9aa2 text_style: theme.font_regular{font_size: 8} }
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
    /// The newest picture's parts (its tiles, or all of it), as textures.
    #[rust]
    parts: Vec<ShownPart>,
    /// GPU parts drawn before them, held until no frame in flight can still sample them.
    #[rust]
    retired: VecDeque<ShownPart>,
    /// The whole picture's size.
    #[rust]
    size: [u32; 2],
    /// 1:1 device pixels instead of fit-to-window.
    #[rust]
    pub one_to_one: bool,
    /// Fit to window at a whole-number scale (each remote pixel a whole block of device pixels:
    /// nothing resampled) when the picture fits at 1x or more; a smaller window still scales down.
    #[rust(true)]
    pub integer_scale: bool,
    /// Device pixels per remote pixel as last drawn, and whether that is a whole block (no
    /// resampling).
    #[rust]
    pub scale: f64,
    #[rust]
    pub whole: bool,
    /// The local pointer, for the 1:1 view's pan.
    #[rust]
    pointer: Vec2d,
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
}

/// GPU pictures kept alive after they were replaced (frames in flight may sample them).
const RETIRED_PICTURES: usize = 3;

/// A part of the shown picture: its rows from `y0` on, `size`, and its texture(s).
pub struct ShownPart {
    y0: u32,
    size: [u32; 2],
    tex: PartTex,
}

enum PartTex {
    /// A decoder's GPU picture: its luma and chroma planes as textures over its memory (the
    /// picture held so the decoder does not reuse it meanwhile).
    Gpu { luma: Texture, chroma: Texture, full_chroma: bool, _picture: DecodedPicture },
    /// CPU NV12 in a texture of its own (replaced in place).
    Cpu(Texture),
}

impl RemoteView {
    /// The newest decoded picture (`width` x `height`), all its parts at once: GPU parts are
    /// sampled where the decoder put them (nothing is copied), CPU parts uploaded.
    pub fn set_picture(&mut self, cx: &mut Cx, width: u32, height: u32, parts: Vec<Part>, frame: u32, decoded_ns: u64) {
        let mut old = std::mem::take(&mut self.parts);
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
            self.parts.push(ShownPart { y0: part.y0, size: [part.width, part.height], tex });
        }
        // a frame in flight may still sample the last few GPU pictures: the GPU is at most a
        // couple behind
        for o in old.drain(..) {
            if matches!(o.tex, PartTex::Gpu { .. }) {
                self.retired.push_back(o);
            }
        }
        while self.retired.len() > RETIRED_PICTURES * self.parts.len().max(1) {
            self.retired.pop_front();
        }
        self.size = [width, height];
        self.pending = Some((frame, decoded_ns));
        self.draw_bg.redraw(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.parts.clear();
        self.retired.clear();
        self.size = [0, 0];
        self.draw_bg.redraw(cx);
    }

    pub fn set_pointer(&mut self, cx: &mut Cx, abs: Vec2d) {
        self.pointer = abs;
        if self.one_to_one {
            self.draw_bg.redraw(cx);
        }
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    /// Where the picture goes, on whole device pixels (a fractional offset would blur every
    /// pixel): 1:1, fit at a whole-number scale, or fit (letterboxed, aspect locked).
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
        if self.one_to_one {
            // larger than the view: it follows the pointer (pointer at an edge shows that edge)
            let axis = |view_pos: f64, view_len: f64, len: f64, at: f64| {
                if len <= view_len {
                    view_pos + (view_len - len) * 0.5
                } else {
                    let t = ((at - view_pos) / view_len.max(1.0)).clamp(0.0, 1.0);
                    view_pos - (len - view_len) * t
                }
            };
            return (to_logical(axis(vx, vw, w, self.pointer.x * dpi), axis(vy, vh, h, self.pointer.y * dpi), w, h), true);
        }
        let fit = (vw / w).min(vh / h);
        let whole = self.integer_scale && fit >= 1.0;
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
        let full_h = self.size[1].max(1) as f64;
        // whole device-pixel blocks (1:1, 2x, ...): each remote pixel as it is (nearest); scaled
        // otherwise: a sharper (bicubic) resample keeps text crisp
        self.draw_nv12.sharp = if whole { 2.0 } else { 1.0 };
        for part in &self.parts {
            // each part's rows of the placed picture: the parts tile it exactly
            let top = rect.pos.y + rect.size.y * part.y0 as f64 / full_h;
            let bottom = rect.pos.y + rect.size.y * (part.y0 + part.size[1]) as f64 / full_h;
            let [pw, ph] = part.size;
            match &part.tex {
                PartTex::Gpu { luma, chroma, full_chroma, .. } => self.draw_nv12.set_planes(luma, chroma, pw, ph, VideoColor::BT709, *full_chroma),
                PartTex::Cpu(t) => self.draw_nv12.set_nv12(t, pw, ph, VideoColor::BT709, None),
            }
            // a draw call per part: each samples its own textures
            self.draw_nv12.new_draw_call(cx);
            self.draw_nv12.draw_abs(cx, Rect { pos: dvec2(rect.pos.x, top), size: dvec2(rect.size.x, bottom - top) });
        }
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
        let mut y = view.pos.y + 6.0;
        for line in &self.lines {
            self.draw_label.draw_abs(cx, dvec2(view.pos.x + 8.0, y), line);
            y += 15.0;
        }
        if let Some(note) = &self.note {
            let pos = dvec2(view.pos.x + view.size.x * 0.5 - 180.0, view.pos.y + view.size.y * 0.5 - 12.0);
            self.draw_note_bg.draw_abs(cx, Rect { pos: pos - dvec2(10.0, 6.0), size: dvec2(380.0, 30.0) });
            self.draw_label.draw_abs(cx, pos, note);
        }
        DrawStep::done()
    }
}

/// One row of the side panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub detail: String,
    pub state: State,
}

#[derive(Clone, Debug, Default)]
pub enum ConnListAction {
    Select(usize),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ConnList {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_row: DrawQuad,
    #[live]
    draw_dot: DrawColor,
    #[live]
    draw_name: DrawText,
    #[live]
    draw_detail: DrawText,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    pub selected: usize,
    #[rust]
    hover: Option<usize>,
    #[rust]
    rects: Vec<Rect>,
}

const ROW_H: f64 = 44.0;

impl ConnList {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<Row>) {
        if rows != self.rows {
            self.rows = rows;
            self.draw_row.redraw(cx);
        }
    }
}

impl Widget for ConnList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.uid;
        let hit = |abs: Vec2d, rects: &[Rect]| rects.iter().position(|r| r.contains(abs));
        match event {
            Event::MouseMove(e) => {
                let h = hit(e.abs, &self.rects);
                if h != self.hover {
                    self.hover = h;
                    self.draw_row.redraw(cx);
                }
            }
            Event::MouseDown(e) => {
                if let Some(i) = hit(e.abs, &self.rects) {
                    self.selected = i;
                    self.draw_row.redraw(cx);
                    cx.widget_action(uid, ConnListAction::Select(i));
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let area = cx.walk_turtle(walk);
        self.rects.clear();
        for (i, row) in self.rows.iter().enumerate() {
            let r = Rect { pos: dvec2(area.pos.x, area.pos.y + i as f64 * ROW_H), size: dvec2(area.size.x, ROW_H) };
            self.rects.push(r);
            self.draw_row.draw_vars.set_dyn_instance(cx, id!(selected), &[(i == self.selected) as u8 as f32]);
            self.draw_row.draw_vars.set_dyn_instance(cx, id!(hover), &[(Some(i) == self.hover) as u8 as f32]);
            self.draw_row.draw_abs(cx, r);
            self.draw_dot.color = match row.state {
                State::Connected => vec4(0.30, 0.85, 0.45, 1.0),
                State::Connecting => vec4(0.95, 0.75, 0.25, 1.0),
                State::Offline(_) => vec4(0.55, 0.55, 0.58, 1.0),
            };
            self.draw_dot.draw_abs(cx, Rect { pos: r.pos + dvec2(10.0, 16.0), size: dvec2(9.0, 9.0) });
            self.draw_name.draw_abs(cx, r.pos + dvec2(28.0, 7.0), &row.name);
            self.draw_detail.draw_abs(cx, r.pos + dvec2(28.0, 25.0), &row.detail);
        }
        DrawStep::done()
    }
}

/// The overlay's rate line (always shown): fixed field widths in a monospaced face, so it
/// never jitters as the digits change.
/// Pictures shown a second and the rate, then what the desktop streams at (`stream`: its `Rate`)
/// and this display's refresh (`display`, 0 = not measured yet).
pub fn rate_line(fps: f64, mbps: f64, stream: Option<u16>, display: u32) -> String {
    let stream = stream.map_or("…".to_string(), |s| s.to_string());
    let display = if display == 0 { "…".to_string() } else { display.to_string() };
    format!("{:>3.0} fps {:>6.1} Mbit/s · stream {stream}/s · display {display} Hz", fps.clamp(0.0, 999.0), mbps.clamp(0.0, 9999.9))
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
