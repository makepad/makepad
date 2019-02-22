use crate::shader::*;
use crate::cx::*;
use crate::cxdrawing::*;

#[derive(Clone)]
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

    pub fn begin<'a>(&mut self, cx:&'a mut Cx, layout:&Layout)->&'a mut Draw{
        let draw_id = self.draw_abs(cx, true, 0.0,0.0,0.0,0.0).draw_id;
        
        cx.begin_instance(layout, draw_id)
    }

    // write the rect instance
    pub fn end(&mut self, cx:&mut Cx)->Area{
        cx.end_instance()
    }

    pub fn set_uniforms(&mut self, dc:&mut Draw){
        if dc.first{
            dc.ufloat("fac", 1.0);
        }
    }

    pub fn draw_sized<'a>(&mut self, cx:&'a mut Cx, w:Value, h:Value, margin:Margin)->&'a mut Draw{
        
        let dc = cx.drawing.instance_aligned(cx.shaders.get(self.shader_id), &mut cx.turtle);
        
        self.set_uniforms(dc);
        
        let geom = cx.turtle.walk_wh(w, h, margin, None);
        
        // lets store our instance onto the turtle

        dc.rect("x,y,w,h", geom);
        dc.vec4("color", &self.color);

        dc
    }

    pub fn draw_abs<'a>(&mut self, cx:&'a mut Cx, align:bool, x:f32, y:f32, w:f32, h:f32)->&'a mut Draw{
        let dc = if align{
           cx.drawing.instance_aligned(cx.shaders.get(self.shader_id), &mut cx.turtle)
        }
        else{
           cx.drawing.instance(cx.shaders.get(self.shader_id))
        };
        self.set_uniforms(dc);

        dc.float("x", x);
        dc.float("y", y);
        dc.float("w", w);
        dc.float("h", h);
        dc.vec4("color", &self.color);

        dc
    }
}