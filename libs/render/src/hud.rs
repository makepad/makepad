//! 2D overlay drawing, moved verbatim from gamemaker's draw_walk: HUD text
//! slots + gauges + crosshair, and the billboard nametags. The draw structs
//! are lent by the host widget (they carry its theme styling).

use makepad_draw::*;
use makepad_game_sim::{HudAnchor, HudBar, HudSlot};

/// HUD: named text slots pinned to anchors (slots sharing an anchor stack
/// downward in insertion order), plus gauges. "center" is the big banner,
/// "hint" the small top-left control help — their historical looks are the
/// slot-name defaults. Color/size of 0 = defaults.
pub fn draw_hud_overlay(
    cx: &mut Cx2d,
    rect: Rect,
    draw_hud: &mut DrawText,
    draw_dot: &mut DrawColor,
    slots: &[(String, HudSlot)],
    bars: &[HudBar],
    crosshair: bool,
) {
    let default_color = vec4(1.0, 1.0, 1.0, 0.93);
    // Per-anchor stacking cursors (y offset from the anchor's origin).
    let mut cursors: [f64; 7] = [0.0; 7];
    let anchor_index = |a: HudAnchor| match a {
        HudAnchor::TopLeft => 0usize,
        HudAnchor::Top => 1,
        HudAnchor::TopRight => 2,
        HudAnchor::Center => 3,
        HudAnchor::BottomLeft => 4,
        HudAnchor::Bottom => 5,
        HudAnchor::BottomRight => 6,
    };
    let margin = 12.0f64;
    // (x anchor: -1 left, 0 center, 1 right; base y; stack direction)
    let anchor_home = |a: HudAnchor, rect: Rect| -> (f64, f64, f64) {
        match a {
            HudAnchor::TopLeft => (rect.pos.x + margin, rect.pos.y + 10.0, 1.0),
            HudAnchor::Top => (rect.pos.x + rect.size.x * 0.5, rect.pos.y + 84.0, 1.0),
            HudAnchor::TopRight => (rect.pos.x + rect.size.x - margin, rect.pos.y + 10.0, 1.0),
            HudAnchor::Center => (rect.pos.x + rect.size.x * 0.5, rect.pos.y + 42.0, 1.0),
            HudAnchor::BottomLeft => (rect.pos.x + margin, rect.pos.y + rect.size.y - 26.0, -1.0),
            HudAnchor::Bottom => (
                rect.pos.x + rect.size.x * 0.5,
                rect.pos.y + rect.size.y - 26.0,
                -1.0,
            ),
            HudAnchor::BottomRight => (
                rect.pos.x + rect.size.x - margin,
                rect.pos.y + rect.size.y - 26.0,
                -1.0,
            ),
        }
    };
    for (name, slot) in slots {
        if slot.text.is_empty() {
            continue;
        }
        let default_size = match name.as_str() {
            "center" => 22.0,
            "top" => 15.0,
            "hint" => 9.0,
            _ => 12.0,
        };
        let size = if slot.size > 0.0 { slot.size } else { default_size };
        draw_hud.text_style.font_size = size;
        draw_hud.color = if slot.color.w > 0.0 {
            slot.color
        } else {
            default_color
        };
        let (home_x, home_y, stack_dir) = anchor_home(slot.anchor, rect);
        let layout = draw_hud.layout(cx, 0.0, 0.0, None, false, Align::default(), &slot.text);
        let width = layout.size_in_lpxs.width as f64;
        let x = match slot.anchor {
            HudAnchor::Top | HudAnchor::Center | HudAnchor::Bottom => home_x - width * 0.5,
            HudAnchor::TopRight | HudAnchor::BottomRight => home_x - width,
            _ => home_x,
        };
        let ai = anchor_index(slot.anchor);
        let y = home_y + cursors[ai] * stack_dir;
        cursors[ai] += size as f64 * 1.55;
        draw_hud.draw_abs(cx, dvec2(x, y), &slot.text);
    }
    // Gauges stack after the texts of their anchor.
    for bar in bars {
        let (home_x, home_y, stack_dir) = anchor_home(bar.anchor, rect);
        let ai = anchor_index(bar.anchor);
        let y = home_y + cursors[ai] * stack_dir + 3.0;
        cursors[ai] += 16.0;
        let bar_w = 140.0f64;
        let x = match bar.anchor {
            HudAnchor::Top | HudAnchor::Center | HudAnchor::Bottom => home_x - bar_w * 0.5,
            HudAnchor::TopRight | HudAnchor::BottomRight => home_x - bar_w,
            _ => home_x,
        };
        // Track, then fill.
        draw_dot.color = vec4(0.05, 0.06, 0.1, 0.65);
        draw_dot.draw_abs(
            cx,
            Rect {
                pos: dvec2(x, y),
                size: dvec2(bar_w, 10.0),
            },
        );
        draw_dot.color = bar.color;
        draw_dot.draw_abs(
            cx,
            Rect {
                pos: dvec2(x + 1.0, y + 1.0),
                size: dvec2((bar_w - 2.0) * bar.fraction.clamp(0.0, 1.0) as f64, 8.0),
            },
        );
    }
    // Restore defaults for anyone else using these draws.
    draw_hud.text_style.font_size = 22.0;
    draw_hud.color = default_color;
    draw_dot.color = vec4(1.0, 1.0, 1.0, 0.9);

    if crosshair {
        let dot = 5.0;
        draw_dot.draw_abs(
            cx,
            Rect {
                pos: dvec2(
                    rect.pos.x + (rect.size.x - dot) * 0.5,
                    rect.pos.y + (rect.size.y - dot) * 0.5,
                ),
                size: dvec2(dot, dot),
            },
        );
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
use makepad_game_sim::hud::{
    text_size_for, CAPTION_SIZE, HUD_REFERENCE_HEIGHT, TEXT_SIZE,
};
use makepad_game_sim::{
    CrosshairStyle, HudDoc, HudElement, HudKind, HudValue,
};

/// The look every element falls back to. One dark plate palette, so a HUD
/// built out of defaults already reads as a game's rather than as a debug
/// overlay — which is the whole difference a author should not have to spend
/// their one attempt on.
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
            plate: vec4(0.055, 0.063, 0.086, 0.82),
            plate_border: vec4(1.0, 1.0, 1.0, 0.11),
            plate_radius: 6.0,
            plate_border_width: 1.0,
            ink: vec4(0.88, 0.92, 0.97, 1.0),
            caption: vec4(0.56, 0.63, 0.73, 1.0),
            accent: vec4(0.27, 0.82, 0.48, 1.0),
            low: vec4(1.0, 0.27, 0.22, 1.0),
            track: vec4(1.0, 1.0, 1.0, 0.10),
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
}

/// The draw structs the host lends (they carry its theme styling).
pub struct HudDraws<'a> {
    pub shape: &'a mut DrawHudShape,
    pub image: &'a mut DrawHudImage,
    pub text: &'a mut DrawText,
}

