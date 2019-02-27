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

            fn vertex()->vec4{
                return vec4(pos*vec2(w, h)+vec2(x, y),0.,1.) * camera_projection;
            }

            fn pixel()->vec4{
                return vec4(color.rgb*color.a, color.a);
            }

        }));

        //sh.log =1;
    }

    pub fn begin<'a>(&mut self, cx:&'a mut Cx, layout:&Layout)->&'a mut DrawCall{
        let draw_call_id = self.draw_abs(cx, true, 0.0,0.0,0.0,0.0).draw_call_id;
        cx.turtle.begin(layout);
        cx.drawing.push_instance(draw_call_id)
    }

    // write the rect instance
    pub fn end(&mut self, cx:&mut Cx)->Area{
        let rect = cx.turtle.end(&mut cx.drawing ,&cx.shaders);
        cx.drawing.pop_instance(&cx.shaders, rect)
    }

    pub fn set_uniforms(&mut self, dc:&mut DrawCall){
        if dc.first{
            dc.ufloat("fac", 1.0);
        }
    }

    pub fn draw_sized<'a>(&mut self, cx:&'a mut Cx, w:Value, h:Value, margin:Margin)->&'a mut DrawCall{
        
        let dc = cx.drawing.instance_aligned(cx.shaders.get(self.shader_id), &mut cx.turtle);
        
        self.set_uniforms(dc);
        
        let geom = cx.turtle.walk_wh(w, h, margin, None);
        
        // lets store our instance onto the turtle
        let data = [
            /*x,y,w,h*/geom.x,geom.y,geom.w,geom.h,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];

        dc.instance.extend_from_slice(&data);

        dc
    }

    pub fn draw_abs<'a>(&mut self, cx:&'a mut Cx, align:bool, x:f32, y:f32, w:f32, h:f32)->&'a mut DrawCall{
        let dc = if align{
           cx.drawing.instance_aligned(cx.shaders.get(self.shader_id), &mut cx.turtle)
        }
        else{
           cx.drawing.instance(cx.shaders.get(self.shader_id))
        };
        self.set_uniforms(dc);

        let data = [
            /*x,y,w,h*/x,y,w,h,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];
        dc.instance.extend_from_slice(&data);

        dc
    }
}