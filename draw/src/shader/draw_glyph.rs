use {
    crate::{
        cx_2d::*,
        draw_list_2d::ManyInstances,
        makepad_platform::*,
        turtle::*,
        vector::{
            bands::{build_bands, BandMode, BandOptions, BandScratch},
            PathCmd, VectorPath,
        },
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
        ..mod.glyph_coverage.SlugCoverage

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
        aa_pad_px: uniform(float(1.0))
        slug_viewport_px: uniform(vec2(1.0, 1.0))
        axis_relief: 0.0
        stem_darken: 0.0
        stem_darken_max: 0.125
        fill_flags: 0.0

        pos: varying(vec2f)
        world: varying(vec4f)



        vertex: fn() {
            let pad_lpx = self.aa_pad_px / max(self.draw_pass.dpi_factor, 0.0001)
            let content_rect_pos = self.rect_pos + vec2(pad_lpx, pad_lpx)
            let content_rect_size = vec2(
                max(self.rect_size.x - 2.0 * pad_lpx, 0.0001),
                max(self.rect_size.y - 2.0 * pad_lpx, 0.0001)
            )
            let p = mix(content_rect_pos, content_rect_pos + content_rect_size, self.geom.pos)
            let jac = vec4(1.0 / content_rect_size.x, 0.0, 0.0, 1.0 / content_rect_size.y)
            let corner = self.geom.pos * 2.0 - 1.0
            let normal = if dot(corner, corner) > 0.000001 {
                corner
            } else {
                vec2(1.0, 0.0)
            }
            let dilated = self.slug_dilate(p, self.geom.pos, jac, normal)
            let p_clipped = clamp(dilated.zw, self.draw_clip.xy, self.draw_clip.zw)
            self.pos = vec2(
                dilated.x + (p_clipped.x - dilated.z) * jac.x,
                dilated.y + (p_clipped.y - dilated.w) * jac.w
            )
            self.world = self.draw_list.view_transform * vec4(
                p_clipped.x,
                p_clipped.y,
                self.draw_depth + self.layer_order * 0.000001 + self.draw_call.zbias,
                1.
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }













        // where this glyph's curves are in the curve texture
        slug_curve_offset: fn() -> float {
            return self.curve_offset
        }

        slug_curve_count: fn() -> float {
            return self.curve_count
        }

        slug_band_offset: fn() -> float {
            return self.band_offset
        }

        slug_band_count: fn() -> float {
            return self.band_count
        }

        slug_fill_flags: fn() -> float {
            return self.fill_flags
        }

        alpha_at: fn(sample: vec2, px_x: float, px_y: float) -> float {
            return self.alpha_at_banded(sample, px_x, px_y)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }

        get_color: fn(){
            self.color
        }

        pixel: fn() {
            if self.curve_count < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }

            let sample = self.pos
            let px_x = max(abs(dFdx(sample.x)) + abs(dFdy(sample.x)), 0.00001)
            let px_y = max(abs(dFdx(sample.y)) + abs(dFdy(sample.y)), 0.00001)
            // One loop over the supersample grid (4x4, 2x2, or the pixel centre),
            // so alpha_at and its four curve-scan loops are inlined once. Unrolled
            // into 21 calls they made Adreno 630's compiler spend ~3.5 s on each
            // text program. Offsets match the unrolled ones: (i + 0.5) / grid - 0.5 px.
            let grid = if self.aa_4x4 > 0.5 {4.0} else if self.aa_2x2 > 0.5 {2.0} else {1.0}
            // 1/grid is exact (1, 1/2, 1/4): offsets and the average round
            // exactly as the unrolled sums did, and the row index is exact
            // (no division, which some GPUs approximate).
            let inv_grid = if self.aa_4x4 > 0.5 {0.25} else if self.aa_2x2 > 0.5 {0.5} else {1.0}
            var alpha_sum = 0.0
            var sample_index = 0.0
            loop {
                if sample_index >= grid * grid { break }
                let row = floor(sample_index * inv_grid)
                let col = sample_index - row * grid
                let offset = (vec2(col, row) + 0.5 - grid * 0.5) * inv_grid * vec2(px_x, px_y)
                alpha_sum = alpha_sum + self.alpha_at(sample + offset, px_x, px_y)
                sample_index = sample_index + 1.0
            }
            let alpha_base = clamp(alpha_sum * inv_grid * inv_grid, 0.0, 1.0)
            let darken = clamp(max(px_x, px_y) * self.stem_darken, 0.0, self.stem_darken_max)
            let edge_weight = clamp(1.0 - abs(alpha_base * 2.0 - 1.0), 0.0, 1.0)
            let alpha = clamp(alpha_base + darken * edge_weight, 0.0, 1.0)
            let color = self.get_color();
            return vec4(color.rgb * color.a * alpha, color.a * alpha)
        }
    }
}

