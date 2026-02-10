use {
    crate::{
        cx_2d::*,
        draw_list_2d::ManyInstances,
        makepad_platform::*,
        text::glyph_outline::{Command as OutlineCommand, GlyphOutline},
        turtle::*,
        vector::{PathCmd, VectorPath},
    },
    std::mem,
};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawGlyph = mod.std.set_type_default() do #(DrawGlyph::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)

        curve_texture: texture_2d(float)
        band_texture: texture_2d(float)

        color: #fff
        max_band_curves: 512.0
        aa_2x2: 0.0
        aa_4x4: 0.0
        aa_pad_px: 0.0
        axis_relief: 0.0
        stem_darken: 0.0
        stem_darken_max: 0.125

        pos: varying(vec2f)
        world: varying(vec4f)

        vertex: fn() {
            let p = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom.pos)
            let p_clipped = clamp(p, self.draw_clip.xy, self.draw_clip.zw)
            let mut local = (p_clipped - self.rect_pos) / self.rect_size
            if self.aa_pad_px > 0.0001 {
                let pad_uv = vec2(
                    min(self.aa_pad_px / max(self.rect_size.x, 1.0), 0.45),
                    min(self.aa_pad_px / max(self.rect_size.y, 1.0), 0.45)
                )
                local = (local - pad_uv) / vec2(
                    max(1.0 - 2.0 * pad_uv.x, 0.0001),
                    max(1.0 - 2.0 * pad_uv.y, 0.0001)
                )
            }
            self.pos = local
            self.world = self.draw_list.view_transform * vec4(
                p_clipped.x,
                p_clipped.y,
                self.draw_depth + self.layer_order * 0.000001 + self.draw_call.zbias,
                1.
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        fetch_curve_texel: fn(texel_idx: float) -> vec4 {
            let tex_size = self.curve_texture.size()
            let row = floor(texel_idx / tex_size.x)
            let col = texel_idx - row * tex_size.x
            let uv = vec2(
                (col + 0.5) / tex_size.x,
                (row + 0.5) / tex_size.y
            )
            return self.curve_texture.sample(uv)
        }

        fetch_band_texel: fn(texel_idx: float) -> vec4 {
            let tex_size = self.band_texture.size()
            let row = floor(texel_idx / tex_size.x)
            let col = texel_idx - row * tex_size.x
            let uv = vec2(
                (col + 0.5) / tex_size.x,
                (row + 0.5) / tex_size.y
            )
            return self.band_texture.sample(uv)
        }

        pick_channel: fn(v: vec4, channel: float) -> float {
            if channel < 0.5 {
                return v.x
            }
            if channel < 1.5 {
                return v.y
            }
            if channel < 2.5 {
                return v.z
            }
            return v.w
        }

        root_contribution: fn(
            t: float,
            a: float,
            b: float,
            p1: vec2,
            p2: vec2,
            p3: vec2,
            sample: vec2,
            px_size: float
        ) -> float {
            if t >= -0.000001 && t < 0.999999 {
                let s = 1.0 - t
                let cx = s * s * p1.x + 2.0 * s * t * p2.x + t * t * p3.x
                let dist = cx - sample.x
                let frac = clamp(dist / max(px_size, 0.00001) + 0.5, 0.0, 1.0)
                let dy = 2.0 * a * t + b
                if dy > 0.0 { return frac }
                if dy < 0.0 { return -frac }
            }
            return 0.0
        }

        linear_root_contribution: fn(
            t: float,
            dy: float,
            p1: vec2,
            p2: vec2,
            p3: vec2,
            sample: vec2,
            px_size: float
        ) -> float {
            if t >= -0.000001 && t < 0.999999 {
                let s = 1.0 - t
                let cx = s * s * p1.x + 2.0 * s * t * p2.x + t * t * p3.x
                let dist = cx - sample.x
                let frac = clamp(dist / max(px_size, 0.00001) + 0.5, 0.0, 1.0)
                if dy > 0.0 { return frac }
                if dy < 0.0 { return -frac }
            }
            return 0.0
        }

        curve_coverage: fn(p1: vec2, p2: vec2, p3: vec2, sample: vec2, px_size: float) -> float {
            let y1 = p1.y - sample.y
            let y2 = p2.y - sample.y
            let y3 = p3.y - sample.y

            let a = y1 - 2.0 * y2 + y3
            let b = 2.0 * (y2 - y1)
            let c = y1

            if abs(a) < 0.000001 {
                if abs(b) < 0.000001 {
                    return 0.0
                }
                let t = -c / b
                return self.linear_root_contribution(t, b, p1, p2, p3, sample, px_size)
            }

            let disc = b * b - 4.0 * a * c
            if disc < 0.0 {
                return 0.0
            }

            let sqrt_disc = sqrt(max(disc, 0.0))
            let inv_2a = 0.5 / a
            let t1 = (-b - sqrt_disc) * inv_2a
            let t2 = (-b + sqrt_disc) * inv_2a

            var coverage = 0.0
            coverage = coverage + self.root_contribution(t1, a, b, p1, p2, p3, sample, px_size)
            coverage = coverage + self.root_contribution(t2, a, b, p1, p2, p3, sample, px_size)
            return coverage
        }

        alpha_at: fn(sample: vec2, px_x: float, px_y: float) -> float {
            var coverage_x = 0.0
            var coverage_y = 0.0

            if self.band_count > 0.5 {
                let num_bands = max(floor(self.band_count + 0.5), 1.0)
                let band_idx = clamp(floor(sample.y * num_bands), 0.0, num_bands - 1.0)
                let band_info = self.fetch_band_texel(self.band_offset + band_idx)
                let list_offset = floor(band_info.x + 0.5)
                let list_count = min(floor(band_info.y + 0.5), self.max_band_curves)

                var j = 0.0
                loop {
                    if j >= list_count { break }

                    let packed_idx = floor(j * 0.25)
                    let channel = j - packed_idx * 4.0
                    let idx_data = self.fetch_band_texel(list_offset + packed_idx)
                    let curve_idx = self.pick_channel(idx_data, channel)

                    let t0 = self.fetch_curve_texel(curve_idx * 2.0)
                    let t1 = self.fetch_curve_texel(curve_idx * 2.0 + 1.0)
                    let p1 = vec2(t0.x, t0.y)
                    let p2 = vec2(t0.z, t0.w)
                    let p3 = vec2(t1.x, t1.y)

                    coverage_x = coverage_x + self.curve_coverage(p1, p2, p3, sample, px_x)
                    coverage_y = coverage_y + self.curve_coverage(
                        vec2(p1.y, p1.x),
                        vec2(p2.y, p2.x),
                        vec2(p3.y, p3.x),
                        vec2(sample.y, sample.x),
                        px_y,
                    )

                    j = j + 1.0
                }
            } else {
                let limit = min(floor(self.curve_count + 0.5), self.max_band_curves)
                var i = 0.0
                loop {
                    if i >= limit { break }

                    let curve_idx = self.curve_offset + i
                    let t0 = self.fetch_curve_texel(curve_idx * 2.0)
                    let t1 = self.fetch_curve_texel(curve_idx * 2.0 + 1.0)
                    let p1 = vec2(t0.x, t0.y)
                    let p2 = vec2(t0.z, t0.w)
                    let p3 = vec2(t1.x, t1.y)

                    coverage_x = coverage_x + self.curve_coverage(p1, p2, p3, sample, px_x)
                    coverage_y = coverage_y + self.curve_coverage(
                        vec2(p1.y, p1.x),
                        vec2(p2.y, p2.x),
                        vec2(p3.y, p3.x),
                        vec2(sample.y, sample.x),
                        px_y,
                    )

                    i = i + 1.0
                }
            }

            let ax = clamp(abs(coverage_x), 0.0, 1.0)
            let ay = clamp(abs(coverage_y), 0.0, 1.0)
            let a_min = min(ax, ay)
            let a_max = max(ax, ay)
            // If one axis reports near-solid coverage while the other axis
            // drops sharply (numeric miss around tangencies/cusps), blend a bit
            // toward the average. Keeps isolated pinhole artifacts from appearing.
            let mismatch = a_max - a_min
            let a_avg = 0.5 * (ax + ay)
            let mismatch_t = clamp((mismatch - 0.15) / 0.35, 0.0, 1.0)
            // Only apply relief once coverage is already interior on average.
            // This avoids gray fringes near sharp outer edges (e.g. '4').
            let interior_t = clamp((a_avg - 0.55) / 0.30, 0.0, 1.0)
            let relief = clamp(self.axis_relief * mismatch_t * interior_t, 0.0, 1.0)
            return clamp(mix(a_min, a_avg, relief), 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }

        pixel: fn() {
            if self.curve_count < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }

            let sample = self.pos
            let px_x = max(abs(dFdx(sample.x)) + abs(dFdy(sample.x)), 0.00001)
            let px_y = max(abs(dFdx(sample.y)) + abs(dFdy(sample.y)), 0.00001)
            let alpha_base = if self.aa_4x4 > 0.5 {
                let x0 = px_x * 0.125
                let x1 = px_x * 0.375
                let y0 = px_y * 0.125
                let y1 = px_y * 0.375
                let a0 = self.alpha_at(sample + vec2(-x1, -y1), px_x, px_y)
                let a1 = self.alpha_at(sample + vec2(-x0, -y1), px_x, px_y)
                let a2 = self.alpha_at(sample + vec2( x0, -y1), px_x, px_y)
                let a3 = self.alpha_at(sample + vec2( x1, -y1), px_x, px_y)
                let a4 = self.alpha_at(sample + vec2(-x1, -y0), px_x, px_y)
                let a5 = self.alpha_at(sample + vec2(-x0, -y0), px_x, px_y)
                let a6 = self.alpha_at(sample + vec2( x0, -y0), px_x, px_y)
                let a7 = self.alpha_at(sample + vec2( x1, -y0), px_x, px_y)
                let a8 = self.alpha_at(sample + vec2(-x1,  y0), px_x, px_y)
                let a9 = self.alpha_at(sample + vec2(-x0,  y0), px_x, px_y)
                let a10 = self.alpha_at(sample + vec2( x0,  y0), px_x, px_y)
                let a11 = self.alpha_at(sample + vec2( x1,  y0), px_x, px_y)
                let a12 = self.alpha_at(sample + vec2(-x1,  y1), px_x, px_y)
                let a13 = self.alpha_at(sample + vec2(-x0,  y1), px_x, px_y)
                let a14 = self.alpha_at(sample + vec2( x0,  y1), px_x, px_y)
                let a15 = self.alpha_at(sample + vec2( x1,  y1), px_x, px_y)
                clamp(
                    (a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9 + a10 + a11 + a12 + a13 + a14 + a15)
                        * 0.0625,
                    0.0,
                    1.0
                )
            } else if self.aa_2x2 > 0.5 {
                let offset = vec2(px_x * 0.25, px_y * 0.25)
                let a0 = self.alpha_at(sample + vec2(-offset.x, -offset.y), px_x, px_y)
                let a1 = self.alpha_at(sample + vec2(offset.x, -offset.y), px_x, px_y)
                let a2 = self.alpha_at(sample + vec2(-offset.x, offset.y), px_x, px_y)
                let a3 = self.alpha_at(sample + vec2(offset.x, offset.y), px_x, px_y)
                clamp((a0 + a1 + a2 + a3) * 0.25, 0.0, 1.0)
            } else {
                self.alpha_at(sample, px_x, px_y)
            }
            // Apply stem darkening only around the transition band.
            // This avoids lifting fully transparent background pixels.
            let darken = clamp(max(px_x, px_y) * self.stem_darken, 0.0, self.stem_darken_max)
            let edge_weight = clamp(1.0 - abs(alpha_base * 2.0 - 1.0), 0.0, 1.0)
            let alpha = clamp(alpha_base + darken * edge_weight, 0.0, 1.0)
            return vec4(self.color.rgb * self.color.a * alpha, self.color.a * alpha)
        }
    }
}

