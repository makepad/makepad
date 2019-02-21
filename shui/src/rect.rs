use crate::shader::*;
use crate::cx::*;
use crate::cxdrawing::*;

pub struct Quad{
    pub shader_id:usize,
    pub id:u32,
    pub color: Vec4
}

impl Style for Quad{
    fn style(cx:&mut Cx)->Self{
        let mut sh = Shader::def(); 
        Self::def_shader(&mut sh);
        Self{
            shader_id:cx.shaders.add(sh),
            id:0,
            color:color("green")
        }
    }
}

impl Quad{
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
                return vec4(pos*vec2(w, h)+vec2(x, y),0.,1.) * camera_projection;
            }

            fn pixel()->vec4{
                return vec4(color.rgb*color.a, color.a);
            }

        }));

        //sh.log =1;
    }

    // allocate the instance slot
    pub fn begin<'a>(&mut self, cx:&'a mut Cx, lay:&Lay)->&'a Draw{
        self.draw_abs(cx,0.0,0.0,0.0,0.0);
        cx.turtle.begin(lay);
        return cx.drawing.push_instance(); // store a ref to our instance
    }

    // write the rect instance
    pub fn end(&mut self, cx:&mut Cx){
        cx.drawing.pop_instance(&cx.shaders, cx.turtle.end());
    }

    pub fn set_uniforms(&mut self, dr:&mut Draw){
        if dr.first{
            dr.ufloat("fac", 1.0);
        }
    }

    pub fn draw_sized<'a>(&mut self, cx:&'a mut Cx, w:f32, h:f32, margin:Margin)->&'a mut Draw{
        let dr = cx.drawing.instance(cx.shaders.get(self.shader_id));
        self.set_uniforms(dr);
        
        let geom = cx.turtle.walk_wh(Value::Const(w), Value::Const(h), margin);

        dr.rect("x,y,w,h", geom);
        dr.vec4("color", &self.color);

        dr
    }

    pub fn draw_abs<'a>(&mut self, cx:&'a mut Cx, x:f32, y:f32, w:f32, h:f32)->&'a mut Draw{
        let dr = cx.drawing.instance(cx.shaders.get(self.shader_id));
        self.set_uniforms(dr);

        dr.float("x", x);
        dr.float("y", y);
        dr.float("w", w);
        dr.float("h", h);
        dr.vec4("color", &self.color);

        dr
    }
}