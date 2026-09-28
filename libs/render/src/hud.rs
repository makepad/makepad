//! 2D overlay drawing, moved verbatim from gamemaker's draw_walk: HUD text
//! slots + gauges + crosshair, and the billboard nametags. The draw structs
//! are lent by the host widget (they carry its theme styling).

use makepad_draw::*;
use makepad_scene::{HudAnchor, HudBar, HudSlot};

#[path = "hud_flight.rs"]
mod flight;
pub use flight::*;

/// The draws the slot HUD is lent: the sans face for banners and hints, the
/// code face for readouts and gauge labels, and one flat-colour rect for
/// plates, ticks and gauges.
pub struct HudOverlayDraws<'a> {
    pub text: &'a mut DrawText,
    pub code: &'a mut DrawText,
    pub rect: &'a mut DrawColor,
}

/// One laid-out piece of the slot HUD: a block of text rows, or a gauge.
struct OverlayItem {
    anchor: HudAnchor,
    lines: Vec<String>,
    size: f32,
    color: Vec4f,
    mono: bool,
    banner: bool,
    /// A gauge: (fraction, label).
    bar: Option<(f32, String)>,
    w: f64,
    h: f64,
    line_h: f64,
}

const PAD_X: f64 = 7.0;
const PAD_Y: f64 = 3.0;
const GAP: f64 = 5.0;
const MARGIN: f64 = 12.0;
const BAR_W: f64 = 120.0;
const BAR_H: f64 = 8.0;
const BAR_LABEL: f32 = 8.0;
/// A slot wraps onto at most this many rows (or as many as its author wrote
/// with newlines, if more); the last one ends in an
/// ellipsis. Beyond it a HUD line is a paragraph, and a paragraph over the
/// game is the collision this layout exists to prevent.
const MAX_ROWS: usize = 4;

fn measure(cx: &mut Cx2d, draw: &mut DrawText, size: f32, text: &str) -> (f64, f64) {
    draw.text_style.font_size = size;
    let l = draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    (l.size_in_lpxs.width as f64, l.size_in_lpxs.height as f64)
}

/// Break `text` into rows no wider than `max_w`: its own newlines first,
/// then greedily by words; a word wider than the row is cut. Past
/// `MAX_ROWS` the last row ends in an ellipsis.
fn wrap_rows(cx: &mut Cx2d, draw: &mut DrawText, size: f32, text: &str, max_w: f64) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut row = String::new();
        for word in para.split(' ') {
            let candidate = if row.is_empty() { word.to_string() } else { format!("{row} {word}") };
            if measure(cx, draw, size, &candidate).0 <= max_w || row.is_empty() {
                row = candidate;
            } else {
                rows.push(std::mem::take(&mut row));
                row = word.to_string();
            }
        }
        rows.push(row);
    }
    // Trailing blanks from a double space, and the empty last row of a text
    // that ends in a newline, would stack empty plates.
    for row in rows.iter_mut() {
        *row = row.trim_end().to_string();
    }
    while rows.len() > 1 && rows.last().is_some_and(|r| r.is_empty()) {
        rows.pop();
    }
    // The cap is on WRAPPING: rows the author wrote are all kept.
    let cap = MAX_ROWS.max(text.trim_end().split('\n').count());
    let mut ellipsize = |row: &mut String, force: bool| {
        if !force && measure(cx, draw, size, row).0 <= max_w {
            return;
        }
        loop {
            let with = format!("{row}…");
            if measure(cx, draw, size, &with).0 <= max_w || row.chars().count() <= 1 {
                *row = with;
                return;
            }
            row.pop();
        }
    };
    if rows.len() > cap {
        rows.truncate(cap);
        let last = rows.last_mut().unwrap();
        ellipsize(last, true);
    }
    for row in rows.iter_mut() {
        ellipsize(row, false);
    }
    rows
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.pos.x < b.pos.x + b.size.x
        && b.pos.x < a.pos.x + a.size.x
        && a.pos.y < b.pos.y + b.size.y
        && b.pos.y < a.pos.y + a.size.y
}

