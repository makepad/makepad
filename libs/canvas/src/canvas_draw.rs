//! A camera-scaled checkerboard, a card in one of a few outline shapes, and the soft shadow such
//! a card casts. The card and its shadow share one signed-distance function, so a shadow follows
//! its card's exact outline at any zoom.

use makepad_widgets_core::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The signed distance, in local units, from `p` to a shape filling a box of `size` whose
    // top-left corner is the origin. Negative inside. `r` rounds the corners of the box, the
    // triangle and the diamond without growing them past the box.
    let CanvasShape = {
        dist: fn(p: vec2, size: vec2, shape: float, r: float) -> float {
            let half = size * 0.5
            let c = p - half
            if shape < 0.5 {
                let rr = clamp(r, 0.0, min(half.x, half.y))
                let q = abs(c) - half + vec2(rr)
                return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - rr
            }
            if shape < 1.5 {
                // An ellipse; this approximation keeps the edge's anti-aliasing even on flat ones.
                let h = max(half, vec2(0.0001))
                let k0 = length(c / h)
                if k0 < 0.5 {
                    return (k0 - 1.0) * min(h.x, h.y)
                }
                return k0 * (k0 - 1.0) / length(c / (h * h))
            }
            if shape < 2.5 {
                // The circle inscribed in the box: it stays round however the box stretches.
                return length(c) - min(half.x, half.y)
            }
            if shape < 3.5 {
                // An isosceles triangle, apex up. It is shrunk about its incentre by the corner
                // radius and grown back, so the rounded triangle fills the box exactly.
                let side = length(vec2(half.x, size.y))
                let rho = max(size.x * size.y / (size.x + 2.0 * side), 0.0001)
                let rr = clamp(r, 0.0, rho * 0.9)
                let k = (rho - rr) / rho
                let incentre = vec2(half.x, size.y - rho)
                let apex = incentre + (vec2(half.x, 0.0) - incentre) * k
                let q = vec2(half.x, size.y) * k
                let t = vec2(abs(p.x - apex.x), p.y - apex.y)
                let a = t - q * clamp(dot(t, q) / dot(q, q), 0.0, 1.0)
                let b = t - q * vec2(clamp(t.x / q.x, 0.0, 1.0), 1.0)
                let d = min(vec2(dot(a, a), t.y * q.x - t.x * q.y), vec2(dot(b, b), q.y - t.y))
                return sqrt(d.x) * (0.0 - sign(d.y)) - rr
            }
            // A diamond, inset by the corner radius along its edge normals and grown back.
            let rr = clamp(r, 0.0, min(half.x, half.y) * 0.5)
            let reach = length(half)
            let b = max(half - rr * reach * vec2(1.0 / max(half.y, 0.0001), 1.0 / max(half.x, 0.0001)), vec2(0.0001))
            let q = abs(c)
            let h = clamp(((b.x - 2.0 * q.x) * b.x - (b.y - 2.0 * q.y) * b.y) / dot(b, b), -1.0, 1.0)
            let d = length(q - 0.5 * b * vec2(1.0 - h, 1.0 + h))
            return d * sign(q.x * b.y + q.y * b.x - b.x * b.y) - rr
        }
        // Device pixels per local unit at `p`, under any camera scale.
        px: fn(p: vec2) -> float {
            return 1.0 / max(length(dFdx(p)), 0.000001)
        }
    }

    set_type_default() do #(DrawCanvasGrid::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let p = self.pos * self.rect_size + self.rect_pos - self.origin
            let c = floor(p / self.cell)
            let parity = fract((c.x + c.y) * 0.5) * 2.0
            return mix(self.color_a, self.color_b, parity)
        }
    }

    set_type_default() do #(DrawCanvasCard::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: theme.color_bg_container
        border_color: theme.color_bevel
        border_size: 1.0
        border_radius: 16.0
        shadow_color: theme.color_shadow
        shadow_radius: 12.0
        shadow_offset: vec2(0.0, 0.0)
        shape: 0.0
        shade: 0.0
        rim: 0.0

        rect_size2: varying(vec2(0.0))
        rect_size3: varying(vec2(0.0))
        rect_pos2: varying(vec2(0.0))
        rect_shift: varying(vec2(0.0))

        vertex: fn() {
            let min_offset = min(self.shadow_offset, vec2(0.0, 0.0))
            self.rect_size2 = self.rect_size + 2.0 * vec2(self.shadow_radius)
            self.rect_size3 = self.rect_size2 + abs(self.shadow_offset)
            self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset
            self.rect_shift = -min_offset
            return self.clip_and_transform_vertex(self.rect_pos2, self.rect_size3)
        }

        pixel: fn() {
            let lp = self.pos * self.rect_size3 - vec2(self.shadow_radius) - self.rect_shift
            let size = self.rect_size
            let d = CanvasShape.dist(lp, size, self.shape, self.border_radius)
            let px = CanvasShape.px(lp)
            var out = vec4(0.0)
            if self.shadow_radius > 0.0 && d > -1.0 {
                let m = self.shadow_radius
                let o = self.shadow_offset + self.rect_shift
                let v = GaussShadow.rounded_box_shadow(
                    vec2(m) + o,
                    self.rect_size2 + o,
                    self.pos * (self.rect_size3 + vec2(m)),
                    m * 0.5,
                    self.border_radius * 2.0
                )
                let s = self.shadow_color * v
                out = vec4(s.rgb * s.a, s.a)
            }
            // The tone runs from a little lighter at the top to a little darker at the bottom.
            let v = clamp(lp.y / max(size.y, 0.0001), 0.0, 1.0)
            var rgb = clamp(self.color.rgb * (1.0 + self.shade * (0.9 - 2.0 * v)), vec3(0.0), vec3(1.0))
            if self.rim > 0.0 {
                // A lit rim along the top edge and a faint dark one along the bottom, inside the
                // outline, fading out once it is thinner than a pixel.
                let width = max(1.25, 1.0 / px)
                let band = clamp(1.0 + d / width, 0.0, 1.0)
                let fade = smoothstep(0.35, 0.9, 1.25 * px)
                rgb = mix(rgb, vec3(1.0), self.rim * band * fade * (1.0 - smoothstep(0.0, 0.5, v)))
                rgb = mix(rgb, vec3(0.0), self.rim * 0.4 * band * fade * smoothstep(0.6, 1.0, v))
            }
            let cover = clamp(0.5 - d * px, 0.0, 1.0)
            let fa = self.color.a * cover
            out = vec4(rgb * fa, fa) + out * (1.0 - fa)
            if self.border_size > 0.0 {
                // The border lies inside the outline.
                let inner = clamp(0.5 - (d + self.border_size) * px, 0.0, 1.0)
                let ba = self.border_color.a * max(cover - inner, 0.0)
                out = vec4(self.border_color.rgb * ba, ba) + out * (1.0 - ba)
            }
            return out
        }
    }

    set_type_default() do #(DrawCanvasShadow::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: #0000
        shape: 0.0
        radius: 0.0
        blur: 8.0
        offset: vec2(0.0, 4.0)
        contact: 0.0
        contact_blur: 1.5
        hole: 1.0

        margin: varying(float)

        vertex: fn() {
            self.margin = self.blur + max(abs(self.offset.x), abs(self.offset.y)) + self.contact_blur + 1.0
            return self.clip_and_transform_vertex(
                self.rect_pos - vec2(self.margin),
                self.rect_size + vec2(2.0 * self.margin)
            )
        }

        pixel: fn() {
            let lp = self.pos * (self.rect_size + vec2(2.0 * self.margin)) - vec2(self.margin)
            let size = self.rect_size
            let px = CanvasShape.px(lp)
            // A wide, offset ambient shadow and a tight contact shadow under the edge.
            let da = CanvasShape.dist(lp - self.offset, size, self.shape, self.radius)
            let ambient = 1.0 - smoothstep(-0.5 * self.blur, self.blur, da)
            var alpha = self.color.a * ambient * ambient
            if self.contact > 0.0 {
                let dc = CanvasShape.dist(lp - self.offset * 0.2, size, self.shape, self.radius)
                alpha = alpha + self.contact * (1.0 - smoothstep(-0.5 * self.contact_blur, self.contact_blur, dc))
            }
            // Cut away under the shape itself, from `hole` pixels inside its outline, so the
            // shadow never shows through a translucent card or lies over an item it lifts.
            let ds = CanvasShape.dist(lp, size, self.shape, self.radius)
            alpha = clamp(alpha, 0.0, 1.0) * clamp(0.5 + ds * px + self.hole, 0.0, 1.0)
            return vec4(self.color.rgb * alpha, alpha)
        }
    }
}

