//! World-space polyline sampling of `<path>` elements that carry a
//! `marker-end` attribute — in practice, diagram edges (mermaid arrows).
//!
//! The main `render_svg` pipeline already draws these paths as strokes. This
//! module provides a sidecar view used by overlays (e.g. animated flow dots)
//! that need to position something *along* the edge.
//!
//! The sampler is a plain uniform-t parametric walk: each `LineTo` gets 8
//! samples and each `BezierTo` gets 16. That's not true arc-length
//! parameterisation, so motion speed varies slightly on sharp curves —
//! acceptable for the animated-dot use case and avoids a square-root per
//! segment.

use crate::document::{SvgDefs, SvgDocument, SvgNode, SvgPaint, SvgStyle, Transform2d};
use crate::path::{PathCmd, VectorPath};
use crate::units::viewbox_transform;

/// One resolved edge ready to be traversed by an overlay.
#[derive(Clone, Debug)]
pub struct SvgEdge {
    /// Polyline points in world space (after viewBox + ancestor transforms).
    pub points: Vec<(f32, f32)>,
    /// Stroke color of the source path. Overlay can tint its glyphs to
    /// match (flow dots often look best in the edge's own colour).
    pub color: (f32, f32, f32, f32),
}

/// Collect every path that has a `marker-end` set — a reliable proxy for
/// "directed connector / edge" in mermaid-style SVGs. Shape fills (rects,
/// circles …) never set marker-end so this filter is precise for our use.
pub fn collect_edges(doc: &SvgDocument) -> Vec<SvgEdge> {
    let (lw, lh) = doc.logical_size();
    let base_xf = if let Some(ref vb) = doc.viewbox {
        let (sx, sy, tx, ty) = viewbox_transform(vb, lw, lh);
        Transform2d {
            a: sx,
            c: 0.0,
            e: tx,
            b: 0.0,
            d: sy,
            f: ty,
        }
    } else {
        Transform2d::identity()
    };
    let mut out = Vec::new();
    walk(&doc.root, &doc.defs, &base_xf, &mut out);
    out
}

fn walk(nodes: &[SvgNode], defs: &SvgDefs, parent_xf: &Transform2d, out: &mut Vec<SvgEdge>) {
    for node in nodes {
        match node {
            SvgNode::Group(g) => {
                let xf = g.transform.then(parent_xf);
                walk(&g.children, defs, &xf, out);
            }
            SvgNode::Use(u) => {
                if let Some(sym) = defs.symbols.get(&u.href) {
                    let mut local_xf = u.transform.clone();
                    local_xf = Transform2d::translate(u.x, u.y).then(&local_xf);
                    if let Some(ref vb) = sym.viewbox {
                        let w = u.width.unwrap_or(vb.width);
                        let h = u.height.unwrap_or(vb.height);
                        let (sx, sy, tx, ty) = viewbox_transform(vb, w, h);
                        let vb_xf = Transform2d {
                            a: sx,
                            c: 0.0,
                            e: tx,
                            b: 0.0,
                            d: sy,
                            f: ty,
                        };
                        local_xf = vb_xf.then(&local_xf);
                    }
                    let xf = local_xf.then(parent_xf);
                    walk(&sym.children, defs, &xf, out);
                }
            }
            SvgNode::Path(p) if p.style.marker_end.is_some() => {
                let xf = p.transform.then(parent_xf);
                let points = sample_path_world(&p.path, &xf);
                if points.len() >= 2 {
                    let color = resolve_stroke_color(&p.style);
                    out.push(SvgEdge { points, color });
                }
            }
            _ => {}
        }
    }
}

