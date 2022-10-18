use crate::makepad_draw_2d::*;

live_design!{
    import makepad_draw_2d::shader::std::*;
    import crate::theme::*;
    
    ScrollShadow= {{ScrollShadow}} {
        
        shadow_size: 4.0,
        
        fn pixel(self) -> vec4 { // TODO make the corner overlap properly with a distance field eq.
            let is_viz = clamp(self.scroll * 0.1, 0., 1.);
            let pos = self.pos;
            let base = COLOR_BG_EDITOR.xyz;
            let alpha = 0.0;
            if self.shadow_is_top > 0.5 {
                alpha = pow(pos.y, 0.5);
            }
            else {
                alpha = pow(pos.x, 0.5);
            }
            //turn vec4(base,is_viz);
            return Pal::premul(mix(vec4(#000.xyz, is_viz), vec4(base, 0.), alpha));
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct ScrollShadow {
    shadow_size: f32,
    draw_super: DrawQuad,
    shadow_is_top: f32,
    scroll: f32,
}

impl ScrollShadow {
    pub fn draw(&mut self, cx: &mut Cx2d, offset: DVec2) {
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
        self.scroll = scroll.y  as f32;
        self.draw_abs(cx, Rect {
            pos: rect.pos + dvec2(0., offset.y) + scroll,
            size: dvec2(rect.size.x, shadow_size)
        });
    }
}
