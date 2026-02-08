use super::animate::{
    eval_color_animation, eval_float_animation, eval_path_animation, eval_transform_animation,
};
/// SVG renderer: walks an SvgDocument and emits DrawVector calls.
use super::document::*;
use super::units::viewbox_transform;
use crate::shader::draw_vector::DrawVector;
use crate::vector::{GradientStop, VectorPaint, VectorPath};

pub fn render_svg(
    dv: &mut DrawVector,
    doc: &SvgDocument,
    offset_x: f32,
    offset_y: f32,
    target_w: f32,
    target_h: f32,
    time: f32,
) {
    let offset = Transform2d::translate(offset_x, offset_y);
    let base_transform = if let Some(ref vb) = doc.viewbox {
        let (sx, sy, tx, ty) = viewbox_transform(vb, target_w, target_h);
        offset.then(&Transform2d {
            a: sx,
            c: 0.0,
            e: tx,
            b: 0.0,
            d: sy,
            f: ty,
        })
    } else {
        offset
    };

    render_nodes(dv, &doc.root, &doc.defs, &base_transform, time);
}

fn render_nodes(
    dv: &mut DrawVector,
    nodes: &[SvgNode],
    defs: &SvgDefs,
    parent_xf: &Transform2d,
    time: f32,
) {
    for node in nodes {
        match node {
            SvgNode::Group(group) => render_group(dv, group, defs, parent_xf, time),
            SvgNode::Path(path) => render_path(dv, path, defs, parent_xf, time),
            SvgNode::Rect(rect) => render_rect(dv, rect, defs, parent_xf, time),
            SvgNode::Circle(circ) => render_circle(dv, circ, defs, parent_xf, time),
            SvgNode::Ellipse(ell) => render_ellipse(dv, ell, defs, parent_xf, time),
            SvgNode::Line(line) => render_line(dv, line, defs, parent_xf, time),
            SvgNode::Polyline(poly) => render_polyline(dv, poly, defs, parent_xf, time),
            SvgNode::Polygon(poly) => render_polygon(dv, poly, defs, parent_xf, time),
        }
    }
}

fn render_group(
    dv: &mut DrawVector,
    group: &SvgGroup,
    defs: &SvgDefs,
    parent_xf: &Transform2d,
    time: f32,
) {
    let mut xf = parent_xf.then(&group.transform);

    // Apply animated transforms
    for at in &group.animate_transforms {
        if let Some(anim_xf) = eval_transform_animation(at, time) {
            xf = xf.then(&anim_xf);
        }
    }

    render_nodes(dv, &group.children, defs, &xf, time);
}

fn apply_animated_style(style: &SvgStyle, animations: &[SvgAnimate], time: f32) -> SvgStyle {
    let mut s = style.clone();
    for anim in animations {
        match anim.attribute {
            AnimateAttribute::Fill => {
                if let Some(color) = eval_color_animation(anim, time) {
                    s.fill = Some(SvgPaint::Color(color.0, color.1, color.2, color.3));
                }
            }
            AnimateAttribute::Stroke => {
                if let Some(color) = eval_color_animation(anim, time) {
                    s.stroke = Some(SvgPaint::Color(color.0, color.1, color.2, color.3));
                }
            }
            AnimateAttribute::StrokeWidth => {
                if let Some(w) = eval_float_animation(anim, time) {
                    s.stroke_width = w;
                }
            }
            AnimateAttribute::Opacity => {
                if let Some(o) = eval_float_animation(anim, time) {
                    s.opacity = o.clamp(0.0, 1.0);
                }
            }
            AnimateAttribute::FillOpacity => {
                if let Some(o) = eval_float_animation(anim, time) {
                    s.fill_opacity = o.clamp(0.0, 1.0);
                }
            }
            AnimateAttribute::StrokeOpacity => {
                if let Some(o) = eval_float_animation(anim, time) {
                    s.stroke_opacity = o.clamp(0.0, 1.0);
                }
            }
            _ => {}
        }
    }
    s
}

fn render_path(
    dv: &mut DrawVector,
    svg_path: &SvgPath,
    defs: &SvgDefs,
    parent_xf: &Transform2d,
    time: f32,
) {
    let mut xf = parent_xf.then(&svg_path.transform);
    for at in &svg_path.animate_transforms {
        if let Some(anim_xf) = eval_transform_animation(at, time) {
            xf = xf.then(&anim_xf);
        }
    }

    let style = apply_animated_style(&svg_path.style, &svg_path.animations, time);

    // Check for path morphing (animate d)
    let animated_path;
    let use_path = if let Some(anim) = svg_path
        .animations
        .iter()
        .find(|a| matches!(a.attribute, AnimateAttribute::D))
    {
        if let Some(p) = eval_path_animation(anim, time) {
            animated_path = p;
            &animated_path
        } else {
            &svg_path.path
        }
    } else {
        &svg_path.path
    };

    emit_shape(dv, |dv| emit_path(dv, use_path, &xf), &style, defs, &xf);
}

