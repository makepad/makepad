//! Screenshots: Super+Shift+4 selects a rectangle (Space switches to picking a
//! window), Super+Shift+3 takes the whole screen. The picture is saved as a PNG
//! in ~/Screenshots and a thumbnail floats in the corner of the screen for a few
//! seconds; dragging it onto a window hands that app the file's path, clicking it
//! opens the picture.
//!
//! The pixels are the WM's own readback of the composited frame (the window's
//! screen-capture sink, taken before any selection overlay is drawn), cropped
//! here. Encoding and writing run on a worker thread; the thumbnail appears
//! when it is done. This file is the state, the pure geometry and the overlay
//! widget; the keyboard and pointer belong to `screenshot_app.rs`.

use crate::desk::WmState;
use crate::shell::{
    alpha, rgb,
    ui::{rect, HAlign, ShellDraw},
    ShellTokens,
};
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The thumbnail's largest size, its distance from the screen's corner, how
/// long it stays and how long it takes to come in and go out (seconds).
pub const THUMB_MAX: (f64, f64) = (240.0, 160.0);
pub const THUMB_MARGIN: f64 = 20.0;
pub const THUMB_LIFE: f64 = 5.0;
pub const THUMB_EASE: f64 = 0.22;
/// A press on the thumbnail becomes a drag after the pointer moved this far.
pub const DRAG_SLOP: f64 = 4.0;
/// Releasing a dragged thumbnail within this far of the screen's right edge
/// sends it away (dismisses it); the file stays saved.
pub const SWIPE_EDGE: f64 = 36.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A rectangle the person selects (or a window they pick).
    Region,
    /// The whole screen the pointer is on.
    Screen,
}

/// A presented frame as the readback delivers it: device pixels, top-down RGBA.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What the writer thread made.
pub struct Saved {
    pub path: PathBuf,
    /// The picture's own size in pixels.
    pub size: (u32, u32),
    /// A BGRA thumbnail for the overlay, `thumb_size` pixels.
    pub thumb: Vec<u32>,
    pub thumb_size: (usize, usize),
}

pub type SaveSlot = Arc<Mutex<Option<Result<Saved, String>>>>;

pub struct Select {
    pub frame: Arc<Frame>,
    /// Device pixels per point of the window.
    pub scale: f64,
    /// The screen being captured, in window points: the selection stays in it.
    pub screen: Rect,
    /// Space: the next click takes the window under the pointer.
    pub pick_window: bool,
    /// Where the drag began.
    pub anchor: Option<Vec2d>,
    pub cursor: Vec2d,
    /// The window under the pointer while picking (its shown rect).
    pub window: Option<Rect>,
}

pub enum Phase {
    Idle,
    /// A frame has been asked for: the next one presented is taken.
    Grabbing { kind: Kind, sink: u64, slot: Arc<Mutex<Option<Frame>>>, tries: u32, screen: Rect },
    Selecting(Select),
    /// A picked window's own texture is being read back (`Cx::request_render_texture_capture`):
    /// `px` is the size of the frame inside it.
    Capturing { texture: Texture, px: (u32, u32), tries: u32 },
}

pub struct ThumbDrag {
    pub from: Vec2d,
    pub cursor: Vec2d,
    pub moved: bool,
}

pub struct Thumb {
    pub path: PathBuf,
    pub texture: Texture,
    pub thumb_size: (usize, usize),
    /// Seconds of its life left; held while the pointer is on it.
    pub left: f64,
    pub hovered: bool,
    /// 0 = out of view, 1 = in place (the ease in and out).
    pub shown: f64,
    /// Sent away or opened: it eases out and goes.
    pub leaving: bool,
    pub drag: Option<ThumbDrag>,
    /// The screen it floats on, in window points.
    pub screen: Rect,
}

pub struct Shot {
    pub phase: Phase,
    pub saving: Option<SaveSlot>,
    pub thumb: Option<Thumb>,
}