/// HUD: named text slots and gauges pinned to seven anchors. Each anchor is
/// a stack (top anchors grow downward, bottom ones upward, in insertion
/// order); every slot is wrapped to its anchor's width and ellipsised past
/// four rows; and the stacks are placed so they never cover each other or
/// `avoid` (the composed HUD's panels): corners first, then the top strip,
/// the banner and the bottom strip, each pushed clear of what is already
/// down. Every block stands on a dark plate so it reads over a white sky and
/// a black floor alike. "center" is the banner, "hint" the small control
/// help. Color/size of 0 = defaults; explicit slot colours stay exact.
pub fn draw_hud_overlay(
    cx: &mut Cx2d,
    rect: Rect,
    draws: &mut HudOverlayDraws,
    slots: &[(String, HudSlot)],
    bars: &[HudBar],
    crosshair: bool,
    avoid: &[Rect],
) {
    let style = HudStyle::default();
    let default_color = style.ink;
    let hint_color = style.caption;
    let corner_w = (rect.size.x * 0.38).max(120.0);
    let strip_w = (rect.size.x - 2.0 * MARGIN).max(80.0);

    let mut items: Vec<OverlayItem> = Vec::new();
    for (name, slot) in slots {
        if slot.text.is_empty() {
            continue;
        }
        // A slot NAMED "center" is the banner wherever the author left it at
        // the default anchor: that is what the name has always promised.
        let anchor = if name == "center" && slot.anchor == HudAnchor::TopLeft { HudAnchor::Center } else { slot.anchor };
        let banner = anchor == HudAnchor::Center;
        let default_size = match name.as_str() {
            "center" => 22.0,
            "top" => 15.0,
            "hint" => 9.0,
            _ => 12.0,
        };
        let size = if slot.size > 0.0 { slot.size } else { default_size };
        let color = if slot.color.w > 0.0 {
            slot.color
        } else if name == "hint" {
            hint_color
        } else {
            default_color
        };
        // Readouts in the corners are numbers people watch change: the code
        // face keeps their digits from jittering. A corner slot with no digit
        // in it is prose (a level's one-line brief), and prose, banners and
        // hints stay in the sans face.
        let mono = matches!(anchor, HudAnchor::TopLeft | HudAnchor::TopRight | HudAnchor::BottomRight)
            && name != "hint"
            && slot.text.chars().any(|c| c.is_ascii_digit());
        // Corner readouts keep to a narrow column; prose in a corner (the
        // control help) may take most of the width, and the stacks placed
        // after it step clear of whatever it covers.
        let max_w = match anchor {
            HudAnchor::Top | HudAnchor::Center | HudAnchor::Bottom => strip_w,
            _ if mono => corner_w,
            _ => (rect.size.x * 0.62).max(corner_w),
        } - 2.0 * PAD_X;
        let draw = if mono { &mut *draws.code } else { &mut *draws.text };
        let lines = wrap_rows(cx, draw, size, &slot.text, max_w.max(40.0));
        let mut w: f64 = 0.0;
        let mut line_h: f64 = 0.0;
        for line in &lines {
            let (lw, lh) = measure(cx, draw, size, if line.is_empty() { " " } else { line });
            w = w.max(lw);
            line_h = line_h.max(lh);
        }
        let h = line_h * lines.len() as f64;
        items.push(OverlayItem {
            anchor,
            lines,
            size,
            color,
            mono,
            banner,
            bar: None,
            w: w + 2.0 * PAD_X,
            h: h + 2.0 * PAD_Y,
            line_h,
        });
    }
    for bar in bars {
        let label = if bar.name.starts_with('_') { String::new() } else { bar.name.to_uppercase() };
        let label_w = if label.is_empty() { 0.0 } else { measure(cx, draws.code, BAR_LABEL, &label).0 + 6.0 };
        items.push(OverlayItem {
            anchor: bar.anchor,
            lines: Vec::new(),
            size: BAR_LABEL,
            color: bar.color,
            mono: true,
            banner: false,
            bar: Some((bar.fraction.clamp(0.0, 1.0), label)),
            w: label_w + BAR_W + 2.0 * PAD_X,
            h: 14.0,
            line_h: 0.0,
        });
    }

    // Place the stacks. Each is one block: its width the widest item, its
    // height the items and gaps; the block moves as a whole.
    let order = [
        HudAnchor::TopLeft,
        HudAnchor::TopRight,
        HudAnchor::BottomLeft,
        HudAnchor::BottomRight,
        HudAnchor::Top,
        HudAnchor::Center,
        HudAnchor::Bottom,
    ];
    let mut taken: Vec<Rect> = avoid.to_vec();
    let mut placed: Vec<(usize, f64, f64)> = Vec::new();
    for anchor in order {
        let members: Vec<usize> = (0..items.len()).filter(|i| items[*i].anchor == anchor).collect();
        if members.is_empty() {
            continue;
        }
        let block_w = members.iter().map(|i| items[*i].w).fold(0.0, f64::max);
        let block_h = members.iter().map(|i| items[*i].h).sum::<f64>() + GAP * (members.len() - 1) as f64;
        let down = matches!(anchor, HudAnchor::TopLeft | HudAnchor::Top | HudAnchor::TopRight | HudAnchor::Center);
        let x = match anchor {
            HudAnchor::TopLeft | HudAnchor::BottomLeft => rect.pos.x + MARGIN,
            HudAnchor::TopRight | HudAnchor::BottomRight => rect.pos.x + rect.size.x - MARGIN - block_w,
            _ => rect.pos.x + (rect.size.x - block_w) * 0.5,
        };
        let mut y = match anchor {
            HudAnchor::Center => rect.pos.y + rect.size.y * 0.22,
            _ if down => rect.pos.y + MARGIN,
            _ => rect.pos.y + rect.size.y - MARGIN - block_h,
        };
        // Step clear of every block already down: below it for a top stack,
        // above it for a bottom one. Bounded by the pane: a stack that cannot
        // fit keeps its last spot rather than leaving the screen.
        for _ in 0..taken.len() + 1 {
            let block = Rect { pos: dvec2(x, y), size: dvec2(block_w, block_h) };
            let Some(hit) = taken.iter().find(|t| overlaps(block, **t)) else { break };
            let next = if down { hit.pos.y + hit.size.y + GAP } else { hit.pos.y - GAP - block_h };
            if next < rect.pos.y || next + block_h > rect.pos.y + rect.size.y {
                break;
            }
            y = next;
        }
        taken.push(Rect { pos: dvec2(x, y), size: dvec2(block_w, block_h) });
        // Within the block, top stacks read downward, bottom stacks upward
        // (the first item nearest its edge), each item aligned to its side.
        let mut cursor = 0.0;
        let seq: Vec<usize> = if down { members.clone() } else { members.iter().rev().cloned().collect() };
        for i in seq {
            let it = &items[i];
            let ix = match anchor {
                HudAnchor::TopLeft | HudAnchor::BottomLeft => x,
                HudAnchor::TopRight | HudAnchor::BottomRight => x + block_w - it.w,
                _ => x + (block_w - it.w) * 0.5,
            };
            placed.push((i, ix, y + cursor));
            cursor += it.h + GAP;
        }
    }

    for (i, x, y) in placed {
        let it = &items[i];
        let r = Rect { pos: dvec2(x, y), size: dvec2(it.w, it.h) };
        // The plate: near-black glass, lighter than nothing, darker than any
        // sky. Readouts carry a short accent tick on their leading edge, the
        // banner a hairline above and below.
        draws.rect.color = vec4(style.plate.x, style.plate.y, style.plate.z, if it.banner { 0.72 } else { 0.58 });
        draws.rect.draw_abs(cx, r);
        let tick = if it.bar.is_some() { it.color } else { style.accent };
        if it.banner {
            draws.rect.color = vec4(style.accent.x, style.accent.y, style.accent.z, 0.55);
            draws.rect.draw_abs(cx, Rect { pos: r.pos, size: dvec2(r.size.x, 1.0) });
            draws.rect.draw_abs(cx, Rect { pos: dvec2(r.pos.x, r.pos.y + r.size.y - 1.0), size: dvec2(r.size.x, 1.0) });
        } else if it.mono {
            let right = matches!(it.anchor, HudAnchor::TopRight | HudAnchor::BottomRight);
            let tx = if right { r.pos.x + r.size.x - 2.0 } else { r.pos.x };
            draws.rect.color = vec4(tick.x, tick.y, tick.z, 0.85);
            draws.rect.draw_abs(cx, Rect { pos: dvec2(tx, r.pos.y), size: dvec2(2.0, r.size.y) });
        }
        if let Some((fraction, label)) = &it.bar {
            let mut bx = x + PAD_X;
            if !label.is_empty() {
                draws.code.text_style.font_size = BAR_LABEL;
                draws.code.color = style.caption;
                draws.code.draw_abs(cx, dvec2(bx, y + 2.5), label);
                bx += measure(cx, draws.code, BAR_LABEL, label).0 + 6.0;
            }
            let by = y + (it.h - BAR_H) * 0.5;
            draws.rect.color = vec4(it.color.x, it.color.y, it.color.z, 0.18);
            draws.rect.draw_abs(cx, Rect { pos: dvec2(bx, by), size: dvec2(BAR_W, BAR_H) });
            draws.rect.color = it.color;
            draws.rect.draw_abs(cx, Rect { pos: dvec2(bx, by), size: dvec2(BAR_W * *fraction as f64, BAR_H) });
            continue;
        }
        let draw = if it.mono { &mut *draws.code } else { &mut *draws.text };
        draw.text_style.font_size = it.size;
        for (row, line) in it.lines.iter().enumerate() {
            let (lw, _) = measure(cx, draw, it.size, line);
            let lx = match it.anchor {
                HudAnchor::TopRight | HudAnchor::BottomRight => x + it.w - PAD_X - lw,
                HudAnchor::Top | HudAnchor::Center | HudAnchor::Bottom => x + (it.w - lw) * 0.5,
                _ => x + PAD_X,
            };
            let ly = y + PAD_Y + it.line_h * row as f64;
            // A one-point dark shadow on top of the plate: the plate carries
            // legibility, the shadow crispness over its translucent edge.
            draw.color = vec4(0.0, 0.0, 0.0, it.color.w * 0.6);
            draw.draw_abs(cx, dvec2(lx + 1.0, ly + 1.0), line);
            draw.color = it.color;
            draw.draw_abs(cx, dvec2(lx, ly), line);
        }
    }
    // Restore defaults for anyone else using these draws.
    draws.text.text_style.font_size = 22.0;
    draws.text.color = default_color;
    draws.rect.color = vec4(1.0, 1.0, 1.0, 0.9);

    if crosshair {
        let c = dvec2(rect.pos.x + rect.size.x * 0.5, rect.pos.y + rect.size.y * 0.5);
        // A dot and four short ticks: the dot for aim, the ticks so it
        // survives a white background.
        draws.rect.color = vec4(0.0, 0.0, 0.0, 0.5);
        draws.rect.draw_abs(cx, Rect { pos: c - dvec2(3.5, 3.5), size: dvec2(7.0, 7.0) });
        draws.rect.color = vec4(1.0, 1.0, 1.0, 0.95);
        draws.rect.draw_abs(cx, Rect { pos: c - dvec2(2.0, 2.0), size: dvec2(4.0, 4.0) });
        draws.rect.color = vec4(style.accent.x, style.accent.y, style.accent.z, 0.9);
        for (dx, dy, w, h) in [(-11.0, -0.75, 5.0, 1.5), (6.0, -0.75, 5.0, 1.5), (-0.75, -11.0, 1.5, 5.0), (-0.75, 6.0, 1.5, 5.0)] {
            draws.rect.draw_abs(cx, Rect { pos: c + dvec2(dx, dy), size: dvec2(w, h) });
        }
        draws.rect.color = vec4(1.0, 1.0, 1.0, 0.9);
    }
}

/// Billboard nametags: project each anchor into the pane and draw in the 2D
/// overlay — always camera-facing and never hidden by geometry, like the
/// Godot Label3D (billboard + no_depth_test).
pub fn draw_billboard_labels(
    cx: &mut Cx2d,
    rect: Rect,
    scene: &SceneState3D,
    draw_label: &mut DrawText,
    labels: &[(Vec3f, String, Vec4f, f32)],
) {
    for (anchor, text, color, size) in labels {
        let clip = scene.projection.transform_vec4(
            scene
                .view
                .transform_vec4(vec4(anchor.x, anchor.y, anchor.z, 1.0)),
        );
        if clip.w <= 0.1 {
            continue; // behind the camera
        }
        let ndc_x = clip.x / clip.w;
        let ndc_y = clip.y / clip.w;
        if ndc_x < -1.1 || ndc_x > 1.1 || ndc_y < -1.1 || ndc_y > 1.1 {
            continue;
        }
        let px = rect.pos.x + (ndc_x as f64 + 1.0) * 0.5 * rect.size.x;
        let py = rect.pos.y + (1.0 - ndc_y as f64) * 0.5 * rect.size.y;
        draw_label.text_style.font_size = if *size > 0.0 { *size } else { 11.0 };
        draw_label.color = if color.w > 0.0 {
            *color
        } else {
            vec4(1.0, 1.0, 1.0, 0.87)
        };
        // Centre on the anchor (draw_abs is left-anchored).
        let width = draw_label
            .layout(cx, 0.0, 0.0, None, false, Align::default(), text)
            .size_in_lpxs
            .width as f64;
        let at = dvec2(px - width * 0.5, py);
        // Poor-man's outline (Godot Label3D has outline_size 24):
        // four dark offset copies keep names readable against the
        // bright sky.
        let fill = draw_label.color;
        draw_label.color = vec4(0.06, 0.07, 0.1, fill.w * 0.9);
        for (ox, oy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            draw_label.draw_abs(cx, at + dvec2(ox, oy), text);
        }
        draw_label.color = fill;
        draw_label.draw_abs(cx, at, text);
    }
    draw_label.text_style.font_size = 11.0;
    draw_label.color = vec4(1.0, 1.0, 1.0, 0.87);
}

