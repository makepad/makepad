use render::*;

#[derive(Clone)]
pub struct CodeIcon {
    pub quad: Quad,
    pub margin: Margin,
    pub width: f32,
    pub height: f32,
}

impl Style for CodeIcon {
    fn style(cx: &mut Cx) -> Self {
        Self {
            width: 14.0,
            height: 14.0,
            margin: Margin {l: 0., t: 0.5, r: 4., b: 0.},
            quad: Quad {
                shader: cx.add_shader(Self::def_code_icon_shader(), "CodeIcon"),
                ..Style::style(cx)
            }
        }
    }
}

pub enum CodeIconType {
    Warning,
    Error,
    Ok,
    Wait
}

impl CodeIconType {
    fn shader_float(&self) -> f32 {
        match self {
            CodeIconType::Warning => 1.,
            CodeIconType::Error => 2.,
            CodeIconType::Ok => 3.,
            CodeIconType::Wait => 4.,
        }
    }
}

impl CodeIcon {
    pub fn def_code_icon_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            let icon_id: float<Instance>;
            
            fn pixel() -> vec4 {
                let col = color;
                if abs(icon_id - 4.) < 0.1 { //Wait
                    df_viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                    df_circle(5., 5., 4.);
                    df_fill_keep(color("orange"));
                    df_stroke(color("gray"), 0.5);
                    df_move_to(3., 5.);
                    df_line_to(3., 5.);
                    df_move_to(5., 5.);
                    df_line_to(5., 5.);
                    df_move_to(7., 5.);
                    df_line_to(7., 5.);
                    df_stroke(color("black"), 0.8);
                    return df_result;
                }
                if abs(icon_id - 3.) < 0.1 { //OK
                    df_viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                    df_circle(5., 5., 4.);
                    df_fill_keep(color("#555"));
                    df_stroke(color("#555"), 0.5);
                    let sz = 1.;
                    df_move_to(5., 5.);
                    df_line_to(5., 5.);
                    df_stroke(color("#aaa"), 0.8);
                    return df_result;
                }
                else if abs(icon_id - 2.) < 0.1 { // Error
                    df_viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                    df_circle(5., 5., 4.);
                    df_fill_keep(color("#c00"));
                    df_stroke(color("gray"), 0.5);
                    let sz = 1.;
                    df_move_to(5. - sz, 5. - sz);
                    df_line_to(5. + sz, 5. + sz);
                    df_move_to(5. - sz, 5. + sz);
                    df_line_to(5. + sz, 5. - sz);
                    df_stroke(color("black"), 0.8);
                    return df_result;
                }
                else { // Warning
                    df_viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                    df_move_to(5., 1.);
                    df_line_to(9., 9.);
                    df_line_to(1., 9.);
                    df_close_path();
                    df_fill_keep(vec4(253.0 / 255.0, 205.0 / 255.0, 59.0 / 255.0, 1.0));
                    df_stroke(color("gray"), 0.5);
                    df_move_to(5., 3.5);
                    df_line_to(5., 5.25);
                    df_stroke(color("black"), 0.8);
                    df_move_to(5., 7.25);
                    df_line_to(5., 7.5);
                    df_stroke(color("black"), 0.8);
                    return df_result;
                }
            }
        }))
    }
    
    pub fn draw_icon_abs(&mut self, cx: &mut Cx, x: f32, y: f32, icon_type: CodeIconType) -> InstanceArea {
        let inst = self.quad.draw_quad_abs(cx, Rect {x: x, y: y, w: self.width, h: self.height});
        inst.push_float(cx, icon_type.shader_float());
        inst
    }
    
    pub fn draw_icon_walk(&mut self, cx: &mut Cx, icon_type: CodeIconType) -> InstanceArea {
        let geom = cx.walk_turtle(Bounds::Fix(self.width), Bounds::Fix(self.height), self.margin, None);
        self.draw_icon_abs(cx, geom.x, geom.y, icon_type)
    }
    
}
