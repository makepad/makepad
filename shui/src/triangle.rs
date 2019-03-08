use crate::cx::*;

#[derive(Clone)]
pub struct Triangle{
    pub shader_id:usize,
    pub id:u32,
    pub color: Vec4
}

impl Style for Triangle{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_triangle_shader(cx);
        Self{
            shader_id:cx.add_shader(sh),
            id:0,
            color:color("green")
        }
    }
}

impl Triangle{
    pub fn def_triangle_shader(cx:&mut Cx)->Shader{
        // lets add the draw shader lib
        let mut sh = cx.new_shader();

        sh.geometry_vertices = vec![
            0.0,
            1.0,
            2.0
        ];
        sh.geometry_indices = vec![
            0,1,2,
        ];

        sh.add_ast(shader_ast!({
            
            let geom:float<Geometry>;

            let x1:float<Instance>;
            let y1:float<Instance>;
            let x2:float<Instance>;
            let y2:float<Instance>;
            let x3:float<Instance>;
            let y3:float<Instance>;
            let color:vec4<Instance>;

            fn vertex()->vec4{
                let shift:vec2 = -draw_list_scroll;
                let pos:vec2 = 
                    mix(vec2(x1, y1),
                        mix(
                            vec2(x2,y2),
                            vec2(x3,y3),
                            clamp(geom - 1., 0., 1.)
                        ),
                        clamp(geom, 0., 1.)
                    ) + shift;

                return vec4(pos,0.,1.) * camera_projection;
            }

            fn pixel()->vec4{
                return vec4(color.rgb*color.a, color.a);
            }

        }));
        sh
    }

    pub fn draw_triangle(&mut self, cx:&mut Cx, x1:f32, y1:f32, x2:f32, y2:f32, x3:f32, y3:f32)->Area{
        let area = cx.new_aligned_instance(self.shader_id);
        let pos = cx.turtle_origin();
        let data = [
            /*x,y,w,h*/pos.x+x1,pos.y+y1,pos.x+x2,pos.y+y2,pos.x+x3,pos.y+y3,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];
        area.push_data(cx, &data);
        area
    }
}