const CURVE_TEX_WIDTH: usize = 2048;
const BAND_TEX_WIDTH: usize = 2048;
const DEFAULT_NUM_BANDS: usize = 24;
const GLYPH_FILL_FLAG_EVEN_ODD: u32 = 0x1000;
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
    pub flags: u32,
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
    flags: u32,
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
    pending_flags: u32,
    #[rust]
    curve_data: Vec<f32>,
    #[rust]
    band_data: Vec<f32>,
    #[rust]
    band_scratch: BandScratch,
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
    #[live(0.0)]
    pub draw_depth: f32,
    #[live(1.0)]
    pub depth_clip: f32,
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
    pub axis_relief: f32,
    #[live(0.0)]
    pub stem_darken: f32,
    #[live(0.125)]
    pub stem_darken_max: f32,
    #[live(0.0)]
    pub fill_flags: f32,
}

impl DrawGlyph {
    pub fn begin_shape(&mut self) {
        self.ensure_initialized();
        self.path.clear();
        self.pending_layers.clear();
        self.pending_color = vec4(1.0, 1.0, 1.0, 1.0);
        self.pending_flags = 0;
    }

    pub fn set_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.pending_color = vec4(r, g, b, a);
    }

    pub fn set_color_vec4(&mut self, color: Vec4f) {
        self.pending_color = color;
    }

    pub fn set_even_odd_fill(&mut self, even_odd: bool) {
        self.pending_flags = if even_odd {
            GLYPH_FILL_FLAG_EVEN_ODD
        } else {
            0
        };
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
            flags: self.pending_flags,
        });
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
            for curve in layer.curves {
                let nq = QuadCurve {
                    p0: normalize_point(curve.p0, shape_bounds, inv_w, inv_h),
                    p1: normalize_point(curve.p1, shape_bounds, inv_w, inv_h),
                    p2: normalize_point(curve.p2, shape_bounds, inv_w, inv_h),
                };
                self.curve_data.extend_from_slice(&[
                    nq.p0.x, nq.p0.y, nq.p1.x, nq.p1.y, nq.p2.x, nq.p2.y, 0.0, 0.0,
                ]);
            }
            let (band_offset, actual_band_count) = if band_count > 0 {
                build_bands(
                    curve_offset,
                    &self.curve_data[curve_offset * 8..],
                    &mut self.band_data,
                    BandOptions {
                        bounds: [0.0, 0.0, 1.0, 1.0],
                        count: band_count,
                        reach: 0.0,
                        mode: BandMode::Coverage,
                    },
                    &mut self.band_scratch,
                )
            } else {
                (0, 0)
            };
            layers.push(GlyphLayerRef {
                color: layer.color,
                curve_offset,
                curve_count,
                band_offset,
                band_count: actual_band_count,
                flags: layer.flags,
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
        self.pending_flags = 0;
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
        let pad = (self.get_aa_pad_px(cx.cx.cx) / cx.current_dpi_factor() as f32).max(0.0) as f64;
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
        self.pending_flags = 0;
        self.curve_dirty = true;
        self.band_dirty = true;
    }

    pub fn set_aa_pad_px(&mut self, cx: &Cx, aa_pad_px: f32) {
        self.draw_vars
            .set_uniform(cx, live_id!(aa_pad_px), &[aa_pad_px]);
    }

    pub fn get_aa_pad_px(&self, cx: &mut Cx) -> f32 {
        let mut value = [0.0];
        self.draw_vars
            .get_uniform(cx, live_id!(aa_pad_px), &mut value);
        value[0]
    }

    fn apply_layer(&mut self, layer: &GlyphLayerRef, order: f32) {
        self.color = layer.color;
        self.curve_offset = layer.curve_offset as f32;
        self.curve_count = layer.curve_count as f32;
        self.band_offset = layer.band_offset as f32;
        self.band_count = layer.band_count as f32;
        self.fill_flags = layer.flags as f32;
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

    fn update_draw_vars(&mut self, cx: &mut Cx2d) {
        self.ensure_initialized();
        self.upload_textures(cx.cx.cx);
        // The camera itself is read by the shader at render time
        // (`slug_dilate`); only the viewport is the draw's.
        let viewport = cx.current_pass_size();
        let dpi_factor = cx.current_dpi_factor() as f32;
        let viewport_px = [
            (viewport.x as f32 * dpi_factor).max(1.0),
            (viewport.y as f32 * dpi_factor).max(1.0),
        ];

        self.draw_vars
            .set_uniform(cx.cx, live_id!(slug_viewport_px), &viewport_px);
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
            let height = texels.div_ceil(width);
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
            let height = texels.div_ceil(width);
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
