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

        arrow: fn() -> vec4 {
            var path = vec2(1e20, 0.0)
            path = self.path_edge(path, vec2(3.0, 2.0), vec2(3.0, 19.0))
            path = self.path_edge(path, vec2(3.0, 19.0), vec2(7.5, 15.0))
            path = self.path_edge(path, vec2(7.5, 15.0), vec2(11.0, 22.0))
            path = self.path_edge(path, vec2(11.0, 22.0), vec2(14.0, 20.5))
            path = self.path_edge(path, vec2(14.0, 20.5), vec2(10.5, 13.5))
            path = self.path_edge(path, vec2(10.5, 13.5), vec2(17.0, 13.5))
            path = self.path_edge(path, vec2(17.0, 13.5), vec2(3.0, 2.0))
            return self.path_pixel(path)
        }

        hand: fn(kind: float) -> vec4 {
            var path = vec2(1e20, 0.0)
            if kind < 0.5 {
                // Pointing finger: hotspot at its tip.
                path = self.path_edge(path, vec2(8.0, 12.0), vec2(8.0, 4.0))
                path = self.path_edge(path, vec2(8.0, 4.0), vec2(9.0, 2.0))
                path = self.path_edge(path, vec2(9.0, 2.0), vec2(11.0, 2.0))
                path = self.path_edge(path, vec2(11.0, 2.0), vec2(12.0, 4.0))
                path = self.path_edge(path, vec2(12.0, 4.0), vec2(12.0, 10.0))
                path = self.path_edge(path, vec2(12.0, 10.0), vec2(15.0, 9.0))
                path = self.path_edge(path, vec2(15.0, 9.0), vec2(20.0, 12.0))
                path = self.path_edge(path, vec2(20.0, 12.0), vec2(20.0, 16.0))
                path = self.path_edge(path, vec2(20.0, 16.0), vec2(17.0, 22.0))
                path = self.path_edge(path, vec2(17.0, 22.0), vec2(9.0, 22.0))
                path = self.path_edge(path, vec2(9.0, 22.0), vec2(3.0, 14.0))
                path = self.path_edge(path, vec2(3.0, 14.0), vec2(3.0, 12.0))
                path = self.path_edge(path, vec2(3.0, 12.0), vec2(5.0, 11.0))
                path = self.path_edge(path, vec2(5.0, 11.0), vec2(8.0, 12.0))
            } else {
                let top = 4.0 + (kind - 1.0) * 5.0
                path = self.path_edge(path, vec2(6.0, 13.0), vec2(5.0, top + 1.0))
                path = self.path_edge(path, vec2(5.0, top + 1.0), vec2(8.0, top))
                path = self.path_edge(path, vec2(8.0, top), vec2(9.0, 11.0))
                path = self.path_edge(path, vec2(9.0, 11.0), vec2(9.0, top - 1.0))
                path = self.path_edge(path, vec2(9.0, top - 1.0), vec2(12.0, top - 1.0))
                path = self.path_edge(path, vec2(12.0, top - 1.0), vec2(12.0, 11.0))
                path = self.path_edge(path, vec2(12.0, 11.0), vec2(13.0, top))
                path = self.path_edge(path, vec2(13.0, top), vec2(16.0, top))
                path = self.path_edge(path, vec2(16.0, top), vec2(16.0, 12.0))
                path = self.path_edge(path, vec2(16.0, 12.0), vec2(17.0, top + 2.0))
                path = self.path_edge(path, vec2(17.0, top + 2.0), vec2(20.0, top + 2.0))
                path = self.path_edge(path, vec2(20.0, top + 2.0), vec2(20.0, 16.0))
                path = self.path_edge(path, vec2(20.0, 16.0), vec2(17.0, 22.0))
                path = self.path_edge(path, vec2(17.0, 22.0), vec2(8.0, 22.0))
                path = self.path_edge(path, vec2(8.0, 22.0), vec2(2.0, 15.0))
                path = self.path_edge(path, vec2(2.0, 15.0), vec2(3.0, 12.0))
                path = self.path_edge(path, vec2(3.0, 12.0), vec2(6.0, 13.0))
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
            match self.shape {
                CursorShape.Hidden => { return vec4(0.0) }
                CursorShape.Default => { return self.arrow() }
                CursorShape.Arrow => { return self.arrow() }
                CursorShape.Hand => { return self.hand(0.0) }
                CursorShape.Grab => { return self.hand(1.0) }
                CursorShape.Grabbing => { return self.hand(2.0) }
                CursorShape.Text => {
                    sdf.move_to(8.0, 3.0)
                    sdf.line_to(16.0, 3.0)
                    sdf.move_to(12.0, 3.0)
                    sdf.line_to(12.0, 21.0)
                    sdf.move_to(8.0, 21.0)
                    sdf.line_to(16.0, 21.0)
                }
                CursorShape.Crosshair => {
                    sdf.move_to(12.0, 2.0)
                    sdf.line_to(12.0, 22.0)
                    sdf.move_to(2.0, 12.0)
                    sdf.line_to(22.0, 12.0)
                }
                CursorShape.Move => {
                    return self.resize(vec2(1.0, 0.0), 1.0, 0.0)
                        + self.resize(vec2(0.0, 1.0), 1.0, 0.0)
                            * (1.0 - self.resize(vec2(1.0, 0.0), 1.0, 0.0).w)
                }
                CursorShape.Wait => {
                    var path = vec2(1e20, 0.0)
                    path = self.path_edge(path, vec2(6.0, 3.0), vec2(18.0, 3.0))
                    path = self.path_edge(path, vec2(18.0, 3.0), vec2(18.0, 6.0))
                    path = self.path_edge(path, vec2(18.0, 6.0), vec2(13.0, 12.0))
                    path = self.path_edge(path, vec2(13.0, 12.0), vec2(18.0, 18.0))
                    path = self.path_edge(path, vec2(18.0, 18.0), vec2(18.0, 21.0))
                    path = self.path_edge(path, vec2(18.0, 21.0), vec2(6.0, 21.0))
                    path = self.path_edge(path, vec2(6.0, 21.0), vec2(6.0, 18.0))
                    path = self.path_edge(path, vec2(6.0, 18.0), vec2(11.0, 12.0))
                    path = self.path_edge(path, vec2(11.0, 12.0), vec2(6.0, 6.0))
                    path = self.path_edge(path, vec2(6.0, 6.0), vec2(6.0, 3.0))
                    return self.path_pixel(path)
                }
                CursorShape.NotAllowed => {
                    sdf.circle(12.0, 12.0, 8.0)
                    sdf.move_to(6.5, 6.5)
                    sdf.line_to(17.5, 17.5)
                }
                CursorShape.Help => {
                    sdf.move_to(15.0, 13.0)
                    sdf.line_to(16.0, 11.0)
                    sdf.line_to(20.0, 11.0)
                    sdf.line_to(22.0, 13.0)
                    sdf.line_to(21.0, 15.0)
                    sdf.line_to(18.0, 17.0)
                    sdf.line_to(18.0, 18.0)
                    sdf.move_to(18.0, 20.5)
                    sdf.line_to(18.0, 21.0)
                    sdf.stroke_keep(self.border_color, 3.5)
                    sdf.stroke(self.color, 1.75)
                    return sdf.result + self.arrow() * (1.0 - sdf.result.w)
                }
                CursorShape.NResize => { return self.resize(vec2(0.0, -1.0), 0.0, 0.0) }
                CursorShape.NeResize => { return self.resize(vec2(0.707107, -0.707107), 0.0, 0.0) }
                CursorShape.EResize => { return self.resize(vec2(1.0, 0.0), 0.0, 0.0) }
                CursorShape.SeResize => { return self.resize(vec2(0.707107, 0.707107), 0.0, 0.0) }
                CursorShape.SResize => { return self.resize(vec2(0.0, 1.0), 0.0, 0.0) }
                CursorShape.SwResize => { return self.resize(vec2(-0.707107, 0.707107), 0.0, 0.0) }
                CursorShape.WResize => { return self.resize(vec2(-1.0, 0.0), 0.0, 0.0) }
                CursorShape.NwResize => { return self.resize(vec2(-0.707107, -0.707107), 0.0, 0.0) }
                CursorShape.NsResize => { return self.resize(vec2(0.0, 1.0), 1.0, 0.0) }
                CursorShape.NeswResize => { return self.resize(vec2(0.707107, -0.707107), 1.0, 0.0) }
                CursorShape.EwResize => { return self.resize(vec2(1.0, 0.0), 1.0, 0.0) }
                CursorShape.NwseResize => { return self.resize(vec2(0.707107, 0.707107), 1.0, 0.0) }
                CursorShape.ColResize => { return self.resize(vec2(1.0, 0.0), 1.0, 1.0) }
                CursorShape.RowResize => { return self.resize(vec2(0.0, 1.0), 1.0, 1.0) }
                _ => { return self.arrow() }
            }
            sdf.stroke_keep(self.border_color, 3.5)
            sdf.stroke(self.color, 1.75)
            return sdf.result
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