const CURVE_TEX_WIDTH: usize = 2048;
const BAND_TEX_WIDTH: usize = 2048;
const DEFAULT_NUM_BANDS: usize = 24;
// Keep cubic approximation tight; loose flattening can cause local stem thinning
// on curved symbols (e.g. infinity) even when AA is otherwise correct.
const CUBIC_TO_QUAD_TOLERANCE: f32 = 0.05;
const MAX_CUBIC_SPLIT_DEPTH: usize = 12;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GlyphShapeId(pub usize);

#[derive(Clone, Copy, Debug)]
pub struct GlyphLayerRef {
    pub color: Vec4f,
    pub curve_offset: usize,
    pub curve_count: usize,
    pub band_offset: usize,
    pub band_count: usize,
}

#[derive(Clone, Debug)]
pub struct GlyphShape {
    pub origin: Vec2f,
    pub size: Vec2f,
    pub layers: Vec<GlyphLayerRef>,
}

#[derive(Clone, Copy, Debug, Default)]
struct P2 {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug)]
struct QuadCurve {
    p0: P2,
    p1: P2,
    p2: P2,
}

#[derive(Clone, Copy, Debug, Default)]
struct BBox {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    valid: bool,
}

impl BBox {
    fn include(&mut self, p: P2) {
        if !self.valid {
            self.min_x = p.x;
            self.min_y = p.y;
            self.max_x = p.x;
            self.max_y = p.y;
            self.valid = true;
            return;
        }
        self.min_x = self.min_x.min(p.x);
        self.min_y = self.min_y.min(p.y);
        self.max_x = self.max_x.max(p.x);
        self.max_y = self.max_y.max(p.y);
    }

