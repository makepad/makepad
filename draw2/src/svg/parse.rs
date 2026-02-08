/// Main SVG parser: takes an SVG string, parses it with the HTML parser,
/// and walks the HtmlDoc to build an SvgDocument.
use makepad_html::{parse_html, HtmlWalker};
use makepad_live_id::*;

use super::animate::{parse_animate_element, parse_animate_transform_element};
use super::document::*;
use super::gradient::{parse_linear_gradient, parse_radial_gradient, parse_stop};
use super::path_data::parse_path_data;
use super::style::parse_style_from_element;
use super::transform::parse_transform;
use super::units::{parse_length, parse_number, parse_points, parse_viewbox};
use crate::vector::VectorPath;

pub fn parse_svg(svg_str: &str) -> SvgDocument {
    let mut errors = None;
    let doc = parse_html(svg_str, &mut errors, InternLiveId::No);
    let mut walker = doc.new_walker();
    let mut svg_doc = SvgDocument::default();

    // Find the <svg> root element
    while !walker.done() {
        if let Some(tag) = walker.open_tag_lc() {
            if tag == live_id!(svg) {
                parse_svg_root(&mut walker, &mut svg_doc);
                break;
            }
        }
        walker.walk();
    }

    svg_doc.resolve_gradient_hrefs();
    svg_doc
}

fn parse_svg_root(walker: &mut HtmlWalker, doc: &mut SvgDocument) {
    // Extract viewBox, width, height
    if let Some(vb) = walker.find_attr_lc(live_id!(viewbox)) {
        doc.viewbox = parse_viewbox(vb);
    }
    if let Some(w) = walker.find_attr_lc(live_id!(width)) {
        doc.width = parse_length(w);
    }
    if let Some(h) = walker.find_attr_lc(live_id!(height)) {
        doc.height = parse_length(h);
    }

    let default_style = SvgStyle::default();
    walker.walk();

    while !walker.done() {
        if walker.close_tag_lc() == Some(live_id!(svg)) {
            break;
        }
        if let Some(tag) = walker.open_tag_lc() {
            match tag {
                t if t == live_id!(defs) => {
                    parse_defs(walker, &mut doc.defs);
                }
                _ => {
                    if let Some(node) = parse_node(walker, &default_style) {
                        doc.root.push(node);
                        continue; // parse_node already advanced walker
                    }
                }
            }
        }
        walker.walk();
    }
}

fn parse_defs(walker: &mut HtmlWalker, defs: &mut SvgDefs) {
    walker.walk();
    while !walker.done() {
        if walker.close_tag_lc() == Some(live_id!(defs)) {
            break;
        }
        if let Some(tag) = walker.open_tag_lc() {
            match tag {
                t if t == live_id!(lineargradient) => {
                    let (id, mut grad) = parse_linear_gradient(walker);
                    parse_gradient_stops(walker, &mut grad, live_id!(lineargradient));
                    if let Some(id) = id {
                        defs.gradients.insert(id, grad);
                    }
                    continue;
                }
                t if t == live_id!(radialgradient) => {
                    let (id, mut grad) = parse_radial_gradient(walker);
                    parse_gradient_stops(walker, &mut grad, live_id!(radialgradient));
                    if let Some(id) = id {
                        defs.gradients.insert(id, grad);
                    }
                    continue;
                }
                _ => {
                    walker.jump_to_close();
                }
            }
        }
        walker.walk();
    }
}

fn parse_gradient_stops(walker: &mut HtmlWalker, grad: &mut SvgGradient, close_tag: LiveId) {
    walker.walk();
    while !walker.done() {
        if walker.close_tag_lc() == Some(close_tag) {
            break;
        }
        if let Some(tag) = walker.open_tag_lc() {
            if tag == live_id!(stop) {
                grad.stops.push(parse_stop(walker));
                // Skip to close of <stop> (usually self-closing)
                walker.walk(); // move past the stop element
                continue;
            }
        }
        walker.walk();
    }
}

fn parse_node(walker: &mut HtmlWalker, parent_style: &SvgStyle) -> Option<SvgNode> {
    let tag = walker.open_tag_lc()?;
    match tag {
        t if t == live_id!(g) => Some(parse_group(walker, parent_style)),
        t if t == live_id!(path) => Some(parse_path(walker, parent_style)),
        t if t == live_id!(rect) => Some(parse_rect(walker, parent_style)),
        t if t == live_id!(circle) => Some(parse_circle(walker, parent_style)),
        t if t == live_id!(ellipse) => Some(parse_ellipse(walker, parent_style)),
        t if t == live_id!(line) => Some(parse_line(walker, parent_style)),
        t if t == live_id!(polyline) => Some(parse_polyline(walker, parent_style)),
        t if t == live_id!(polygon) => Some(parse_polygon(walker, parent_style)),
        _ => {
            walker.jump_to_close();
            walker.walk();
            None
        }
    }
}

