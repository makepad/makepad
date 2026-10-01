use makepad_draw::*;

// Vector cursors stay sharp at the display scale and composite on the GPU.
script_mod! {
    use mod.prelude.widgets_internal.*
    let CursorShape = #(CursorShape::script_api(vm))

    mod.widgets.DrawMouseCursor = mod.draw.DrawQuad {
        shape: instance(CursorShape.Default)
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

        // The filled silhouettes as point lists (each closes back to its
        // first point), walked by one loop with one path_edge call site: a
        // D3D compile inlines every call site (fifty here took a second).
        // kind: 0 arrow, 1 pointing hand, 2 open hand, 3 closed hand, 4 wait.
        path_count: fn(kind: float) -> float {
            if kind < 0.5 { return 7.0 }
            if kind < 1.5 { return 14.0 }
            if kind < 3.5 { return 17.0 }
            return 10.0
        }
        path_point: fn(kind: float, i: float) -> vec2 {
            if kind < 0.5 {
                if i < 0.5 { return vec2(3.0, 2.0) }
                if i < 1.5 { return vec2(3.0, 19.0) }
                if i < 2.5 { return vec2(7.5, 15.0) }
                if i < 3.5 { return vec2(11.0, 22.0) }
                if i < 4.5 { return vec2(14.0, 20.5) }
                if i < 5.5 { return vec2(10.5, 13.5) }
                if i < 6.5 { return vec2(17.0, 13.5) }
            }
            if kind < 1.5 {
                if i < 0.5 { return vec2(8.0, 12.0) }
                if i < 1.5 { return vec2(8.0, 4.0) }
                if i < 2.5 { return vec2(9.0, 2.0) }
                if i < 3.5 { return vec2(11.0, 2.0) }
                if i < 4.5 { return vec2(12.0, 4.0) }
                if i < 5.5 { return vec2(12.0, 10.0) }
                if i < 6.5 { return vec2(15.0, 9.0) }
                if i < 7.5 { return vec2(20.0, 12.0) }
                if i < 8.5 { return vec2(20.0, 16.0) }
                if i < 9.5 { return vec2(17.0, 22.0) }
                if i < 10.5 { return vec2(9.0, 22.0) }
                if i < 11.5 { return vec2(3.0, 14.0) }
                if i < 12.5 { return vec2(3.0, 12.0) }
                if i < 13.5 { return vec2(5.0, 11.0) }
            }
            if kind < 3.5 {
                // The open hand's fingers rise as it closes.
                let top = 4.0 + ((kind - 1.0) - 1.0) * 5.0
                if i < 0.5 { return vec2(6.0, 13.0) }
                if i < 1.5 { return vec2(5.0, top + 1.0) }
                if i < 2.5 { return vec2(8.0, top) }
                if i < 3.5 { return vec2(9.0, 11.0) }
                if i < 4.5 { return vec2(9.0, top - 1.0) }
                if i < 5.5 { return vec2(12.0, top - 1.0) }
                if i < 6.5 { return vec2(12.0, 11.0) }
                if i < 7.5 { return vec2(13.0, top) }
                if i < 8.5 { return vec2(16.0, top) }
                if i < 9.5 { return vec2(16.0, 12.0) }
                if i < 10.5 { return vec2(17.0, top + 2.0) }
                if i < 11.5 { return vec2(20.0, top + 2.0) }
                if i < 12.5 { return vec2(20.0, 16.0) }
                if i < 13.5 { return vec2(17.0, 22.0) }
                if i < 14.5 { return vec2(8.0, 22.0) }
                if i < 15.5 { return vec2(2.0, 15.0) }
                if i < 16.5 { return vec2(3.0, 12.0) }
            }
                if i < 0.5 { return vec2(6.0, 3.0) }
                if i < 1.5 { return vec2(18.0, 3.0) }
                if i < 2.5 { return vec2(18.0, 6.0) }
                if i < 3.5 { return vec2(13.0, 12.0) }
                if i < 4.5 { return vec2(18.0, 18.0) }
                if i < 5.5 { return vec2(18.0, 21.0) }
                if i < 6.5 { return vec2(6.0, 21.0) }
                if i < 7.5 { return vec2(6.0, 18.0) }
                if i < 8.5 { return vec2(11.0, 12.0) }
                if i < 9.5 { return vec2(6.0, 6.0) }
            return vec2(0.0, 0.0)
        }
        path_fill: fn(kind: float) -> vec4 {
            let n = self.path_count(kind)
            var path = vec2(1e20, 0.0)
            var a = vec2(0.0, 0.0)
            var i = 0.0
            while i < n + 0.5 {
                // Point i, and the first again to close the path.
                let b = self.path_point(kind, i - n * step(n - 0.5, i))
                if i > 0.5 {
                    path = self.path_edge(path, a, b)
                }
                a = b
                i = i + 1.0
            }
            return self.path_pixel(path)
        }

        // The stroked shapes as pen moves (z 0: move to, 1: line to).
        // kind: 0 text, 1 crosshair, 2 not allowed, 3 help.
        stroke_count: fn(kind: float) -> float {
            if kind < 0.5 { return 6.0 }
            if kind < 1.5 { return 4.0 }
            if kind < 2.5 { return 2.0 }
            return 9.0
        }
        stroke_point: fn(kind: float, i: float) -> vec3 {
            if kind < 0.5 {
                if i < 0.5 { return vec3(8.0, 3.0, 0.0) }
                if i < 1.5 { return vec3(16.0, 3.0, 1.0) }
                if i < 2.5 { return vec3(12.0, 3.0, 0.0) }
                if i < 3.5 { return vec3(12.0, 21.0, 1.0) }
                if i < 4.5 { return vec3(8.0, 21.0, 0.0) }
                if i < 5.5 { return vec3(16.0, 21.0, 1.0) }
            }
            if kind < 1.5 {
                if i < 0.5 { return vec3(12.0, 2.0, 0.0) }
                if i < 1.5 { return vec3(12.0, 22.0, 1.0) }
                if i < 2.5 { return vec3(2.0, 12.0, 0.0) }
                if i < 3.5 { return vec3(22.0, 12.0, 1.0) }
            }
            if kind < 2.5 {
                if i < 0.5 { return vec3(6.5, 6.5, 0.0) }
                if i < 1.5 { return vec3(17.5, 17.5, 1.0) }
            }
                if i < 0.5 { return vec3(15.0, 13.0, 0.0) }
                if i < 1.5 { return vec3(16.0, 11.0, 1.0) }
                if i < 2.5 { return vec3(20.0, 11.0, 1.0) }
                if i < 3.5 { return vec3(22.0, 13.0, 1.0) }
                if i < 4.5 { return vec3(21.0, 15.0, 1.0) }
                if i < 5.5 { return vec3(18.0, 17.0, 1.0) }
                if i < 6.5 { return vec3(18.0, 18.0, 1.0) }
                if i < 7.5 { return vec3(18.0, 20.5, 0.0) }
                if i < 8.5 { return vec3(18.0, 21.0, 1.0) }
            return vec3(0.0, 0.0, 0.0)
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
            // Each shape is a filled path, a set of strokes, or resize
            // arrows; each kind is drawn from one call site below.
            var fill = -1.0
            var stroke = -1.0
            var resizes = 0.0
            var axis = vec2(0.0, 0.0)
            var heads = 0.0
            var divider = 0.0
            match self.shape {
                CursorShape.Hidden => { return vec4(0.0) }
                CursorShape.Default => { fill = 0.0 }
                CursorShape.Arrow => { fill = 0.0 }
                CursorShape.Hand => { fill = 1.0 }
                CursorShape.Grab => { fill = 2.0 }
                CursorShape.Grabbing => { fill = 3.0 }
                CursorShape.Wait => { fill = 4.0 }
                CursorShape.Text => { stroke = 0.0 }
                CursorShape.Crosshair => { stroke = 1.0 }
                CursorShape.NotAllowed => { stroke = 2.0 }
                // The question mark over the arrow.
                CursorShape.Help => {
                    stroke = 3.0
                    fill = 0.0
                }
                // Both double arrows, the horizontal one over the vertical.
                CursorShape.Move => {
                    resizes = 2.0
                    axis = vec2(1.0, 0.0)
                    heads = 1.0
                }
                CursorShape.NResize => {
                    resizes = 1.0
                    axis = vec2(0.0, -1.0)
                }
                CursorShape.NeResize => {
                    resizes = 1.0
                    axis = vec2(0.707107, -0.707107)
                }
                CursorShape.EResize => {
                    resizes = 1.0
                    axis = vec2(1.0, 0.0)
                }
                CursorShape.SeResize => {
                    resizes = 1.0
                    axis = vec2(0.707107, 0.707107)
                }
                CursorShape.SResize => {
                    resizes = 1.0
                    axis = vec2(0.0, 1.0)
                }
                CursorShape.SwResize => {
                    resizes = 1.0
                    axis = vec2(-0.707107, 0.707107)
                }
                CursorShape.WResize => {
                    resizes = 1.0
                    axis = vec2(-1.0, 0.0)
                }
                CursorShape.NwResize => {
                    resizes = 1.0
                    axis = vec2(-0.707107, -0.707107)
                }
                CursorShape.NsResize => {
                    resizes = 1.0
                    axis = vec2(0.0, 1.0)
                    heads = 1.0
                }
                CursorShape.NeswResize => {
                    resizes = 1.0
                    axis = vec2(0.707107, -0.707107)
                    heads = 1.0
                }
                CursorShape.EwResize => {
                    resizes = 1.0
                    axis = vec2(1.0, 0.0)
                    heads = 1.0
                }
                CursorShape.NwseResize => {
                    resizes = 1.0
                    axis = vec2(0.707107, 0.707107)
                    heads = 1.0
                }
                CursorShape.ColResize => {
                    resizes = 1.0
                    axis = vec2(1.0, 0.0)
                    heads = 1.0
                    divider = 1.0
                }
                CursorShape.RowResize => {
                    resizes = 1.0
                    axis = vec2(0.0, 1.0)
                    heads = 1.0
                    divider = 1.0
                }
                _ => { fill = 0.0 }
            }
            if resizes > 0.5 {
                var out = vec4(0.0, 0.0, 0.0, 0.0)
                var r = 0.0
                while r < resizes - 0.5 {
                    // The second (Move's vertical) under the first.
                    let c = self.resize(mix(axis, vec2(0.0, 1.0), r), heads, divider)
                    out = out + c * (1.0 - out.w)
                    r = r + 1.0
                }
                return out
            }
            var over = vec4(0.0, 0.0, 0.0, 0.0)
            if stroke > -0.5 {
                if stroke > 1.5 && stroke < 2.5 {
                    sdf.circle(12.0, 12.0, 8.0)
                }
                let n = self.stroke_count(stroke)
                var i = 0.0
                while i < n - 0.5 {
                    let q = self.stroke_point(stroke, i)
                    if q.z < 0.5 {
                        sdf.move_to(q.x, q.y)
                    } else {
                        sdf.line_to(q.x, q.y)
                    }
                    i = i + 1.0
                }
                sdf.stroke_keep(self.border_color, 3.5)
                sdf.stroke(self.color, 1.75)
                if fill < -0.5 {
                    return sdf.result
                }
                over = sdf.result
            }
            return over + self.path_fill(fill) * (1.0 - over.w)
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

#[derive(Clone, Copy, Script, ScriptHook)]
#[repr(u32)]
enum CursorShape {
    Hidden = 0,
    #[pick]
    Default = 1,
    Crosshair = 2,
    Hand = 3,
    Arrow = 4,
    Move = 5,
    Text = 6,
    Wait = 7,
    Help = 8,
    NotAllowed = 9,
    Grab = 10,
    Grabbing = 11,
    NResize = 12,
    NeResize = 13,
    EResize = 14,
    SeResize = 15,
    SResize = 16,
    SwResize = 17,
    WResize = 18,
    NwResize = 19,
    NsResize = 20,
    NeswResize = 21,
    EwResize = 22,
    NwseResize = 23,
    ColResize = 24,
    RowResize = 25,
}

pub(crate) fn shape_value(cursor: MouseCursor) -> f32 {
    let shape = match cursor {
        MouseCursor::Hidden => CursorShape::Hidden,
        MouseCursor::Default => CursorShape::Default,
        MouseCursor::Crosshair => CursorShape::Crosshair,
        MouseCursor::Hand => CursorShape::Hand,
        MouseCursor::Arrow => CursorShape::Arrow,
        MouseCursor::Move => CursorShape::Move,
        MouseCursor::Text => CursorShape::Text,
        MouseCursor::Wait => CursorShape::Wait,
        MouseCursor::Help => CursorShape::Help,
        MouseCursor::NotAllowed => CursorShape::NotAllowed,
        MouseCursor::Grab => CursorShape::Grab,
        MouseCursor::Grabbing => CursorShape::Grabbing,
        MouseCursor::NResize => CursorShape::NResize,
        MouseCursor::NeResize => CursorShape::NeResize,
        MouseCursor::EResize => CursorShape::EResize,
        MouseCursor::SeResize => CursorShape::SeResize,
        MouseCursor::SResize => CursorShape::SResize,
        MouseCursor::SwResize => CursorShape::SwResize,
        MouseCursor::WResize => CursorShape::WResize,
        MouseCursor::NwResize => CursorShape::NwResize,
        MouseCursor::NsResize => CursorShape::NsResize,
        MouseCursor::NeswResize => CursorShape::NeswResize,
        MouseCursor::EwResize => CursorShape::EwResize,
        MouseCursor::NwseResize => CursorShape::NwseResize,
        MouseCursor::ColResize => CursorShape::ColResize,
        MouseCursor::RowResize => CursorShape::RowResize,
    };
    // Shader enum attributes are integer lanes in the f32 instance storage.
    f32::from_bits(shape as u32)
}