    fn union_with(&mut self, other: BBox) {
        if !other.valid {
            return;
        }
        if !self.valid {
            *self = other;
            return;
        }
        self.min_x = self.min_x.min(other.min_x);
        self.min_y = self.min_y.min(other.min_y);
        self.max_x = self.max_x.max(other.max_x);
        self.max_y = self.max_y.max(other.max_y);
        self.valid = true;
    }

    fn width(&self) -> f32 {
        self.max_x - self.min_x
    }

    fn height(&self) -> f32 {
        self.max_y - self.min_y
    }
}

#[derive(Clone, Debug)]
struct PendingLayer {
    color: Vec4f,
    curves: Vec<QuadCurve>,
    bounds: BBox,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGlyph {
    #[rust]
    pub many_instances: Option<ManyInstances>,
    #[rust]
    pub path: VectorPath,
    #[rust]
    pending_layers: Vec<PendingLayer>,
    #[rust]
    pending_color: Vec4f,
    #[rust]
    curve_data: Vec<f32>,
    #[rust]
    band_data: Vec<f32>,
    #[rust]
    curve_texture: Option<Texture>,
    #[rust]
    band_texture: Option<Texture>,
    #[rust]
    curve_dirty: bool,
    #[rust]
    band_dirty: bool,
    #[rust]
    shapes: Vec<GlyphShape>,
    #[rust]
    curve_tex_width: usize,
    #[rust]
    band_tex_width: usize,
    #[rust]
    default_num_bands: usize,
    #[rust]
    initialized: bool,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub rect_pos: Vec2f,
    #[live]
    pub rect_size: Vec2f,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub draw_depth: f32,
    #[live(vec4(1., 1., 1., 1.))]
    pub color: Vec4f,
    #[live]
    pub curve_offset: f32,
    #[live]
    pub curve_count: f32,
    #[live]
    pub band_offset: f32,
    #[live]
    pub band_count: f32,
    #[live]
    pub layer_order: f32,
    #[live(512.0)]
    pub max_band_curves: f32,
    #[live(0.0)]
    pub aa_2x2: f32,
    #[live(0.0)]
    pub aa_4x4: f32,
    #[live(0.0)]
    pub aa_pad_px: f32,
    #[live(0.0)]
    pub axis_relief: f32,
    #[live(0.0)]
    pub stem_darken: f32,
    #[live(0.125)]
    pub stem_darken_max: f32,
}

impl DrawGlyph {
    pub fn begin_shape(&mut self) {
        self.ensure_initialized();
        self.path.clear();
        self.pending_layers.clear();
        self.pending_color = vec4(1.0, 1.0, 1.0, 1.0);
    }