fn parse_common_attrs(walker: &HtmlWalker) -> (Option<String>, Transform2d) {
    let id = walker.find_attr_lc(live_id!(id)).map(|s| s.to_string());
    let transform = walker
        .find_attr_lc(live_id!(transform))
        .map(|s| parse_transform(s))
        .unwrap_or_default();
    (id, transform)
}

fn parse_child_animations(
    walker: &mut HtmlWalker,
    close_tag: LiveId,
    animations: &mut Vec<SvgAnimate>,
    animate_transforms: &mut Vec<SvgAnimateTransform>,
) {
    walker.walk();
    while !walker.done() {
        if walker.close_tag_lc() == Some(close_tag) {
            break;
        }
        if let Some(tag) = walker.open_tag_lc() {
            if tag == live_id!(animate) {
                animations.push(parse_animate_element(walker));
                walker.walk();
                continue;
            }
            if tag == live_id!(animatetransform) {
                animate_transforms.push(parse_animate_transform_element(walker));
                walker.walk();
                continue;
            }
            // Skip unknown child elements
            walker.jump_to_close();
        }
        walker.walk();
    }
}

fn parse_group(walker: &mut HtmlWalker, parent_style: &SvgStyle) -> SvgNode {
    let style = parse_style_from_element(walker, parent_style);
    let (id, transform) = parse_common_attrs(walker);
    let mut group = SvgGroup {
        id,
        style: style.clone(),
        transform,
        children: Vec::new(),
        animations: Vec::new(),
        animate_transforms: Vec::new(),
    };

    walker.walk();
    while !walker.done() {
        if walker.close_tag_lc() == Some(live_id!(g)) {
            walker.walk();
            return SvgNode::Group(group);
        }
        if let Some(tag) = walker.open_tag_lc() {
            if tag == live_id!(animate) {
                group.animations.push(parse_animate_element(walker));
                walker.walk();
                continue;
            }
            if tag == live_id!(animatetransform) {
                group
                    .animate_transforms
                    .push(parse_animate_transform_element(walker));
                walker.walk();
                continue;
            }
            if let Some(node) = parse_node(walker, &group.style) {
                group.children.push(node);
                continue;
            }
        }
        walker.walk();
    }
    SvgNode::Group(group)
}

fn parse_path(walker: &mut HtmlWalker, parent_style: &SvgStyle) -> SvgNode {
    let style = parse_style_from_element(walker, parent_style);
    let (id, transform) = parse_common_attrs(walker);
    let mut path = VectorPath::new();
    if let Some(d) = walker.find_attr_lc(live_id!(d)) {
        parse_path_data(d, &mut path);
    }

    let mut svg_path = SvgPath {
        id,
        style,
        transform,
        path,
        animations: Vec::new(),
        animate_transforms: Vec::new(),
    };

    // Check for child animations
    parse_child_animations(
        walker,
        live_id!(path),
        &mut svg_path.animations,
        &mut svg_path.animate_transforms,
    );
    walker.walk();
    SvgNode::Path(svg_path)
}

fn parse_rect(walker: &mut HtmlWalker, parent_style: &SvgStyle) -> SvgNode {
    let style = parse_style_from_element(walker, parent_style);
    let (id, transform) = parse_common_attrs(walker);
    let x = walker
        .find_attr_lc(live_id!(x))
        .and_then(parse_length)
        .unwrap_or(0.0);
    let y = walker
        .find_attr_lc(live_id!(y))
        .and_then(parse_length)
        .unwrap_or(0.0);
    let width = walker
        .find_attr_lc(live_id!(width))
        .and_then(parse_length)
        .unwrap_or(0.0);
    let height = walker
        .find_attr_lc(live_id!(height))
        .and_then(parse_length)
        .unwrap_or(0.0);
    let rx = walker
        .find_attr_lc(live_id!(rx))
        .and_then(parse_length)
        .unwrap_or(0.0);
    let ry = walker
        .find_attr_lc(live_id!(ry))
        .and_then(parse_length)
        .unwrap_or(rx);

    let mut rect = SvgRect {
        id,
        style,
        transform,
        x,
        y,
        width,
        height,
        rx,
        ry,
        animations: Vec::new(),
        animate_transforms: Vec::new(),
    };

    parse_child_animations(
        walker,
        live_id!(rect),
        &mut rect.animations,
        &mut rect.animate_transforms,
    );
    walker.walk();
    SvgNode::Rect(rect)
}