/// The outline a [`DrawCanvasCard`] fills and a [`DrawCanvasShadow`] follows, inside its rect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CanvasShape {
    /// A box with rounded corners.
    #[default]
    Box,
    Ellipse,
    /// The circle inscribed in the rect, centred: it stays round however the rect stretches.
    Circle,
    /// An isosceles triangle with its apex at the top centre.
    Triangle,
    Diamond,
}

impl CanvasShape {
    /// The value of a draw's `shape` field.
    pub fn code(self) -> f32 {
        self as u32 as f32
    }
}

/// The checkerboard ground: two greys a few percent apart, cell size and
/// origin in local units so the pattern scales with the camera.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCanvasGrid {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub cell: f32,
    #[live]
    pub origin: Vec2f,
    #[live]
    pub color_a: Vec4f,
    #[live]
    pub color_b: Vec4f,
}

/// One card: a [`CanvasShape`] filling its rect, with a vertical tone (`shade`), a lit top rim
/// (`rim`) and a border inside its outline. `border_radius` rounds the box, triangle and
/// diamond. Strokes and rims thinner than a pixel fade rather than shimmer.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCanvasCard {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub border_color: Vec4f,
    #[live]
    pub border_size: f32,
    #[live]
    pub border_radius: f32,
    #[live]
    pub shadow_color: Vec4f,
    #[live]
    pub shadow_radius: f32,
    #[live]
    pub shadow_offset: Vec2f,
    #[live]
    pub shape: f32,
    #[live]
    pub shade: f32,
    #[live]
    pub rim: f32,
}

/// The soft shadow a card of the same rect, `shape` and corner `radius` casts: an ambient
/// shadow of `color` blurred over `blur` units and moved by `offset`, plus an optional tight
/// `contact` shadow (its own alpha) under the edge. It is cut away from `hole` pixels inside the
/// outline. A separate shader, so a layer's shadows batch into one draw call under its cards.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCanvasShadow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub shape: f32,
    #[live]
    pub radius: f32,
    #[live]
    pub blur: f32,
    #[live]
    pub offset: Vec2f,
    #[live]
    pub contact: f32,
    #[live]
    pub contact_blur: f32,
    #[live]
    pub hole: f32,
}