    pub fn set_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.pending_color = vec4(r, g, b, a);
    }

    pub fn set_color_vec4(&mut self, color: Vec4f) {
        self.pending_color = color;
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(x, y);
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(x, y);
    }

    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path.quad_to(cx, cy, x, y);
    }

    pub fn bezier_to(&mut self, cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) {
        self.path.bezier_to(cx1, cy1, cx2, cy2, x, y);
    }

    pub fn close(&mut self) {
        self.path.close();
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.path.rect(x, y, w, h);
    }

    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        self.path.rounded_rect(x, y, w, h, r);
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32) {
        self.path.circle(cx, cy, r);
    }

    pub fn fill_layer(&mut self) {
        self.ensure_initialized();
        let (curves, bounds) = path_to_quads(&self.path);
        self.path.clear();
        if curves.is_empty() || !bounds.valid {
            return;
        }
        self.pending_layers.push(PendingLayer {
            color: self.pending_color,
            curves,
            bounds,
        });
    }

    pub fn add_curves_layer(&mut self, color: Vec4f, curves: &[(Vec2f, Vec2f, Vec2f)]) {
        self.ensure_initialized();
        if curves.is_empty() {
            return;
        }
        let mut out = Vec::with_capacity(curves.len());
        let mut bounds = BBox::default();
        for (p0, p1, p2) in curves {
            let q = QuadCurve {
                p0: p2f(*p0),
                p1: p2f(*p1),
                p2: p2f(*p2),
            };
            bounds.include(q.p0);
            bounds.include(q.p1);
            bounds.include(q.p2);
            out.push(q);
        }
        self.pending_layers.push(PendingLayer {
            color,
            curves: out,
            bounds,
        });
    }

    pub fn add_outline_layer(&mut self, outline: &GlyphOutline, color: Vec4f) {
        self.ensure_initialized();
        self.path.clear();
        self.pending_color = color;
        for command in outline.commands().iter().copied() {
            match command {
                OutlineCommand::MoveTo(p) => self.path.move_to(p.x, p.y),
                OutlineCommand::LineTo(p) => self.path.line_to(p.x, p.y),
                OutlineCommand::QuadTo(c, p) => self.path.quad_to(c.x, c.y, p.x, p.y),
                OutlineCommand::CurveTo(c1, c2, p) => {
                    self.path.bezier_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y)
                }
                OutlineCommand::Close => self.path.close(),
            }
        }
        self.fill_layer();
    }

    pub fn commit_shape(&mut self, num_bands: Option<usize>) -> Option<GlyphShapeId> {
        self.ensure_initialized();
        if self.pending_layers.is_empty() {
            return None;
        }

        let mut shape_bounds = BBox::default();
        for layer in &self.pending_layers {
            shape_bounds.union_with(layer.bounds);
        }
        if !shape_bounds.valid {
            self.pending_layers.clear();
            return None;
        }

        let width = shape_bounds.width();
        let height = shape_bounds.height();
        if width <= 0.000001 || height <= 0.000001 {
            self.pending_layers.clear();
            return None;
        }

        let inv_w = 1.0 / width;
        let inv_h = 1.0 / height;
        let band_count = num_bands.unwrap_or(self.default_num_bands);
        let mut layers = Vec::with_capacity(self.pending_layers.len());

        let pending = mem::take(&mut self.pending_layers);
        for layer in pending {
            let curve_offset = self.curve_count_total();
            let curve_count = layer.curves.len();
            let mut normalized_curves = Vec::with_capacity(curve_count);
            for curve in layer.curves {
                let nq = QuadCurve {
                    p0: normalize_point(curve.p0, shape_bounds, inv_w, inv_h),
                    p1: normalize_point(curve.p1, shape_bounds, inv_w, inv_h),
                    p2: normalize_point(curve.p2, shape_bounds, inv_w, inv_h),
                };
                normalized_curves.push(nq);
                self.curve_data.extend_from_slice(&[
                    nq.p0.x, nq.p0.y, nq.p1.x, nq.p1.y, nq.p2.x, nq.p2.y, 0.0, 0.0,
                ]);
            }
            let (band_offset, actual_band_count) = if band_count > 0 {
                self.build_bands(curve_offset, &normalized_curves, band_count)
            } else {
                (0, 0)
            };
            layers.push(GlyphLayerRef {
                color: layer.color,
                curve_offset,
                curve_count,
                band_offset,
                band_count: actual_band_count,
            });
        }

        self.curve_dirty = true;
        self.band_dirty = true;

        let shape = GlyphShape {
            origin: Vec2f {
                x: shape_bounds.min_x,
                y: shape_bounds.min_y,
            },
            size: Vec2f {
                x: width,
                y: height,
            },
            layers,
        };
        let shape_id = GlyphShapeId(self.shapes.len());
        self.shapes.push(shape);
        self.path.clear();
        Some(shape_id)
    }

    pub fn clear_shapes(&mut self) {
        self.ensure_initialized();
        self.path.clear();
        self.pending_layers.clear();
        self.shapes.clear();
        self.curve_data.clear();
        self.band_data.clear();
        self.curve_dirty = true;
        self.band_dirty = true;
    }

    pub fn shape(&self, shape_id: GlyphShapeId) -> Option<&GlyphShape> {
        self.shapes.get(shape_id.0)
    }

    pub fn draw_shape_walk(&mut self, cx: &mut Cx2d, walk: Walk, shape_id: GlyphShapeId) -> Rect {
        let rect = cx.walk_turtle(walk);
        self.draw_shape_abs(cx, shape_id, rect);
        rect
    }

    pub fn draw_shape_abs(&mut self, cx: &mut Cx2d, shape_id: GlyphShapeId, rect: Rect) {
        let Some(shape) = self.shapes.get(shape_id.0) else {
            return;
        };
        let layers = shape.layers.clone();
        self.draw_layers_abs(cx, rect, &layers);
    }

    pub fn draw_shape(&mut self, cx: &mut Cx2d, shape_id: GlyphShapeId, pos: Vec2f, size: Vec2f) {
        self.draw_shape_abs(
            cx,
            shape_id,
            rect(pos.x as f64, pos.y as f64, size.x as f64, size.y as f64),
        );
    }

    pub fn draw_shape_natural_size(&mut self, cx: &mut Cx2d, shape_id: GlyphShapeId, pos: Vec2f) {
        let Some(shape) = self.shapes.get(shape_id.0) else {
            return;
        };
        let size = shape.size;
        self.draw_shape(cx, shape_id, pos, size);
    }

    pub fn draw_layers_abs(&mut self, cx: &mut Cx2d, rect: Rect, layers: &[GlyphLayerRef]) {
        if layers.is_empty() {
            return;
        }
        self.update_draw_vars(cx);
        let pad = self.aa_pad_px.max(0.0) as f64;
        let rect = if pad > 0.0 {
            crate::makepad_platform::Rect {
                pos: DVec2 {
                    x: rect.pos.x - pad,
                    y: rect.pos.y - pad,
                },
                size: DVec2 {
                    x: rect.size.x + pad * 2.0,
                    y: rect.size.y + pad * 2.0,
                },
            }
        } else {
            rect
        };
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();

        if layers.len() == 1 {
            self.apply_layer(&layers[0], 0.0);
            self.push_instance(cx);
            return;
        }

        let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
            return;
        };
        for (index, layer) in layers.iter().enumerate() {
            self.apply_layer(layer, index as f32);
            instances
                .instances
                .extend_from_slice(self.draw_vars.as_slice());
        }
        let new_area = cx.end_many_instances(instances);
        let old_area = self.draw_vars.area;
        self.draw_vars.area = cx.update_area_refs(old_area, new_area);
    }

    fn ensure_initialized(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.curve_tex_width = CURVE_TEX_WIDTH;
        self.band_tex_width = BAND_TEX_WIDTH;
        self.default_num_bands = DEFAULT_NUM_BANDS;
        self.pending_color = vec4(1.0, 1.0, 1.0, 1.0);
        self.curve_dirty = true;
        self.band_dirty = true;
    }

    fn apply_layer(&mut self, layer: &GlyphLayerRef, order: f32) {
        self.color = layer.color;
        self.curve_offset = layer.curve_offset as f32;
        self.curve_count = layer.curve_count as f32;
        self.band_offset = layer.band_offset as f32;
        self.band_count = layer.band_count as f32;
        self.layer_order = order;
    }

    fn push_instance(&mut self, cx: &mut Cx2d) {
        if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            let old_area = self.draw_vars.area;
            self.draw_vars.area = cx.update_area_refs(old_area, new_area);
        }
    }

    fn curve_count_total(&self) -> usize {
        self.curve_data.len() / 8
    }

    fn build_bands(
        &mut self,
        curve_offset: usize,
        curves: &[QuadCurve],
        num_bands: usize,
    ) -> (usize, usize) {
        if curves.is_empty() || num_bands == 0 {
            return (0, 0);
        }

        let band_offset = self.band_data.len() / 4;
        let metadata_floats = num_bands * 4;
        self.band_data
            .resize(self.band_data.len() + metadata_floats, 0.0);
        let mut band_lists = vec![Vec::<f32>::new(); num_bands];
        let bands_f = num_bands as f32;
        let max_band = (num_bands - 1) as isize;

        for (curve_index, curve) in curves.iter().enumerate() {
            let y_min = curve.p0.y.min(curve.p1.y).min(curve.p2.y).clamp(0.0, 1.0);
            let y_max = curve.p0.y.max(curve.p1.y).max(curve.p2.y).clamp(0.0, 1.0);
            let mut lo = (y_min * bands_f).floor() as isize;
            let mut hi = (y_max * bands_f).floor() as isize;
            lo = lo.clamp(0, max_band);
            hi = hi.clamp(0, max_band);
            if hi < lo {
                std::mem::swap(&mut lo, &mut hi);
            }
            let absolute_curve = (curve_offset + curve_index) as f32;
            for band in lo..=hi {
                band_lists[band as usize].push(absolute_curve);
            }
        }

        let mut list_texel_offset = band_offset + num_bands;
        for (band, list) in band_lists.into_iter().enumerate() {
            let meta = (band_offset + band) * 4;
            self.band_data[meta] = list_texel_offset as f32;
            self.band_data[meta + 1] = list.len() as f32;
            self.band_data[meta + 2] = 0.0;
            self.band_data[meta + 3] = 0.0;

            for chunk in list.chunks(4) {
                let mut texel = [0.0f32; 4];
                for (i, value) in chunk.iter().enumerate() {
                    texel[i] = *value;
                }
                self.band_data.extend_from_slice(&texel);
                list_texel_offset += 1;
            }
        }

        (band_offset, num_bands)
    }

    fn update_draw_vars(&mut self, cx: &mut Cx2d) {
        self.ensure_initialized();
        self.upload_textures(cx.cx.cx);
        self.draw_vars.texture_slots[0] = self.curve_texture.clone();
        self.draw_vars.texture_slots[1] = self.band_texture.clone();
    }

    fn upload_textures(&mut self, cx: &mut Cx) {
        let curve_texture = self.curve_texture.get_or_insert_with(|| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width: 1,
                    height: 1,
                    data: None,
                    updated: TextureUpdated::Empty,
                },
            )
        });
        let band_texture = self.band_texture.get_or_insert_with(|| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width: 1,
                    height: 1,
                    data: None,
                    updated: TextureUpdated::Empty,
                },
            )
        });

        if self.curve_dirty {
            let width = if self.curve_data.is_empty() {
                1
            } else {
                self.curve_tex_width.max(1)
            };
            let texels = (self.curve_data.len() / 4).max(1);
            let height = (texels + width - 1) / width;
            let mut data = if self.curve_data.is_empty() {
                vec![0.0f32; width * height * 4]
            } else {
                self.curve_data.clone()
            };
            data.resize(width * height * 4, 0.0);
            *curve_texture.get_format(cx) = TextureFormat::VecRGBAf32 {
                width,
                height,
                data: Some(data),
                updated: TextureUpdated::Full,
            };
            self.curve_dirty = false;
        }

        if self.band_dirty {
            let width = if self.band_data.is_empty() {
                1
            } else {
                self.band_tex_width.max(1)
            };
            let texels = (self.band_data.len() / 4).max(1);
            let height = (texels + width - 1) / width;
            let mut data = if self.band_data.is_empty() {
                vec![0.0f32; width * height * 4]
            } else {
                self.band_data.clone()
            };
            data.resize(width * height * 4, 0.0);
            *band_texture.get_format(cx) = TextureFormat::VecRGBAf32 {
                width,
                height,
                data: Some(data),
                updated: TextureUpdated::Full,
            };
            self.band_dirty = false;
        }
    }
}