// ---------------------------------------------------------------------------
// The HUD document renderer
// ---------------------------------------------------------------------------

use crate::shaders::{DrawHudImage, DrawHudShape};
use makepad_scene::hud::{
    text_size_for, CAPTION_SIZE, HUD_REFERENCE_HEIGHT, TEXT_SIZE,
};
use makepad_scene::{
    hud_affine_mul, hud_counted, hud_pose, CrosshairStyle, HudDoc, HudElement, HudKind, HudMapDot,
    HudMapFit, HudSeen, HudValue, HUD_AFFINE_IDENTITY,
};

/// The look every element falls back to. One dark plate palette, so a HUD
/// built out of defaults already reads as a game's rather than as a debug
/// overlay — which is the whole difference a author should not have to spend
/// their one attempt on. The palette is the sandbox window's cyberpunk one:
/// near-black glass plates with a cyan hairline, ice-white readouts, cyan
/// gauges, a hot red for low. The plate stays dark and mostly opaque, so the
/// readout holds over a white sky and a black floor alike (UI_DESIGN.md §4).
#[derive(Clone, Copy, Debug)]
pub struct HudStyle {
    pub plate: Vec4f,
    pub plate_border: Vec4f,
    pub plate_radius: f32,
    pub plate_border_width: f32,
    pub ink: Vec4f,
    pub caption: Vec4f,
    pub accent: Vec4f,
    pub low: Vec4f,
    pub track: Vec4f,
}

impl Default for HudStyle {
    fn default() -> Self {
        Self {
            plate: vec4(0.027, 0.031, 0.047, 0.80),
            plate_border: vec4(0.078, 0.941, 1.0, 0.38),
            plate_radius: 6.0,
            plate_border_width: 1.0,
            ink: vec4(0.90, 0.98, 1.0, 1.0),
            caption: vec4(0.55, 0.66, 0.75, 1.0),
            accent: vec4(0.078, 0.941, 1.0, 1.0),
            low: vec4(1.0, 0.23, 0.36, 1.0),
            track: vec4(0.078, 0.941, 1.0, 0.10),
        }
    }
}

/// Everything the renderer cannot work out on its own: what a bind reads,
/// what a catalog image is, and what an SVG icon's source text is.
pub struct HudBinder<'a> {
    /// A number for a `value`/`max`/`count`. `of` is the element's `of` field
    /// (0 = the local player).
    pub number: &'a mut dyn FnMut(&HudValue, u64) -> Option<f32>,
    /// A bind that reads as text — a weapon's name, a player's name.
    pub string: &'a mut dyn FnMut(&str, u64) -> Option<String>,
    /// A catalog image key to a texture and its pixel size.
    pub image: &'a mut dyn FnMut(&str) -> Option<(Texture, f32, f32)>,
    /// Draw one named glyph (a built-in icon or an `svg:` resource path) into
    /// a rect, tinted. The host owns the glyph draws because an SVG has to be
    /// tessellated ONCE and kept: re-parsing the same icon every frame churns
    /// the shared vector/glyph atlas hard enough to evict the application's
    /// own text, which is exactly what it looked like.
    pub glyph: &'a mut dyn FnMut(&mut Cx2d, Rect, &str, Vec4f) -> bool,
    /// The dots a map element shows this frame (its `dots` resolved against
    /// the world, the pane's subject flagged). Appends to the given list.
    pub map_dots: &'a mut dyn FnMut(&HudElement, &mut Vec<HudMapDot>),
    /// The flight HUD's frame for a `flight` element (the subject aircraft,
    /// projected through this pane's camera); None draws nothing.
    pub flight: &'a mut dyn FnMut(&HudElement, Rect) -> Option<FlightHud>,
}

/// The draw structs the host lends (they carry its theme styling).
pub struct HudDraws<'a> {
    pub shape: &'a mut DrawHudShape,
    pub image: &'a mut DrawHudImage,
    pub text: &'a mut DrawText,
    /// Draw lists the host keeps between frames: an element drawn under a
    /// transform (a pop's scale, a callout's skew) is drawn into one of
    /// these with the transform as its view matrix. Grown on demand; only
    /// elements that are transformed THIS frame take one.
    pub lists: &'a mut Vec<DrawList2d>,
    /// How many of `lists` this redraw has already used. A list can hold
    /// one recording per redraw, so every pane drawn in the same redraw
    /// (split screen) continues from here instead of starting at 0 and
    /// overwriting the pane before it; the host zeroes it once per redraw.
    pub lists_used: &'a mut usize,
}

