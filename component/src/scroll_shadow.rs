#![allow(unused)]
use crate::makepad_platform::*;

live_register!{
    use makepad_platform::shader::std::*;
    use crate::theme::*;
    
    ScrollShadow:{{ScrollShadow}}{
        
        no_h_scroll: true
        no_v_scroll: true

        shadow_size: 4.0,
        varying is_viz: float;
        
        fn scroll(self) -> vec2 {
            if self.shadow_is_top > 0.5 {
                self.is_viz = clamp(self.draw_scroll.w * 0.1, 0., 1.);
            }
            else {
                self.is_viz = clamp(self.draw_scroll.z * 0.1, 0., 1.);
            }
            return self.draw_scroll.xy;
        }
        
        fn pixel(self) -> vec4 { // TODO make the corner overlap properly with a distance field eq.
            let base = COLOR_BG_EDITOR.xyz;
            let alpha = 0.0;
            if self.shadow_is_top > 0.5 {
                alpha = pow(self.geom_pos.y, 0.5);
            }
            else{
                alpha = pow(self.geom_pos.x, 0.5);
            }
            return Pal::premul(mix(vec4(base, self.is_viz), vec4(base, 0.), alpha));
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct ScrollShadow {
    shadow_size: f32,
    deref_target: DrawQuad,
    shadow_is_top: f32,
}

impl ScrollShadow{
    pub fn draw(&mut self, cx: &mut Cx2d, view:&View, offset:Vec2){
        let shadow_size = self.shadow_size;
        let rect = cx.get_turtle_rect();

        self.shadow_is_top = 0.0;
        self.draw_abs(cx, Rect {
            pos: rect.pos + vec2(offset.x,0.0),
            size: vec2(shadow_size, rect.size.y)
        });


        self.shadow_is_top = 1.0;
        self.draw_abs(cx, Rect {
            pos: rect.pos + vec2(0., offset.y),
            size: vec2(rect.size.x, shadow_size)
        });
        self.draw_vars.area.set_no_scroll(cx, true, true);
    }
}
