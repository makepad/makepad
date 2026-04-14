use {
    super::{
        font::{Font, FontId, GlyphId},
        geom::{Point, Rect, Size},
        glyph_outline::{Command, GlyphOutline},
    },
    crate::makepad_platform::*,
    fxhash::FxHashMap,
    std::cmp::Ordering,
};

const CURVE_TEX_WIDTH: usize = 2048;
const BAND_TEX_WIDTH: usize = 2048;
const DEFAULT_NUM_BANDS: usize = 24;
const CUBIC_TO_QUAD_TOLERANCE: f32 = 0.05;
const MAX_CUBIC_SPLIT_DEPTH: usize = 12;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SlugGlyphKey {
    font_id: FontId,
    glyph_id: GlyphId,
}

#[derive(Clone, Copy, Debug)]
pub struct SlugGlyphInfo {
    pub origin_in_ems: Point<f32>,
    pub size_in_ems: Size<f32>,
    pub curve_offset: usize,
    pub curve_count: usize,
    pub band_offset: usize,
    pub band_count: usize,
    pub fill_flags: u32,
}

pub struct SlugAtlas {
    curve_data: Vec<f32>,
    band_data: Vec<f32>,
    curve_texture: Texture,
    band_texture: Texture,
    curve_dirty: bool,
    band_dirty: bool,
    cache_generation: u64,
    uploaded_generation: u64,
    cached_glyphs: FxHashMap<SlugGlyphKey, SlugGlyphInfo>,
}

