use crate::cx::*;

#[derive(Clone)]
pub struct Blit {
    pub shader: Shader,
    pub do_scroll: bool
}

impl Style for Blit {
    fn style(cx: &mut Cx) -> Self {
        let sh = Self::def_blit_shader(cx);
        Self {
            shader: cx.add_shader(sh, "Blit"),
            do_scroll:false,
        }
    }
}

impl Blit {
    pub fn def_blit_shader(cx: &mut Cx) -> CxShader {
        // lets add the draw shader lib
        let mut sh = cx.new_shader();
        
        sh.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sh.geometry_indices = vec![0, 1, 2, 2, 3, 0];
        
        sh.add_ast(shader_ast!({
            
            let geom: vec2<Geometry>;
            let x: float<Instance>;
            let y: float<Instance>;
            let w: float<Instance>;
            let h: float<Instance>;
            let pos: vec2<Varying>;
            let view_do_scroll: float<Uniform>;
            let texturez:texture2d<Texture>;
            //let dpi_dilate: float<Uniform>;
            
            fn vertex() -> vec4 {
                // return vec4(geom.x-0.5, geom.y, 0., 1.);
                let shift: vec2 = -view_scroll * view_do_scroll;
                let clipped: vec2 = clamp(
                    geom * vec2(w, h) + vec2(x, y) + shift,
                    view_clip.xy,
                    view_clip.zw
                );
                pos = (clipped - shift - vec2(x, y)) / vec2(w, h);
                // only pass the clipped position forward
                return vec4(clipped.x, clipped.y, 0., 1.) * camera_projection;
            }
            
            fn pixel() -> vec4 {
                //return color("red");
                return vec4(sample2d(texturez, geom.xy).rgb, 1.0);
            }
            
        }));
        sh
    }
    
    
    pub fn begin_blit(&mut self, cx: &mut Cx, texture:&Texture, layout: &Layout) -> InstanceArea {
        let inst = self.draw_blit(cx, texture, Rect::zero());
        let area = inst.clone().into_area();
        cx.begin_turtle(layout, area);
        inst
    }
    
    pub fn end_blit(&mut self, cx: &mut Cx, inst: &InstanceArea) -> Area {
        let area = inst.clone().into_area();
        let rect = cx.end_turtle(area);
        area.set_rect(cx, &rect);
        area
    }
    
    pub fn draw_blit_walk(&mut self, cx: &mut Cx, texture:&Texture, w: Bounds, h: Bounds, margin: Margin) -> InstanceArea {
        let geom = cx.walk_turtle(w, h, margin, None);
        let inst = self.draw_blit_abs(cx, texture, geom);
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_blit(&mut self, cx: &mut Cx, texture:&Texture, rect: Rect) -> InstanceArea {
        let pos = cx.get_turtle_origin();
        let inst = self.draw_blit_abs(cx, texture, Rect {x: rect.x + pos.x, y: rect.y + pos.y, w: rect.w, h: rect.h});
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_blit_abs(&mut self, cx: &mut Cx, texture:&Texture, rect: Rect) -> InstanceArea {
        let inst = cx.new_instance_draw_call(&self.shader, 1);
        if inst.need_uniforms_now(cx) {
            inst.push_uniform_float(cx, if self.do_scroll {1.0}else {0.0});
            inst.push_uniform_texture_2d(cx, texture);
        }
        //println!("{:?} {}", area, cx.current_draw_list_id);
        let data = [
            /*x,y,w,h*/rect.x,
            rect.y,
            rect.w,
            rect.h,
        ];
        inst.push_slice(cx, &data);
        inst
    }
}
