//! Sidecar text extraction for [`SvgDocument`].
//!
//! The main [`crate::document::SvgDocument`] render path (in `makepad-draw`)
//! tessellates geometry into `DrawVector`. It cannot shape glyphs. To render
//! `<text>` elements, a caller walks the document a second time with
//! [`collect_text_cmds`], then issues one `DrawText::draw_abs` per returned
//! [`SvgTextCmd`].
//!
//! Returned coordinates are already composed with the document's viewBox and
//! all ancestor `transform=""` attributes, and the animation stack is ignored
//! (snapshot at `t=0` semantics). Font size is scaled by the world transform's
//! uniform scale factor so Makepad's text renderer receives a world-space
//! value.

use crate::document::{
    SvgDefs, SvgDocument, SvgNode, SvgPaint, SvgStyle, SvgTextAnchor, Transform2d,
};
use crate::units::viewbox_transform;

/// One resolved text run ready for the caller's text rasteriser.
///
/// - `x`, `y`: world-space anchor. `y` is the text baseline (SVG convention).
/// - `font_size`: world-space size; already multiplied by the transform's
///   uniform scale factor.
/// - `color`: resolved RGBA (premultiplication is the caller's job).
#[derive(Clone, Debug)]
pub struct SvgTextCmd {
    pub x: f32,
    pub y: f32,
    pub font_size: f32,
    pub color: (f32, f32, f32, f32),
    pub text: String,
    pub text_anchor: SvgTextAnchor,
    pub font_family: Option<String>,
}

/// Walk `doc` and return every `<text>` node as a flat list.
///
/// Pass the same target width / height you hand to the shape renderer so the
/// viewBox scale matches. Setting both to the document's logical size (i.e.
/// "no extra scaling") gives you the raw SVG coordinate space.
pub fn collect_text_cmds(doc: &SvgDocument) -> Vec<SvgTextCmd> {
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

fn walk(nodes: &[SvgNode], defs: &SvgDefs, parent_xf: &Transform2d, out: &mut Vec<SvgTextCmd>) {
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
            SvgNode::Text(t) => {
                let xf = t.transform.then(parent_xf);
                let (wx, wy) = xf.apply(t.x, t.y);
                let scale = xf.scale_factor();
                let color = resolve_fill_color(&t.style, defs);
                let font_size = t.font_size * scale;
                // Keep multi-line content as a single cmd with embedded '\n'.
                // Line stacking is the caller's concern because line height
                // depends on the *rendered* font size (in screen-pt, after
                // any SVG-px-to-Makepad-pt conversion), not the SVG coord
                // font-size; doing it here with SVG units produced gaps
                // that didn't match the actual rendered font height.
                out.push(SvgTextCmd {
                    x: wx,
                    y: wy,
                    font_size,
                    color,
                    text: t.content.clone(),
                    text_anchor: t.text_anchor,
                    font_family: t.font_family.clone(),
                });
            }
            // Shape nodes: no text to collect, and they cannot wrap <text>.
            _ => {}
        }
    }
}

fn resolve_fill_color(style: &SvgStyle, defs: &SvgDefs) -> (f32, f32, f32, f32) {
    // SVG default fill for <text> is black (UA stylesheet), but SvgStyle::default
    // uses white for dark UI. Honor the explicit style if set; otherwise fall
    // back to the currentColor channel (which is CSS-default black).
    let alpha = (style.opacity * style.fill_opacity).clamp(0.0, 1.0);
    let paint = match &style.fill {
        Some(p) => p,
        None => return (style.color.0, style.color.1, style.color.2, style.color.3 * alpha),
    };
    match paint {
        SvgPaint::None => (0.0, 0.0, 0.0, 0.0),
        SvgPaint::Color(r, g, b, a) => (*r, *g, *b, *a * alpha),
        SvgPaint::CurrentColor => (
            style.color.0,
            style.color.1,
            style.color.2,
            style.color.3 * alpha,
        ),
        SvgPaint::GradientRef(id) => {
            // Best effort: use the first gradient stop. Text gradient fills
            // aren't a mermaid output pattern, so this is fine.
            if let Some(g) = defs.gradients.get(id) {
                if let Some(stop) = g.stops.first() {
                    let c = stop.color;
                    return (c[0], c[1], c[2], c[3] * alpha);
                }
            }
            (0.0, 0.0, 0.0, alpha)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_svg;

    #[test]
    fn collects_single_text() {
        let svg = r#"<svg viewBox="0 0 100 100"><text x="10" y="20" font-size="14" fill="red">Hi</text></svg>"#;
        let cmds = collect_text_cmds(&parse_svg(svg));
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].text, "Hi");
        // viewBox 0..100 mapped to logical_size 100x100 => scale 1.0
        assert!((cmds[0].x - 10.0).abs() < 0.01);
        assert!((cmds[0].y - 20.0).abs() < 0.01);
        assert!((cmds[0].font_size - 14.0).abs() < 0.01);
    }

    #[test]
    fn applies_group_transform() {
        let svg = r#"<svg viewBox="0 0 100 100"><g transform="translate(30,40)"><text x="0" y="0">X</text></g></svg>"#;
        let cmds = collect_text_cmds(&parse_svg(svg));
        assert_eq!(cmds.len(), 1);
        assert!((cmds[0].x - 30.0).abs() < 0.01);
        assert!((cmds[0].y - 40.0).abs() < 0.01);
    }

    #[test]
    fn scales_font_size_by_transform() {
        let svg = r#"<svg viewBox="0 0 100 100"><g transform="scale(2)"><text x="0" y="0" font-size="10">S</text></g></svg>"#;
        let cmds = collect_text_cmds(&parse_svg(svg));
        assert_eq!(cmds.len(), 1);
        assert!((cmds[0].font_size - 20.0).abs() < 0.1);
    }
}
