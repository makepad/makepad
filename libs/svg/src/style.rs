/// SVG style & presentation attribute parser.
/// Extracts fill, stroke, opacity, etc. from element attributes.
use makepad_html::HtmlWalker;
use makepad_live_id::*;

use crate::color::parse_color;
use crate::document::{SvgPaint, SvgStyle};
use crate::path::{FillRule, LineCap, LineJoin};
use crate::units::{parse_length, parse_number};

/// Parse style from element attributes. Inline `style` attribute overrides presentation attributes.
pub fn parse_style_from_element(walker: &HtmlWalker, parent_style: &SvgStyle) -> SvgStyle {
    let mut style = parent_style.clone();

    // First: presentation attributes
    apply_presentation_attrs(walker, &mut style);

    // Second: inline style attribute overrides
    if let Some(style_str) = walker.find_attr_lc(live_id!(style)) {
        apply_inline_style(style_str, &mut style);
    }

    style
}

fn apply_presentation_attrs(walker: &HtmlWalker, style: &mut SvgStyle) {
    if let Some(v) = walker.find_attr_lc(live_id!(fill)) {
        style.fill = Some(parse_svg_paint(v));
    }
    if let Some(v) = walker.find_attr_lc(live_id!(stroke)) {
        style.stroke = Some(parse_svg_paint(v));
    }
    if let Some(v) = walker.find_attr_lc(live_id!(opacity)) {
        if let Some(n) = parse_number(v) {
            style.opacity = n.clamp(0.0, 1.0);
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(fill - opacity)) {
        if let Some(n) = parse_number(v) {
            style.fill_opacity = n.clamp(0.0, 1.0);
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(stroke - opacity)) {
        if let Some(n) = parse_number(v) {
            style.stroke_opacity = n.clamp(0.0, 1.0);
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(stroke - width)) {
        if let Some(n) = parse_length(v) {
            style.stroke_width = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(stroke - linecap)) {
        style.stroke_linecap = parse_linecap(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(stroke - linejoin)) {
        style.stroke_linejoin = parse_linejoin(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(stroke - miterlimit)) {
        if let Some(n) = parse_number(v) {
            style.stroke_miterlimit = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(stroke - dasharray)) {
        style.stroke_dasharray = parse_dasharray(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(stroke - dashoffset)) {
        if let Some(n) = parse_length(v) {
            style.stroke_dashoffset = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(fill - rule)) {
        style.fill_rule = parse_fill_rule(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(data - shader - id)) {
        if let Some(n) = parse_number(v) {
            style.shader_id = n;
        }
    }
}

fn apply_inline_style(style_str: &str, style: &mut SvgStyle) {
    for decl in style_str.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        if let Some((key, value)) = decl.split_once(':') {
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "fill" => {
                    style.fill = Some(parse_svg_paint(value));
                }
                "stroke" => {
                    style.stroke = Some(parse_svg_paint(value));
                }
                "opacity" => {
                    if let Some(n) = parse_number(value) {
                        style.opacity = n.clamp(0.0, 1.0);
                    }
                }
                "fill-opacity" => {
                    if let Some(n) = parse_number(value) {
                        style.fill_opacity = n.clamp(0.0, 1.0);
                    }
                }
                "stroke-opacity" => {
                    if let Some(n) = parse_number(value) {
                        style.stroke_opacity = n.clamp(0.0, 1.0);
                    }
                }
                "stroke-width" => {
                    if let Some(n) = parse_length(value) {
                        style.stroke_width = n;
                    }
                }
                "stroke-linecap" => {
                    style.stroke_linecap = parse_linecap(value);
                }
                "stroke-linejoin" => {
                    style.stroke_linejoin = parse_linejoin(value);
                }
                "stroke-miterlimit" => {
                    if let Some(n) = parse_number(value) {
                        style.stroke_miterlimit = n;
                    }
                }
                "stroke-dasharray" => {
                    style.stroke_dasharray = parse_dasharray(value);
                }
                "stroke-dashoffset" => {
                    if let Some(n) = parse_length(value) {
                        style.stroke_dashoffset = n;
                    }
                }
                "fill-rule" => {
                    style.fill_rule = parse_fill_rule(value);
                }
                _ => {}
            }
        }
    }
}

pub fn parse_svg_paint(s: &str) -> SvgPaint {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") {
        return SvgPaint::None;
    }
    // url(#id) reference for gradients
    if let Some(rest) = s.strip_prefix("url(") {
        if let Some(inner) = rest.strip_suffix(')') {
            let inner = inner.trim().trim_matches(|c| c == '\'' || c == '"');
            if let Some(id) = inner.strip_prefix('#') {
                return SvgPaint::GradientRef(id.to_string());
            }
        }
    }
    if let Some((r, g, b, a)) = parse_color(s) {
        SvgPaint::Color(r, g, b, a)
    } else {
        SvgPaint::None
    }
}

fn parse_linecap(s: &str) -> LineCap {
    match s.trim().to_ascii_lowercase().as_str() {
        "round" => LineCap::Round,
        "square" => LineCap::Square,
        _ => LineCap::Butt,
    }
}

fn parse_linejoin(s: &str) -> LineJoin {
    match s.trim().to_ascii_lowercase().as_str() {
        "round" => LineJoin::Round,
        "bevel" => LineJoin::Bevel,
        _ => LineJoin::Miter,
    }
}

fn parse_fill_rule(s: &str) -> FillRule {
    match s.trim().to_ascii_lowercase().as_str() {
        "evenodd" => FillRule::EvenOdd,
        _ => FillRule::NonZero,
    }
}

fn parse_dasharray(s: &str) -> Option<Vec<f32>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") || s.is_empty() {
        return None;
    }
    let dashes: Vec<f32> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<f32>().ok())
        .collect();
    if dashes.is_empty() {
        None
    } else {
        Some(dashes)
    }
}