fn render_rect(
    dv: &mut DrawVector,
    rect: &SvgRect,
    defs: &SvgDefs,
    parent_xf: &Transform2d,
    time: f32,
) {
    let mut xf = parent_xf.then(&rect.transform);
    for at in &rect.animate_transforms {
        if let Some(anim_xf) = eval_transform_animation(at, time) {
            xf = xf.then(&anim_xf);
        }
    }
    let style = apply_animated_style(&rect.style, &rect.animations, time);

    let r = rect.rx.max(rect.ry);
    emit_shape(
        dv,
        |dv| {
            if r > 0.0 {
                emit_rounded_rect(dv, rect.x, rect.y, rect.width, rect.height, r, &xf);
            } else {
                emit_rect(dv, rect.x, rect.y, rect.width, rect.height, &xf);
            }
        },
        &style,
        defs,
        &xf,
    );
}

fn render_circle(
    dv: &mut DrawVector,
    circ: &SvgCircle,
    defs: &SvgDefs,
    parent_xf: &Transform2d,
    time: f32,
) {
    let mut xf = parent_xf.then(&circ.transform);
    for at in &circ.animate_transforms {
        if let Some(anim_xf) = eval_transform_animation(at, time) {
            xf = xf.then(&anim_xf);
        }
    }
    let style = apply_animated_style(&circ.style, &circ.animations, time);

    emit_shape(
        dv,
        |dv| emit_ellipse(dv, circ.cx, circ.cy, circ.r, circ.r, &xf),
        &style,
        defs,
        &xf,
    );
}

fn render_ellipse(
    dv: &mut DrawVector,
    ell: &SvgEllipse,
    defs: &SvgDefs,
    parent_xf: &Transform2d,
    time: f32,
) {
    let mut xf = parent_xf.then(&ell.transform);
    for at in &ell.animate_transforms {
        if let Some(anim_xf) = eval_transform_animation(at, time) {
            xf = xf.then(&anim_xf);
        }
    }
    let style = apply_animated_style(&ell.style, &ell.animations, time);

    emit_shape(
        dv,
        |dv| emit_ellipse(dv, ell.cx, ell.cy, ell.rx, ell.ry, &xf),
        &style,
        defs,
        &xf,
    );
}

fn render_line(
    dv: &mut DrawVector,
    line: &SvgLine,
    defs: &SvgDefs,
    parent_xf: &Transform2d,
    time: f32,
) {
    let mut xf = parent_xf.then(&line.transform);
    for at in &line.animate_transforms {
        if let Some(anim_xf) = eval_transform_animation(at, time) {
            xf = xf.then(&anim_xf);
        }
    }
    let style = apply_animated_style(&line.style, &line.animations, time);

    emit_shape(
        dv,
        |dv| {
            let (x1, y1) = xf.apply(line.x1, line.y1);
            let (x2, y2) = xf.apply(line.x2, line.y2);
            dv.move_to(x1, y1);
            dv.line_to(x2, y2);
        },
        &style,
        defs,
        &xf,
    );
}

fn render_polyline(
    dv: &mut DrawVector,
    poly: &SvgPolyline,
    defs: &SvgDefs,
    parent_xf: &Transform2d,
    time: f32,
) {
    let mut xf = parent_xf.then(&poly.transform);
    for at in &poly.animate_transforms {
        if let Some(anim_xf) = eval_transform_animation(at, time) {
            xf = xf.then(&anim_xf);
        }
    }
    let style = apply_animated_style(&poly.style, &poly.animations, time);

    emit_shape(
        dv,
        |dv| emit_points(dv, &poly.points, false, &xf),
        &style,
        defs,
        &xf,
    );
}

fn render_polygon(
    dv: &mut DrawVector,
    poly: &SvgPolygon,
    defs: &SvgDefs,
    parent_xf: &Transform2d,
    time: f32,
) {
    let mut xf = parent_xf.then(&poly.transform);
    for at in &poly.animate_transforms {
        if let Some(anim_xf) = eval_transform_animation(at, time) {
            xf = xf.then(&anim_xf);
        }
    }
    let style = apply_animated_style(&poly.style, &poly.animations, time);

    emit_shape(
        dv,
        |dv| emit_points(dv, &poly.points, true, &xf),
        &style,
        defs,
        &xf,
    );
}

// ---- Emit helpers: build path in DrawVector ----

