use crate::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.DrawBrowserTaskTexture = mod.std.set_type_default() do #(DrawBrowserTaskTexture::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_texture: texture_2d(float)
        pixel: fn(){
            let uv = self.pos
            var color = self.color_texture.sample_as_bgra(uv)
            if self.blur_radius > 0.5 {
                let sx = self.blur_radius / self.tex_size.x
                let sy = self.blur_radius / self.tex_size.y
                let w0 = 0.2042
                let w1 = 0.1240
                let w2 = 0.0752
                color = self.color_texture.sample_as_bgra(uv) * w0
                color += self.color_texture.sample_as_bgra(uv + vec2(sx, 0.0)) * w1
                color += self.color_texture.sample_as_bgra(uv + vec2(-sx, 0.0)) * w1
                color += self.color_texture.sample_as_bgra(uv + vec2(0.0, sy)) * w1
                color += self.color_texture.sample_as_bgra(uv + vec2(0.0, -sy)) * w1
                color += self.color_texture.sample_as_bgra(uv + vec2(sx, sy)) * w2
                color += self.color_texture.sample_as_bgra(uv + vec2(-sx, sy)) * w2
                color += self.color_texture.sample_as_bgra(uv + vec2(sx, -sy)) * w2
                color += self.color_texture.sample_as_bgra(uv + vec2(-sx, -sy)) * w2
            }
            return color * self.opacity
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawBrowserTaskTexture {
    #[deref]
    pub draw_super: DrawQuad,
    #[live(0.0)]
    pub blur_radius: f32,
    #[live(1.0)]
    pub opacity: f32,
    #[live(vec2(1.0, 1.0))]
    pub tex_size: Vec2f,
}

impl DrawBrowserTaskTexture {
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_super.rect_pos = rect.pos.into();
        self.draw_super.rect_size = rect.size.into();
        self.draw_super.draw(cx);
    }
}
