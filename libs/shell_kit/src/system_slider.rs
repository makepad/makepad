use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    set_type_default() do #(DrawSystemSlider::script_shader(vm)) {
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let y = (self.rect_size.y - 6.0) * 0.5
            let travel = max(1.0, self.rect_size.x - 12.0)
            sdf.box(1.0, y, self.rect_size.x - 2.0, 6.0, 3.0)
            sdf.fill(self.track_color)
            sdf.box(1.0, y, max(1.0, travel * self.progress + 5.0), 6.0, 3.0)
            sdf.fill(self.fill_color)
            sdf.box(1.0 + travel * self.progress, 1.0, 10.0, self.rect_size.y - 2.0, 3.0)
            sdf.fill(self.cap_color)
            return sdf.result * self.opacity
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSystemSlider {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub track_color: Vec4f,
    #[live]
    pub fill_color: Vec4f,
    #[live]
    pub cap_color: Vec4f,
    #[live]
    pub progress: f32,
    #[live(1.0)]
    pub opacity: f32,
}
