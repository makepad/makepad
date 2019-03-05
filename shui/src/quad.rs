use crate::cx::*;

#[derive(Clone)]
pub struct Quad{
    pub shader_id:usize,
    pub id:u32,
    pub color: Vec4
}

impl Style for Quad{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_shader(cx);
        Self{
            shader_id:cx.add_shader(sh),
            id:0,
            color:color("green")
        }
    }
}

impl Quad{
    pub fn def_shader(cx:&mut Cx)->Shader{
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

    pub fn begin(&mut self, cx:&mut Cx, layout:&Layout)->Area{
        let area = self.draw_abs(cx, true, 0.0,0.0,0.0,0.0);
        cx.begin_instance(area, layout);
        area
    }

    // write the rect instance
    pub fn end(&mut self, cx:&mut Cx)->Area{
        cx.end_instance()
    }

    pub fn draw_sized(&mut self, cx:&mut Cx, w:Value, h:Value, margin:Margin)->Area{
        let area = cx.new_aligned_instance(self.shader_id);

        let geom = cx.walk_turtle(w, h, margin, None);
        
        // lets store our instance onto the turtle
        let data = [
            /*x,y,w,h*/geom.x,geom.y,geom.w,geom.h,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];
        area.push_data(cx, &data);

        area
    }

    pub fn draw_abs(&mut self, cx:&mut Cx, align:bool, x:f32, y:f32, w:f32, h:f32)->Area{
        let area = if align{
           cx.new_aligned_instance(self.shader_id)
        }
        else{
           cx.new_instance(self.shader_id)
        };

        let data = [
            /*x,y,w,h*/x,y,w,h,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];
        area.push_data(cx, &data);
        area
    }
}