impl Default for Shot {
    fn default() -> Self {
        Self { phase: Phase::Idle, saving: None, thumb: None }
    }
}

impl Shot {
    /// A capture is in progress (frame asked for, or selecting): a second hotkey waits.
    pub fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }
    pub fn selecting(&self) -> bool {
        matches!(self.phase, Phase::Selecting(_))
    }
    /// Something needs frames: the grab, the writer, the thumbnail's clock.
    pub fn animating(&self) -> bool {
        self.busy() || self.saving.is_some() || self.thumb.is_some()
    }
}

// ---------------------------------------------------------------- geometry

/// The rectangle between two corners, inside `bounds`.
pub fn selection_rect(a: Vec2d, b: Vec2d, bounds: Rect) -> Rect {
    let clamp = |p: Vec2d| {
        dvec2(
            p.x.clamp(bounds.pos.x, bounds.pos.x + bounds.size.x),
            p.y.clamp(bounds.pos.y, bounds.pos.y + bounds.size.y),
        )
    };
    let (a, b) = (clamp(a), clamp(b));
    rect(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
}

/// `r` (window points) as a pixel rect of a frame `scale` pixels per point,
/// inside a `width` x `height` frame. None when nothing is left of it.
pub fn pixel_rect(r: Rect, scale: f64, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let x0 = (r.pos.x * scale).round().clamp(0.0, width as f64);
    let y0 = (r.pos.y * scale).round().clamp(0.0, height as f64);
    let x1 = ((r.pos.x + r.size.x) * scale).round().clamp(0.0, width as f64);
    let y1 = ((r.pos.y + r.size.y) * scale).round().clamp(0.0, height as f64);
    (x1 > x0 && y1 > y0).then(|| (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// A readback's BGRA rows (`width` x `height`, tightly packed) as the top-left
/// `px` of it, RGBA. None when `px` does not fit.
pub fn bgra_to_frame(bytes: &[u8], width: usize, height: usize, px: (u32, u32)) -> Option<Frame> {
    let (w, h) = (px.0 as usize, px.1 as usize);
    if w == 0 || h == 0 || w > width || h > height || bytes.len() < width * height * 4 {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for p in bytes[y * width * 4..(y * width + w) * 4].chunks_exact(4) {
            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    Some(Frame { width: px.0, height: px.1, rgba })
}

pub fn crop(frame: &Frame, (x, y, w, h): (u32, u32, u32, u32)) -> Frame {
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for row in y..y + h {
        let start = (row as usize * frame.width as usize + x as usize) * 4;
        rgba.extend_from_slice(&frame.rgba[start..start + w as usize * 4]);
    }
    Frame { width: w, height: h, rgba }
}

/// A box-filtered copy of `frame` no larger than `max` pixels each way, as
/// BGRA `u32`s (what a texture takes).
pub fn thumbnail(frame: &Frame, max: (usize, usize)) -> ((usize, usize), Vec<u32>) {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let k = (w as f64 / max.0 as f64).max(h as f64 / max.1 as f64).max(1.0);
    let (tw, th) = (((w as f64 / k).round() as usize).max(1), ((h as f64 / k).round() as usize).max(1));
    let mut out = Vec::with_capacity(tw * th);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1).min(h));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1).min(w));
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = (y * w + x) * 4;
                    r += frame.rgba[p] as u32;
                    g += frame.rgba[p + 1] as u32;
                    b += frame.rgba[p + 2] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            out.push(0xff00_0000 | ((r / n) << 16) | ((g / n) << 8) | (b / n));
        }
    }
    ((tw, th), out)
}

