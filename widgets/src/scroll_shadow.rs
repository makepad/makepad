use crate::makepad_draw::*;

live_design!{
    DrawScrollShadowBase = {{DrawScrollShadow}} {}
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawScrollShadow {
    #[live] shadow_size: f32,
    #[deref] draw_super: DrawQuad,
    #[live] shadow_is_top: f32,
    #[live] scroll: f32,
}

impl DrawScrollShadow {
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
