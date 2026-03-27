use crate::mp_surface::{MpSurface, MpSurfaceColorFormat};
use makepad_widgets::makepad_draw::DrawProjectiveQuad;
use makepad_widgets::*;

const BASE_CARD_W: f64 = 320.0;
const BASE_CARD_H: f64 = 240.0;

// Simple widget-sized plane that stays in normal 2D layout and applies
// CSS-style 3D transforms only to its compositor-owned surface texture.

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.Css3dPlaneBase = #(Css3dPlane::register_widget(vm))
    mod.widgets.Css3dPlane = set_type_default() do mod.widgets.Css3dPlaneBase{
        width: 320
        height: 240
        draw_color: mod.draw.DrawColor{}
        accent_color: #x4f8cff
        action_color: #x4f8cff
        variant: 0.0
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Css3dPlane {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_projective_quad: DrawProjectiveQuad,
    #[live]
    draw_color: DrawColor,
    #[live(vec4(0.31, 0.55, 1.0, 1.0))]
    accent_color: Vec4f,
    #[live(vec4(0.31, 0.55, 1.0, 1.0))]
    action_color: Vec4f,
    #[live(0.0)]
    variant: f32,
    #[rust(Mat4f::identity())]
    transform_matrix: Mat4f,
    #[rust(Mat4f::identity())]
    perspective_matrix: Mat4f,
    #[rust]
    surface: Option<MpSurface>,
    #[rust(true)]
    surface_dirty: bool,
    #[rust]
    surface_draw_list: Option<DrawList2d>,
}

impl Css3dPlane {
    pub fn set_matrices(&mut self, transform_matrix: Mat4f, perspective_matrix: Mat4f) {
        self.transform_matrix = transform_matrix;
        self.perspective_matrix = perspective_matrix;
    }

    fn rgb(hex: u32) -> Vec4f {
        vec4(
            ((hex >> 16) & 0xff) as f32 / 255.0,
            ((hex >> 8) & 0xff) as f32 / 255.0,
            (hex & 0xff) as f32 / 255.0,
            1.0,
        )
    }

    fn draw_surface_rect(
        &mut self,
        cx: &mut Cx2d,
        size: DVec2,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        color: Vec4f,
    ) {
        let sx = size.x / BASE_CARD_W;
        let sy = size.y / BASE_CARD_H;
        self.draw_color.color = color;
        self.draw_color.draw_abs(
            cx,
            Rect {
                pos: dvec2(x0 * sx, y0 * sy),
                size: dvec2((x1 - x0) * sx, (y1 - y0) * sy),
            },
        );
    }

    fn draw_card_surface(&mut self, cx: &mut Cx2d, size: DVec2) {
        let bg = Self::rgb(0xf4f7fb);
        let dark = Self::rgb(0x1b2634);
        let mid = Self::rgb(0x708296);
        let mid_light = Self::rgb(0x8da1b5);
        let border = Self::rgb(0xc2ccd7);
        let panel = Self::rgb(0xffffff);
        let quiet = Self::rgb(0x77889b);
        let quiet_light = Self::rgb(0x97a8ba);
        let subtle = Self::rgb(0x243646);
        let subtle_text = Self::rgb(0x3f5368);
        let subtle_text_light = Self::rgb(0x7b8ea1);
        let muted = Self::rgb(0xeef2f7);
        let button_bg = Self::rgb(0xedf2f7);
        let frame = Self::rgb(0x203040);

        self.draw_surface_rect(cx, size, 0.0, 0.0, BASE_CARD_W, BASE_CARD_H, bg);
        self.draw_surface_rect(cx, size, 0.0, 0.0, BASE_CARD_W, 10.0, self.accent_color);
        self.draw_surface_rect(cx, size, 0.0, BASE_CARD_H - 1.0, BASE_CARD_W, BASE_CARD_H, frame);
        self.draw_surface_rect(cx, size, 0.0, 0.0, 1.0, BASE_CARD_H, frame);
        self.draw_surface_rect(cx, size, BASE_CARD_W - 1.0, 0.0, BASE_CARD_W, BASE_CARD_H, frame);

        self.draw_surface_rect(cx, size, 22.0, 30.0, 166.0, 42.0, dark);
        self.draw_surface_rect(cx, size, 22.0, 54.0, 286.0, 62.0, mid);
        self.draw_surface_rect(cx, size, 22.0, 70.0, 258.0, 78.0, mid_light);

        self.draw_surface_rect(cx, size, 22.0, 104.0, 298.0, 150.0, panel);
        self.draw_surface_rect(cx, size, 22.0, 104.0, 298.0, 105.0, border);
        self.draw_surface_rect(cx, size, 22.0, 149.0, 298.0, 150.0, border);
        self.draw_surface_rect(cx, size, 22.0, 104.0, 23.0, 150.0, border);
        self.draw_surface_rect(cx, size, 297.0, 104.0, 298.0, 150.0, border);
        self.draw_surface_rect(cx, size, 34.0, 120.0, 184.0, 128.0, mid_light);

        match self.variant.round() as i32 {
            0 => {
                self.draw_surface_rect(cx, size, 22.0, 170.0, 136.0, 210.0, self.action_color);
                self.draw_surface_rect(cx, size, 42.0, 184.0, 112.0, 192.0, panel);
                self.draw_surface_rect(cx, size, 160.0, 176.0, 286.0, 184.0, quiet);
                self.draw_surface_rect(cx, size, 160.0, 192.0, 248.0, 200.0, quiet_light);
            }
            1 => {
                self.draw_surface_rect(cx, size, 22.0, 170.0, 44.0, 192.0, subtle);
                self.draw_surface_rect(cx, size, 26.0, 174.0, 40.0, 188.0, self.action_color);
                self.draw_surface_rect(cx, size, 58.0, 176.0, 226.0, 184.0, subtle_text);
                self.draw_surface_rect(cx, size, 58.0, 194.0, 264.0, 202.0, subtle_text_light);
                self.draw_surface_rect(cx, size, 238.0, 170.0, 298.0, 210.0, button_bg);
                self.draw_surface_rect(cx, size, 252.0, 184.0, 284.0, 192.0, quiet);
                self.draw_surface_rect(cx, size, 238.0, 170.0, 239.0, 210.0, border);
                self.draw_surface_rect(cx, size, 297.0, 170.0, 298.0, 210.0, border);
                self.draw_surface_rect(cx, size, 238.0, 170.0, 298.0, 171.0, border);
                self.draw_surface_rect(cx, size, 238.0, 209.0, 298.0, 210.0, border);
            }
            _ => {
                self.draw_surface_rect(cx, size, 22.0, 170.0, 122.0, 210.0, muted);
                self.draw_surface_rect(cx, size, 22.0, 170.0, 23.0, 210.0, border);
                self.draw_surface_rect(cx, size, 121.0, 170.0, 122.0, 210.0, border);
                self.draw_surface_rect(cx, size, 22.0, 170.0, 122.0, 171.0, border);
                self.draw_surface_rect(cx, size, 22.0, 209.0, 122.0, 210.0, border);
                self.draw_surface_rect(cx, size, 138.0, 170.0, 298.0, 210.0, self.action_color);
                self.draw_surface_rect(cx, size, 176.0, 184.0, 260.0, 192.0, panel);
            }
        }
    }
}

impl Widget for Css3dPlane {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }

        if self.surface.is_none() {
            self.surface = Some(MpSurface::new(
                cx.cx.cx,
                rect.size,
                MpSurfaceColorFormat::BgraU8,
                false,
            ));
            self.surface_dirty = true;
        }
        if self.surface_draw_list.is_none() {
            self.surface_draw_list = Some(DrawList2d::new(cx.cx.cx));
        }

        if self.surface.as_ref().unwrap().size() != rect.size {
            self.surface.as_mut().unwrap().resize(cx.cx.cx, rect.size);
            self.surface_dirty = true;
        }

        if self.surface_dirty {
            self.surface.as_mut().unwrap().begin(cx, None);
            cx.set_pass_shift_scale(self.surface.as_ref().unwrap().pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
            self.surface_draw_list.as_mut().unwrap().begin_always(cx);
            cx.begin_root_turtle_for_pass(Layout::default());
            self.draw_card_surface(cx, rect.size);
            cx.end_pass_sized_turtle();
            self.surface_draw_list.as_mut().unwrap().end(cx);
            self.surface.as_mut().unwrap().end(cx);
            self.surface_dirty = false;
        }

        self.draw_projective_quad
            .set_matrices(self.transform_matrix, self.perspective_matrix);
        self.draw_projective_quad
            .set_texture(Some(self.surface.as_ref().unwrap().color_texture().clone()));
        self.draw_projective_quad.draw_abs(cx, rect);
        DrawStep::done()
    }
}