pub fn encode_png(frame: &Frame) -> Result<Vec<u8>, String> {
    use makepad_widgets::makepad_zune_png::{
        makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
        PngEncoder,
    };
    let expected = frame.width as usize * frame.height as usize * 4;
    if frame.rgba.len() != expected || expected == 0 {
        return Err("the frame has no pixels".into());
    }
    let options = EncoderOptions::default()
        .set_width(frame.width as usize)
        .set_height(frame.height as usize)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    let mut out = Vec::new();
    PngEncoder::new(&frame.rgba, options).encode(&mut out).map_err(|e| format!("png: {e:?}"))?;
    Ok(out)
}

/// `screenshot-YYYY-MM-DD-HHMMSS.png`.
pub fn file_name(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> String {
    format!("screenshot-{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}{second:02}.png")
}

/// `dir/name`, or `dir/name-2.png`, `-3`... when taken.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = name.strip_suffix(".png").unwrap_or(name);
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}.png")))
        .find(|p| !p.exists())
        .expect("an unused name")
}

pub fn screenshots_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Screenshots"))
}

/// The writer: encodes the picture, saves it and makes the overlay's thumbnail.
pub fn save_async(frame: Frame, slot: SaveSlot) {
    std::thread::spawn(move || {
        let result = (|| {
            let dir = screenshots_dir().ok_or("no home folder")?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let now = makepad_civil_time::local(makepad_civil_time::now_secs()).ok_or("no local time")?;
            let name = file_name(now.year, now.month, now.day, now.hour, now.minute, now.second);
            let path = unique_path(&dir, &name);
            let png = encode_png(&frame)?;
            std::fs::write(&path, png).map_err(|e| format!("{}: {e}", path.display()))?;
            let (thumb_size, thumb) = thumbnail(&frame, (THUMB_MAX.0 as usize * 2, THUMB_MAX.1 as usize * 2));
            Ok(Saved { path, size: (frame.width, frame.height), thumb, thumb_size })
        })();
        *slot.lock().unwrap() = Some(result);
    });
}

/// Where the thumbnail sits on `screen` when `shown` of the way in: its
/// corner rect eased in from the right.
pub fn thumb_rect(screen: Rect, thumb_size: (usize, usize), shown: f64) -> Rect {
    let k = (THUMB_MAX.0 / thumb_size.0.max(1) as f64).min(THUMB_MAX.1 / thumb_size.1.max(1) as f64).min(1.0);
    let w = (thumb_size.0 as f64 * k).round().max(40.0);
    let h = (thumb_size.1 as f64 * k).round().max(30.0);
    let rest_x = screen.pos.x + screen.size.x - w - THUMB_MARGIN;
    let y = screen.pos.y + screen.size.y - h - THUMB_MARGIN;
    let t = shown.clamp(0.0, 1.0);
    let ease = 1.0 - (1.0 - t).powi(3);
    rect(rest_x + (1.0 - ease) * (w + THUMB_MARGIN + 8.0), y, w, h)
}

