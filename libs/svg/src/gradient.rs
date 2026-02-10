/// SVG gradient parser for <linearGradient> and <radialGradient> elements.
use makepad_html::HtmlWalker;
use makepad_live_id::*;

use crate::color::parse_color;
use crate::document::{GradientUnits, SpreadMethod, SvgGradient};
use crate::paint::GradientStop;
use crate::transform::parse_transform;
use crate::units::parse_number;

pub fn parse_linear_gradient(walker: &HtmlWalker) -> (Option<String>, SvgGradient) {
    let mut grad = SvgGradient::new_linear();
    let id = walker.find_attr_lc(live_id!(id)).map(|s| s.to_string());

    if let Some(v) = walker.find_attr_lc(live_id!(x1)) {
        if let Some(n) = parse_number(v) {
            grad.x1 = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(y1)) {
        if let Some(n) = parse_number(v) {
            grad.y1 = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(x2)) {
        if let Some(n) = parse_number(v) {
            grad.x2 = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(y2)) {
        if let Some(n) = parse_number(v) {
            grad.y2 = n;
        }
    }

    if let Some(v) = walker.find_attr_lc(live_id!(gradientunits)) {
        grad.units = parse_gradient_units(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(gradienttransform)) {
        grad.transform = parse_transform(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(spreadmethod)) {
        grad.spread = parse_spread_method(v);
    }

    // xlink:href or href for inheritance
    grad.href = walker
        .find_attr_lc(live_id!(href))
        .or_else(|| walker.find_attr_lc(live_id!(xlink:href)))
        .and_then(|s| s.strip_prefix('#'))
        .map(|s| s.to_string());

    (id, grad)
}

pub fn parse_radial_gradient(walker: &HtmlWalker) -> (Option<String>, SvgGradient) {
    let mut grad = SvgGradient::new_radial();
    let id = walker.find_attr_lc(live_id!(id)).map(|s| s.to_string());

    if let Some(v) = walker.find_attr_lc(live_id!(cx)) {
        if let Some(n) = parse_number(v) {
            grad.cx = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(cy)) {
        if let Some(n) = parse_number(v) {
            grad.cy = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(r)) {
        if let Some(n) = parse_number(v) {
            grad.r = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(fx)) {
        if let Some(n) = parse_number(v) {
            grad.fx = n;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(fy)) {
        if let Some(n) = parse_number(v) {
            grad.fy = n;
        }
    }

    if let Some(v) = walker.find_attr_lc(live_id!(gradientunits)) {
        grad.units = parse_gradient_units(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(gradienttransform)) {
        grad.transform = parse_transform(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(spreadmethod)) {
        grad.spread = parse_spread_method(v);
    }

    grad.href = walker
        .find_attr_lc(live_id!(href))
        .or_else(|| walker.find_attr_lc(live_id!(xlink:href)))
        .and_then(|s| s.strip_prefix('#'))
        .map(|s| s.to_string());

    (id, grad)
}

pub fn parse_stop(walker: &HtmlWalker) -> GradientStop {
    let offset = walker
        .find_attr_lc(live_id!(offset))
        .and_then(|s| {
            let s = s.trim();
            if let Some(pct) = s.strip_suffix('%') {
                pct.trim().parse::<f32>().ok().map(|v| v / 100.0)
            } else {
                s.parse::<f32>().ok()
            }
        })
        .unwrap_or(0.0);

    let mut r = 0.0f32;
    let mut g = 0.0f32;
    let mut b = 0.0f32;
    let mut a = 1.0f32;

    if let Some(color_str) = walker.find_attr_lc(live_id!(stop - color)) {
        if let Some((cr, cg, cb, ca)) = parse_color(color_str) {
            r = cr;
            g = cg;
            b = cb;
            a = ca;
        }
    }
    if let Some(opacity_str) = walker.find_attr_lc(live_id!(stop - opacity)) {
        if let Some(op) = opacity_str.trim().parse::<f32>().ok() {
            a *= op.clamp(0.0, 1.0);
        }
    }

    // Also check inline style attribute
    if let Some(style_str) = walker.find_attr_lc(live_id!(style)) {
        for decl in style_str.split(';') {
            if let Some((key, value)) = decl.split_once(':') {
                let key = key.trim().to_ascii_lowercase();
                let value = value.trim();
                match key.as_str() {
                    "stop-color" => {
                        if let Some((cr, cg, cb, ca)) = parse_color(value) {
                            r = cr;
                            g = cg;
                            b = cb;
                            a = ca;
                        }
                    }
                    "stop-opacity" => {
                        if let Some(op) = value.parse::<f32>().ok() {
                            a *= op.clamp(0.0, 1.0);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Premultiply alpha
    GradientStop {
        offset,
        color: [r * a, g * a, b * a, a],
    }
}

fn parse_gradient_units(s: &str) -> GradientUnits {
    match s.trim() {
        "userSpaceOnUse" => GradientUnits::UserSpaceOnUse,
        _ => GradientUnits::ObjectBoundingBox,
    }
}

fn parse_spread_method(s: &str) -> SpreadMethod {
    match s.trim() {
        "reflect" => SpreadMethod::Reflect,
        "repeat" => SpreadMethod::Repeat,
        _ => SpreadMethod::Pad,
    }
}