fn parse_circle(walker: &mut HtmlWalker, parent_style: &SvgStyle) -> SvgNode {
    let style = parse_style_from_element(walker, parent_style);
    let (id, transform) = parse_common_attrs(walker);
    let cx = walker
        .find_attr_lc(live_id!(cx))
        .and_then(parse_number)
        .unwrap_or(0.0);
    let cy = walker
        .find_attr_lc(live_id!(cy))
        .and_then(parse_number)
        .unwrap_or(0.0);
    let r = walker
        .find_attr_lc(live_id!(r))
        .and_then(parse_number)
        .unwrap_or(0.0);

    let mut circ = SvgCircle {
        id,
        style,
        transform,
        cx,
        cy,
        r,
        animations: Vec::new(),
        animate_transforms: Vec::new(),
    };

    parse_child_animations(
        walker,
        live_id!(circle),
        &mut circ.animations,
        &mut circ.animate_transforms,
    );
    walker.walk();
    SvgNode::Circle(circ)
}

fn parse_ellipse(walker: &mut HtmlWalker, parent_style: &SvgStyle) -> SvgNode {
    let style = parse_style_from_element(walker, parent_style);
    let (id, transform) = parse_common_attrs(walker);
    let cx = walker
        .find_attr_lc(live_id!(cx))
        .and_then(parse_number)
        .unwrap_or(0.0);
    let cy = walker
        .find_attr_lc(live_id!(cy))
        .and_then(parse_number)
        .unwrap_or(0.0);
    let rx = walker
        .find_attr_lc(live_id!(rx))
        .and_then(parse_number)
        .unwrap_or(0.0);
    let ry = walker
        .find_attr_lc(live_id!(ry))
        .and_then(parse_number)
        .unwrap_or(0.0);

    let mut ell = SvgEllipse {
        id,
        style,
        transform,
        cx,
        cy,
        rx,
        ry,
        animations: Vec::new(),
        animate_transforms: Vec::new(),
    };

    parse_child_animations(
        walker,
        live_id!(ellipse),
        &mut ell.animations,
        &mut ell.animate_transforms,
    );
    walker.walk();
    SvgNode::Ellipse(ell)
}

fn parse_line(walker: &mut HtmlWalker, parent_style: &SvgStyle) -> SvgNode {
    let style = parse_style_from_element(walker, parent_style);
    let (id, transform) = parse_common_attrs(walker);
    let x1 = walker
        .find_attr_lc(live_id!(x1))
        .and_then(parse_number)
        .unwrap_or(0.0);
    let y1 = walker
        .find_attr_lc(live_id!(y1))
        .and_then(parse_number)
        .unwrap_or(0.0);
    let x2 = walker
        .find_attr_lc(live_id!(x2))
        .and_then(parse_number)
        .unwrap_or(0.0);
    let y2 = walker
        .find_attr_lc(live_id!(y2))
        .and_then(parse_number)
        .unwrap_or(0.0);

    let mut line = SvgLine {
        id,
        style,
        transform,
        x1,
        y1,
        x2,
        y2,
        animations: Vec::new(),
        animate_transforms: Vec::new(),
    };

    parse_child_animations(
        walker,
        live_id!(line),
        &mut line.animations,
        &mut line.animate_transforms,
    );
    walker.walk();
    SvgNode::Line(line)
}

fn parse_polyline(walker: &mut HtmlWalker, parent_style: &SvgStyle) -> SvgNode {
    let style = parse_style_from_element(walker, parent_style);
    let (id, transform) = parse_common_attrs(walker);
    let points = walker
        .find_attr_lc(live_id!(points))
        .map(|s| parse_points(s))
        .unwrap_or_default();

    let mut poly = SvgPolyline {
        id,
        style,
        transform,
        points,
        animations: Vec::new(),
        animate_transforms: Vec::new(),
    };

    parse_child_animations(
        walker,
        live_id!(polyline),
        &mut poly.animations,
        &mut poly.animate_transforms,
    );
    walker.walk();
    SvgNode::Polyline(poly)
}

fn parse_polygon(walker: &mut HtmlWalker, parent_style: &SvgStyle) -> SvgNode {
    let style = parse_style_from_element(walker, parent_style);
    let (id, transform) = parse_common_attrs(walker);
    let points = walker
        .find_attr_lc(live_id!(points))
        .map(|s| parse_points(s))
        .unwrap_or_default();

    let mut poly = SvgPolygon {
        id,
        style,
        transform,
        points,
        animations: Vec::new(),
        animate_transforms: Vec::new(),
    };

    parse_child_animations(
        walker,
        live_id!(polygon),
        &mut poly.animations,
        &mut poly.animate_transforms,
    );
    walker.walk();
    SvgNode::Polygon(poly)
}
