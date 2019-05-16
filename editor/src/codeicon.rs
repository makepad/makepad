use widget::*;

#[derive(Clone)]
pub struct CodeIcon{
    pub quad:Quad,
    pub margin:Margin,
    pub width:f32,
    pub height:f32,
}

impl Style for CodeIcon{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_code_icon_shader(cx);
        Self{
            width:14.0,
            height:14.0,
            margin:Margin{l:0.,t:1.,r:4.,b:0.},
            quad:Quad{
                shader_id:cx.add_shader(sh, "CodeIcon"),
                ..Style::style(cx)
            }
        }
    }
}

pub enum CodeIconType{
    Warning,
    Error
}

impl CodeIconType{
    fn shader_float(&self)->f32{
        match self{
            CodeIconType::Warning=>1.,
            CodeIconType::Error=>2.,
        }
    }
}

impl CodeIcon{
    pub fn def_code_icon_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({
            let icon_id:float<Instance>;

            fn pixel()->vec4{
                let col = color;
                // lets draw a triangle
                if abs(icon_id - 2.) < 0.1{
                    df_viewport(pos*vec2(10.));// * vec2(w, h));
                    df_circle(5.,5.,4.);
                    df_fill_keep(color("#c00"));
                    df_stroke(color("gray"),0.5);
                    let sz = 1.;
                    df_move_to(5.-sz,5.-sz);
                    df_line_to(5.+sz,5.+sz);
                    df_move_to(5.-sz,5.+sz);
                    df_line_to(5.+sz,5.-sz);
                    df_stroke(color("black"),0.8);
                    return df_result;
                }
                else if abs(icon_id - 1.) < 0.1{
                    df_viewport(pos*vec2(10.));// * vec2(w, h));
                    df_move_to(5.,1.);
                    df_line_to(9.,9.);
                    df_line_to(1.,9.);
                    df_close_path();
                    df_fill_keep(vec4(253.0/255.0,205.0/255.0,59.0/255.0,1.0));
                    df_stroke(color("gray"),0.5);
                    df_move_to(5.,3.5);
                    df_line_to(5.,5.25);
                    df_stroke(color("black"),0.8);
                    df_move_to(5.,7.25);
                    df_line_to(5.,7.5);
                    df_stroke(color("black"),0.8);
                    return df_result;
                }

                //return df_stroke(color("white"),1.);
                
                //return vec4(df_clip*0.1+0.5,0.0,0.0,1.0);//df_hsv2rgb(vec4(pos.x,0.5,0.3,1.0));
               // df_fill_keep(color("red"));
                //return df_stroke(color("white"), 1.0);
/*  
                let thickness =  0.8 + dpi_dilate*0.5;
                if abs(indent_id - indent_sel) < 0.1{
                    col *= vec4(1.25);
                    thickness *= 1.3;
                };
                df_viewport(pos * vec2(w, h));
                df_move_to(1.,-1.);
                df_line_to(1.,h+1.);
                return df_stroke(col,thickness);*/
            }

        }));
        sh
    }

    pub fn draw_icon_abs(&mut self, cx:&mut Cx, x:f32, y:f32, icon_type:CodeIconType)->InstanceArea{
        let inst = self.quad.draw_quad_abs(cx, Rect{x:x, y:y, w:self.width, h:self.height});
        inst.push_float(cx, icon_type.shader_float());
        inst
    }

    pub fn draw_icon_walk(&mut self, cx:&mut Cx, icon_type:CodeIconType)->InstanceArea{
        let geom = cx.walk_turtle(Bounds::Fix(self.width),Bounds::Fix(self.height), self.margin, None);
        self.draw_icon_abs(cx, geom.x, geom.y, icon_type)
    }

}
