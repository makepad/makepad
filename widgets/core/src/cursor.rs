use makepad_draw::*;

// Vector cursors stay sharp at the display scale and composite on the GPU.
script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.DrawMouseCursor = mod.draw.DrawQuad {
        // A shape comes from the CPU (`set_shape`), so a D3D compile of this has no table to
        // inline. x: its silhouette's points, y: its strokes' pen moves (those points come first),
        // z: its resize arrows, w: 1 with the stroked circle.
        counts: instance(vec4(0.0))
        // The resize arrows: xy their axis, z: 1 with the second head, w: 1 with the divider.
        arrow: instance(vec4(0.0))
        // The points, one per float (`packed_point`).
        points0: instance(vec4(0.0))
        points1: instance(vec4(0.0))
        points2: instance(vec4(0.0))
        points3: instance(vec4(0.0))
        points4: instance(vec4(0.0))
        color: uniform(theme.color_black)
        border_color: uniform(theme.color_white)

        // Closed cursor silhouettes are concave. Sdf2d's line-path clipping
        // intersects edge half-planes, which cannot fill those silhouettes.
        // Carry nearest-edge distance and signed winding for the complete path.
        path_edge: fn(path: vec2, a: vec2, b: vec2) -> vec2 {
            let p = self.pos * 24.0
            let edge = b - a
            let rel = p - a
            let t = clamp(dot(rel, edge) / max(dot(edge, edge), 0.00001), 0.0, 1.0)
            let distance = min(path.x, length(rel - edge * t))
            let side = edge.x * rel.y - edge.y * rel.x
            var winding = path.y
            if a.y <= p.y && b.y > p.y && side > 0.0 {
                winding += 1.0
            }
            if a.y > p.y && b.y <= p.y && side < 0.0 {
                winding -= 1.0
            }
            return vec2(distance, winding)
        }

        path_pixel: fn(path: vec2) -> vec4 {
            let sdf = Sdf2d.viewport(self.pos * 24.0)
            sdf.shape = path.x
            if abs(path.y) > 0.5 {
                sdf.shape = -path.x
            }
            sdf.fill_keep(self.color)
            sdf.stroke(self.border_color, 1.25)
            return sdf.result
        }

        // Point i of the shape: xy where it is, z: 1 when the pen draws to it.
        point: fn(i: float) -> vec3 {
            var four = self.points4
            if i < 15.5 { four = self.points3 }
            if i < 11.5 { four = self.points2 }
            if i < 7.5 { four = self.points1 }
            if i < 3.5 { four = self.points0 }
            let c = i - 4.0 * floor(i / 4.0)
            let pick = vec4(step(c, 0.5), step(0.5, c) * step(c, 1.5), step(1.5, c) * step(c, 2.5), step(2.5, c))
            // (the rounding: an interpolated instance value can be a hair off)
            let packed = floor(dot(four, pick) + 0.5)
            let pen = step(4095.5, packed)
            let xy = packed - 4096.0 * pen
            let x = floor(xy / 64.0)
            return vec3(x * 0.5, (xy - x * 64.0) * 0.5, pen)
        }

        // The filled silhouette: `n` points from point `first`, closing back to the first of them.
        path_fill: fn(first: float, n: float) -> vec4 {
            var path = vec2(1e20, 0.0)
            var a = vec2(0.0, 0.0)
            var i = 0.0
            while i < n + 0.5 {
                let b = self.point(first + i - n * step(n - 0.5, i)).xy
                if i > 0.5 {
                    path = self.path_edge(path, a, b)
                }
                a = b
                i = i + 1.0
            }
            return self.path_pixel(path)
        }

        resize: fn(axis: vec2, double_head: float, divider: float) -> vec4 {
            let p = self.pos * 24.0 - vec2(12.0)
            let q = vec2(dot(p, axis), dot(p, vec2(-axis.y, axis.x)))
            let sdf = Sdf2d.viewport(q)
            sdf.move_to(-8.0, 0.0)
            sdf.line_to(8.0, 0.0)
            sdf.move_to(4.0, -4.0)
            sdf.line_to(8.0, 0.0)
            sdf.line_to(4.0, 4.0)
            if double_head > 0.5 {
                sdf.move_to(-4.0, -4.0)
                sdf.line_to(-8.0, 0.0)
                sdf.line_to(-4.0, 4.0)
            }
            if divider > 0.5 {
                sdf.move_to(-1.5, -8.0)
                sdf.line_to(-1.5, 8.0)
                sdf.move_to(1.5, -8.0)
                sdf.line_to(1.5, 8.0)
            }
            sdf.stroke_keep(self.border_color, 3.5)
            sdf.stroke(self.color, 1.75)
            return sdf.result
        }

        pixel: fn() -> vec4 {
            let sdf = Sdf2d.viewport(self.pos * 24.0)
            let fill = self.counts.x
            let strokes = self.counts.y
            if self.counts.z > 0.5 {
                var out = vec4(0.0, 0.0, 0.0, 0.0)
                var r = 0.0
                while r < self.counts.z - 0.5 {
                    // The second (Move's vertical) under the first.
                    let c = self.resize(mix(self.arrow.xy, vec2(0.0, 1.0), r), self.arrow.z, self.arrow.w)
                    out = out + c * (1.0 - out.w)
                    r = r + 1.0
                }
                return out
            }
            var over = vec4(0.0, 0.0, 0.0, 0.0)
            if strokes > 0.5 {
                if self.counts.w > 0.5 {
                    sdf.circle(12.0, 12.0, 8.0)
                }
                var i = 0.0
                while i < strokes - 0.5 {
                    let q = self.point(i)
                    if q.z < 0.5 {
                        sdf.move_to(q.x, q.y)
                    } else {
                        sdf.line_to(q.x, q.y)
                    }
                    i = i + 1.0
                }
                sdf.stroke_keep(self.border_color, 3.5)
                sdf.stroke(self.color, 1.75)
                over = sdf.result
            }
            // No return inside the branch above: an NVIDIA D3D11 driver draws garbage for correct
            // bytecode when one sits next to that loop (the software rasterizer draws it right).
            // Help is the shape that took it, the only one with strokes and a silhouette.
            if fill < 0.5 {
                return over
            }
            return over + self.path_fill(strokes, fill) * (1.0 - over.w)
        }
    }
}