fn p2f(v: Vec2f) -> P2 {
    P2 { x: v.x, y: v.y }
}

fn normalize_point(p: P2, bounds: BBox, inv_w: f32, inv_h: f32) -> P2 {
    P2 {
        x: (p.x - bounds.min_x) * inv_w,
        y: (p.y - bounds.min_y) * inv_h,
    }
}

fn path_to_quads(path: &VectorPath) -> (Vec<QuadCurve>, BBox) {
    let mut curves = Vec::new();
    let mut bounds = BBox::default();
    let mut current = None::<P2>;
    let mut contour_start = None::<P2>;

    for command in &path.cmds {
        match *command {
            PathCmd::MoveTo(x, y) => {
                let p = P2 { x, y };
                current = Some(p);
                contour_start = Some(p);
                bounds.include(p);
            }
            PathCmd::LineTo(x, y) => {
                let Some(p0) = current else {
                    continue;
                };
                let p2 = P2 { x, y };
                let p1 = midpoint(p0, p2);
                push_quad(&mut curves, &mut bounds, QuadCurve { p0, p1, p2 });
                current = Some(p2);
            }
            PathCmd::BezierTo(c1x, c1y, c2x, c2y, x, y) => {
                let Some(p0) = current else {
                    continue;
                };
                let p1 = P2 { x: c1x, y: c1y };
                let p2 = P2 { x: c2x, y: c2y };
                let p3 = P2 { x, y };
                bounds.include(p0);
                bounds.include(p1);
                bounds.include(p2);
                bounds.include(p3);
                cubic_to_quads_recursive(p0, p1, p2, p3, 0, &mut curves, &mut bounds);
                current = Some(p3);
            }
            PathCmd::Close => {
                if let (Some(p0), Some(ps)) = (current, contour_start) {
                    if !same_point(p0, ps) {
                        let p1 = midpoint(p0, ps);
                        push_quad(&mut curves, &mut bounds, QuadCurve { p0, p1, p2: ps });
                    }
                    current = Some(ps);
                }
            }
            PathCmd::Winding(_) => {}
        }
    }

    (curves, bounds)
}