impl SlugAtlas {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            curve_data: Vec::new(),
            band_data: Vec::new(),
            curve_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width: 1,
                    height: 1,
                    data: None,
                    updated: TextureUpdated::Empty,
                },
            ),
            band_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width: 1,
                    height: 1,
                    data: None,
                    updated: TextureUpdated::Empty,
                },
            ),
            curve_dirty: false,
            band_dirty: false,
            cache_generation: 0,
            uploaded_generation: 0,
            cached_glyphs: FxHashMap::default(),
        }
    }

    pub fn curve_texture(&self) -> &Texture {
        &self.curve_texture
    }

    pub fn band_texture(&self) -> &Texture {
        &self.band_texture
    }

    pub fn cache_generation(&self) -> u64 {
        self.cache_generation
    }

    pub fn uploaded_generation(&self) -> u64 {
        self.uploaded_generation
    }

    pub fn get_or_cache_glyph(&mut self, font: &Font, glyph_id: GlyphId) -> Option<SlugGlyphInfo> {
        let key = SlugGlyphKey {
            font_id: font.id(),
            glyph_id,
        };
        if let Some(info) = self.cached_glyphs.get(&key).copied() {
            return Some(info);
        }

        let outline = font.glyph_outline(glyph_id)?;
        let info = self.build_glyph(font, &outline)?;
        self.cached_glyphs.insert(key, info);
        Some(info)
    }

    pub fn prepare_textures(&mut self, cx: &mut Cx) -> bool {
        let mut changed = false;

        if self.curve_dirty {
            let width = if self.curve_data.is_empty() {
                1
            } else {
                CURVE_TEX_WIDTH.max(1)
            };
            let texels = (self.curve_data.len() / 4).max(1);
            let height = texels.div_ceil(width);
            let mut data = if self.curve_data.is_empty() {
                vec![0.0f32; width * height * 4]
            } else {
                self.curve_data.clone()
            };
            data.resize(width * height * 4, 0.0);
            *self.curve_texture.get_format(cx) = TextureFormat::VecRGBAf32 {
                width,
                height,
                data: Some(data),
                updated: TextureUpdated::Full,
            };
            self.curve_dirty = false;
            changed = true;
        }

        if self.band_dirty {
            let width = if self.band_data.is_empty() {
                1
            } else {
                BAND_TEX_WIDTH.max(1)
            };
            let texels = (self.band_data.len() / 4).max(1);
            let height = texels.div_ceil(width);
            let mut data = if self.band_data.is_empty() {
                vec![0.0f32; width * height * 4]
            } else {
                self.band_data.clone()
            };
            data.resize(width * height * 4, 0.0);
            *self.band_texture.get_format(cx) = TextureFormat::VecRGBAf32 {
                width,
                height,
                data: Some(data),
                updated: TextureUpdated::Full,
            };
            self.band_dirty = false;
            changed = true;
        }

        if changed {
            self.uploaded_generation = self.cache_generation;
        }

        changed
    }

    fn build_glyph(&mut self, font: &Font, outline: &GlyphOutline) -> Option<SlugGlyphInfo> {
        let bounds = outline.bounds_in_ems();
        if bounds.size.width <= 0.000001 || bounds.size.height <= 0.000001 {
            return None;
        }

        let curves = outline_to_normalized_quads(outline, bounds, font.units_per_em());
        if curves.is_empty() {
            return None;
        }

        let curve_offset = self.curve_data.len() / 8;
        for curve in &curves {
            self.curve_data.extend_from_slice(&[
                curve.p0.x, curve.p0.y, curve.p1.x, curve.p1.y, curve.p2.x, curve.p2.y, 0.0, 0.0,
            ]);
        }

        let (band_offset, band_count) = self.build_bands(curve_offset, &curves, DEFAULT_NUM_BANDS);
        self.curve_dirty = true;
        self.band_dirty = true;
        self.cache_generation = self.cache_generation.wrapping_add(1);

        Some(SlugGlyphInfo {
            origin_in_ems: bounds.origin,
            size_in_ems: bounds.size,
            curve_offset,
            curve_count: curves.len(),
            band_offset,
            band_count,
            fill_flags: 0,
        })
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
        let metadata_floats = num_bands * 2 * 4;
        self.band_data
            .resize(self.band_data.len() + metadata_floats, 0.0);
        let mut horizontal_bands = vec![Vec::<usize>::new(); num_bands];
        let mut vertical_bands = vec![Vec::<usize>::new(); num_bands];
        let bands_f = num_bands as f32;
        let epsilon = 1.0 / 1024.0;

        for (curve_index, curve) in curves.iter().enumerate() {
            if !curve_is_horizontal(curve) {
                if let Some((lo, hi)) = band_range(
                    curve.p0.y.min(curve.p1.y).min(curve.p2.y) - epsilon,
                    curve.p0.y.max(curve.p1.y).max(curve.p2.y) + epsilon,
                    bands_f,
                    num_bands,
                ) {
                    for band in lo..=hi {
                        horizontal_bands[band].push(curve_index);
                    }
                }
            }

            if !curve_is_vertical(curve) {
                if let Some((lo, hi)) = band_range(
                    curve.p0.x.min(curve.p1.x).min(curve.p2.x) - epsilon,
                    curve.p0.x.max(curve.p1.x).max(curve.p2.x) + epsilon,
                    bands_f,
                    num_bands,
                ) {
                    for band in lo..=hi {
                        vertical_bands[band].push(curve_index);
                    }
                }
            }
        }

        for list in &mut horizontal_bands {
            list.sort_by(|a, b| {
                curve_max_x(curves[*b])
                    .partial_cmp(&curve_max_x(curves[*a]))
                    .unwrap_or(Ordering::Equal)
            });
        }
        for list in &mut vertical_bands {
            list.sort_by(|a, b| {
                curve_max_y(curves[*b])
                    .partial_cmp(&curve_max_y(curves[*a]))
                    .unwrap_or(Ordering::Equal)
            });
        }

        let mut list_texel_offset = band_offset + num_bands * 2;
        for (band, list) in horizontal_bands
            .into_iter()
            .chain(vertical_bands.into_iter())
            .enumerate()
        {
            let meta = (band_offset + band) * 4;
            self.band_data[meta] = list_texel_offset as f32;
            self.band_data[meta + 1] = list.len() as f32;
            self.band_data[meta + 2] = 0.0;
            self.band_data[meta + 3] = 0.0;

            for chunk in list.chunks(4) {
                let mut texel = [0.0f32; 4];
                for (i, value) in chunk.iter().enumerate() {
                    texel[i] = (curve_offset + *value) as f32;
                }
                self.band_data.extend_from_slice(&texel);
                list_texel_offset += 1;
            }
        }

        (band_offset, num_bands)
    }
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

