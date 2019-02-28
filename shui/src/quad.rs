use crate::shader::*;
use crate::cx_shared::*;
use crate::cxdrawing_shared::*;
use crate::cxturtle::*;
use crate::area::*;

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
            shader_id:cx.drawing.add_shader(sh),
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

    pub fn begin(&mut self, cx:&mut Cx, layout:&Layout)->Area{
        let area = self.draw_abs(cx, true, 0.0,0.0,0.0,0.0);
        cx.begin_instance(&area, layout);
        area
    }

    // write the rect instance
    pub fn end(&mut self, cx:&mut Cx)->Area{
        cx.end_instance()
    }

    pub fn set_uniforms(&mut self, cd:&mut CxDrawing, area:&Area){
        if area.need_uniforms_now(cd){
            area.uniform_float(cd, "fac", 1.0);
        }
    }

    pub fn draw_sized(&mut self, cx:&mut Cx, w:Value, h:Value, margin:Margin)->Area{
        let area = cx.new_aligned_instance(self.shader_id);
        let cd = &mut cx.drawing;

        self.set_uniforms(cd, &area);
        
        let geom = cx.turtle.walk_wh(w, h, margin, None);
        
        // lets store our instance onto the turtle
        let data = [
            /*x,y,w,h*/geom.x,geom.y,geom.w,geom.h,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];
        area.append_data(cd, &data);

        area
    }

    pub fn draw_abs(&mut self, cx:&mut Cx, align:bool, x:f32, y:f32, w:f32, h:f32)->Area{
        let area = if align{
           cx.new_aligned_instance(self.shader_id)
        }
        else{
           cx.new_instance(self.shader_id)
        };

        let cd = &mut cx.drawing;
        self.set_uniforms(cd, &area);

        let data = [
            /*x,y,w,h*/x,y,w,h,
            /*color*/self.color.x,self.color.y,self.color.z,self.color.w
        ];
        area.append_data(cd, &data);
        area
    }
}