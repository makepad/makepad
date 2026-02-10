use crate::{
    cx_2d::Cx2d,
    makepad_platform::*,
    shader::draw_text::{DrawText, PreparedTextRun},
    text::{geom::Point, rasterizer::RasterizedGlyph},
};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawRotatedText = mod.std.set_type_default() do #(DrawRotatedText::script_shader(vm)){
        ..mod.draw.DrawText

        rotated_pos: varying(vec2f)

        vertex: fn() {
            let p = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom.pos)
            let origin = self.rotation_origin
            let scaled = (p - origin) * self.label_scale
            let cs = cos(self.rotation)
            let sn = sin(self.rotation)
            let rotated = vec2(
                scaled.x * cs - scaled.y * sn,
                scaled.x * sn + scaled.y * cs
            ) + origin

            self.pos = self.geom.pos
            self.t = mix(self.t_min, self.t_max, self.geom.pos.xy)
            self.rotated_pos = rotated

            let half_extent = self.rect_size * self.label_scale * 0.5
            let cr = length(half_extent) + 2.0
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            )

            if rotated.x + cr < clip.x || rotated.y + cr < clip.y
                || rotated.x - cr > clip.z || rotated.y - cr > clip.w {
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0)
                return
            }

            let shifted = rotated + self.draw_list.view_shift
            self.world = self.draw_list.view_transform * vec4(
                shifted.x,
                shifted.y,
                self.glyph_depth + self.draw_call.zbias,
                1.
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        pixel: fn() {
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            )
            if self.rotated_pos.x < clip.x || self.rotated_pos.y < clip.y
                || self.rotated_pos.x > clip.z || self.rotated_pos.y > clip.w {
                discard()
            }
            return self.sample_text_pixel()
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRotatedText {
    #[deref]
    pub draw_super: DrawText,
    #[live(0.0)]
    pub rotation: f32,
    #[live(1.0)]
    pub label_scale: f32,
    #[live(vec2(0.0, 0.0))]
    pub rotation_origin: Vec2f,
}

/// A single glyph positioned along a path, ready to draw.
#[derive(Clone, Copy, Debug)]
pub struct PathGlyphInstance {
    pub glyph_origin: Point<f32>,
    pub rotation_origin: Point<f32>,
    pub font_size_in_lpxs: f32,
    pub rasterized: RasterizedGlyph,
    pub angle: f32,
}

/// Result of placing text along a path. Contains the bounding rect,
/// center point, and a range into an external glyph buffer.
#[derive(Clone, Debug)]
pub struct PathTextPlacement {
    pub bounds: Rect,
    pub center: Vec2d,
    pub glyph_start: usize,
    pub glyph_end: usize,
}

impl DrawRotatedText {
    /// Draw a single glyph at an arbitrary position with rotation.
    pub fn draw_glyph_at(
        &mut self,
        cx: &mut Cx2d,
        glyph_origin: Point<f32>,
        rotation_origin: Point<f32>,
        font_size_in_lpxs: f32,
        rasterized_glyph: RasterizedGlyph,
        rotation: f32,
        label_scale: f32,
    ) {
        self.rotation = rotation;
        self.label_scale = label_scale;
        self.rotation_origin = vec2(rotation_origin.x, rotation_origin.y);
        self.draw_super.draw_rasterized_glyph_abs(
            cx,
            glyph_origin,
            font_size_in_lpxs,
            rasterized_glyph,
            self.draw_super.color,
        );
    }

    /// Draw a sequence of pre-placed glyphs from a buffer slice.
    pub fn draw_path_glyphs(&mut self, cx: &mut Cx2d, glyphs: &[PathGlyphInstance]) {
        for glyph in glyphs {
            self.draw_glyph_at(
                cx,
                glyph.glyph_origin,
                glyph.rotation_origin,
                glyph.font_size_in_lpxs,
                glyph.rasterized,
                glyph.angle,
                1.0,
            );
        }
    }

    /// Place glyphs from a `PreparedTextRun` along a polyline path.
    ///
    /// Glyphs are appended to `out_glyphs` (caller reuses the buffer to avoid allocs).
    /// Returns placement info (bounds + center + range into `out_glyphs`) on success,
    /// or `None` if the text doesn't fit or the path curves too sharply.
    ///
    /// * `path` / `cumulative` — the screen-space polyline and its cumulative arc-lengths.
    /// * `start_distance` — where along the path to start placing text.
    /// * `reverse` — whether to walk the path backwards (for readability).
    /// * `baseline_shift` — vertical offset from the path centerline.
    /// * `max_glyph_turn` — maximum angle change between consecutive glyphs (radians).
    /// * `angle_blend` — smoothing factor for glyph angles (0..1).
    pub fn place_text_along_path(
        &self,
        run: &PreparedTextRun,
        path: &[Vec2d],
        cumulative: &[f64],
        start_distance: f64,
        reverse: bool,
        baseline_shift: f32,
        label_angle_bias: f32,
        max_glyph_turn: f32,
        angle_blend: f32,
        path_center: Vec2d,
        out_glyphs: &mut Vec<PathGlyphInstance>,
    ) -> Option<PathTextPlacement> {
        if path.len() < 2 || run.glyphs.is_empty() {
            return None;
        }
        let total_length = *cumulative.last()?;
        let text_width = run.width_in_lpxs;
        if total_length < text_width as f64 + 4.0 {
            return None;
        }

        // Compute mid-path angle for the label direction
        let mid_distance = start_distance + text_width as f64 * 0.5;
        let probe_delta = (text_width as f64 * 0.25).clamp(12.0, 42.0);
        let mid_tangent_angle =
            sample_tangent_angle_raw(path, cumulative, mid_distance, probe_delta)?;
        let label_mid_angle = wrap_angle_pi(mid_tangent_angle + label_angle_bias);

        let label_half_height =
            ((run.ascender_in_lpxs - run.descender_in_lpxs).abs() as f64 * 0.5).max(3.0);

        let glyph_start = out_glyphs.len();
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut prev_angle: Option<f32> = None;

        for glyph in &run.glyphs {
            if glyph.advance_in_lpxs <= 0.0 {
                continue;
            }
            let pen_distance = start_distance + glyph.pen_x_in_lpxs as f64;
            let advance_half = glyph.advance_in_lpxs as f64 * 0.5;
            let path_pen_distance = if reverse {
                total_length - pen_distance
            } else {
                pen_distance
            };
            let path_center_distance = if reverse {
                path_pen_distance - advance_half
            } else {
                path_pen_distance + advance_half
            };

            let pen_point = sample_point_at_distance(path, cumulative, path_pen_distance)?;
            let center_point = sample_point_at_distance(path, cumulative, path_center_distance)?;

            let angle_sample_delta = (glyph.advance_in_lpxs as f64 * 1.45).clamp(10.0, 30.0);
            let raw_angle = sample_tangent_angle_raw(
                path,
                cumulative,
                path_center_distance,
                angle_sample_delta,
            )?;
            let raw_label_angle = wrap_angle_pi(raw_angle + label_angle_bias);
            let angle = if let Some(prev) = prev_angle {
                let target = nearest_equivalent_angle(prev, raw_label_angle);
                smooth_angle(prev, target, angle_blend)
            } else {
                nearest_equivalent_angle(label_mid_angle, raw_label_angle)
            };
            if let Some(prev) = prev_angle {
                let turn = wrap_angle_pi(angle - prev).abs();
                if turn > max_glyph_turn {
                    // Too sharp — discard all glyphs we added for this label
                    out_glyphs.truncate(glyph_start);
                    return None;
                }
            }
            prev_angle = Some(angle);

            let tangent = dvec2((angle as f64).cos(), (angle as f64).sin());
            let normal = dvec2(-tangent.y, tangent.x);
            let baseline_pen_origin = pen_point + normal * baseline_shift as f64;
            let baseline_center = center_point + normal * baseline_shift as f64;
            let glyph_origin = baseline_pen_origin + tangent * glyph.offset_x_in_lpxs as f64;

            let half_width = (glyph.advance_in_lpxs.abs() as f64 * 0.62).max(2.0);
            min_x = min_x.min(baseline_center.x - half_width);
            min_y = min_y.min(baseline_center.y - label_half_height);
            max_x = max_x.max(baseline_center.x + half_width);
            max_y = max_y.max(baseline_center.y + label_half_height);

            out_glyphs.push(PathGlyphInstance {
                glyph_origin: Point::new(glyph_origin.x as f32, glyph_origin.y as f32),
                rotation_origin: Point::new(
                    baseline_pen_origin.x as f32,
                    baseline_pen_origin.y as f32,
                ),
                font_size_in_lpxs: glyph.font_size_in_lpxs,
                rasterized: glyph.rasterized,
                angle,
            });
        }

        let glyph_end = out_glyphs.len();
        if glyph_end == glyph_start || !min_x.is_finite() || !min_y.is_finite() {
            out_glyphs.truncate(glyph_start);
            return None;
        }

        let bounds = rect(
            min_x - 2.0,
            min_y - 2.0,
            (max_x - min_x + 4.0).max(1.0),
            (max_y - min_y + 4.0).max(1.0),
        );
        Some(PathTextPlacement {
            bounds,
            center: path_center,
            glyph_start,
            glyph_end,
        })
    }
}

// --- Polyline sampling helpers (self-contained, no allocations) ---

fn sample_point_at_distance(points: &[Vec2d], cumulative: &[f64], distance: f64) -> Option<Vec2d> {
    if points.len() < 2 || cumulative.len() != points.len() {
        return None;
    }
    let total = *cumulative.last()?;
    let d = distance.clamp(0.0, total);

    let idx = match cumulative.binary_search_by(|v| v.partial_cmp(&d).unwrap()) {
        Ok(i) => return Some(points[i]),
        Err(i) => i,
    };
    if idx == 0 {
        return Some(points[0]);
    }
    if idx >= points.len() {
        return Some(*points.last()?);
    }
    let seg_start = cumulative[idx - 1];
    let seg_end = cumulative[idx];
    let seg_len = seg_end - seg_start;
    if seg_len < 1e-12 {
        return Some(points[idx - 1]);
    }
    let t = (d - seg_start) / seg_len;
    let a = points[idx - 1];
    let b = points[idx];
    Some(dvec2(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t))
}

fn sample_tangent_angle_raw(
    points: &[Vec2d],
    cumulative: &[f64],
    distance: f64,
    delta: f64,
) -> Option<f32> {
    let d0 = (distance - delta * 0.5).max(0.0);
    let d1 = (distance + delta * 0.5).min(*cumulative.last()?);
    let p0 = sample_point_at_distance(points, cumulative, d0)?;
    let p1 = sample_point_at_distance(points, cumulative, d1)?;
    let dx = p1.x - p0.x;
    let dy = p1.y - p0.y;
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return None;
    }
    Some((dy as f32).atan2(dx as f32))
}

fn wrap_angle_pi(mut angle: f32) -> f32 {
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    while angle < -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    angle
}

fn nearest_equivalent_angle(reference: f32, angle: f32) -> f32 {
    let mut out = angle;
    while out - reference > std::f32::consts::PI {
        out -= std::f32::consts::TAU;
    }
    while out - reference < -std::f32::consts::PI {
        out += std::f32::consts::TAU;
    }
    out
}

fn smooth_angle(previous: f32, current: f32, blend: f32) -> f32 {
    let mut next = current;
    while next - previous > std::f32::consts::PI {
        next -= std::f32::consts::TAU;
    }
    while next - previous < -std::f32::consts::PI {
        next += std::f32::consts::TAU;
    }
    previous + (next - previous) * blend.clamp(0.0, 1.0)
}
