use makepad_render::*;
use crate::widgetstyle::*;

#[derive(Clone)]
pub struct ScrollShadow {
    pub bg: Quad,
    pub z: f32,
}

impl ScrollShadow {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            bg: Quad ::new(cx),
            z: 10.,
        }
    }
    
    pub fn shadow_size() -> FloatId {uid!()}
    pub fn shadow_top() -> InstanceFloat {uid!()}
    pub fn shader_bg() -> ShaderId {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        
        Self::shadow_size().set(cx, 4.0);
        
        Self::shader_bg().set(cx, Quad::def_quad_shader().compose(shader_ast !({
            let is_viz: float<Varying>;
            let shadow_top: Self::shadow_top();
            fn scroll() -> vec2 {
                if shadow_top > 0.5 {
                    is_viz = clamp(draw_scroll.w*0.1,0.,1.)
                }
                else {
                    is_viz = clamp(draw_scroll.z*0.1,0.,1.)
                }
                return draw_scroll.xy;
            }
            
            fn pixel() -> vec4 { // TODO make the corner overlap properly with a distance field eq.
                if shadow_top > 0.5{
                    return mix(vec4(0., 0., 0., is_viz), vec4(0., 0., 0., 0.), pow(geom.y, 0.5));
                }
                return mix(vec4(0., 0., 0., is_viz), vec4(0., 0., 0., 0.), pow(geom.x, 0.5));
            }
        })));
    }
    
    pub fn draw_shadow_top(&mut self, cx:&mut Cx){
        self.draw_shadow_top_at(cx, Rect {
            x: 0.,
            y: 0.,
            w: cx.get_width_total(),
            h: 0.
        });
    }
    
    pub fn draw_shadow_top_at(&mut self, cx:&mut Cx, rect:Rect){
        self.bg.shader = Self::shader_bg().get(cx);
        self.bg.z = self.z;
        let inst = self.bg.draw_quad_rel(cx, Rect{h:ScrollShadow::shadow_size().get(cx),..rect});
        inst.set_do_scroll(cx, false, false);
        inst.push_float(cx, 1.0);
    }

    pub fn draw_shadow_left(&mut self, cx:&mut Cx){
        self.draw_shadow_left_at(cx, Rect {
            x: 0.,
            y: 0.,
            w: 0.,
            h: cx.get_height_total()
        });
    } 

    pub fn draw_shadow_left_at(&mut self, cx:&mut Cx, rect:Rect){
        self.bg.shader = Self::shader_bg().get(cx);
        self.bg.z = self.z;
        let inst = self.bg.draw_quad_rel(cx, Rect{w:ScrollShadow::shadow_size().get(cx),..rect});
        inst.set_do_scroll(cx, false, false);
        inst.push_float(cx, 0.0);
    }
    
}
