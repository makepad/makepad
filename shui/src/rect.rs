use crate::math::*;
use crate::shader::*;
use crate::cx::*;
use crate::cxdrawing::*;

pub struct Rect{
    pub shader_id:usize,
    pub id:u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: Vec4
}

impl Style for Rect{
    fn style(cx:&mut Cx)->Self{
        let mut sh = Shader::def(); 
        Self::def_shader(&mut sh);
        Self{
            shader_id:cx.shaders.add(sh),
            id:0,
            x:0.0,
            y:0.0,
            w:1.0,
            h:1.0,
            color:Vec4{x:1.0,y:0.0,z:0.0,w:0.0}
        }
    }
}

impl Rect{
    pub fn def_shader(sh: &mut Shader){
        // lets add the draw shader lib
        Cx::def_shader(sh);

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

        sh.add_ast(shader_ast!(||{
            
            let pos:vec2<Geometry>;
            let fac:float<Uniform>;
            let x:float<Instance>;
            let y:float<Instance>;
            let w:float<Instance>;
            let h:float<Instance>;
            let color:vec4<Instance>;
            
            fn my_fn(inv:vec4)->vec4{
                return inv;
            }

            fn vertex()->vec4{
                return vec4(pos*vec2(w, h)+vec2(x, y),0.,1.);
            }

            fn pixel()->vec4{
                return vec4(color.rgb*fac, fac);
            }

        }));

        //sh.log =1;
    }

    pub fn draw_at<'a>(&mut self, cx:&'a mut Cx, x:f32, y:f32, w:f32, h:f32)->&'a mut Draw{
        let dr = cx.drawing.instance(cx.shaders.get(self.shader_id));

        if dr.first{
            dr.ufloat("fac", 1.0);
        }

        dr.float("x", x);
        dr.float("y", y);
        dr.float("w", w);
        dr.float("h", h);
        dr.vec4("color", &self.color);
        dr
    }
}