/// Walk every `PathCmd` and emit world-space samples. The first MoveTo fixes
/// the pen; LineTo/BezierTo append sampled points ahead of the pen. Close
/// segments emit a line back to the subpath start.
fn sample_path_world(path: &VectorPath, xf: &Transform2d) -> Vec<(f32, f32)> {
    const LINE_STEPS: usize = 8;
    const BEZIER_STEPS: usize = 16;

    let mut out = Vec::with_capacity(path.cmds.len() * LINE_STEPS);
    let mut cur = (0.0f32, 0.0f32);
    let mut sub_start = cur;

    for cmd in &path.cmds {
        match cmd {
            PathCmd::MoveTo(x, y) => {
                cur = (*x, *y);
                sub_start = cur;
                out.push(xf.apply(cur.0, cur.1));
            }
            PathCmd::LineTo(x, y) => {
                for i in 1..=LINE_STEPS {
                    let t = i as f32 / LINE_STEPS as f32;
                    let px = cur.0 + (x - cur.0) * t;
                    let py = cur.1 + (y - cur.1) * t;
                    out.push(xf.apply(px, py));
                }
                cur = (*x, *y);
            }
            PathCmd::BezierTo(cx1, cy1, cx2, cy2, x, y) => {
                // Cubic Bernstein basis: (1-t)^3 P0 + 3(1-t)^2 t P1 + 3(1-t) t^2 P2 + t^3 P3
                for i in 1..=BEZIER_STEPS {
                    let t = i as f32 / BEZIER_STEPS as f32;
                    let u = 1.0 - t;
                    let bx = u * u * u * cur.0
                        + 3.0 * u * u * t * cx1
                        + 3.0 * u * t * t * cx2
                        + t * t * t * x;
                    let by = u * u * u * cur.1
                        + 3.0 * u * u * t * cy1
                        + 3.0 * u * t * t * cy2
                        + t * t * t * y;
                    out.push(xf.apply(bx, by));
                }
                cur = (*x, *y);
            }
            PathCmd::Close => {
                for i in 1..=LINE_STEPS {
                    let t = i as f32 / LINE_STEPS as f32;
                    let px = cur.0 + (sub_start.0 - cur.0) * t;
                    let py = cur.1 + (sub_start.1 - cur.1) * t;
                    out.push(xf.apply(px, py));
                }
                cur = sub_start;
            }
            PathCmd::Winding(_) => {}
        }
    }
    out
}

fn resolve_stroke_color(style: &SvgStyle) -> (f32, f32, f32, f32) {
    let alpha = (style.opacity * style.stroke_opacity).clamp(0.0, 1.0);
    match &style.stroke {
        Some(SvgPaint::Color(r, g, b, a)) => (*r, *g, *b, *a * alpha),
        Some(SvgPaint::CurrentColor) => (
            style.color.0,
            style.color.1,
            style.color.2,
            style.color.3 * alpha,
        ),
        _ => (0.6, 0.7, 0.78, alpha), // slate-400 fallback matches Theme::dark
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_svg;

    #[test]
    fn collects_edges_only() {
        // A shape (rect) + an edge (path with marker-end). Only the edge
        // should show up, with at least two sample points.
        let svg = r##"<svg viewBox="0 0 100 100">
            <rect x="0" y="0" width="10" height="10"/>
            <path d="M0 0 L100 0" stroke="#f00" marker-end="url(#arrow)"/>
        </svg>"##;
        let edges = collect_edges(&parse_svg(svg));
        assert_eq!(edges.len(), 1, "only the path with marker-end is an edge");
        assert!(edges[0].points.len() >= 2);
        let first = edges[0].points[0];
        let last = *edges[0].points.last().unwrap();
        assert!((first.0 - 0.0).abs() < 0.01);
        assert!((last.0 - 100.0).abs() < 0.5);
    }

    #[test]
    fn path_without_marker_end_is_ignored() {
        let svg = r##"<svg viewBox="0 0 100 100">
            <path d="M0 0 L100 0" stroke="#f00"/>
        </svg>"##;
        let edges = collect_edges(&parse_svg(svg));
        assert!(edges.is_empty());
    }
}
