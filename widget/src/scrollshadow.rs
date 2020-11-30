use makepad_render::*;

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawScrollShadow {
    #[default_shader(self::shader_bg)]
    pub base: DrawQuad,
    pub shadow_top: f32
}

#[derive(Clone)]
pub struct ScrollShadow {
    pub bg: DrawScrollShadow,
}

impl ScrollShadow {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            bg: DrawScrollShadow ::new(cx, default_shader!())
                .with_draw_depth(10.),
        }
    }
    
    pub fn with_draw_depth(self, depth: f32) -> Self {
        Self {bg: self.bg.with_draw_depth(depth)}
    }
    
    pub fn style(cx: &mut Cx) {
        DrawScrollShadow::register_draw_input(cx);
        live_body!(cx, r#"
            self::shadow_size: 4.0;
            self::shader_bg: Shader {
                use makepad_render::drawquad::shader::*;
                
                draw_input: self::DrawScrollShadow;
                varying is_viz: float;
                
                fn scroll() -> vec2 {
                    if shadow_top > 0.5 {
                        is_viz = clamp(draw_scroll.w * 0.1, 0., 1.);
                    }
                    else {
                        is_viz = clamp(draw_scroll.z * 0.1, 0., 1.);
                    }
                    return draw_scroll.xy;
                }
                
                fn pixel() -> vec4 { // TODO make the corner overlap properly with a distance field eq.
                    if shadow_top > 0.5 {
                        return mix(vec4(0., 0., 0., is_viz), vec4(0., 0., 0., 0.), pow(geom.y, 0.5));
                    }
                    return mix(vec4(0., 0., 0., is_viz), vec4(0., 0., 0., 0.), pow(geom.x, 0.5));
                }
            }
        "#);
    }
    
    pub fn draw_shadow_top(&mut self, cx: &mut Cx) {
        self.draw_shadow_top_at(cx, Rect {
            pos: vec2(0., 0.),
            size: vec2(cx.get_width_total(), 0.)
        });
    }
    
    pub fn draw_shadow_top_at(&mut self, cx: &mut Cx, rect: Rect) {
        let size = live_float!(cx, self::shadow_size);
        self.bg.shadow_top = 1.0;
        self.bg.draw_quad_rel(cx, Rect {
            pos: rect.pos,
            size: vec2(rect.size.x, size)
        });
        self.bg.area().set_do_scroll(cx, false, false);
    }
    
    pub fn draw_shadow_left(&mut self, cx: &mut Cx) {
        self.draw_shadow_left_at(cx, Rect {
            pos: vec2(0., 0.),
            size: vec2(0., cx.get_height_total())
        });
    }
    
    pub fn draw_shadow_left_at(&mut self, cx: &mut Cx, rect: Rect) {
        let size = live_float!(cx, self::shadow_size);
        self.bg.shadow_top = 0.0;
        self.bg.draw_quad_rel(cx, Rect {
            pos: rect.pos,
            size: vec2(size, rect.size.y)
        });
        self.bg.area().set_do_scroll(cx, false, false);
    }
    
}
