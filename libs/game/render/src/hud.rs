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
