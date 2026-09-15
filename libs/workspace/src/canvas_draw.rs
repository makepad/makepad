//! The two canvas primitives every Studio surface shares: a camera-scaled
//! checkerboard ground and one card quad that carries its own gauss shadow,
//! border and selection outline. They used to live in the removed flow graph
//! crate; Studio owns them now so no surface depends on a graph editor.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

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
        outline_color: #0000
        outline_size: 0.0
        shadow_color: theme.color_shadow
        shadow_radius: 12.0
        shadow_offset: vec2(0.0, 0.0)

        rect_size2: varying(vec2(0.0))
        rect_size3: varying(vec2(0.0))
        rect_pos2: varying(vec2(0.0))
        rect_shift: varying(vec2(0.0))
        sdf_rect_pos: varying(vec2(0.0))
        sdf_rect_size: varying(vec2(0.0))

        vertex: fn() {
            let min_offset = min(self.shadow_offset, vec2(0.0, 0.0))
            self.rect_size2 = self.rect_size + 2.0 * vec2(self.shadow_radius)
            self.rect_size3 = self.rect_size2 + abs(self.shadow_offset)
            self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset
            self.sdf_rect_size = self.rect_size2
                - vec2(self.shadow_radius * 2.0 + self.border_size * 2.0)
            self.sdf_rect_pos = -min_offset + vec2(self.border_size + self.shadow_radius)
            self.rect_shift = -min_offset
            return self.clip_and_transform_vertex(self.rect_pos2, self.rect_size3)
        }

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size3)
            sdf.box(
                self.sdf_rect_pos.x,
                self.sdf_rect_pos.y,
                self.sdf_rect_size.x,
                self.sdf_rect_size.y,
                self.border_radius
            )
            if sdf.shape > -1.0 {
                let m = self.shadow_radius
                let o = self.shadow_offset + self.rect_shift
                let v = GaussShadow.rounded_box_shadow(
                    vec2(m) + o,
                    self.rect_size2 + o,
                    self.pos * (self.rect_size3 + vec2(m)),
                    m * 0.5,
                    self.border_radius * 2.0
                )
                sdf.clear(self.shadow_color * v)
            }
            sdf.fill_keep(self.color)
            if self.border_size > 0.0 {
                sdf.stroke_keep(self.border_color, self.border_size)
            }
            if self.outline_size > 0.0 {
                sdf.stroke(self.outline_color, self.outline_size)
            }
            return sdf.result
        }
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

/// One card including its shadow, so the shadow follows the card's exact
/// transformed rectangle rather than a separately batched estimate.
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
    pub outline_color: Vec4f,
    #[live]
    pub outline_size: f32,
    #[live]
    pub shadow_color: Vec4f,
    #[live]
    pub shadow_radius: f32,
    #[live]
    pub shadow_offset: Vec2f,
}
