use render::*;
use crate::widgetstyle::*;

#[derive(Clone)]
pub struct ScrollShadow {
    pub quad: Quad,
    pub z: f32,
}

impl ScrollShadow {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            quad: Quad {
                shader: cx.add_shader(Self::def_shadow_shader(), "ScrollShadow"),
                ..Quad::proto(cx)
            },
            z: 0.,
        }
    }
    
    pub fn shadow_size() -> FloatId {uid!()}
    pub fn shadow_top() -> InstanceFloat {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        Self::shadow_size().set(cx, 6.0);
    }
    
    pub fn def_shadow_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast !({
            let is_viz: float<Varying>;
            let shadow_top: Self::shadow_top();
            fn scroll() -> vec2 {
                if shadow_top > 0.5 {
                    if draw_scroll.y > 0. {
                        is_viz = 1.0
                    }
                    else {
                        is_viz = 0.0;
                    }
                }
                else {
                    if draw_scroll.x > 0. {
                        is_viz = 1.0
                    }
                    else {
                        is_viz = 0.0;
                    }
                }
                return vec2(0., 0.);
            }
            
            fn pixel() -> vec4 { // TODO make the corner overlap properly with a distance field eq.
                if shadow_top > 0.5{
                    return mix(vec4(0., 0., 0., is_viz), vec4(0., 0., 0., 0.), pow(geom.y, 0.5));
                }
                return mix(vec4(0., 0., 0., is_viz), vec4(0., 0., 0., 0.), pow(geom.x, 0.5));
            }
        }))
    }
    
    pub fn draw_shadow_top(&mut self, cx:&mut Cx, rect:Rect){
        self.quad.z = self.z;
        let inst = self.quad.draw_quad_rel(cx, Rect{h:ScrollShadow::shadow_size().get(cx),..rect});
        inst.push_float(cx, 1.0);
    }

    pub fn draw_shadow_left(&mut self, cx:&mut Cx, rect:Rect){
        self.quad.z = self.z;
        let inst = self.quad.draw_quad_rel(cx, Rect{w:ScrollShadow::shadow_size().get(cx),..rect});
        inst.push_float(cx, 0.0);
    }
    
}
