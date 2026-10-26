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

    mod.widgets.TimelineBase = #(Timeline::register_widget(vm))
    mod.widgets.Timeline = set_type_default() do mod.widgets.TimelineBase {
        width: Fill height: Fill
        draw_bg +: { color: #x0c0d10ee }
        draw_bar +: { color: #x6b7a99 }
        draw_text +: {
            color: #xe8e8ea
            text_style: theme.font_code{font_size: 9}
        }
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
    /// Room kept free at the bottom (the history timeline over the view).
    #[rust]
    pub inset_bottom: f64,
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
            self.parts.push(ShownPart { x0: part.x0, y0: part.y0, size: [part.width, part.height], color: part.color, tex });
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
    fn place(&self, cx: &Cx2d, mut view: Rect) -> (Rect, bool) {
        view.size.y = (view.size.y - self.inset_bottom).max(1.0);
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
        let full_w = self.size[0].max(1) as f64;
        // the picture area in device pixels and the scale: a desktop that follows its viewer
        // takes this size (fullscreen = the screen, 1:1)
        let dpi = cx.current_dpi_factor();
        let (w, h, scale) = view_window_px(view, dpi);
        crate::conn::set_view_window(w, h, scale);
        // whole device-pixel blocks (1:1, 2x, ...): each remote pixel as it is (nearest); scaled
        // otherwise: a sharper (bicubic) resample keeps text crisp
        self.draw_nv12.sharp = if whole { 2.0 } else { 1.0 };
        for part in &self.parts {
            // each part's rect of the placed picture: the parts tile it exactly
            let top = rect.pos.y + rect.size.y * part.y0 as f64 / full_h;
            let bottom = rect.pos.y + rect.size.y * (part.y0 + part.size[1]) as f64 / full_h;
            let left = rect.pos.x + rect.size.x * part.x0 as f64 / full_w;
            let right = rect.pos.x + rect.size.x * (part.x0 + part.size[0]) as f64 / full_w;
            let [pw, ph] = part.size;
            match &part.tex {
                PartTex::Gpu { luma, chroma, full_chroma, .. } => self.draw_nv12.set_planes(luma, chroma, pw, ph, part.color, *full_chroma),
                PartTex::Cpu(t) => self.draw_nv12.set_nv12(t, pw, ph, part.color, None),
            }
            // a draw call per part: each samples its own textures
            self.draw_nv12.new_draw_call(cx);
            self.draw_nv12.draw_abs(cx, Rect { pos: dvec2(left, top), size: dvec2(right - left, bottom - top) });
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
        // the lines on a dark plate of their own: readable over any picture, never mixed with
        // what the desktop draws under them (its own icons)
        if !self.lines.is_empty() {
            let chars = self.lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f64;
            self.draw_note_bg.draw_abs(cx, Rect { pos: dvec2(view.pos.x + 4.0, y - 3.0), size: dvec2(chars * 6.6 + 10.0, self.lines.len() as f64 * 15.0 + 5.0) });
        }
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

/// The picture area as a desktop that follows its viewer takes it: the view's size in whole
/// device pixels (`Rect::device_round`, the one rounding rule) and the scale in hundredths.
pub fn view_window_px(view: Rect, dpi: f64) -> (u32, u32, u32) {
    let d = view.device_round(dpi);
    ((d.size.x * dpi).round() as u32, (d.size.y * dpi).round() as u32, (dpi * 100.0).round() as u32)
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
    /// The row's "reconnect": drop the session and open it again now.
    Reconnect(usize),
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
    /// Each row's "reconnect" label.
    #[rust]
    reconnect_rects: Vec<Rect>,
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
                if let Some(i) = hit(e.abs, &self.reconnect_rects) {
                    cx.widget_action(uid, ConnListAction::Reconnect(i));
                } else if let Some(i) = hit(e.abs, &self.rects) {
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
        self.reconnect_rects.clear();
        for (i, row) in self.rows.iter().enumerate() {
            let r = Rect { pos: dvec2(area.pos.x, area.pos.y + i as f64 * ROW_H), size: dvec2(area.size.x, ROW_H) };
            self.rects.push(r);
            self.draw_row.draw_vars.set_dyn_instance(cx, id!(selected), &[(i == self.selected) as u8 as f32]);
            self.draw_row.draw_vars.set_dyn_instance(cx, id!(hover), &[(Some(i) == self.hover) as u8 as f32]);
            self.draw_row.draw_abs(cx, r);
            self.draw_dot.color = match row.state {
                State::Connected => vec4(0.30, 0.85, 0.45, 1.0),
                State::Connecting | State::Reconnecting => vec4(0.95, 0.75, 0.25, 1.0),
                State::Offline(_) => vec4(0.55, 0.55, 0.58, 1.0),
            };
            self.draw_dot.draw_abs(cx, Rect { pos: r.pos + dvec2(10.0, 16.0), size: dvec2(9.0, 9.0) });
            self.draw_name.draw_abs(cx, r.pos + dvec2(28.0, 7.0), &row.name);
            self.draw_detail.draw_abs(cx, r.pos + dvec2(28.0, 25.0), &row.detail);
            // "reconnect" at the row's right end (on hover or for the selected row)
            let rr = Rect { pos: dvec2(r.pos.x + r.size.x - 62.0, r.pos.y + 4.0), size: dvec2(58.0, 18.0) };
            if i == self.selected || Some(i) == self.hover {
                self.draw_detail.draw_abs(cx, rr.pos + dvec2(4.0, 3.0), "reconnect");
            }
            self.reconnect_rects.push(rr);
        }
        DrawStep::done()
    }
}

/// The overlay's one line (always shown): pictures shown a second, the rate that arrives, the
/// stream's codec and chroma, and what limits it only when something does (dropped pictures only
/// when there are any). Fixed field widths in a monospaced face, so it never jitters.
pub fn summary_line(shown: (u32, u32), mbps: f64, stream_rate: Option<&crate::conn::StreamRate>) -> String {
    use makepad_desk_link::limit;
    let mut line = format!("{:>3} fps · {:>4.0} Mbit/s", shown.0.min(999), mbps.clamp(0.0, 9999.0));
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

/// What the timeline shows: the paused history, the shown picture, the range to save.
pub struct TimelineData {
    pub snap: std::sync::Arc<crate::history::Snapshot>,
    pub cur: usize,
    pub range: (Option<usize>, Option<usize>),
    /// The play speed (None: stopped).
    pub playing: Option<f64>,
    /// Status: the scrub decoder (cache, work, failure) and the last save.
    pub status: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TimelineAction {
    Seek(usize),
    Step(i64),
    Live,
    Play(f64),
    Stop,
    In,
    Out,
    SaveClip,
    SaveFrame,
    #[default]
    None,
}

/// The history timeline (shown while the view is paused): an overview of the whole history
/// (each column a slice of time: picture sizes, key frames yellow, gaps / skipped frames /
/// failed decodes red), a strip of the pictures around the shown one (one cell each, sizes as
/// bars), the shown picture's numbers, and the buttons.
#[derive(Script, ScriptHook, Widget)]
pub struct Timeline {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_bar: DrawColor,
    #[live]
    draw_text: DrawText,
    #[rust]
    pub data: Option<TimelineData>,
    #[rust]
    buttons: Vec<(Rect, TimelineAction)>,
    #[rust]
    overview: Rect,
    #[rust]
    strip: Rect,
    /// The strip's first picture and its cell width.
    #[rust]
    strip_first: usize,
    #[rust]
    strip_cell: f64,
    /// 1: dragging in the overview, 2: in the strip.
    #[rust]
    drag: u8,
}

/// The timeline's height (points).
pub const TIMELINE_H: f64 = 124.0;

const KEY_COLOR: Vec4f = vec4(0.95, 0.80, 0.25, 1.0);
const MARK_COLOR: Vec4f = vec4(0.95, 0.30, 0.30, 1.0);
const BAR_COLOR: Vec4f = vec4(0.42, 0.48, 0.60, 1.0);

impl Timeline {
    pub fn set_data(&mut self, cx: &mut Cx, data: Option<TimelineData>) {
        self.data = data;
        self.draw_bg.redraw(cx);
    }

    fn pick(&self, abs: Vec2d) -> Option<usize> {
        let d = self.data.as_ref()?;
        let n = d.snap.len();
        if n == 0 {
            return None;
        }
        if self.drag == 1 {
            let (a, b) = (d.snap.pictures[0].built_ns, d.snap.pictures[n - 1].built_ns);
            let u = ((abs.x - self.overview.pos.x) / self.overview.size.x.max(1.0)).clamp(0.0, 1.0);
            return Some(d.snap.at_time(a + ((b - a) as f64 * u) as u64));
        }
        let k = ((abs.x - self.strip.pos.x) / self.strip_cell.max(1.0)).floor().max(0.0) as usize;
        Some((self.strip_first + k).min(n - 1))
    }
}

impl Widget for Timeline {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.data.is_none() {
            return;
        }
        let uid = self.uid;
        match event {
            Event::MouseDown(e) => {
                if let Some((_, a)) = self.buttons.iter().find(|(r, _)| r.contains(e.abs)) {
                    cx.widget_action(uid, *a);
                    return;
                }
                self.drag = if self.overview.contains(e.abs) { 1 } else if self.strip.contains(e.abs) { 2 } else { 0 };
                if let Some(i) = (self.drag != 0).then(|| self.pick(e.abs)).flatten() {
                    cx.widget_action(uid, TimelineAction::Seek(i));
                }
            }
            Event::MouseMove(e) if self.drag != 0 => {
                if let Some(i) = self.pick(e.abs) {
                    cx.widget_action(uid, TimelineAction::Seek(i));
                }
            }
            Event::MouseUp(_) => self.drag = 0,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let area = cx.walk_turtle(walk);
        self.buttons.clear();
        let Some(d) = self.data.as_ref() else {
            self.overview = Rect::default();
            self.strip = Rect::default();
            return DrawStep::done();
        };
        let snap = &d.snap;
        let n = snap.len();
        let panel = Rect { pos: dvec2(area.pos.x, area.pos.y + area.size.y - TIMELINE_H), size: dvec2(area.size.x, TIMELINE_H) };
        self.draw_bg.draw_abs(cx, panel);
        let x0 = panel.pos.x + 10.0;
        let w = (panel.size.x - 20.0).max(10.0);
        if n == 0 {
            self.draw_text.draw_abs(cx, dvec2(x0, panel.pos.y + 10.0), "the history is empty");
            return DrawStep::done();
        }
        let cur = d.cur.min(n - 1);
        let max_bytes = snap.pictures.iter().map(|p| p.bytes).max().unwrap_or(1).max(2) as f64;
        // square root: small pictures stay visible next to key frames
        let bar_h = |bytes: usize, full: f64| ((bytes as f64 / max_bytes).sqrt().clamp(0.06, 1.0)) * full;
        use crate::history::flag;
        let flagged = |i: usize| snap.flags(i) & (flag::GAP | flag::SKIP | flag::FAILED) != 0;

        // the overview: the whole history over time, one column per 2 points
        let ov = Rect { pos: dvec2(x0, panel.pos.y + 8.0), size: dvec2(w, 26.0) };
        self.overview = ov;
        let (t0, t1) = (snap.pictures[0].built_ns, snap.pictures[n - 1].built_ns.max(snap.pictures[0].built_ns + 1));
        let x_of = |t: u64| ov.pos.x + (t - t0) as f64 / (t1 - t0) as f64 * ov.size.x;
        if let (Some(a), Some(b)) = (d.range.0, d.range.1) {
            let (a, b) = (a.min(b), a.max(b).min(n - 1));
            self.draw_bar.color = vec4(0.25, 0.45, 0.85, 0.35);
            let (xa, xb) = (x_of(snap.pictures[a].built_ns), x_of(snap.pictures[b].built_ns));
            self.draw_bar.draw_abs(cx, Rect { pos: dvec2(xa, ov.pos.y), size: dvec2((xb - xa).max(2.0), ov.size.y) });
        }
        let columns = (ov.size.x / 2.0).floor().max(1.0) as usize;
        // (largest picture, a key frame, a flagged picture) per column
        let mut col: Vec<(usize, bool, bool)> = vec![(0, false, false); columns];
        for i in 0..n {
            let c = (((snap.pictures[i].built_ns - t0) as f64 / (t1 - t0) as f64 * columns as f64) as usize).min(columns - 1);
            let p = &snap.pictures[i];
            col[c].0 = col[c].0.max(p.bytes);
            col[c].1 |= p.key;
            col[c].2 |= flagged(i);
        }
        for (c, (bytes, key, flag)) in col.iter().enumerate() {
            if *bytes == 0 && !flag {
                continue;
            }
            let h = bar_h(*bytes, ov.size.y - 4.0);
            let x = ov.pos.x + c as f64 * 2.0;
            self.draw_bar.color = if *key { KEY_COLOR } else { BAR_COLOR };
            self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x, ov.pos.y + ov.size.y - h), size: dvec2(2.0, h) });
            if *flag {
                self.draw_bar.color = MARK_COLOR;
                self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x, ov.pos.y), size: dvec2(2.0, 3.0) });
            }
        }
        self.draw_bar.color = vec4(1.0, 1.0, 1.0, 1.0);
        self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x_of(snap.pictures[cur].built_ns) - 1.0, ov.pos.y - 3.0), size: dvec2(2.0, ov.size.y + 6.0) });

        // the strip: one cell per picture around the shown one
        let st = Rect { pos: dvec2(x0, ov.pos.y + ov.size.y + 8.0), size: dvec2(w, 30.0) };
        self.strip = st;
        let cell = 12.0;
        let cells = (st.size.x / cell).floor().max(1.0) as usize;
        let first = cur.saturating_sub(cells / 2).min(n.saturating_sub(cells));
        self.strip_first = first;
        self.strip_cell = cell;
        for k in 0..cells.min(n - first) {
            let i = first + k;
            let p = &snap.pictures[i];
            let x = st.pos.x + k as f64 * cell;
            if i == cur {
                self.draw_bar.color = vec4(1.0, 1.0, 1.0, 0.22);
                self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x, st.pos.y - 2.0), size: dvec2(cell, st.size.y + 4.0) });
            }
            if let (Some(a), Some(b)) = (d.range.0, d.range.1) {
                if (a.min(b)..=a.max(b)).contains(&i) {
                    self.draw_bar.color = vec4(0.25, 0.45, 0.85, 0.30);
                    self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x, st.pos.y), size: dvec2(cell, st.size.y) });
                }
            }
            let h = bar_h(p.bytes, st.size.y - 6.0);
            self.draw_bar.color = if p.key { KEY_COLOR } else { BAR_COLOR };
            self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x + 2.0, st.pos.y + st.size.y - h), size: dvec2(cell - 4.0, h) });
            if snap.flags(i) != 0 {
                self.draw_bar.color = MARK_COLOR;
                self.draw_bar.draw_abs(cx, Rect { pos: dvec2(x + 2.0, st.pos.y), size: dvec2(cell - 4.0, 3.0) });
            }
        }

        // the shown picture's numbers
        let p = &snap.pictures[cur];
        let last = &snap.pictures[n - 1];
        let marks = snap.marks(cur);
        let info = format!(
            "frame {} · {}/{} · {:+.3} s · {:>7.1} KB{} · decode {:.1} ms · dt {:.2} ms{}{}",
            p.frame,
            cur + 1,
            n,
            -(last.built_ns.saturating_sub(p.built_ns) as f64 / 1e9),
            p.bytes as f64 / 1024.0,
            if p.count > 1 { format!(" ({} parts)", p.count) } else { String::new() },
            p.decode_us as f64 / 1000.0,
            snap.dt_ns(cur) as f64 / 1e6,
            if marks.is_empty() { String::new() } else { format!(" · {}", marks.join(", ")) },
            match (d.range.0, d.range.1) {
                (Some(a), Some(b)) => format!(" · range {}..{}", snap.pictures[a.min(b)].frame, snap.pictures[a.max(b).min(n - 1)].frame),
                (Some(a), None) | (None, Some(a)) => format!(" · range from {}", snap.pictures[a].frame),
                _ => String::new(),
            }
        );
        self.draw_text.draw_abs(cx, dvec2(x0, st.pos.y + st.size.y + 6.0), &info);

        // the buttons, then the status
        let by = panel.pos.y + TIMELINE_H - 26.0;
        let mut bx = x0;
        let play_label = |speed: f64| if d.playing == Some(speed) { "Stop".to_string() } else if speed == 1.0 { "Play 1x".to_string() } else { "Play 1/4x".to_string() };
        let buttons = [
            ("Live (Esc)".to_string(), TimelineAction::Live),
            ("< 1".to_string(), TimelineAction::Step(-1)),
            ("1 >".to_string(), TimelineAction::Step(1)),
            (play_label(1.0), if d.playing == Some(1.0) { TimelineAction::Stop } else { TimelineAction::Play(1.0) }),
            (play_label(0.25), if d.playing == Some(0.25) { TimelineAction::Stop } else { TimelineAction::Play(0.25) }),
            ("In".to_string(), TimelineAction::In),
            ("Out".to_string(), TimelineAction::Out),
            ("Save clip".to_string(), TimelineAction::SaveClip),
            ("Save frame".to_string(), TimelineAction::SaveFrame),
        ];
        for (label, action) in buttons {
            let bw = label.chars().count() as f64 * 7.2 + 16.0;
            let r = Rect { pos: dvec2(bx, by), size: dvec2(bw, 20.0) };
            self.draw_bar.color = vec4(1.0, 1.0, 1.0, 0.10);
            self.draw_bar.draw_abs(cx, r);
            self.draw_text.draw_abs(cx, dvec2(bx + 8.0, by + 4.0), &label);
            self.buttons.push((r, action));
            bx += bw + 6.0;
        }
        self.draw_text.draw_abs(cx, dvec2(bx + 8.0, by + 4.0), &d.status);
        DrawStep::done()
    }
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