fn push_quad(curves: &mut Vec<QuadCurve>, bounds: &mut BBox, curve: QuadCurve) {
    bounds.include(curve.p0);
    bounds.include(curve.p1);
    bounds.include(curve.p2);
    curves.push(curve);
}

fn same_point(a: P2, b: P2) -> bool {
    (a.x - b.x).abs() <= 0.000001 && (a.y - b.y).abs() <= 0.000001
}

fn midpoint(a: P2, b: P2) -> P2 {
    P2 {
        x: (a.x + b.x) * 0.5,
        y: (a.y + b.y) * 0.5,
    }
}

fn eval_quad(p0: P2, p1: P2, p2: P2, t: f32) -> P2 {
    let s = 1.0 - t;
    P2 {
        x: s * s * p0.x + 2.0 * s * t * p1.x + t * t * p2.x,
        y: s * s * p0.y + 2.0 * s * t * p1.y + t * t * p2.y,
    }
}

fn eval_cubic(p0: P2, p1: P2, p2: P2, p3: P2, t: f32) -> P2 {
    let s = 1.0 - t;
    let s2 = s * s;
    let t2 = t * t;
    P2 {
        x: p0.x * s2 * s + 3.0 * p1.x * s2 * t + 3.0 * p2.x * s * t2 + p3.x * t2 * t,
        y: p0.y * s2 * s + 3.0 * p1.y * s2 * t + 3.0 * p2.y * s * t2 + p3.y * t2 * t,
    }
}

