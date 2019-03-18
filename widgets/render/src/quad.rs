use crate::cx::*;

#[derive(Clone)]
pub struct Quad{
    pub shader_id:usize,
    pub id:u32,
    pub color: Vec4
}

impl Style for Quad{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_quad_shader(cx);
        Self{
            shader_id:cx.add_shader(sh, "Quad"),
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

            fn vertex()->vec4{
                let shift:vec2 = -draw_list_scroll;
                let clipped:vec2 = clamp(
                    geom*vec2(w, h) + vec2(x, y) + shift,
                    draw_list_clip.xy,
                    draw_list_clip.zw
                );
                pos = (clipped - shift - vec2(x,y)) / vec2(w, h);
                // only pass the clipped position forward
                //pos = clipped;
                return vec4(clipped,0.,1.) * camera_projection;
            }

            fn pixel()->vec4{
                return vec4(color.rgb*color.a, color.a);
            }

        }));
        sh
    }

    pub fn begin_quad(&mut self, cx:&mut Cx, layout:&Layout)->Area{
        let area = self.draw_quad(cx, 0.0,0.0,0.0,0.0);
        cx.begin_turtle(layout, area);
        cx.push_instance_area_stack(area.clone());
        area
    }

    pub fn end_quad(&mut self, cx:&mut Cx)->Area{
        let area = cx.pop_instance_area_stack();
        let rect = cx.end_turtle(area);
        area.set_rect(cx, &rect);
        area
    }

    pub fn draw_quad_walk(&mut self, cx:&mut Cx, w:Bounds, h:Bounds, margin:Margin)->Area{
        let area = cx.new_aligned_instance(self.shader_id);
        let geom = cx.walk_turtle(w, h, margin, None);
        
        let data = [
            /*x,y,w,h*/geom.x,geom.y,geom.w,geom.h,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];
        area.push_data(cx, &data);

        area
    }

    pub fn draw_quad(&mut self, cx:&mut Cx, x:f32, y:f32, w:f32, h:f32)->Area{
        let area = cx.new_aligned_instance(self.shader_id);
        //println!("{:?} {}", area, cx.current_draw_list_id);
        let pos = cx.turtle_origin();
        let data = [
            /*x,y,w,h*/pos.x+x,pos.y+y,w,h,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];
        area.push_data(cx, &data);
        area
    }

}