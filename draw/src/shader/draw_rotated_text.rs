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

        // Camera-delta transform (best-effort label tracking while the
        // map rotates/tilts between re-places): a full 2x2 matrix about
        // the view pivot — tilt does NOT commute with rotation, so the
        // exact delta S(t1)*R(d)*S(1/t0) is a general matrix. The async
        // re-place trues up with identity.
        cam_a: uniform(1.0)
        cam_b: uniform(0.0)
        cam_c: uniform(0.0)
        cam_d: uniform(1.0)
        cam_pivot: uniform(vec2(0.0, 0.0))
        // Pan/zoom delta applied BEFORE the camera matrix: glyphs are
        // emitted in CACHED placement space and ride these uniforms every
        // frame, exactly like tile geometry rides map_offset — no CPU
        // re-transform between frames, so labels can never trail the map.
        cam_scale: uniform(1.0)
        cam_shift: uniform(vec2(0.0, 0.0))
        // The Inception fold, in LOCKSTEP with DrawMapVector's vertex
        // branch (map view.rs) and SpaceWarp on the CPU (map overlay.rs) —
        // labels are emitted UNWARPED (plain tilt projection) and fold
        // here per frame, which is what keeps them glued to the tiles
        // while the camera rotates instead of trailing a CPU re-place.
        // space_warp: x = tween amount, y = fold start r0 (pre-tilt ground
        // px), z = curl radius, w = sin(tilt). space_warp2: x = kappa
        // (perspective 1/D), y = unused here (labels carry lift in screen
        // px), z = bend cap angle, w = cos(tilt).
        space_warp: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        space_warp2: uniform(vec4(0.0, 0.0, 0.0, 1.0))

        // Fold one camera-delta'd GROUND position (lift already removed)
        // and re-apply `lift` scaled by the local perspective factor —
        // exactly how the CPU placement used to treat lifts (vertical
        // screen shifts × w), so the at-rest picture is unchanged.
        warp_ground: fn(ground: vec2, lift: float) -> vec2 {
            let cos_t = max(self.space_warp2.w, 0.05)
            let sin_t = self.space_warp.w
            let wg = (self.cam_pivot.y - ground.y) / cos_t
            var wf = wg
            var wu = 0.0
            let wa = wg - self.space_warp.y
            if wa > 0.0 {
                let wr = max(self.space_warp.z, 1.0)
                let cap = self.space_warp2.z
                let th = min(wa / wr, cap)
                let sth = sin(th)
                let cth = cos(th)
                wf = self.space_warp.y + wr * sth
                wu = wr * (1.0 - cth)
                let we = wa - wr * cap
                if we > 0.0 {
                    wf = wf + we * cos_t
                    wu = wu + we * sin_t
                }
            }
            let bf = wg + (wf - wg) * self.space_warp.x
            let bu = wu * self.space_warp.x
            let zrel = bf * sin_t - bu * cos_t
            let pw = 1.0 / max(1.0 + self.space_warp2.x * zrel, 0.12)
            return vec2(
                self.cam_pivot.x + (ground.x - self.cam_pivot.x) * pw,
                self.cam_pivot.y - (bf * cos_t + bu * sin_t) * pw - lift * pw
            )
        }
        // self.upright (instance from the Rust struct): 1.0 = screen-upright
        // label (place names, pin/brand text) — its ANCHOR tracks the camera
        // delta but its orientation must not; the re-place keeps such labels
        // horizontal, so rotating them live would snap back on regen.

        vertex: fn() {
            let p = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom.pos)
            let origin = self.rotation_origin
            let scaled = (p - origin) * self.label_scale
            let cs = cos(self.rotation)
            let sn = sin(self.rotation)
            var rotated = vec2(
                scaled.x * cs - scaled.y * sn,
                scaled.x * sn + scaled.y * cs
            ) + origin
            if self.upright > 0.5 {
                let anchor2 = origin * self.cam_scale + self.cam_shift
                let anchor_rel = anchor2 + vec2(0.0, self.lift) - self.cam_pivot
                let cam_ground = vec2(
                    anchor_rel.x * self.cam_a + anchor_rel.y * self.cam_b,
                    anchor_rel.x * self.cam_c + anchor_rel.y * self.cam_d
                ) + self.cam_pivot
                var cam_anchor = cam_ground - vec2(0.0, self.lift)
                if self.space_warp.x > 0.0001 {
                    // The anchor folds with the ground; the glyph offsets
                    // stay rigid screen px (upright text never bends).
                    cam_anchor = self.warp_ground(cam_ground, self.lift)
                }
                var offs = rotated - origin
                if self.billboard < 0.5 {
                    // Street-cap/city names scale with the gesture; text
                    // inside zoom-constant pins keeps its pixel size.
                    offs = offs * self.cam_scale
                }
                rotated = offs + cam_anchor
            } else {
                let q = rotated * self.cam_scale + self.cam_shift
                let cam_rel = q + vec2(0.0, self.lift) - self.cam_pivot
                let cam_ground = vec2(
                    cam_rel.x * self.cam_a + cam_rel.y * self.cam_b,
                    cam_rel.x * self.cam_c + cam_rel.y * self.cam_d
                ) + self.cam_pivot
                rotated = cam_ground - vec2(0.0, self.lift)
                if self.space_warp.x > 0.0001 {
                    // Per-vertex like the tiles: a street name crossing
                    // the fold bends glyph by glyph with the road.
                    rotated = self.warp_ground(cam_ground, self.lift)
                }
            }

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
    #[live(0.0)]
    pub upright: f32,
    /// Screen-px lift already baked into this label's placement (terrain
    /// ground + marker stalk). The camera delta re-projects the GROUND
    /// anchor and re-applies the lift, so lifted labels track rotation.
    #[live(0.0)]
    pub lift: f32,
    /// 1.0 = pin-interior text: anchor tracks the pan/zoom delta but glyph
    /// offsets and size stay constant screen px (like the pin mesh).
    #[live(0.0)]
    pub billboard: f32,
}