fn outline_to_normalized_quads(
    outline: &GlyphOutline,
    bounds: Rect<f32>,
    units_per_em: f32,
) -> Vec<QuadCurve> {
    let mut curves = Vec::new();
    let inv_units = 1.0 / units_per_em.max(0.000001);
    let inv_w = 1.0 / bounds.size.width.max(0.000001);
    let inv_h = 1.0 / bounds.size.height.max(0.000001);
    let mut current = None::<P2>;
    let mut contour_start = None::<P2>;

    for command in outline.commands().iter().copied() {
        match command {
            Command::MoveTo(p) => {
                let point = scale_point(p, inv_units);
                current = Some(point);
                contour_start = Some(point);
            }
            Command::LineTo(p) => {
                let Some(p0) = current else {
                    continue;
                };
                let p2 = scale_point(p, inv_units);
                let p1 = midpoint(p0, p2);
                curves.push(QuadCurve {
                    p0: normalize_point(p0, bounds, inv_w, inv_h),
                    p1: normalize_point(p1, bounds, inv_w, inv_h),
                    p2: normalize_point(p2, bounds, inv_w, inv_h),
                });
                current = Some(p2);
            }
            Command::QuadTo(c, p) => {
                let Some(p0) = current else {
                    continue;
                };
                let p1 = scale_point(c, inv_units);
                let p2 = scale_point(p, inv_units);
                curves.push(QuadCurve {
                    p0: normalize_point(p0, bounds, inv_w, inv_h),
                    p1: normalize_point(p1, bounds, inv_w, inv_h),
                    p2: normalize_point(p2, bounds, inv_w, inv_h),
                });
                current = Some(p2);
            }
            Command::CurveTo(c1, c2, p) => {
                let Some(p0) = current else {
                    continue;
                };
                let p1 = scale_point(c1, inv_units);
                let p2 = scale_point(c2, inv_units);
                let p3 = scale_point(p, inv_units);
                cubic_to_quads_recursive(p0, p1, p2, p3, 0, bounds, inv_w, inv_h, &mut curves);
                current = Some(p3);
            }
            Command::Close => {
                if let (Some(p0), Some(ps)) = (current, contour_start) {
                    if !same_point(p0, ps) {
                        let p1 = midpoint(p0, ps);
                        curves.push(QuadCurve {
                            p0: normalize_point(p0, bounds, inv_w, inv_h),
                            p1: normalize_point(p1, bounds, inv_w, inv_h),
                            p2: normalize_point(ps, bounds, inv_w, inv_h),
                        });
                    }
                    current = Some(ps);
                }
            }
        }
    }

    curves
}

fn scale_point(point: Point<f32>, inv_units: f32) -> P2 {
    P2 {
        x: point.x * inv_units,
        y: point.y * inv_units,
    }
}

fn normalize_point(point: P2, bounds: Rect<f32>, inv_w: f32, inv_h: f32) -> P2 {
    P2 {
        x: (point.x - bounds.origin.x) * inv_w,
        // Font outlines are Y-up, but DrawGlyph normalized quad space is Y-down.
        y: (bounds.origin.y + bounds.size.height - point.y) * inv_h,
    }
}

fn band_range(
    min_value: f32,
    max_value: f32,
    bands_f: f32,
    num_bands: usize,
) -> Option<(usize, usize)> {
    if num_bands == 0 {
        return None;
    }
    let max_band = (num_bands - 1) as isize;
    let mut lo = (min_value.clamp(0.0, 1.0) * bands_f).floor() as isize;
    let mut hi = (max_value.clamp(0.0, 1.0) * bands_f).floor() as isize;
    lo = lo.clamp(0, max_band);
    hi = hi.clamp(0, max_band);
    if hi < lo {
        std::mem::swap(&mut lo, &mut hi);
    }
    Some((lo as usize, hi as usize))
}

fn curve_is_horizontal(curve: &QuadCurve) -> bool {
    (curve.p0.y - curve.p1.y).abs() <= 0.000001 && (curve.p0.y - curve.p2.y).abs() <= 0.000001
}

fn curve_is_vertical(curve: &QuadCurve) -> bool {
    (curve.p0.x - curve.p1.x).abs() <= 0.000001 && (curve.p0.x - curve.p2.x).abs() <= 0.000001
}

fn curve_max_x(curve: QuadCurve) -> f32 {
    curve.p0.x.max(curve.p1.x).max(curve.p2.x)
}

fn curve_max_y(curve: QuadCurve) -> f32 {
    curve.p0.y.max(curve.p1.y).max(curve.p2.y)
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
    bounds: Rect<f32>,
    inv_w: f32,
    inv_h: f32,
    out: &mut Vec<QuadCurve>,
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
        out.push(QuadCurve {
            p0: normalize_point(q.p0, bounds, inv_w, inv_h),
            p1: normalize_point(q.p1, bounds, inv_w, inv_h),
            p2: normalize_point(q.p2, bounds, inv_w, inv_h),
        });
        return;
    }

    let p01 = midpoint(p0, p1);
    let p12 = midpoint(p1, p2);
    let p23 = midpoint(p2, p3);
    let p012 = midpoint(p01, p12);
    let p123 = midpoint(p12, p23);
    let p0123 = midpoint(p012, p123);

    cubic_to_quads_recursive(p0, p01, p012, p0123, depth + 1, bounds, inv_w, inv_h, out);
    cubic_to_quads_recursive(p0123, p123, p23, p3, depth + 1, bounds, inv_w, inv_h, out);
}
