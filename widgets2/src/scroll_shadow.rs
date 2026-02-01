use crate::makepad_draw::*;

script_mod!{
    use mod.prelude.widgets_internal.*
    
    set_type_default() do #(DrawScrollShadow::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    
    mod.widgets.DrawScrollShadow = {
        shadow_size: 4.0
        
        pixel: fn() {
            let is_viz = clamp(self.scroll * 0.1, 0., 1.)
            let pos = self.pos
            let base = theme.color_bg_container.xyz
            let mut alpha = 0.0
            if self.shadow_is_top > 0.5 {
                alpha = pow(pos.y, 0.5)
            }
            else {
                alpha = pow(pos.x, 0.5)
            }
            return Pal.premul(mix(vec4(#000.xyz, is_viz), vec4(base, 0.), alpha))
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawScrollShadow {
    #[deref] draw_super: DrawQuad,
    #[live] shadow_size: f32,
    #[live] shadow_is_top: f32,
    #[live] scroll: f32,
}

impl DrawScrollShadow {
    pub fn draw(&mut self, cx: &mut Cx2d, offset: Vec2d) {
        let shadow_size = self.shadow_size as f64;
        let rect = cx.turtle().rect();
        let scroll = cx.turtle().scroll();
        
        self.shadow_is_top = 0.0;
        self.scroll = scroll.x as f32;
        self.draw_abs(cx, Rect {
            pos: rect.pos + dvec2(offset.x, 0.0) + scroll,
            size: dvec2(shadow_size, rect.size.y)
        });
        
        self.shadow_is_top = 1.0;
        self.scroll = scroll.y as f32;
        self.draw_abs(cx, Rect {
            pos: rect.pos + dvec2(0., offset.y) + scroll,
            size: dvec2(rect.size.x, shadow_size)
        });
    }
}