pub(crate) fn hotspot(cursor: MouseCursor, size: DVec2) -> DVec2 {
    let point = match cursor {
        MouseCursor::Default | MouseCursor::Arrow | MouseCursor::Help => dvec2(3.0, 2.0),
        MouseCursor::Hand => dvec2(10.0, 2.0),
        _ => dvec2(12.0, 12.0),
    };
    point * size / 24.0
}

/// A point of a shape's strokes, and whether the pen draws to it (else it moves there).
const fn pen(x: f32, y: f32, draws: bool) -> (f32, f32, bool) {
    (x, y, draws)
}

// The filled silhouettes (each closes back to its first point).
const ARROW: [(f32, f32); 7] = [(3.0, 2.0), (3.0, 19.0), (7.5, 15.0), (11.0, 22.0), (14.0, 20.5), (10.5, 13.5), (17.0, 13.5)];
const POINTING_HAND: [(f32, f32); 14] = [
    (8.0, 12.0), (8.0, 4.0), (9.0, 2.0), (11.0, 2.0), (12.0, 4.0), (12.0, 10.0), (15.0, 9.0),
    (20.0, 12.0), (20.0, 16.0), (17.0, 22.0), (9.0, 22.0), (3.0, 14.0), (3.0, 12.0), (5.0, 11.0),
];
const WAIT: [(f32, f32); 10] = [
    (6.0, 3.0), (18.0, 3.0), (18.0, 6.0), (13.0, 12.0), (18.0, 18.0), (18.0, 21.0), (6.0, 21.0), (6.0, 18.0), (11.0, 12.0), (6.0, 6.0),
];
/// The open hand; its fingers come down to `top` as it closes.
const fn hand(top: f32) -> [(f32, f32); 17] {
    [
        (6.0, 13.0), (5.0, top + 1.0), (8.0, top), (9.0, 11.0), (9.0, top - 1.0), (12.0, top - 1.0), (12.0, 11.0), (13.0, top), (16.0, top),
        (16.0, 12.0), (17.0, top + 2.0), (20.0, top + 2.0), (20.0, 16.0), (17.0, 22.0), (8.0, 22.0), (2.0, 15.0), (3.0, 12.0),
    ]
}
const OPEN_HAND: [(f32, f32); 17] = hand(4.0);
const CLOSED_HAND: [(f32, f32); 17] = hand(9.0);

// The stroked shapes as pen moves.
const TEXT: [(f32, f32, bool); 6] = [pen(8.0, 3.0, false), pen(16.0, 3.0, true), pen(12.0, 3.0, false), pen(12.0, 21.0, true), pen(8.0, 21.0, false), pen(16.0, 21.0, true)];
const CROSSHAIR: [(f32, f32, bool); 4] = [pen(12.0, 2.0, false), pen(12.0, 22.0, true), pen(2.0, 12.0, false), pen(22.0, 12.0, true)];
const NOT_ALLOWED: [(f32, f32, bool); 2] = [pen(6.5, 6.5, false), pen(17.5, 17.5, true)];
const QUESTION_MARK: [(f32, f32, bool); 9] = [
    pen(15.0, 13.0, false), pen(16.0, 11.0, true), pen(20.0, 11.0, true), pen(22.0, 13.0, true), pen(21.0, 15.0, true),
    pen(18.0, 17.0, true), pen(18.0, 18.0, true), pen(18.0, 20.5, false), pen(18.0, 21.0, true),
];