/// Draw the whole HUD document over `rect`, and report the fraction each
/// gauge was asked to show so the host can settle its trailing chip bars.
/// Every drawn element's rect is appended to `occupied`, so the slot HUD
/// drawn after it (`draw_hud_overlay`) can keep its stacks clear of them.
/// `pane` is which split-screen pane this is (0 = the main one): motion
/// state is kept per pane, and moving elements report into `seen` with it.
pub fn draw_hud_doc(
    cx: &mut Cx2d,
    rect: Rect,
    doc: &HudDoc,
    draws: &mut HudDraws,
    style: &HudStyle,
    binder: &mut HudBinder,
    spread: f32,
    occupied: &mut Vec<Rect>,
    seen: &mut Vec<HudSeen>,
    pane: usize,
) -> Vec<(String, f32)> {
    // The HUD is authored against a 1080-high pane. A pane narrower than
    // 1.2 : 1 (a split-screen slice) scales by its WIDTH instead, so a bar
    // laid out for a landscape screen still fits across a portrait one.
    // Every landscape pane scales exactly as before.
    let scale = (rect.size.y as f32 / HUD_REFERENCE_HEIGHT)
        .min(rect.size.x as f32 / (HUD_REFERENCE_HEIGHT * 1.2))
        .max(0.35);
    // Motion is paid for only by a document that has some: a still HUD takes
    // the plain path below with no pose work, no lists and no reports.
    let moving = doc.elements.iter().any(moves);
    let mut fractions: Vec<(String, f32)> = Vec::new();
    // A `when:` bind that is off this frame takes its element OUT of the
    // layout (not just the draw), so a panel shrinks to what it shows
    // instead of keeping empty rows for the hidden ones.
    let gated = doc.elements.iter().any(|e| e.show && !e.when.is_empty()).then(|| {
        let mut d = doc.clone();
        for e in d.elements.iter_mut() {
            if e.show && !e.when.is_empty() && !visible(e, binder) {
                e.show = false;
            }
        }
        d
    });
    let doc = gated.as_ref().unwrap_or(doc);

    // Flashes go UNDER the elements: a damage vignette must not wash out the
    // number that tells you how much damage it was.
    for e in &doc.elements {
        if e.kind == HudKind::Flash && e.show {
            draw_flash(cx, rect, e, draws, style);
        }
    }

    let placed = {
        let mut measure = |text: &str, size: f32| {
            draws.text.text_style.font_size = size;
            let l = draws
                .text
                .layout(cx, 0.0, 0.0, None, false, Align::default(), text);
            (l.size_in_lpxs.width as f32, l.size_in_lpxs.height as f32)
        };
        let mut text_of = |e: &HudElement| element_text(e, binder);
        makepad_scene::hud_layout(
            doc,
            rect.size.x as f32,
            rect.size.y as f32,
            scale,
            &mut measure,
            &mut text_of,
        )
    };

    // One scratch list for every map this frame (normally one map).
    let mut map_dots: Vec<HudMapDot> = Vec::new();
    // Each moving element's pose as an affine about its centre, composed with
    // its containers' (a panel that pops carries its children with it), and
    // its opacity likewise. Indexed like `doc.elements`; empty when still.
    let mut poses: Vec<Option<([f32; 6], f32)>> = Vec::new();
    let mut lists_used = *draws.lists_used;
    if moving {
        let mut rects: Vec<Option<Rect>> = vec![None; doc.elements.len()];
        for p in &placed {
            rects[p.index] = Some(Rect {
                pos: dvec2(rect.pos.x + p.x as f64, rect.pos.y + p.y as f64),
                size: dvec2(p.w as f64, p.h as f64),
            });
        }
        poses = vec![None; doc.elements.len()];
        for i in 0..doc.elements.len() {
            pose_of(doc, i, pane, &rects, scale, &mut poses);
        }
    }
    for p in &placed {
        let e = &doc.elements[p.index];
        if !visible_in(doc, e, binder) {
            if moving && moves(e) {
                seen.push(HudSeen { name: e.name.clone(), pane, visible: false, rect: [0.0; 4], number: None, text_key: 0 });
            }
            continue;
        }
        let at = Rect {
            pos: dvec2(rect.pos.x + p.x as f64, rect.pos.y + p.y as f64),
            size: dvec2(p.w as f64, p.h as f64),
        };
        if !matches!(e.kind, HudKind::Flash | HudKind::Marker) {
            occupied.push(at);
        }
        let pose = if moving { poses[p.index] } else { None };
        let Some((affine, opacity)) = pose else {
            draw_element(cx, at, e, doc, draws, style, scale, binder, &mut map_dots, &mut fractions);
            continue;
        };
        let (number, text_key) = motion_report(e, binder);
        // Moving elements report for their tweens; the still children of a
        // moving container report too, so the container's exit can take
        // them along from where they last stood.
        seen.push(HudSeen {
            name: e.name.clone(),
            pane,
            visible: true,
            rect: [p.x, p.y, p.w, p.h],
            number,
            text_key,
        });
        // A counting number draws the value it has counted to so far.
        let counted = number.filter(|_| e.motion.count > 0.0).map(|n| hud_counted(e, pane, n)).filter(|c| Some(*c) != number);
        let moved = (opacity < 1.0 || counted.is_some()).then(|| {
            let mut m = faded(e, opacity);
            if let Some(c) = counted {
                m.value = HudValue::Fixed(c);
            }
            m
        });
        let faded_style = (opacity < 1.0).then(|| faded_style(style, opacity));
        let e = moved.as_ref().unwrap_or(e);
        let style = faded_style.as_ref().unwrap_or(style);
        with_transform(cx, draws, &mut lists_used, affine, |cx, draws| {
            draw_element(cx, at, e, doc, draws, style, scale, binder, &mut map_dots, &mut fractions);
        });
    }
    // Elements that are no longer laid out but still playing their exit,
    // drawn where they last stood; and the report that they are gone.
    if moving {
        let placed_set: Vec<bool> = {
            let mut v = vec![false; doc.elements.len()];
            for p in &placed {
                v[p.index] = true;
            }
            v
        };
        for (i, e) in doc.elements.iter().enumerate() {
            if placed_set[i] || !moves(e) {
                continue;
            }
            seen.push(HudSeen { name: e.name.clone(), pane, visible: false, rect: [0.0; 4], number: None, text_key: 0 });
            let st = &e.motion_state(pane);
            if !(st.exit_age.is_finite() && st.exit_age < e.motion.exit.secs) || st.rect[2] <= 0.0 {
                continue;
            }
            let at = Rect {
                pos: dvec2(rect.pos.x + st.rect[0] as f64, rect.pos.y + st.rect[1] as f64),
                size: dvec2(st.rect[2] as f64, st.rect[3] as f64),
            };
            let pose = hud_pose(e, pane, scale);
            let centre = vec2f((at.pos.x + at.size.x * 0.5) as f32, (at.pos.y + at.size.y * 0.5) as f32);
            let affine = pose.affine(centre);
            let fs = faded_style(style, pose.opacity);
            // The element and everything inside it, as they last stood.
            let mut going: Vec<(Rect, HudElement)> = vec![(at, faded(e, pose.opacity))];
            for c in doc.elements.iter().filter(|c| inside(doc, c, &e.name)) {
                let r = c.motion_state(pane).rect;
                if r[2] > 0.0 {
                    let cat = Rect {
                        pos: dvec2(rect.pos.x + r[0] as f64, rect.pos.y + r[1] as f64),
                        size: dvec2(r[2] as f64, r[3] as f64),
                    };
                    going.push((cat, faded(c, pose.opacity)));
                }
            }
            with_transform(cx, draws, &mut lists_used, affine, |cx, draws| {
                for (cat, m) in &going {
                    draw_element(cx, *cat, m, doc, draws, &fs, scale, binder, &mut map_dots, &mut fractions);
                }
            });
        }
    }

    *draws.lists_used = lists_used;
    // The flight HUD owns the whole pane, under the markers and crosshair.
    for e in &doc.elements {
        if e.kind == HudKind::Flight && visible(e, binder) {
            if let Some(hud) = (binder.flight)(e, rect) {
                draw_flight(cx, rect, e, draws, &hud);
            }
        }
    }

    for e in &doc.elements {
        if e.kind == HudKind::Marker && e.show && e.pulse.alive() {
            draw_marker(cx, rect, e, draws, style, scale);
        }
    }
    if let Some(c) = &doc.crosshair {
        draw_crosshair(cx, rect, c, draws, scale, spread);
    }
    restore(draws, style);
    fractions
}

/// Whether an element takes part in motion this frame: declared motion, or
/// a one-shot punch still running.
fn moves(e: &HudElement) -> bool {
    !e.motion.is_still()
        || e.motion_panes.iter().any(|st| {
            (st.punch_age.is_finite() && st.punch_age < st.punch.secs)
                || (st.shake_age.is_finite() && st.shake_age < st.shake.secs)
        })
}

/// The composed pose of element `i` (its own over its containers'), memoised
/// in `poses`. None when neither it nor any container moves.
fn pose_of(
    doc: &HudDoc,
    i: usize,
    pane: usize,
    rects: &[Option<Rect>],
    units: f32,
    poses: &mut Vec<Option<([f32; 6], f32)>>,
) -> Option<([f32; 6], f32)> {
    if let Some(p) = poses[i] {
        return Some(p);
    }
    let e = &doc.elements[i];
    let parent = if e.parent.is_empty() {
        None
    } else {
        doc.elements
            .iter()
            .position(|p| p.name == e.parent)
            .filter(|p| *p != i)
            .and_then(|p| pose_of(doc, p, pane, rects, units, poses))
    };
    let own = if moves(e) {
        rects[i].map(|r| {
            let pose = hud_pose(e, pane, units);
            let centre = vec2f((r.pos.x + r.size.x * 0.5) as f32, (r.pos.y + r.size.y * 0.5) as f32);
            (pose.affine(centre), pose.opacity)
        })
    } else {
        None
    };
    let out = match (parent, own) {
        (None, None) => return None,
        (Some(p), None) => p,
        (None, Some(o)) => o,
        (Some((pa, po)), Some((oa, oo))) => (hud_affine_mul(pa, oa), po * oo),
    };
    poses[i] = Some(out);
    Some(out)
}

/// Whether `e` sits (at any depth) inside the element named `ancestor`.
fn inside(doc: &HudDoc, e: &HudElement, ancestor: &str) -> bool {
    let mut at = e;
    for _ in 0..16 {
        if at.parent.is_empty() {
            return false;
        }
        if at.parent == ancestor {
            return true;
        }
        match doc.elements.iter().find(|p| p.name == at.parent) {
            Some(p) => at = p,
            None => return false,
        }
    }
    false
}

/// What change detection watches on an element: its number and its text.
fn motion_report(e: &HudElement, binder: &mut HudBinder) -> (Option<f32>, u64) {
    let number = match e.kind {
        HudKind::Bar | HudKind::Ring | HudKind::Text if !e.value.is_none() => (binder.number)(&e.value, e.of),
        _ => None,
    };
    let text_key = if e.kind == HudKind::Text {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        element_text(e, binder).hash(&mut h);
        h.finish() | 1
    } else {
        0
    };
    (number, text_key)
}

/// A copy of the element with every colour it names taken to `a` of its
/// alpha (a zero alpha still means "the style's own", which the faded style
/// supplies).
fn faded(e: &HudElement, a: f32) -> HudElement {
    let mut m = e.clone();
    if a < 1.0 {
        for c in [&mut m.color, &mut m.track, &mut m.border_color, &mut m.low_color, &mut m.dot_color, &mut m.self_color, &mut m.lead_color] {
            c.w *= a;
        }
    }
    m
}

fn faded_style(style: &HudStyle, a: f32) -> HudStyle {
    let mut s = *style;
    for c in [&mut s.plate, &mut s.plate_border, &mut s.ink, &mut s.caption, &mut s.accent, &mut s.low, &mut s.track] {
        c.w *= a;
    }
    s
}

/// Draw through `affine` (pane pixels to pane pixels): into the next pooled
/// draw list with it as the view matrix, or straight through when it is the
/// identity.
fn with_transform(
    cx: &mut Cx2d,
    draws: &mut HudDraws,
    used: &mut usize,
    affine: [f32; 6],
    draw: impl FnOnce(&mut Cx2d, &mut HudDraws),
) {
    if affine == HUD_AFFINE_IDENTITY {
        draw(cx, draws);
        return;
    }
    if draws.lists.len() <= *used {
        draws.lists.push(DrawList2d::new(cx));
    }
    let id = draws.lists[*used].id();
    *used += 1;
    // Always re-recorded: the list is drawn from inside the host's own
    // redraw, and what it holds (a pose, a counted number) changes every
    // frame it is used. A `redraw_list` here would only take effect on the
    // NEXT redraw and leave this one showing last frame's pose.
    if draws.lists[*used - 1].begin_maybe(cx, true).is_redrawing() {
        let [a, b, c, d, tx, ty] = affine;
        let m = Mat4f { v: [a, b, 0.0, 0.0, c, d, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, tx, ty, 0.0, 1.0] };
        let gen = cx.next_uniform_gen();
        cx.draw_lists[id].set_uniform_view_transform(&m, gen);
        draw(cx, draws);
        draws.lists[*used - 1].end(cx);
    }
}

