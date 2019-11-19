use crate::cx::*;

#[derive(Clone)]
pub struct Blit {
    pub shader: Shader,
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub alpha: f32,
    pub do_scroll: bool
}

impl Blit {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            alpha: 1.0,
            min_x:0.0,
            min_y:0.0,
            max_x:1.0,
            max_y:1.0,
            shader: cx.add_shader(Self::def_blit_shader(), "Blit"),
            do_scroll:false,
        }
    }
    
    pub fn instance_x()->InstanceFloat{uid!()}
    pub fn instance_y()->InstanceFloat{uid!()}
    pub fn instance_w()->InstanceFloat{uid!()}
    pub fn instance_h()->InstanceFloat{uid!()}
    pub fn instance_min_x()->InstanceFloat{uid!()}
    pub fn instance_min_y()->InstanceFloat{uid!()}
    pub fn instance_max_x()->InstanceFloat{uid!()}
    pub fn instance_max_y()->InstanceFloat{uid!()}
    pub fn instance_z()->InstanceFloat{uid!()}
    pub fn instance_color()->InstanceColor{uid!()}
    pub fn uniform_view_do_scroll()->UniformVec2{uid!()}
    pub fn uniform_alpha()->UniformFloat{uid!()}
    
    pub fn def_blit_shader() -> ShaderGen {
        // lets add the draw shader lib
        let mut sb = ShaderGen::new();
        
        sb.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sb.geometry_indices = vec![0, 1, 2, 2, 3, 0];
        
        sb.compose(shader_ast!({
            
            let geom: vec2<Geometry>;
            let x: Self::instance_x();
            let y: Self::instance_y();
            let w: Self::instance_w();
            let h: Self::instance_h();
            let min_x: Self::instance_min_x();
            let min_y: Self::instance_min_y();
            let max_x: Self::instance_max_x();
            let max_y: Self::instance_max_y();
            let tc: vec2<Varying>;
            let view_do_scroll: Self::uniform_view_do_scroll();
            let alpha: Self::uniform_alpha();
            let texturez:texture2d<Texture>;
            let v_pixel: vec2<Varying>;
            //let dpi_dilate: float<Uniform>;
            
            fn vertex() -> vec4 {
                // return vec4(geom.x-0.5, geom.y, 0., 1.);
                let shift: vec2 = -view_scroll * view_do_scroll;
                let clipped: vec2 = clamp(
                    geom * vec2(w, h) + vec2(x, y) + shift,
                    view_clip.xy,
                    view_clip.zw
                ); 
                let pos = (clipped - shift - vec2(x, y)) / vec2(w, h);
                tc = mix(vec2(min_x,min_y), vec2(max_x,max_y), pos);
                v_pixel = clipped;
                // only pass the clipped position forward
                return camera_projection * vec4(clipped.x, clipped.y, 0., 1.);
            }
            
            fn pixel() -> vec4 {
                return vec4(sample2d(texturez, tc.xy).rgb, alpha);
            }
            
        }))
    }
    
    
    pub fn begin_blit(&mut self, cx: &mut Cx, texture:&Texture, layout: Layout) -> InstanceArea {
        let inst = self.draw_blit(cx, texture, Rect::zero());
        let area = inst.clone().into();
        cx.begin_turtle(layout, area);
        inst
    }
    
    pub fn end_blit(&mut self, cx: &mut Cx, inst: &InstanceArea) -> Area {
        let area = inst.clone().into();
        let rect = cx.end_turtle(area);
        area.set_rect(cx, &rect);
        area
    }
    
    pub fn draw_blit_walk(&mut self, cx: &mut Cx, texture:&Texture, walk:Walk) -> InstanceArea {
        let geom = cx.walk_turtle(walk);
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
            inst.push_uniform_float(cx, self.alpha);
            inst.push_uniform_texture_2d(cx, texture);
        }
        //println!("{:?} {}", area, cx.current_draw_list_id);
        let data = [
            /*x,y,w,h*/rect.x,
            rect.y,
            rect.w,
            rect.h,
            self.min_x,
            self.min_y,
            self.max_x,
            self.max_y
        ];
        inst.push_slice(cx, &data);
        inst
    }
}