fn distance(a: P2, b: P2) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

fn cubic_to_quad_control(p0: P2, p1: P2, p2: P2, p3: P2) -> P2 {
    P2 {
        x: (3.0 * (p1.x + p2.x) - p0.x - p3.x) * 0.25,
        y: (3.0 * (p1.y + p2.y) - p0.y - p3.y) * 0.25,
    }
}

fn cubic_to_quads_recursive(
    p0: P2,
    p1: P2,
    p2: P2,
    p3: P2,
    depth: usize,
    out: &mut Vec<QuadCurve>,
    bounds: &mut BBox,
) {
    let qc = cubic_to_quad_control(p0, p1, p2, p3);
    let q = QuadCurve { p0, p1: qc, p2: p3 };
    let e25 = distance(
        eval_cubic(p0, p1, p2, p3, 0.25),
        eval_quad(q.p0, q.p1, q.p2, 0.25),
    );
    let e50 = distance(
        eval_cubic(p0, p1, p2, p3, 0.50),
        eval_quad(q.p0, q.p1, q.p2, 0.50),
    );
    let e75 = distance(
        eval_cubic(p0, p1, p2, p3, 0.75),
        eval_quad(q.p0, q.p1, q.p2, 0.75),
    );
    let max_err = e25.max(e50).max(e75);

    if max_err <= CUBIC_TO_QUAD_TOLERANCE || depth >= MAX_CUBIC_SPLIT_DEPTH {
        push_quad(out, bounds, q);
        return;
    }

    let p01 = midpoint(p0, p1);
    let p12 = midpoint(p1, p2);
    let p23 = midpoint(p2, p3);
    let p012 = midpoint(p01, p12);
    let p123 = midpoint(p12, p23);
    let p0123 = midpoint(p012, p123);

    cubic_to_quads_recursive(p0, p01, p012, p0123, depth + 1, out, bounds);
    cubic_to_quads_recursive(p0123, p123, p23, p3, depth + 1, out, bounds);
}
