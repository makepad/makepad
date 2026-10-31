//! Slug curve data of a glyph: its outline as normalised quadratic curves, the form the slug
//! coverage shader reads from the curve texture (two RGBA32F texels per curve).

use super::{
    font::{Font, GlyphId},
    geom::{Point, Rect, Size},
    glyph_outline::{Command, GlyphOutline},
};

pub const CUBIC_TO_QUAD_TOLERANCE: f32 = 0.05;
pub const MAX_CUBIC_SPLIT_DEPTH: usize = 12;
/// Floats per curve in the curve texture (two RGBA texels).
pub const CURVE_FLOATS: usize = 8;

/// A glyph's slug curves: its box in ems and the curve floats (`CURVE_FLOATS` per curve).
pub struct SlugCurves {
    pub origin_in_ems: Point<f32>,
    pub size_in_ems: Size<f32>,
    pub curves: Vec<f32>,
}

impl SlugCurves {
    pub fn curve_count(&self) -> usize {
        self.curves.len() / CURVE_FLOATS
    }
}

/// The slug curves of a glyph; `None` for a glyph without an outline (or without area).
pub fn slug_curves(font: &Font, glyph_id: GlyphId) -> Option<SlugCurves> {
    let outline = font.glyph_outline_rc(glyph_id)?;
    let bounds = outline.bounds_in_ems();
    if bounds.size.width <= 0.000001 || bounds.size.height <= 0.000001 {
        return None;
    }
    let curves = outline_to_normalized_quads(&outline, bounds, font.units_per_em());
    if curves.is_empty() {
        return None;
    }
    let mut floats = Vec::with_capacity(curves.len() * CURVE_FLOATS);
    for curve in &curves {
        floats.extend_from_slice(&[curve.p0.x, curve.p0.y, curve.p1.x, curve.p1.y, curve.p2.x, curve.p2.y, 0.0, 0.0]);
    }
    Some(SlugCurves { origin_in_ems: bounds.origin, size_in_ems: bounds.size, curves: floats })
}

#[derive(Clone, Copy, Debug, Default)]
pub struct P2 {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct QuadCurve {
    pub p0: P2,
    pub p1: P2,
    pub p2: P2,
}

pub fn outline_to_normalized_quads(
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

/// Cx-free font publication used by the retained code renderer. This shares the
/// editor's outline conversion, including cubic tolerance and contour closure.
pub fn prepare_code_curves(font: &Font, outline: &GlyphOutline) -> Vec<f32> {
    outline_to_normalized_quads(outline, outline.bounds_in_ems(), font.units_per_em())
        .iter()
        .flat_map(|c| [c.p0.x, c.p0.y, c.p1.x, c.p1.y, c.p2.x, c.p2.y, 0.0, 0.0])
        .collect()
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