/// Draw the whole HUD document over `rect`, and report the fraction each
/// gauge was asked to show so the host can settle its trailing chip bars.
pub fn draw_hud_doc(
    cx: &mut Cx2d,
    rect: Rect,
    doc: &HudDoc,
    draws: &mut HudDraws,
    style: &HudStyle,
    binder: &mut HudBinder,
    spread: f32,
) -> Vec<(String, f32)> {
    let scale = (rect.size.y as f32 / HUD_REFERENCE_HEIGHT).max(0.35);
    let mut fractions: Vec<(String, f32)> = Vec::new();

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
        makepad_game_sim::hud_layout(
            doc,
            rect.size.x as f32,
            rect.size.y as f32,
            scale,
            &mut measure,
            &mut text_of,
        )
    };

    for p in &placed {
        let e = &doc.elements[p.index];
        if !visible(e, binder) {
            continue;
        }
        let at = Rect {
            pos: dvec2(rect.pos.x + p.x as f64, rect.pos.y + p.y as f64),
            size: dvec2(p.w as f64, p.h as f64),
        };
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
            HudKind::Flash | HudKind::Marker => {}
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
    let mine: Vec<&makepad_game_sim::HudLine> = doc
        .lines
        .iter()
        .filter(|l| l.target == e.name)
        .rev()
        .take(e.lines.max(1) as usize)
        .collect();
    // Newest at the bottom, the way every kill feed and console reads.
    let mut y = at.pos.y + at.size.y - size;
    for line in mine {
        // The last fifth of a line's life is its fade; a message that
        // vanishes mid-word looks like a dropped frame.
        let fade = ((line.secs - line.age) / (line.secs * 0.25).max(0.05)).clamp(0.0, 1.0);
        let c = if line.color.w > 0.0 { line.color } else { style.ink };
        draws.text.text_style.font_size = size_f;
        draws.text.color = vec4(c.x, c.y, c.z, c.w * fade);
        draws.text.draw_abs(cx, dvec2(at.pos.x, y), &line.text);
        y -= size * 1.35;
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
    c: &makepad_game_sim::Crosshair,
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
}

