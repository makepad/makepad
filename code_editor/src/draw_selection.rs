use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*

    set_type_default() do #(DrawSelection::script_shader(vm)) {
        ..mod.draw.DrawQuad
        gloopiness: uniform(8.0)
        border_radius: uniform(2.0)
        focus: 1.0
        vertex: fn() {
            let clipped = clamp(
                self.geom.pos * vec2(self.rect_size.x + 16., self.rect_size.y) + self.rect_pos - vec2(8., 0.),
                self.draw_clip.xy,
                self.draw_clip.zw
            )
            self.pos = (clipped - self.rect_pos) / self.rect_size
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * (
                self.draw_list.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_call.zbias, 1.)
            ))
        }

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.rect_pos + self.pos * self.rect_size)
            sdf.box(
                self.rect_pos.x,
                self.rect_pos.y,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            )
            if self.prev_w > 0.0 {
                sdf.box(
                    self.prev_x,
                    self.rect_pos.y - self.rect_size.y,
                    self.prev_w,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.gloop(self.gloopiness)
            }
            if self.next_w > 0.0 {
                sdf.box(
                    self.next_x,
                    self.rect_pos.y + self.rect_size.y,
                    self.next_w,
                    self.rect_size.y,
                    self.border_radius
                )
                sdf.gloop(self.gloopiness)
            }
            return sdf.fill(theme.color_u_1.mix(theme.color_u_3 * 0.8, self.focus))
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSelection {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub prev_x: f32,
    #[live]
    pub prev_w: f32,
    #[live]
    pub next_x: f32,
    #[live]
    pub next_w: f32,
    #[live(1.0)]
    pub focus: f32,
    #[rust]
    pub prev_prev_rect: Option<Rect>,
    #[rust]
    pub prev_rect: Option<Rect>,
}

impl DrawSelection {
    pub fn begin(&mut self) {
        debug_assert!(self.prev_rect.is_none());
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        self.draw_rect_internal(cx, None);
        self.prev_prev_rect = None;
        self.prev_rect = None;
    }

    pub fn draw(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_rect_internal(cx, Some(rect));
        self.prev_prev_rect = self.prev_rect;
        self.prev_rect = Some(rect);
    }

    fn draw_rect_internal(&mut self, cx: &mut Cx2d, rect: Option<Rect>) {
        if let Some(prev_rect) = self.prev_rect {
            if let Some(prev_prev_rect) = self.prev_prev_rect {
                self.prev_x = prev_prev_rect.pos.x as f32;
                self.prev_w = prev_prev_rect.size.x as f32;
            } else {
                self.prev_x = 0.0;
                self.prev_w = 0.0;
            }
            if let Some(rect) = rect {
                self.next_x = rect.pos.x as f32;
                self.next_w = rect.size.x as f32;
            } else {
                self.next_x = 0.0;
                self.next_w = 0.0;
            }
            self.draw_abs(cx, prev_rect);
        }
    }
}
