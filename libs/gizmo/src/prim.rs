//! The picture of a gizmo: screen-space primitives, painted in order.
use makepad_math::*;

/// One mark of a gizmo's picture, in screen space.
///
/// A list of these is painted first to last, so later marks cover earlier
/// ones. Colours are straight RGBA. A fill with zero alpha is no fill and a
/// zero width is no stroke.
#[derive(Clone, Debug, PartialEq)]
pub enum GizmoPrim {
    /// A straight stroke with round ends.
    Line {
        a: Vec2f,
        b: Vec2f,
        width: f32,
        color: Vec4f,
    },
    /// A disc, a ring, or both: `fill` inside `radius`, and a stroke `width`
    /// wide centred on it.
    Circle {
        center: Vec2f,
        radius: f32,
        width: f32,
        color: Vec4f,
        fill: Vec4f,
    },
    /// A stroked piece of a circle. Angles are radians on screen, measured
    /// from +x toward +y (so clockwise, y being down); `sweep` may be
    /// negative.
    Arc {
        center: Vec2f,
        radius: f32,
        start: f32,
        sweep: f32,
        width: f32,
        color: Vec4f,
    },
    /// A convex quadrilateral, filled and optionally outlined. A triangle is
    /// a quad whose last two points are the same.
    Quad {
        pts: [Vec2f; 4],
        fill: Vec4f,
        width: f32,
        color: Vec4f,
    },
    /// A line of text. `align` is the point of the text's box placed at
    /// `pos`: (0, 0) its top left, (0.5, 0.5) its centre. A `backdrop` with
    /// alpha is painted behind it.
    Text {
        pos: Vec2f,
        text: String,
        color: Vec4f,
        align: Vec2f,
        backdrop: Vec4f,
    },
}

impl GizmoPrim {
    pub fn line(a: Vec2f, b: Vec2f, width: f32, color: Vec4f) -> Self {
        GizmoPrim::Line { a, b, width, color }
    }

    pub fn disc(center: Vec2f, radius: f32, fill: Vec4f) -> Self {
        GizmoPrim::Circle {
            center,
            radius,
            width: 0.0,
            color: fill,
            fill,
        }
    }

    pub fn ring(center: Vec2f, radius: f32, width: f32, color: Vec4f) -> Self {
        GizmoPrim::Circle {
            center,
            radius,
            width,
            color,
            fill: vec4(0.0, 0.0, 0.0, 0.0),
        }
    }

    pub fn triangle(a: Vec2f, b: Vec2f, c: Vec2f, fill: Vec4f) -> Self {
        GizmoPrim::Quad {
            pts: [a, b, c, c],
            fill,
            width: 0.0,
            color: fill,
        }
    }

    pub fn text(pos: Vec2f, text: impl Into<String>, color: Vec4f, align: Vec2f) -> Self {
        GizmoPrim::Text {
            pos,
            text: text.into(),
            color,
            align,
            backdrop: vec4(0.0, 0.0, 0.0, 0.0),
        }
    }

    /// The screen box the mark covers, padded by its stroke, as
    /// (min, max). Text has no size here and reports its anchor point.
    pub fn bounds(&self) -> (Vec2f, Vec2f) {
        match self {
            GizmoPrim::Line { a, b, width, .. } => {
                let p = width * 0.5 + 1.0;
                (
                    vec2(a.x.min(b.x) - p, a.y.min(b.y) - p),
                    vec2(a.x.max(b.x) + p, a.y.max(b.y) + p),
                )
            }
            GizmoPrim::Circle {
                center,
                radius,
                width,
                ..
            }
            | GizmoPrim::Arc {
                center,
                radius,
                width,
                ..
            } => {
                let r = radius + width * 0.5 + 1.0;
                (
                    vec2(center.x - r, center.y - r),
                    vec2(center.x + r, center.y + r),
                )
            }
            GizmoPrim::Quad { pts, width, .. } => {
                let p = width * 0.5 + 1.0;
                let mut lo = pts[0];
                let mut hi = pts[0];
                for q in pts.iter() {
                    lo = vec2(lo.x.min(q.x), lo.y.min(q.y));
                    hi = vec2(hi.x.max(q.x), hi.y.max(q.y));
                }
                (vec2(lo.x - p, lo.y - p), vec2(hi.x + p, hi.y + p))
            }
            GizmoPrim::Text { pos, .. } => (*pos, *pos),
        }
    }
}