/// One laid-out element, by kind.
#[allow(clippy::too_many_arguments)]
fn draw_element(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    doc: &HudDoc,
    draws: &mut HudDraws,
    style: &HudStyle,
    scale: f32,
    binder: &mut HudBinder,
    map_dots: &mut Vec<HudMapDot>,
    fractions: &mut Vec<(String, f32)>,
) {
    match e.kind {
        HudKind::Panel => draw_panel(cx, at, e, draws, style, scale),
        HudKind::Bar => {
            let f = gauge_fraction(e, binder);
            fractions.push((e.name.clone(), f));
            draw_bar(cx, at, e, draws, style, scale, f, binder);
        }
        HudKind::Ring => {
            let f = gauge_fraction(e, binder);
            fractions.push((e.name.clone(), f));
            draw_ring(cx, at, e, draws, style, scale, f, binder);
        }
        HudKind::Text => draw_readout(cx, at, e, draws, style, scale, binder),
        HudKind::Icon => draw_icon(cx, at, e, draws, style, scale, binder),
        HudKind::Log => draw_log(cx, at, e, doc, draws, style, scale),
        HudKind::Map => draw_map(cx, at, e, draws, style, scale, binder, map_dots),
        HudKind::Flash | HudKind::Marker | HudKind::Flight => {}
    }
}

/// Visible itself AND inside visible containers: a panel hidden by `when:`
/// or `hud_show` takes everything inside it along (a `free` stack's children
/// would otherwise draw at the hidden panel's origin).
fn visible_in(doc: &HudDoc, e: &HudElement, binder: &mut HudBinder) -> bool {
    let mut at = e;
    for _ in 0..16 {
        if !visible(at, binder) {
            return false;
        }
        if at.parent.is_empty() {
            return true;
        }
        match doc.elements.iter().find(|p| p.name == at.parent) {
            Some(parent) => at = parent,
            None => return true,
        }
    }
    true
}

/// A `when:` bind gates visibility, with a leading `!` inverting it. This is
/// what lets "RELOADING" exist as a declaration rather than as a branch in
/// `on_tick` that can stop running.
fn visible(e: &HudElement, binder: &mut HudBinder) -> bool {
    if !e.show {
        return false;
    }
    if e.when.is_empty() {
        return true;
    }
    let (invert, name) = match e.when.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, e.when.as_str()),
    };
    let on = (binder.number)(&HudValue::Bind(name.to_string()), e.of).unwrap_or(0.0) > 0.5;
    on != invert
}

fn plate(e: &HudElement, style: &HudStyle) -> (Vec4f, Vec4f, f32, f32) {
    let bare = e.style == "bare";
    let frame = e.style == "frame";
    let ground = if e.track.w > 0.0 {
        e.track
    } else if bare || frame {
        vec4(0.0, 0.0, 0.0, 0.0)
    } else {
        style.plate
    };
    let border = if e.border >= 0.0 {
        e.border
    } else if bare {
        0.0
    } else {
        style.plate_border_width
    };
    let border_color = if e.border_color.w > 0.0 {
        e.border_color
    } else {
        style.plate_border
    };
    let radius = if e.radius >= 0.0 { e.radius } else { style.plate_radius };
    (ground, border_color, border, radius)
}

fn draw_panel(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    style: &HudStyle,
    scale: f32,
) {
    let (ground, border_color, border, radius) = plate(e, style);
    if ground.w <= 0.0 && border <= 0.0 {
        return;
    }
    draws.shape.shape = 0.0;
    draws.shape.fill = ground;
    draws.shape.stroke = border_color;
    draws.shape.border = border * scale;
    draws.shape.radius = radius * scale;
    draws.shape.draw_abs(cx, at);
}

/// The colour a gauge or readout draws in, which is the whole of "this is a
/// warning" — a bar that stays green at four hit points is a bar nobody reads.
fn ink_for(e: &HudElement, style: &HudStyle, frac: f32, default: Vec4f) -> Vec4f {
    let base = if e.color.w > 0.0 { e.color } else { default };
    if e.low > 0.0 && frac <= e.low {
        if e.low_color.w > 0.0 {
            e.low_color
        } else {
            style.low
        }
    } else {
        base
    }
}