fn emit_shape(
    dv: &mut DrawVector,
    build_path: impl Fn(&mut DrawVector),
    style: &SvgStyle,
    defs: &SvgDefs,
    xf: &Transform2d,
) {
    let opacity = style.opacity;

    // Fill
    if let Some(ref paint) = style.fill {
        if !matches!(paint, SvgPaint::None) {
            build_path(dv);
            let fill_alpha = style.fill_opacity * opacity;
            set_paint(dv, paint, defs, fill_alpha, xf);
            dv.fill();
            dv.path.clear();
        }
    }

    // Stroke
    if let Some(ref paint) = style.stroke {
        if !matches!(paint, SvgPaint::None) && style.stroke_width > 0.0 {
            build_path(dv);
            let stroke_alpha = style.stroke_opacity * opacity;
            set_paint(dv, paint, defs, stroke_alpha, xf);
            let w = style.stroke_width * xf.scale_factor();
            dv.stroke_opts(
                w,
                style.stroke_linecap,
                style.stroke_linejoin,
                style.stroke_miterlimit,
                1.0,
            );
            dv.path.clear();
        }
    }
}

fn set_paint(dv: &mut DrawVector, paint: &SvgPaint, defs: &SvgDefs, alpha: f32, _xf: &Transform2d) {
    match paint {
        SvgPaint::None => {}
        SvgPaint::Color(r, g, b, a) => {
            let a = a * alpha;
            dv.set_color(*r * a, *g * a, *b * a, a); // premultiplied
        }
        SvgPaint::GradientRef(id) => {
            if let Some(grad) = defs.gradients.get(id) {
                let vp = gradient_to_vector_paint(grad, _xf);
                dv.set_paint(vp);
            } else {
                dv.set_color(0.0, 0.0, 0.0, alpha); // fallback black
            }
        }
    }
}

fn gradient_to_vector_paint(grad: &SvgGradient, xf: &Transform2d) -> VectorPaint {
    let stops: Vec<GradientStop> = grad.stops.iter().map(|s| s.clone()).collect();
    if stops.is_empty() {
        return VectorPaint::solid(0.0, 0.0, 0.0, 1.0);
    }

    match grad.kind {
        GradientKind::Linear => {
            // Apply gradient transform then world transform
            let gxf = xf.then(&grad.transform);
            let (x0, y0) = gxf.apply(grad.x1, grad.y1);
            let (x1, y1) = gxf.apply(grad.x2, grad.y2);
            VectorPaint::LinearGradient {
                x0,
                y0,
                x1,
                y1,
                stops,
            }
        }
        GradientKind::Radial => {
            let gxf = xf.then(&grad.transform);
            let (cx, cy) = gxf.apply(grad.cx, grad.cy);
            let r = grad.r * gxf.scale_factor();
            VectorPaint::RadialGradient { cx, cy, r, stops }
        }
    }
}

use crate::vector::path::PathCmd;

fn emit_path(dv: &mut DrawVector, path: &VectorPath, xf: &Transform2d) {
    for cmd in &path.cmds {
        match cmd {
            PathCmd::MoveTo(x, y) => {
                let (tx, ty) = xf.apply(*x, *y);
                dv.move_to(tx, ty);
            }
            PathCmd::LineTo(x, y) => {
                let (tx, ty) = xf.apply(*x, *y);
                dv.line_to(tx, ty);
            }
            PathCmd::BezierTo(cx1, cy1, cx2, cy2, x, y) => {
                let (tcx1, tcy1) = xf.apply(*cx1, *cy1);
                let (tcx2, tcy2) = xf.apply(*cx2, *cy2);
                let (tx, ty) = xf.apply(*x, *y);
                dv.bezier_to(tcx1, tcy1, tcx2, tcy2, tx, ty);
            }
            PathCmd::Close => {
                dv.close();
            }
            PathCmd::Winding(_) => {} // handled by fill rule
        }
    }
}

fn emit_rect(dv: &mut DrawVector, x: f32, y: f32, w: f32, h: f32, xf: &Transform2d) {
    let (x0, y0) = xf.apply(x, y);
    let (x1, y1) = xf.apply(x + w, y);
    let (x2, y2) = xf.apply(x + w, y + h);
    let (x3, y3) = xf.apply(x, y + h);
    dv.move_to(x0, y0);
    dv.line_to(x1, y1);
    dv.line_to(x2, y2);
    dv.line_to(x3, y3);
    dv.close();
}

fn emit_rounded_rect(
    dv: &mut DrawVector,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    xf: &Transform2d,
) {
    // Build in local space, then transform each point
    let mut local_path = VectorPath::new();
    local_path.rounded_rect(x, y, w, h, r);
    emit_path(dv, &local_path, xf);
}

fn emit_ellipse(dv: &mut DrawVector, cx: f32, cy: f32, rx: f32, ry: f32, xf: &Transform2d) {
    let mut local_path = VectorPath::new();
    local_path.ellipse(cx, cy, rx, ry);
    emit_path(dv, &local_path, xf);
}

fn emit_points(dv: &mut DrawVector, points: &[(f32, f32)], close: bool, xf: &Transform2d) {
    if points.is_empty() {
        return;
    }
    let (x, y) = xf.apply(points[0].0, points[0].1);
    dv.move_to(x, y);
    for pt in &points[1..] {
        let (x, y) = xf.apply(pt.0, pt.1);
        dv.line_to(x, y);
    }
    if close {
        dv.close();
    }
}
