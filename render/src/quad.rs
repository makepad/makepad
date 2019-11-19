use crate::cx::*;

#[derive(Clone)]
pub struct Quad {
    pub shader: Shader,
    pub do_h_scroll: bool,
    pub do_v_scroll: bool,
    pub z:f32,
    pub color: Color
}

impl Quad {
    pub fn proto_with_shader(cx: &mut Cx, shader:ShaderGen, name:&str) -> Self {
        Self {
            shader: cx.add_shader(shader, name),
            ..Self::proto(cx)
        }
    }
    
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            shader: cx.add_shader(Self::def_quad_shader(), "Quad"),
            do_h_scroll:true,
            do_v_scroll:true,
            z:0.0,
            color: color("green")
        }
    }
    
    pub fn instance_x()->InstanceFloat{uid!()}
    pub fn instance_y()->InstanceFloat{uid!()}
    pub fn instance_w()->InstanceFloat{uid!()}
    pub fn instance_h()->InstanceFloat{uid!()}
    pub fn instance_z()->InstanceFloat{uid!()}
    pub fn instance_color()->InstanceColor{uid!()}
    pub fn uniform_view_do_scroll()->UniformVec2{uid!()}
    pub fn uniform_zbias()->UniformFloat{uid!()}
    
    pub fn def_quad_shader() -> ShaderGen {
        // lets add the draw shader lib
        let mut sg = ShaderGen::new();
        sg.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sg.geometry_indices = vec![0, 1, 2, 2, 3, 0];
        
        sg.compose(shader_ast!({
            
            let geom: vec2<Geometry>;
            let pos: vec2<Varying>;
            
            let x: Self::instance_x();
            let y: Self::instance_y();
            let w: Self::instance_w();
            let h: Self::instance_h();
            let z: Self::instance_z(); 
            let color: Self::instance_color();

            let view_do_scroll: Self::uniform_view_do_scroll();
            let zbias: Self::uniform_zbias();
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
                return camera_projection*(camera_view*(view_transform*vec4(clipped.x, clipped.y, z+zbias, 1.)));
            }
            
            fn pixel() -> vec4 {
                return vec4(color.rgb * color.a, color.a);
            }
            
        }))
    }
    
    pub fn begin_quad(&mut self, cx: &mut Cx, layout: Layout) -> InstanceArea {
        let inst = self.draw_quad_rel(cx, Rect::zero());
        let area = inst.clone().into();
        cx.begin_turtle(layout, area);
        inst
    }
    
    pub fn end_quad(&mut self, cx: &mut Cx, inst: &InstanceArea) -> Area {
        // at this point, we should fill in any missing slots.
        
        let area = inst.clone().into();
        let rect = cx.end_turtle(area);
        area.set_rect(cx, &rect);
        area
    }
    
    pub fn draw_quad(&mut self, cx: &mut Cx, walk:Walk) -> InstanceArea {
        let geom = cx.walk_turtle(walk);
        let inst = self.draw_quad_abs(cx, geom);
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_quad_rel(&mut self, cx: &mut Cx, rect: Rect) -> InstanceArea {
        let pos = cx.get_turtle_origin();
        let inst = self.draw_quad_abs(cx, Rect {x: rect.x + pos.x, y: rect.y + pos.y, w: rect.w, h: rect.h});
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_quad_abs(&mut self, cx: &mut Cx, rect: Rect) -> InstanceArea {
        let inst = cx.new_instance(&self.shader, 1);
        if inst.need_uniforms_now(cx) {
            inst.push_uniform_vec2f(
                cx,
                if self.do_h_scroll {1.0}else {0.0},
                if self.do_v_scroll {1.0}else {0.0}
            );
            inst.push_uniform_float(cx, 0.); 
        }
        //println!("{:?} {}", area, cx.current_draw_list_id);
        let data = [
            /*x,y,w,h*/rect.x,
            rect.y,
            rect.w,
            rect.h,
            self.z,
            /*color*/self.color.r,
            self.color.g,
            self.color.b,
            self.color.a
        ];
        inst.push_slice(cx, &data);
        inst
    }
}