// ----------------------------------------------------------------- overlay

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawShotImage::script_shader(vm)) {
        ..mod.draw.DrawQuad
        tex: texture_2d(float)
        fade: 1.0
        pixel: fn() {
            let c = self.tex.sample(self.pos)
            return vec4(c.rgb, 1.0) * self.fade
        }
    }

    mod.widgets.ShotOverlayBase = #(ShotOverlay::register_widget(vm))
    mod.widgets.ShotOverlay = set_type_default() do mod.widgets.ShotOverlayBase {
        width: Fill height: Fill
        d +: {text.text_style: theme.font_regular text_bold.text_style: theme.font_bold}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawShotImage {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub fade: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ShotOverlay {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    d: ShellDraw,
    #[live]
    image: DrawShotImage,
    #[live]
    tokens: ShellTokens,
    #[redraw]
    #[area]
    #[rust]
    area: Area,
}

impl Widget for ShotOverlay {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        if let Some(state) = scope.data.get_mut::<WmState>() {
            let tok = self.tokens;
            let whole = cx.turtle().rect();
            match &state.shot.phase {
                Phase::Selecting(s) => self.draw_select(cx, &tok, s, whole),
                Phase::Idle | Phase::Grabbing { .. } | Phase::Capturing { .. } => {}
            }
            if let Some(thumb) = &state.shot.thumb {
                self.draw_thumb(cx, &tok, thumb);
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl ShotOverlay {
    fn draw_select(&mut self, cx: &mut Cx2d, tok: &ShellTokens, s: &Select, whole: Rect) {
        let dim = alpha(rgb(0, 0, 0), 0.5);
        let line = alpha(tok.popups.text, 0.9);
        let screen = s.screen;
        // The selected rect: a dragged one, or the window under the pointer.
        let selected = if s.pick_window {
            s.window
        } else {
            s.anchor.map(|a| selection_rect(a, s.cursor, screen)).filter(|r| r.size.x >= 1.0 && r.size.y >= 1.0)
        };
        match selected {
            Some(r) => {
                // The outside, dimmed in four parts around the selection.
                let (x0, y0, x1, y1) = (r.pos.x, r.pos.y, r.pos.x + r.size.x, r.pos.y + r.size.y);
                let (sx0, sy0, sx1, sy1) = (screen.pos.x, screen.pos.y, screen.pos.x + screen.size.x, screen.pos.y + screen.size.y);
                self.d.solid(cx, rect(sx0, sy0, sx1 - sx0, y0 - sy0), dim);
                self.d.solid(cx, rect(sx0, y1, sx1 - sx0, sy1 - y1), dim);
                self.d.solid(cx, rect(sx0, y0, x0 - sx0, y1 - y0), dim);
                self.d.solid(cx, rect(x1, y0, sx1 - x1, y1 - y0), dim);
                // The ring.
                self.d.solid(cx, rect(x0 - 1.0, y0 - 1.0, r.size.x + 2.0, 1.0), line);
                self.d.solid(cx, rect(x0 - 1.0, y1, r.size.x + 2.0, 1.0), line);
                self.d.solid(cx, rect(x0 - 1.0, y0, 1.0, r.size.y), line);
                self.d.solid(cx, rect(x1, y0, 1.0, r.size.y), line);
                if s.pick_window {
                    self.d.solid(cx, r, alpha(tok.notifications.countdown, 0.18));
                }
                self.readout(cx, tok, s, r, whole);
            }
            None => {
                self.d.solid(cx, screen, dim);
            }
        }
        if !s.pick_window {
            // The crosshair through the pointer.
            let faint = alpha(tok.popups.text, 0.55);
            self.d.solid(cx, rect(screen.pos.x, s.cursor.y, screen.size.x, 1.0), faint);
            self.d.solid(cx, rect(s.cursor.x, screen.pos.y, 1.0, screen.size.y), faint);
        }
        // The hint, top centre of the screen.
        let text = if s.pick_window {
            "Click a window   Space: select a rectangle   Esc: cancel"
        } else {
            "Drag to select   Space: pick a window   Esc: cancel"
        };
        let px = tok.font.body_small;
        let w = self.d.measure(cx, false, px, text) + 28.0;
        let card = rect(screen.pos.x + (screen.size.x - w) * 0.5, screen.pos.y + 44.0, w, px + 16.0);
        self.d.card(cx, card, &tok.popups);
        self.d.label(cx, card, false, px, tok.popups.text, HAlign::Center, text);
    }

    /// The size of the selection, in pixels of the picture, by its corner.
    fn readout(&mut self, cx: &mut Cx2d, tok: &ShellTokens, s: &Select, r: Rect, whole: Rect) {
        let (w, h) = ((r.size.x * s.scale).round() as u32, (r.size.y * s.scale).round() as u32);
        let text = format!("{w} x {h}");
        let px = tok.font.body_small;
        let tw = self.d.measure(cx, true, px, &text) + 16.0;
        let th = px + 10.0;
        let mut x = r.pos.x + r.size.x - tw;
        let mut y = r.pos.y + r.size.y + 6.0;
        if y + th > s.screen.pos.y + s.screen.size.y {
            y = r.pos.y - th - 6.0;
        }
        if y < whole.pos.y {
            y = r.pos.y + 6.0;
        }
        x = x.max(s.screen.pos.x + 4.0);
        let chip = rect(x, y, tw, th);
        self.d.card(cx, chip, &tok.popups);
        self.d.label(cx, chip, true, px, tok.popups.text, HAlign::Center, &text);
    }

    fn draw_thumb(&mut self, cx: &mut Cx2d, tok: &ShellTokens, thumb: &Thumb) {
        let mut r = thumb_rect(thumb.screen, thumb.thumb_size, thumb.shown);
        let mut fade = 1.0f32;
        if let Some(drag) = thumb.drag.as_ref().filter(|d| d.moved) {
            // Carried: it follows the pointer by the point it was grabbed at.
            let home = thumb_rect(thumb.screen, thumb.thumb_size, 1.0);
            r = rect(
                home.pos.x + drag.cursor.x - drag.from.x,
                home.pos.y + drag.cursor.y - drag.from.y,
                home.size.x,
                home.size.y,
            );
            fade = 0.92;
        }
        let border = tok.popups.border_width.min(2.0);
        let card = rect(r.pos.x - border - 3.0, r.pos.y - border - 3.0, r.size.x + 2.0 * (border + 3.0), r.size.y + 2.0 * (border + 3.0));
        // A soft edge under it: a darker, wider fill.
        self.d.solid(cx, rect(card.pos.x - 2.0, card.pos.y - 1.0, card.size.x + 4.0, card.size.y + 5.0), alpha(rgb(0, 0, 0), 0.28 * fade));
        self.d.card(cx, card, &tok.popups);
        self.image.draw_vars.set_texture(0, &thumb.texture);
        self.image.fade = fade;
        self.image.draw_abs(cx, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32) -> Frame {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[x as u8, y as u8, 7, 255]);
            }
        }
        Frame { width: w, height: h, rgba }
    }

    #[test]
    fn a_selection_is_the_rect_between_two_corners_inside_the_screen() {
        let screen = rect(0.0, 0.0, 1000.0, 600.0);
        let r = selection_rect(dvec2(300.0, 400.0), dvec2(100.0, 150.0), screen);
        assert_eq!((r.pos.x, r.pos.y, r.size.x, r.size.y), (100.0, 150.0, 200.0, 250.0));
        // Dragged past an edge: held at it.
        let r = selection_rect(dvec2(900.0, 500.0), dvec2(1200.0, 700.0), screen);
        assert_eq!((r.pos.x, r.pos.y, r.size.x, r.size.y), (900.0, 500.0, 100.0, 100.0));
    }

    #[test]
    fn points_become_pixels_at_the_frame_scale_and_stay_inside_it() {
        let r = rect(10.0, 20.0, 100.5, 50.0);
        assert_eq!(pixel_rect(r, 2.0, 2560, 1600), Some((20, 40, 201, 100)));
        assert_eq!(pixel_rect(rect(1270.0, 790.0, 100.0, 100.0), 2.0, 2560, 1600), Some((2540, 1580, 20, 20)));
        assert_eq!(pixel_rect(rect(2000.0, 10.0, 10.0, 10.0), 1.0, 1280, 800), None);
        assert_eq!(pixel_rect(rect(5.0, 5.0, 0.2, 10.0), 1.0, 1280, 800), None);
    }

    #[test]
    fn a_window_texture_readback_becomes_the_frame_inside_it() {
        // 4x3 texture of BGRA pixels; the frame is its top-left 3x2.
        let mut bytes = Vec::new();
        for i in 0..12u8 {
            bytes.extend_from_slice(&[i, 100 + i, 200, 0]);
        }
        let f = bgra_to_frame(&bytes, 4, 3, (3, 2)).unwrap();
        assert_eq!((f.width, f.height), (3, 2));
        assert_eq!(&f.rgba[0..4], &[200, 100, 0, 255]);
        assert_eq!(&f.rgba[12..16], &[200, 104, 4, 255], "the second row starts at texel 4");
        assert!(bgra_to_frame(&bytes, 4, 3, (5, 2)).is_none());
        assert!(bgra_to_frame(&bytes[..8], 4, 3, (3, 2)).is_none());
    }

    #[test]
    fn a_crop_holds_the_pixels_of_its_rect() {
        let f = frame(40, 30);
        let c = crop(&f, (5, 10, 8, 4));
        assert_eq!((c.width, c.height), (8, 4));
        assert_eq!(&c.rgba[0..4], &[5, 10, 7, 255]);
        let last = c.rgba.len() - 4;
        assert_eq!(&c.rgba[last..], &[12, 13, 7, 255]);
    }

    #[test]
    fn a_thumbnail_keeps_the_aspect_and_never_grows() {
        let f = frame(1000, 500);
        let ((w, h), px) = thumbnail(&f, (250, 250));
        assert_eq!((w, h), (250, 125));
        assert_eq!(px.len(), 250 * 125);
        assert!(px.iter().all(|p| p >> 24 == 0xff));
        let small = frame(40, 30);
        let ((w, h), _) = thumbnail(&small, (250, 250));
        assert_eq!((w, h), (40, 30));
    }

    #[test]
    fn the_file_is_named_by_the_second_and_a_taken_name_gets_a_number() {
        assert_eq!(file_name(2026, 10, 12, 9, 5, 3), "screenshot-2026-10-12-090503.png");
        let dir = std::env::temp_dir().join(format!("wm-shot-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = file_name(2026, 10, 12, 9, 5, 3);
        let first = unique_path(&dir, &name);
        assert_eq!(first, dir.join(&name));
        std::fs::write(&first, b"x").unwrap();
        let second = unique_path(&dir, &name);
        assert_eq!(second, dir.join("screenshot-2026-10-12-090503-2.png"));
        std::fs::write(&second, b"x").unwrap();
        assert_eq!(unique_path(&dir, &name), dir.join("screenshot-2026-10-12-090503-3.png"));
        std::fs::remove_file(&first).unwrap();
        std::fs::remove_file(&second).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn a_png_encodes_and_decodes_to_the_same_pixels() {
        use makepad_widgets::makepad_zune_png::{makepad_zune_core::bytestream::ZCursor, PngDecoder};
        let f = frame(16, 9);
        let png = encode_png(&f).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let mut d = PngDecoder::new(ZCursor::new(&png));
        let raw = d.decode_raw().unwrap();
        assert_eq!(raw, f.rgba);
        assert!(encode_png(&Frame { width: 0, height: 0, rgba: Vec::new() }).is_err());
    }

    #[test]
    fn the_thumbnail_floats_in_the_screen_corner_and_eases_in_from_the_right() {
        let screen = rect(100.0, 0.0, 1280.0, 800.0);
        let rest = thumb_rect(screen, (480, 320), 1.0);
        assert_eq!((rest.size.x, rest.size.y), (240.0, 160.0));
        assert_eq!(rest.pos.x + rest.size.x, screen.pos.x + screen.size.x - THUMB_MARGIN);
        assert_eq!(rest.pos.y + rest.size.y, screen.pos.y + screen.size.y - THUMB_MARGIN);
        let start = thumb_rect(screen, (480, 320), 0.0);
        assert!(start.pos.x >= screen.pos.x + screen.size.x, "out of view before it comes in");
        let mid = thumb_rect(screen, (480, 320), 0.5);
        assert!(mid.pos.x > rest.pos.x && mid.pos.x < start.pos.x);
        // A small picture is shown at its own size.
        let small = thumb_rect(screen, (100, 60), 1.0);
        assert_eq!((small.size.x, small.size.y), (100.0, 60.0));
    }
}