impl DrawRotatedText {
    /// Camera-delta uniforms: rotate placed glyphs about `pivot` by the
    /// given cos/sin and compress y by `tilt_ratio` — identity when the
    /// placement is fresh.
    pub fn set_camera_delta(&mut self, cx: &mut Cx, m: [f32; 4], pivot: Vec2f) {
        self.draw_vars.set_uniform(cx, live_id!(cam_a), &[m[0]]);
        self.draw_vars.set_uniform(cx, live_id!(cam_b), &[m[1]]);
        self.draw_vars.set_uniform(cx, live_id!(cam_c), &[m[2]]);
        self.draw_vars.set_uniform(cx, live_id!(cam_d), &[m[3]]);
        self.draw_vars
            .set_uniform(cx, live_id!(cam_pivot), &[pivot.x, pivot.y]);
    }

    /// Pan/zoom delta uniforms applied before the camera matrix: cached
    /// glyphs render at `p * scale + shift` per frame, GPU-side.
    pub fn set_pan_delta(&mut self, cx: &mut Cx, scale: f32, shift: Vec2f) {
        self.draw_vars.set_uniform(cx, live_id!(cam_scale), &[scale]);
        self.draw_vars
            .set_uniform(cx, live_id!(cam_shift), &[shift.x, shift.y]);
    }

    /// The Inception-fold uniforms, stamped every frame with the SAME
    /// values the tile shader gets (`warp` = amount/start/radius/sin_t,
    /// `warp2` = kappa/unused/cap and `cos_t` packed in w). Labels are
    /// emitted unwarped and fold in the vertex shader, so they track the
    /// camera exactly like tiles instead of waiting for a CPU re-place.
    pub fn set_space_warp(&mut self, cx: &mut Cx, warp: [f32; 4], warp2: [f32; 3], cos_t: f32) {
        self.draw_vars.set_uniform(cx, live_id!(space_warp), &warp);
        self.draw_vars.set_uniform(
            cx,
            live_id!(space_warp2),
            &[warp2[0], warp2[1], warp2[2], cos_t],
        );
    }
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
    /// Open one shared instance batch so many draw_path_glyphs* calls append
    /// to a single draw call instead of one begin/finish per glyph.
    pub fn begin_glyph_batch(&mut self, cx: &mut Cx2d) {
        if self.draw_super.many_instances.is_some() {
            return;
        }
        self.draw_super.update_draw_vars(cx);
        self.draw_super.many_instances =
            cx.begin_many_aligned_instances(&self.draw_super.draw_vars);
    }

    pub fn end_glyph_batch(&mut self, cx: &mut Cx2d) {
        if let Some(instances) = self.draw_super.many_instances.take() {
            let new_area = cx.end_many_instances(instances);
            let old_area = self.draw_super.draw_vars.area;
            self.draw_super.draw_vars.area = cx.update_area_refs(old_area, new_area);
        }
    }

    /// Re-present a glyph batch recorded into the retained draw list `list`
    /// on an earlier frame: this frame's pass/view uniforms and the camera
    /// values staged since (`set_camera_delta`, `set_pan_delta`,
    /// `set_space_warp`) go onto every glyph call in the list; the glyph
    /// instances themselves stay resident. `false` when the list holds no
    /// call of this shader.
    pub fn refresh_glyph_batch(&mut self, cx: &mut Cx2d, list: DrawListId) -> bool {
        self.draw_super.update_draw_vars(cx);
        self.draw_super
            .draw_vars
            .update_uniforms_on_draw_list(cx, list)
    }

