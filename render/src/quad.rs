use crate::cx::*;

#[derive(Clone)]
pub struct Quad{
    pub shader_id:usize,
    pub id:u32,
    pub do_scroll:bool,
    pub color: Color
}

impl Style for Quad{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_quad_shader(cx);
        Self{
            shader_id:cx.add_shader(sh, "Quad"),
            do_scroll:true,
            id:0,
            color:color("green")
        }
    }
}

impl Quad{
    pub fn def_quad_shader(cx:&mut Cx)->Shader{
        // lets add the draw shader lib
        let mut sh = cx.new_shader();
        sh.geometry_vertices = vec![
            0.0,0.0,
            1.0,0.0,
            1.0,1.0,
            0.0,1.0
        ];
        sh.geometry_indices = vec![
            0,1,2,
            2,3,0
        ];

        sh.add_ast(shader_ast!({

            let geom:vec2<Geometry>;
            let x:float<Instance>;
            let y:float<Instance>;
            let w:float<Instance>;
            let h:float<Instance>;
            let color:vec4<Instance>;
            let pos:vec2<Varying>;
            let draw_list_do_scroll:float<Uniform>;
            let dpi_dilate:float<Uniform>;

            fn vertex()->vec4{
                let shift:vec2 = -draw_list_scroll * draw_list_do_scroll;
                let clipped:vec2 = clamp(
                    geom*vec2(w, h) + vec2(x, y) + shift,
                    draw_list_clip.xy,
                    draw_list_clip.zw
                );
                pos = (clipped - shift - vec2(x,y)) / vec2(w, h);
                // only pass the clipped position forward
                return vec4(clipped,0.,1.) * camera_projection;
            }

            fn pixel()->vec4{
                return vec4(color.rgb*color.a, color.a);
            }

        }));
        sh
    }

    pub fn begin_quad(&mut self, cx:&mut Cx, layout:&Layout)->InstanceArea{
        let inst = self.draw_quad(cx, Rect::zero());
        let area = inst.clone().into_area();
        cx.begin_turtle(layout, area);
        inst
    }

    pub fn end_quad(&mut self, cx:&mut Cx, inst:&InstanceArea)->Area{
        let area = inst.clone().into_area();
        let rect = cx.end_turtle(area);
        area.set_rect(cx, &rect);
        area
    }

    fn do_uniforms(&mut self, cx:&mut Cx, inst:&InstanceArea){
        inst.push_uniform_float(cx, if self.do_scroll{1.0}else{0.0});
        let dpi_dilate = (2.-cx.target_dpi_factor).max(0.).min(1.);
        inst.push_uniform_float(cx, dpi_dilate);
    }

    pub fn draw_quad_walk(&mut self, cx:&mut Cx, w:Bounds, h:Bounds, margin:Margin)->InstanceArea{
        let inst = cx.new_aligned_instance(self.shader_id, 1).inst;
        if inst.need_uniforms_now(cx){
            self.do_uniforms(cx, &inst);
        }
        let geom = cx.walk_turtle(w, h, margin, None);
        
        let data = [
            /*x,y,w,h*/geom.x,geom.y,geom.w,geom.h,
            /*color*/self.color.r,self.color.g,self.color.b,self.color.a
        ];
        inst.push_slice(cx, &data);

        inst
    }
/*
    pub fn draw_quad(&mut self, cx:&mut Cx, x:f32, y:f32, w:f32, h:f32)->InstanceArea{
        let inst = cx.new_aligned_instance(self.shader_id, 1).inst;
        if inst.need_uniforms_now(cx){
            inst.push_uniform_float(cx, if self.do_scroll{1.0}else{0.0});
        }
        //println!("{:?} {}", area, cx.current_draw_list_id);
        let pos = cx.turtle_origin();
        let data = [
            /*x,y,w,h*/pos.x+x,pos.y+y,w,h,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];
        inst.push_slice(cx, &data);
        inst
    }*/

    pub fn draw_quad(&mut self, cx:&mut Cx, rect:Rect)->InstanceArea{
        let inst = cx.new_aligned_instance(self.shader_id, 1).inst;
        if inst.need_uniforms_now(cx){
            self.do_uniforms(cx, &inst);
        }
        //println!("{:?} {}", area, cx.current_draw_list_id);
        let pos = cx.get_turtle_origin();
        let data = [
            /*x,y,w,h*/rect.x + pos.x,rect.y +pos.y,rect.w,rect.h,
            /*color*/self.color.r,self.color.g,self.color.b,self.color.a
        ];
        inst.push_slice(cx, &data);
        inst
    }

    pub fn draw_quad_abs(&mut self, cx:&mut Cx, rect:Rect)->InstanceArea{
        let inst = cx.new_aligned_instance(self.shader_id, 1).inst;
        if inst.need_uniforms_now(cx){
            self.do_uniforms(cx, &inst);
        }
        //println!("{:?} {}", area, cx.current_draw_list_id);
        let data = [
            /*x,y,w,h*/rect.x,rect.y,rect.w,rect.h,
            /*color*/self.color.r,self.color.g,self.color.b,self.color.a
        ];
        inst.push_slice(cx, &data);
        inst
    }
}