fn gauge_fraction(e: &HudElement, binder: &mut HudBinder) -> f32 {
    let v = (binder.number)(&e.value, e.of).unwrap_or(0.0);
    let m = (binder.number)(&e.max, e.of).unwrap_or(0.0);
    if m > 0.0 {
        (v / m).clamp(0.0, 1.0)
    } else {
        // A bind that already reads as a fraction (`hp_frac`) carries no max;
        // one that does not is clamped rather than overflowing its track.
        v.clamp(0.0, 1.0)
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_bar(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    style: &HudStyle,
    scale: f32,
    frac: f32,
    binder: &mut HudBinder,
) {
    let cap_h = if e.label.is_empty() {
        0.0
    } else {
        (CAPTION_SIZE * scale) as f64 + 2.0
    };
    let track_rect = Rect {
        pos: dvec2(at.pos.x, at.pos.y + cap_h),
        size: dvec2(at.size.x, at.size.y - cap_h),
    };
    if !e.label.is_empty() {
        draws.text.text_style.font_size = CAPTION_SIZE * scale;
        draws.text.color = style.caption;
        draws.text.draw_abs(cx, at.pos, &e.label);
    }
    let (_, border_color, border, radius) = plate(e, style);
    let track = if e.track.w > 0.0 { e.track } else { style.track };
    let ink = ink_for(e, style, frac, style.accent);
    draws.shape.shape = 0.0;
    draws.shape.fill = track;
    draws.shape.stroke = border_color;
    draws.shape.border = border * scale;
    draws.shape.radius = radius * scale;
    draws.shape.draw_abs(cx, track_rect);

    let inset = (1.5 * scale) as f64;
    let inner_w = (track_rect.size.x - inset * 2.0).max(0.0);
    let inner_h = (track_rect.size.y - inset * 2.0).max(0.0);
    // The trailing chip: what the bar WAS, drawn dim behind what it is, so a
    // hit reads as an event rather than as a jump.
    if e.chip && !e.chip_value.is_nan() && e.chip_value > frac + 0.001 {
        draws.shape.fill = vec4(ink.x, ink.y, ink.z, 0.35);
        draws.shape.border = 0.0;
        draws.shape.draw_abs(
            cx,
            Rect {
                pos: dvec2(track_rect.pos.x + inset, track_rect.pos.y + inset),
                size: dvec2(inner_w * e.chip_value.clamp(0.0, 1.0) as f64, inner_h),
            },
        );
    }
    draws.shape.fill = ink;
    draws.shape.border = 0.0;
    draws.shape.radius = (radius * scale * 0.7) as f32;
    if e.segments > 1 {
        // Pips: a segmented gauge is read at a glance where a continuous one
        // has to be estimated.
        let n = e.segments.min(32);
        let gap = (2.0 * scale) as f64;
        let seg_w = ((inner_w - gap * (n - 1) as f64) / n as f64).max(1.0);
        let lit = (frac * n as f32).round() as u32;
        for i in 0..n {
            if i >= lit {
                break;
            }
            draws.shape.draw_abs(
                cx,
                Rect {
                    pos: dvec2(
                        track_rect.pos.x + inset + i as f64 * (seg_w + gap),
                        track_rect.pos.y + inset,
                    ),
                    size: dvec2(seg_w, inner_h),
                },
            );
        }
    } else if frac > 0.0 {
        draws.shape.draw_abs(
            cx,
            Rect {
                pos: dvec2(track_rect.pos.x + inset, track_rect.pos.y + inset),
                size: dvec2(inner_w * frac as f64, inner_h),
            },
        );
    }
    if e.show_value {
        let text = number_text(e, binder);
        let vsize = ((track_rect.size.y as f32) * 0.72).max(9.0);
        draws.text.text_style.font_size = vsize;
        draws.text.color = style.ink;
        let w = draws
            .text
            .layout(cx, 0.0, 0.0, None, false, Align::default(), &text)
            .size_in_lpxs
            .width as f64;
        draws.text.draw_abs(
            cx,
            dvec2(
                track_rect.pos.x + track_rect.size.x - w - inset * 2.0,
                track_rect.pos.y + (track_rect.size.y - vsize as f64) * 0.5,
            ),
            &text,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ring(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    style: &HudStyle,
    scale: f32,
    frac: f32,
    binder: &mut HudBinder,
) {
    let thickness = if e.thickness > 0.0 { e.thickness } else { 7.0 } * scale;
    let sweep = if e.sweep != 0.0 { e.sweep } else { std::f32::consts::TAU };
    let from = if e.from != 0.0 { e.from } else { -std::f32::consts::FRAC_PI_2 };
    let ink = ink_for(e, style, frac, style.accent);
    draws.shape.shape = 1.0;
    draws.shape.thickness = thickness;
    draws.shape.from = from;
    draws.shape.sweep = sweep;
    // The track first, as a full sweep, so the ring reads as a dial even
    // when it is nearly empty.
    draws.shape.fill = if e.track.w > 0.0 { e.track } else { style.track };
    draws.shape.frac = 1.0;
    draws.shape.draw_abs(cx, at);
    draws.shape.fill = ink;
    draws.shape.frac = frac;
    draws.shape.draw_abs(cx, at);
    draws.shape.shape = 0.0;
    if e.show_value {
        let text = number_text(e, binder);
        let size = ((at.size.y as f32) * 0.34).max(10.0);
        draws.text.text_style.font_size = size;
        draws.text.color = ink;
        let w = draws
            .text
            .layout(cx, 0.0, 0.0, None, false, Align::default(), &text)
            .size_in_lpxs
            .width as f64;
        draws.text.draw_abs(
            cx,
            dvec2(
                at.pos.x + (at.size.x - w) * 0.5,
                at.pos.y + (at.size.y - size as f64) * 0.5,
            ),
            &text,
        );
    }
}

/// A minimap: the plate, the polyline (a dark under-stroke, then the line),
/// then every dot — others as discs (the leader in its own colour), the
/// subject last as a heading dart so it is never covered.
#[allow(clippy::too_many_arguments)]
fn draw_map(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    style: &HudStyle,
    scale: f32,
    binder: &mut HudBinder,
    dots: &mut Vec<HudMapDot>,
) {
    // A radar is round, centred on the pane's own subject at a fixed
    // scale, and clips what it draws to its circle.
    let radar = e.range > 0.0;
    let (ground, border_color, border, radius) = plate(e, style);
    if ground.w > 0.0 || border > 0.0 {
        draws.shape.shape = if radar { 5.0 } else { 0.0 };
        draws.shape.fill = ground;
        draws.shape.stroke = border_color;
        draws.shape.border = border * scale;
        draws.shape.radius = radius * scale;
        draws.shape.draw_abs(cx, at);
    }
    dots.clear();
    (binder.map_dots)(e, dots);
    let pad = 12.0 * scale;
    let (ox, oy) = (
        at.pos.x + at.size.x * 0.5,
        at.pos.y + at.size.y * 0.5,
    );
    // The radar's usable circle, in pixels from the centre.
    let rim = (at.size.x.min(at.size.y) * 0.5 - pad as f64 * 0.5).max(4.0);
    let fit = if radar {
        let (x, z) = dots
            .iter()
            .find(|d| d.is_self)
            .map(|d| (d.x, d.z))
            .or_else(|| e.points.first().map(|p| (p.x, p.y)))
            .unwrap_or((0.0, 0.0));
        Some(HudMapFit::radar(x, z, e.range, rim as f32))
    } else {
        HudMapFit::new(&e.points, at.size.x as f32, at.size.y as f32, pad, e.rotate)
    };
    let Some(mut fit) = fit else {
        return;
    };
    if e.rotate {
        if let Some(me) = dots.iter().find(|d| d.is_self) {
            fit = fit.heading_up(me.fx, me.fz);
        }
    }
    let to_px = |x: f32, z: f32| {
        let (px, py) = fit.apply(x, z);
        (ox + px as f64, oy + py as f64)
    };
    let width = if e.thickness > 0.0 { e.thickness } else { 3.0 } * scale;
    let line = if e.color.w > 0.0 { e.color } else { style.ink };
    let under = if e.track.w > 0.0 { e.track } else { vec4(0.0, 0.0, 0.0, 0.55) };
    let n = e.points.len();
    let segments = if e.closed { n } else { n.saturating_sub(1) };
    draws.shape.border = 0.0;
    draws.shape.radius = 0.0;
    if radar {
        // A faint ring at half range: distance at a glance.
        draws.shape.shape = 1.0;
        draws.shape.fill = vec4(line.x, line.y, line.z, 0.18);
        draws.shape.thickness = 1.0 * scale;
        draws.shape.from = 0.0;
        draws.shape.sweep = std::f32::consts::TAU;
        draws.shape.frac = 1.0;
        let r = rim * 0.5 + 1.0 + 0.5 * scale as f64;
        draws.shape.draw_abs(cx, Rect { pos: dvec2(ox - r, oy - r), size: dvec2(r * 2.0, r * 2.0) });
    }
    let clip_r = rim - 1.0;
    for (color, w) in [(under, width + 3.0 * scale), (line, width)] {
        draws.shape.fill = color;
        draws.shape.thickness = w;
        for k in 0..segments {
            let a = e.points[k];
            let b = e.points[(k + 1) % n];
            let (x0, y0) = to_px(a.x, a.y);
            let (x1, y1) = to_px(b.x, b.y);
            let ((x0, y0), (x1, y1)) = if radar {
                match clip_to_circle((x0 - ox, y0 - oy), (x1 - ox, y1 - oy), clip_r - w as f64 * 0.5) {
                    Some(((ax, ay), (bx, by))) => ((ax + ox, ay + oy), (bx + ox, by + oy)),
                    None => continue,
                }
            } else {
                ((x0, y0), (x1, y1))
            };
            let half = w as f64 * 0.5 + 1.0;
            // Which diagonal of its box the segment runs along.
            draws.shape.shape = if (x1 - x0) * (y1 - y0) >= 0.0 { 2.0 } else { 3.0 };
            draws.shape.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x0.min(x1) - half, y0.min(y1) - half),
                    size: dvec2((x1 - x0).abs() + half * 2.0, (y1 - y0).abs() + half * 2.0),
                },
            );
        }
    }
    let d = if e.dot_size > 0.0 { e.dot_size } else { 9.0 } * scale;
    let other = if e.dot_color.w > 0.0 { e.dot_color } else { vec4(0.92, 0.94, 0.96, 1.0) };
    let lead = if e.lead_color.w > 0.0 { e.lead_color } else { other };
    let mine = if e.self_color.w > 0.0 { e.self_color } else { style.accent };
    let outline = vec4(0.02, 0.03, 0.05, 0.9);
    let label_size = (11.0 * scale).max(8.0);
    for pass in 0..2 {
        for dot in dots.iter() {
            // Others first, the subject over them.
            if dot.is_self != (pass == 1) {
                continue;
            }
            let (x, y) = to_px(dot.x, dot.z);
            if radar && !dot.is_self && ((x - ox).powi(2) + (y - oy).powi(2)).sqrt() > clip_r {
                continue;
            }
            if dot.is_self {
                let r = (d * 1.9) as f64;
                let (dx, dy) = fit.apply_dir(dot.fx, dot.fz);
                draws.shape.shape = 4.0;
                draws.shape.from = dx.atan2(-dy);
                draws.shape.fill = mine;
                draws.shape.stroke = outline;
                draws.shape.border = 1.5 * scale;
                draws.shape.draw_abs(cx, Rect { pos: dvec2(x - r * 0.5, y - r * 0.5), size: dvec2(r, r) });
                draws.shape.from = 0.0;
            } else {
                let r = d as f64;
                draws.shape.shape = 0.0;
                draws.shape.fill = if dot.rank == 1 { lead } else { other };
                draws.shape.stroke = outline;
                draws.shape.border = 1.5 * scale;
                draws.shape.radius = d * 0.5;
                draws.shape.draw_abs(cx, Rect { pos: dvec2(x - r * 0.5, y - r * 0.5), size: dvec2(r, r) });
                draws.shape.radius = 0.0;
            }
            if e.labels && dot.rank > 0 {
                let text = dot.rank.to_string();
                draws.text.text_style.font_size = label_size;
                draws.text.color = if dot.is_self { mine } else { style.ink };
                draws.text.draw_abs(cx, dvec2(x + d as f64 * 0.8, y - label_size as f64 * 1.1), &text);
            }
        }
    }
    // Targets: in place when in range, an arrow on the rim pointing at
    // them when not.
    if radar && !e.targets.is_empty() {
        let ink = if e.target_color.w > 0.0 { e.target_color } else { style.low };
        for t in &e.targets {
            let (x, y) = to_px(t.x, t.y);
            let (dx, dy) = (x - ox, y - oy);
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= clip_r - d as f64 {
                let r = d as f64 * 1.3;
                draws.shape.shape = 0.0;
                draws.shape.fill = ink;
                draws.shape.stroke = outline;
                draws.shape.border = 1.5 * scale;
                draws.shape.radius = 0.0;
                draws.shape.draw_abs(cx, Rect { pos: dvec2(x - r * 0.5, y - r * 0.5), size: dvec2(r, r) });
            } else {
                let r = d as f64 * 1.8;
                let (ux, uy) = if dist > 1.0e-6 { (dx / dist, dy / dist) } else { (0.0, -1.0) };
                let (px, py) = (ox + ux * (clip_r - r * 0.5), oy + uy * (clip_r - r * 0.5));
                draws.shape.shape = 4.0;
                draws.shape.from = (ux as f32).atan2(-uy as f32);
                draws.shape.fill = ink;
                draws.shape.stroke = outline;
                draws.shape.border = 1.5 * scale;
                draws.shape.draw_abs(cx, Rect { pos: dvec2(px - r * 0.5, py - r * 0.5), size: dvec2(r, r) });
                draws.shape.from = 0.0;
            }
        }
    }
    draws.shape.shape = 0.0;
    draws.shape.border = 0.0;
}

/// The part of the segment a..b (relative to a circle's centre) inside a
/// circle of radius `r`, or None when it misses it.
fn clip_to_circle(a: (f64, f64), b: (f64, f64), r: f64) -> Option<((f64, f64), (f64, f64))> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let qa = dx * dx + dy * dy;
    let qb = 2.0 * (a.0 * dx + a.1 * dy);
    let qc = a.0 * a.0 + a.1 * a.1 - r * r;
    if qa < 1.0e-12 {
        return (qc <= 0.0).then_some((a, b));
    }
    let disc = qb * qb - 4.0 * qa * qc;
    if disc <= 0.0 {
        return None;
    }
    let s = disc.sqrt();
    let t0 = ((-qb - s) / (2.0 * qa)).max(0.0);
    let t1 = ((-qb + s) / (2.0 * qa)).min(1.0);
    if t0 >= t1 {
        return None;
    }
    Some(((a.0 + dx * t0, a.1 + dy * t0), (a.0 + dx * t1, a.1 + dy * t1)))
}

/// The string a Text element shows: prefix, the number or literal, suffix.
fn element_text(e: &HudElement, binder: &mut HudBinder) -> String {
    if e.kind != HudKind::Text {
        return String::new();
    }
    let body = if !e.text.is_empty() {
        e.text.clone()
    } else if let HudValue::Bind(name) = &e.value {
        // A bind that reads as a word (`weapon`) rather than as a number.
        match (binder.string)(name, e.of) {
            Some(s) => s,
            None => number_text(e, binder),
        }
    } else if !e.value.is_none() {
        number_text(e, binder)
    } else {
        String::new()
    };
    format!("{}{}{}", e.prefix, body, e.suffix)
}

fn number_text(e: &HudElement, binder: &mut HudBinder) -> String {
    let v = (binder.number)(&e.value, e.of).unwrap_or(0.0);
    // An infinite reserve is a real state, and "-1" is not how anyone writes
    // it on a HUD.
    if v < 0.0 {
        return "\u{221e}".to_string();
    }
    if e.format == 0 {
        format!("{}", v.round() as i64)
    } else {
        format!("{:.*}", e.format as usize, v)
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_readout(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    style: &HudStyle,
    scale: f32,
    binder: &mut HudBinder,
) {
    if e.style == "sheet" && draw_sheet_readout(cx, at, e, draws, binder) {
        return;
    }
    let size = if e.glyph > 0.0 { e.glyph } else { text_size_for(&e.style) } * scale;
    let text = element_text(e, binder);
    let frac = if e.max.is_none() && e.low > 0.0 {
        (binder.number)(&e.value, e.of).unwrap_or(0.0)
    } else {
        gauge_fraction(e, binder)
    };
    let ink = ink_for(e, style, frac, style.ink);
    let mut x = at.pos.x;
    let mut y = at.pos.y;
    if !e.label.is_empty() {
        draws.text.text_style.font_size = CAPTION_SIZE * scale;
        draws.text.color = style.caption;
        draws.text.draw_abs(cx, dvec2(x, y), &e.label);
        y += (CAPTION_SIZE * scale) as f64 + 1.0;
    }
    if !(e.icon.is_empty() && e.svg.is_empty() && e.image.is_empty()) {
        let d = (size * 0.95) as f64;
        draw_glyph(
            cx,
            Rect { pos: dvec2(x, y + (size as f64 - d) * 0.5), size: dvec2(d, d) },
            e,
            draws,
            ink,
            binder,
        );
        x += d + (4.0 * scale) as f64;
    }
    draws.text.text_style.font_size = size;
    draws.text.color = ink;
    // A banner is centred on its own box and outlined, because it is read
    // against whatever the world happens to be showing behind it.
    if e.style == "banner" {
        let w = draws
            .text
            .layout(cx, 0.0, 0.0, None, false, Align::default(), &text)
            .size_in_lpxs
            .width as f64;
        let bx = at.pos.x + (at.size.x - w) * 0.5;
        draws.text.color = vec4(0.03, 0.04, 0.06, ink.w * 0.85);
        for (ox, oy) in [(-1.5, 0.0), (1.5, 0.0), (0.0, -1.5), (0.0, 1.5)] {
            draws.text.draw_abs(cx, dvec2(bx + ox, y + oy), &text);
        }
        draws.text.color = ink;
        draws.text.draw_abs(cx, dvec2(bx, y), &text);
        return;
    }
    draws.text.draw_abs(cx, dvec2(x, y), &text);
}

/// The cells of a `style: "sheet"` glyph strip, left to right.
pub const SHEET_GLYPHS: &str = "0123456789%-";

/// `style: "sheet"`: the readout drawn from a bitmap glyph strip (`image`) —
/// one row of equal cells holding [`SHEET_GLYPHS`], the way the classic
/// status bars drew their numbers (Doom's STTNUM). Glyphs are as tall as the
/// element's box and keep the cell's proportions; the number is
/// right-aligned in the box, as those bars aligned theirs. Characters the
/// strip does not hold advance one cell blank. False until the strip has
/// loaded, so the text lane stands in for it meanwhile.
fn draw_sheet_readout(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    binder: &mut HudBinder,
) -> bool {
    let Some((texture, w, h)) = (binder.image)(&e.image) else {
        return false;
    };
    let cells = SHEET_GLYPHS.chars().count() as f32;
    if w <= 0.0 || h <= 0.0 {
        return false;
    }
    let text = element_text(e, binder);
    let gh = at.size.y;
    let gw = gh * ((w / cells) / h) as f64;
    draws.image.tint = if e.color.w > 0.0 { e.color } else { vec4(1.0, 1.0, 1.0, 1.0) };
    draws.image.tex_size = vec2f(w, h);
    draws.image.pixelated = 1.0;
    draws.image.draw_vars.set_texture(0, &texture);
    let mut x = at.pos.x + at.size.x;
    for ch in text.chars().rev() {
        x -= gw;
        let Some(i) = SHEET_GLYPHS.chars().position(|g| g == ch) else { continue };
        let u0 = i as f32 / cells;
        draws.image.uv_rect = vec4(u0, 0.0, u0 + 1.0 / cells, 1.0);
        draws.image.draw_abs(cx, Rect { pos: dvec2(x, at.pos.y), size: dvec2(gw, gh) });
    }
    draws.image.uv_rect = vec4(0.0, 0.0, 1.0, 1.0);
    true
}

fn draw_icon(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    style: &HudStyle,
    scale: f32,
    binder: &mut HudBinder,
) {
    let tint = if e.color.w > 0.0 { e.color } else { style.ink };
    draw_glyph(cx, at, e, draws, tint, binder);
    if let Some(count) = (binder.number)(&e.count, e.of) {
        let size = ((at.size.y as f32) * 0.45).max(9.0);
        draws.text.text_style.font_size = size;
        draws.text.color = style.ink;
        let text = format!("{}", count.round() as i64);
        let w = draws
            .text
            .layout(cx, 0.0, 0.0, None, false, Align::default(), &text)
            .size_in_lpxs
            .width as f64;
        draws.text.draw_abs(
            cx,
            dvec2(
                at.pos.x + at.size.x - w,
                at.pos.y + at.size.y - size as f64,
            ),
            &text,
        );
    }
    let _ = scale;
}

/// One picture: a catalog image, an SVG resource, or a built-in glyph. Never
/// a shader — an icon an author can swap is an icon they can author.
fn draw_glyph(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    tint: Vec4f,
    binder: &mut HudBinder,
) {
    let tint = if e.dim {
        vec4(tint.x, tint.y, tint.z, tint.w * 0.28)
    } else {
        tint
    };
    if !e.image.is_empty() {
        if let Some((texture, w, h)) = (binder.image)(&e.image) {
            draws.image.tint = tint;
            draws.image.tex_size = vec2f(w, h);
            draws.image.pixelated = 1.0;
            draws.image.draw_vars.set_texture(0, &texture);
            // Keep the picture's own proportions inside the box it was given;
            // a stretched key sprite reads as a rendering bug.
            let fit = fit_rect(at, w as f64, h as f64);
            draws.image.draw_abs(cx, fit);
            return;
        }
    }
    let name = if !e.svg.is_empty() { &e.svg } else { &e.icon };
    if !name.is_empty() {
        (binder.glyph)(cx, at, name, tint);
    }
}

fn fit_rect(at: Rect, w: f64, h: f64) -> Rect {
    if w <= 0.0 || h <= 0.0 {
        return at;
    }
    let s = (at.size.x / w).min(at.size.y / h);
    let (fw, fh) = (w * s, h * s);
    Rect {
        pos: dvec2(
            at.pos.x + (at.size.x - fw) * 0.5,
            at.pos.y + (at.size.y - fh) * 0.5,
        ),
        size: dvec2(fw, fh),
    }
}

fn draw_log(
    cx: &mut Cx2d,
    at: Rect,
    e: &HudElement,
    doc: &HudDoc,
    draws: &mut HudDraws,
    style: &HudStyle,
    scale: f32,
) {
    let size = (if e.glyph > 0.0 { e.glyph } else { TEXT_SIZE } * scale) as f64;
    let size_f = size as f32;
    let mine: Vec<&makepad_scene::HudLine> = doc
        .lines
        .iter()
        .filter(|l| l.target == e.name)
        .rev()
        .take(e.lines.max(1) as usize)
        .collect();
    // Newest at the bottom, the way every kill feed and console reads. A
    // line wider than the log wraps onto rows of its own (the newest line's
    // rows stay in reading order, bottom row last).
    let mut y = at.pos.y + at.size.y - size;
    for line in mine {
        // The last fifth of a line's life is its fade; a message that
        // vanishes mid-word looks like a dropped frame.
        let fade = ((line.secs - line.age) / (line.secs * 0.25).max(0.05)).clamp(0.0, 1.0);
        let c = if line.color.w > 0.0 { line.color } else { style.ink };
        let rows = wrap_rows(cx, draws.text, size_f, &line.text, at.size.x.max(40.0));
        draws.text.text_style.font_size = size_f;
        draws.text.color = vec4(c.x, c.y, c.z, c.w * fade);
        for row in rows.iter().rev() {
            draws.text.draw_abs(cx, dvec2(at.pos.x, y), row);
            y -= size * 1.35;
        }
    }
}

/// A screen tint. `vignette` paints the edges and leaves the middle clear,
/// which is what makes a damage flash readable rather than blinding; `full`
/// covers the pane (a pickup blink, a death fade); `edge` marks one side.
fn draw_flash(
    cx: &mut Cx2d,
    rect: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    style: &HudStyle,
) {
    let s = e.pulse.strength();
    if s <= 0.0 {
        return;
    }
    let color = if e.color.w > 0.0 { e.color } else { style.low };
    let a = (color.w * s * e.strength.max(0.0)).clamp(0.0, 1.0);
    if a <= 0.001 {
        return;
    }
    draws.shape.shape = 0.0;
    draws.shape.border = 0.0;
    draws.shape.radius = 0.0;
    let paint = |cx: &mut Cx2d, draws: &mut HudDraws, r: Rect, alpha: f32| {
        draws.shape.fill = vec4(color.x, color.y, color.z, alpha);
        draws.shape.draw_abs(cx, r);
    };
    match e.style.as_str() {
        "full" => paint(cx, draws, rect, a),
        "edge" => paint(
            cx,
            draws,
            Rect {
                pos: rect.pos,
                size: dvec2(rect.size.x * 0.16, rect.size.y),
            },
            a,
        ),
        // The vignette is four bands whose alpha falls off in three steps —
        // a gradient without a gradient shader, and indistinguishable from
        // one at the alpha a damage flash actually uses.
        _ => {
            let bands = 4;
            for i in 0..bands {
                let t = (i + 1) as f64 / bands as f64;
                let alpha = a * (1.0 - (i as f32 / bands as f32)) * 0.5;
                let d = rect.size.y * 0.16 * t;
                let w = rect.size.x * 0.10 * t;
                paint(cx, draws, Rect { pos: rect.pos, size: dvec2(rect.size.x, d) }, alpha);
                paint(
                    cx,
                    draws,
                    Rect {
                        pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - d),
                        size: dvec2(rect.size.x, d),
                    },
                    alpha,
                );
                paint(cx, draws, Rect { pos: rect.pos, size: dvec2(w, rect.size.y) }, alpha);
                paint(
                    cx,
                    draws,
                    Rect {
                        pos: dvec2(rect.pos.x + rect.size.x - w, rect.pos.y),
                        size: dvec2(w, rect.size.y),
                    },
                    alpha,
                );
            }
        }
    }
}

fn draw_marker(
    cx: &mut Cx2d,
    rect: Rect,
    e: &HudElement,
    draws: &mut HudDraws,
    style: &HudStyle,
    scale: f32,
) {
    let s = e.pulse.strength();
    if s <= 0.0 {
        return;
    }
    let color = if e.color.w > 0.0 { e.color } else { style.ink };
    let c = vec4(color.x, color.y, color.z, color.w * s);
    let cx0 = rect.pos.x + rect.size.x * 0.5;
    let cy0 = rect.pos.y + rect.size.y * 0.5;
    let len = (10.0 * scale) as f64;
    let gap = (5.0 * scale) as f64;
    let t = (2.0 * scale).max(1.5) as f64;
    draws.shape.shape = 0.0;
    draws.shape.fill = c;
    draws.shape.border = 0.0;
    draws.shape.radius = 0.0;
    let arms: &[(f64, f64)] = if e.style == "x" {
        &[(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)]
    } else {
        &[(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)]
    };
    for (dx, dy) in arms {
        // A diagonal arm is drawn as a short bar on the axis it leans toward;
        // exact diagonals would need a rotated quad and buy nothing at this
        // size.
        let (w, h) = if dy.abs() > 0.5 && dx.abs() > 0.5 {
            (t.max(len * 0.6), t)
        } else if dx.abs() > 0.5 {
            (len, t)
        } else {
            (t, len)
        };
        draws.shape.draw_abs(
            cx,
            Rect {
                pos: dvec2(
                    cx0 + dx * (gap + w * 0.5) - w * 0.5,
                    cy0 + dy * (gap + h * 0.5) - h * 0.5,
                ),
                size: dvec2(w, h),
            },
        );
    }
}

fn draw_crosshair(
    cx: &mut Cx2d,
    rect: Rect,
    c: &makepad_scene::Crosshair,
    draws: &mut HudDraws,
    scale: f32,
    spread: f32,
) {
    if c.style == CrosshairStyle::None {
        return;
    }
    let cx0 = rect.pos.x + rect.size.x * 0.5;
    let cy0 = rect.pos.y + rect.size.y * 0.5;
    let t = (c.thickness * scale).max(1.0) as f64;
    // A gun that cones its shots must show the cone, or the reticle is a lie
    // about where the bullet goes.
    let bloom = if c.spread { spread * rect.size.y as f32 * 0.5 } else { 0.0 };
    let gap = ((c.gap + bloom) * scale) as f64;
    let len = (c.size * scale) as f64;
    draws.shape.shape = 0.0;
    draws.shape.fill = c.color;
    draws.shape.border = 0.0;
    draws.shape.radius = 0.0;
    match c.style {
        CrosshairStyle::Dot => {
            draws.shape.radius = (t * 0.5) as f32;
            draws.shape.draw_abs(
                cx,
                Rect {
                    pos: dvec2(cx0 - t, cy0 - t),
                    size: dvec2(t * 2.0, t * 2.0),
                },
            );
        }
        CrosshairStyle::Cross => {
            for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let (w, h) = if dx != 0.0 { (len, t) } else { (t, len) };
                draws.shape.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(
                            cx0 + dx * (gap + w * 0.5) - w * 0.5,
                            cy0 + dy * (gap + h * 0.5) - h * 0.5,
                        ),
                        size: dvec2(w, h),
                    },
                );
            }
        }
        CrosshairStyle::Ring => {
            draws.shape.shape = 1.0;
            draws.shape.thickness = t as f32;
            draws.shape.from = 0.0;
            draws.shape.sweep = std::f32::consts::TAU;
            draws.shape.frac = 1.0;
            let r = (len + gap) as f64;
            draws.shape.draw_abs(
                cx,
                Rect {
                    pos: dvec2(cx0 - r, cy0 - r),
                    size: dvec2(r * 2.0, r * 2.0),
                },
            );
            draws.shape.shape = 0.0;
        }
        CrosshairStyle::None => {}
    }
    let _ = scale;
}

/// Put the shared draws back the way the rest of the overlay expects them.
fn restore(draws: &mut HudDraws, style: &HudStyle) {
    draws.text.text_style.font_size = 22.0;
    draws.text.color = style.ink;
    draws.shape.shape = 0.0;
    draws.shape.border = 0.0;
    draws.shape.radius = 0.0;
    draws.shape.frac = 1.0;
    draws.image.tint = vec4(1.0, 1.0, 1.0, 1.0);
    draws.image.tex_size = vec2f(0.0, 0.0);
    // DrawHudImage is the pixel-art lane. Keeping its default here also
    // covers the FPS weapon/flash draw that reuses it after the HUD pass.
    draws.image.pixelated = 1.0;
}