    /// Draw a single glyph at an arbitrary position with rotation.
    #[allow(clippy::too_many_arguments)]
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
        self.draw_path_glyphs_offset(cx, glyphs, Vec2f { x: 0.0, y: 0.0 });
    }

    /// Draw pre-placed glyphs shifted by a screen-space offset (used for
    /// halo/outline underdraws).
    pub fn draw_path_glyphs_offset(
        &mut self,
        cx: &mut Cx2d,
        glyphs: &[PathGlyphInstance],
        offset: Vec2f,
    ) {
        self.draw_path_glyphs_scaled(cx, glyphs, 1.0, offset);
    }

    /// Draw pre-placed glyphs through an affine screen transform
    /// (p*scale + offset, glyph size scaled too) — lets a cached label
    /// placement track the map during a zoom gesture.
    pub fn draw_path_glyphs_scaled(
        &mut self,
        cx: &mut Cx2d,
        glyphs: &[PathGlyphInstance],
        scale: f32,
        offset: Vec2f,
    ) {
        // Instances bake their font_scale into font_size_in_lpxs at placement
        // time; the ambient font_scale (left over from whatever run was shaped
        // last) must not rescale them here or glyphs shrink under their pen
        // advances and labels render letter-spaced.
        let saved_font_scale = self.draw_super.font_scale;
        self.draw_super.font_scale = 1.0;
        self.upright = 0.0;
        self.billboard = 0.0;
        for glyph in glyphs {
            self.draw_glyph_at(
                cx,
                Point::new(
                    glyph.glyph_origin.x * scale + offset.x,
                    glyph.glyph_origin.y * scale + offset.y,
                ),
                Point::new(
                    glyph.rotation_origin.x * scale + offset.x,
                    glyph.rotation_origin.y * scale + offset.y,
                ),
                glyph.font_size_in_lpxs * scale,
                glyph.rasterized,
                glyph.angle,
                1.0,
            );
        }
        self.draw_super.font_scale = saved_font_scale;
    }

    /// Billboard variant for text INSIDE zoom-constant pins: the shared
    /// anchor scales/translates with the map (tracking the pin's baked
    /// anchor exactly), but glyph offsets and size stay in constant screen
    /// px — so the text is rigid on the pin at every gesture zoom, like
    /// the pin mesh itself.
    pub fn draw_path_glyphs_billboard(
        &mut self,
        cx: &mut Cx2d,
        glyphs: &[PathGlyphInstance],
        scale: f32,
        offset: Vec2f,
        anchor: Vec2f,
    ) {
        let saved_font_scale = self.draw_super.font_scale;
        self.draw_super.font_scale = 1.0;
        self.upright = 1.0;
        self.billboard = 1.0;
        let scaled_anchor =
            Point::new(anchor.x * scale + offset.x, anchor.y * scale + offset.y);
        for glyph in glyphs {
            self.draw_glyph_at(
                cx,
                Point::new(
                    scaled_anchor.x + (glyph.glyph_origin.x - anchor.x),
                    scaled_anchor.y + (glyph.glyph_origin.y - anchor.y),
                ),
                scaled_anchor,
                glyph.font_size_in_lpxs,
                glyph.rasterized,
                glyph.angle,
                1.0,
            );
        }
        self.upright = 0.0;
        self.billboard = 0.0;
        self.draw_super.font_scale = saved_font_scale;
    }

    /// Draw a straightened (screen-upright) label: every glyph carries the
    /// SAME anchor (the label's world-anchor in cached screen space) so the
    /// camera-delta shader translates the string rigidly to where the next
    /// re-place will put it, without rotating the glyphs. Straightened
    /// glyphs have angle 0, so hijacking rotation_origin as the anchor is
    /// free.
    pub fn draw_path_glyphs_upright(
        &mut self,
        cx: &mut Cx2d,
        glyphs: &[PathGlyphInstance],
        scale: f32,
        offset: Vec2f,
        anchor: Vec2f,
    ) {
        let saved_font_scale = self.draw_super.font_scale;
        self.draw_super.font_scale = 1.0;
        self.upright = 1.0;
        self.billboard = 0.0;
        let anchor = Point::new(anchor.x * scale + offset.x, anchor.y * scale + offset.y);
        for glyph in glyphs {
            self.draw_glyph_at(
                cx,
                Point::new(
                    glyph.glyph_origin.x * scale + offset.x,
                    glyph.glyph_origin.y * scale + offset.y,
                ),
                anchor,
                glyph.font_size_in_lpxs * scale,
                glyph.rasterized,
                glyph.angle,
                1.0,
            );
        }
        self.upright = 0.0;
        self.draw_super.font_scale = saved_font_scale;
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
    #[allow(clippy::too_many_arguments)]
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

    let idx = match cumulative.binary_search_by(|v| v.total_cmp(&d)) {
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