/// What a cursor shape is made of.
#[derive(Default)]
struct Shape {
    fill: &'static [(f32, f32)],
    strokes: &'static [(f32, f32, bool)],
    circle: bool,
    /// The resize arrows: how many, their axis, the second head, the divider.
    resizes: f32,
    axis: (f32, f32),
    heads: bool,
    divider: bool,
}

fn shape_of(cursor: MouseCursor) -> Shape {
    const D: f32 = 0.707107;
    let resize = |axis, heads, divider| Shape { resizes: 1.0, axis, heads, divider, ..Default::default() };
    match cursor {
        MouseCursor::Hidden => Shape::default(),
        MouseCursor::Default | MouseCursor::Arrow => Shape { fill: &ARROW, ..Default::default() },
        MouseCursor::Hand => Shape { fill: &POINTING_HAND, ..Default::default() },
        MouseCursor::Grab => Shape { fill: &OPEN_HAND, ..Default::default() },
        MouseCursor::Grabbing => Shape { fill: &CLOSED_HAND, ..Default::default() },
        MouseCursor::Wait => Shape { fill: &WAIT, ..Default::default() },
        MouseCursor::Text => Shape { strokes: &TEXT, ..Default::default() },
        MouseCursor::Crosshair => Shape { strokes: &CROSSHAIR, ..Default::default() },
        MouseCursor::NotAllowed => Shape { strokes: &NOT_ALLOWED, circle: true, ..Default::default() },
        // The question mark over the arrow.
        MouseCursor::Help => Shape { fill: &ARROW, strokes: &QUESTION_MARK, ..Default::default() },
        // Both double arrows, the horizontal one over the vertical.
        MouseCursor::Move => Shape { resizes: 2.0, axis: (1.0, 0.0), heads: true, ..Default::default() },
        MouseCursor::NResize => resize((0.0, -1.0), false, false),
        MouseCursor::NeResize => resize((D, -D), false, false),
        MouseCursor::EResize => resize((1.0, 0.0), false, false),
        MouseCursor::SeResize => resize((D, D), false, false),
        MouseCursor::SResize => resize((0.0, 1.0), false, false),
        MouseCursor::SwResize => resize((-D, D), false, false),
        MouseCursor::WResize => resize((-1.0, 0.0), false, false),
        MouseCursor::NwResize => resize((-D, -D), false, false),
        MouseCursor::NsResize => resize((0.0, 1.0), true, false),
        MouseCursor::NeswResize => resize((D, -D), true, false),
        MouseCursor::EwResize => resize((1.0, 0.0), true, false),
        MouseCursor::NwseResize => resize((D, D), true, false),
        MouseCursor::ColResize => resize((1.0, 0.0), true, true),
        MouseCursor::RowResize => resize((0.0, 1.0), true, true),
    }
}

/// A point as one float: its half-pixels on the 24-pixel grid, and whether the pen draws to it.
fn packed_point(x: f32, y: f32, draws: bool) -> f32 {
    (x * 2.0) * 64.0 + y * 2.0 + if draws { 4096.0 } else { 0.0 }
}

/// Make `draw` (a `DrawMouseCursor`) draw this cursor's shape.
pub(crate) fn set_shape(draw: &mut DrawQuad, cx: &mut Cx, cursor: MouseCursor) {
    let shape = shape_of(cursor);
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    let mut points = [0.0f32; 20];
    let strokes = shape.strokes.iter().map(|&(x, y, draws)| packed_point(x, y, draws));
    let fill = shape.fill.iter().map(|&(x, y)| packed_point(x, y, false));
    for (slot, point) in points.iter_mut().zip(strokes.chain(fill)) {
        *slot = point;
    }
    let vars = &mut draw.draw_vars;
    vars.set_dyn_instance(cx, id!(counts), &[shape.fill.len() as f32, shape.strokes.len() as f32, shape.resizes, flag(shape.circle)]);
    vars.set_dyn_instance(cx, id!(arrow), &[shape.axis.0, shape.axis.1, flag(shape.heads), flag(shape.divider)]);
    vars.set_dyn_instance(cx, id!(points0), &points[0..4]);
    vars.set_dyn_instance(cx, id!(points1), &points[4..8]);
    vars.set_dyn_instance(cx, id!(points2), &points[8..12]);
    vars.set_dyn_instance(cx, id!(points3), &points[12..16]);
    vars.set_dyn_instance(cx, id!(points4), &points[16..20]);
}
