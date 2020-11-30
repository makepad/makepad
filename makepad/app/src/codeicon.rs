use makepad_render::*;

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawCodeIcon {
    #[default_shader(self::shader_code_icon)]
    base: DrawQuad,
    icon_type: f32,
}

#[derive(Clone)]
pub struct CodeIcon {
    pub code_icon: DrawCodeIcon,
}

pub enum CodeIconType {
    Panic,
    Warning,
    Error,
    Ok,
    Wait
}

impl CodeIconType {
    fn shader_float(&self) -> f32 {
        match self {
            CodeIconType::Panic => 1.,
            CodeIconType::Warning => 2.,
            CodeIconType::Error => 3.,
            CodeIconType::Ok => 4.,
            CodeIconType::Wait => 5.,
        }
    }
}

impl CodeIcon {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            code_icon: DrawCodeIcon::new(cx, default_shader!())
        }
    }
    
    pub fn style(cx: &mut Cx) {
        self::DrawCodeIcon::register_draw_input(cx);
        live_body!(cx, r#"
            self::walk: Walk {
                width: Fix(14.0),
                height: Fix(14.0),
                margin: {l: 0., t: 0.5, r: 4., b: 0.},
            }
            self::shader_code_icon: Shader {
                use makepad_render::drawquad::shader::*;
                
                draw_input: self::DrawCodeIcon;
                
                fn pixel() -> vec4 {
                    if abs(icon_type - 5.) < 0.1 { //Wait
                        let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                        df.circle(5., 5., 4.);
                        df.fill_keep(#ffa500);
                        df.stroke(#be, 0.5);
                        df.move_to(3., 5.);
                        df.line_to(3., 5.);
                        df.move_to(5., 5.);
                        df.line_to(5., 5.);
                        df.move_to(7., 5.);
                        df.line_to(7., 5.);
                        df.stroke(#0, 0.8);
                        return df.result;
                    }
                    if abs(icon_type - 4.) < 0.1 { //OK
                        let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                        df.circle(5., 5., 4.);
                        df.fill_keep(#5);
                        df.stroke(#5, 0.5);
                        let sz = 1.;
                        df.move_to(5., 5.);
                        df.line_to(5., 5.);
                        df.stroke(#a, 0.8);
                        return df.result;
                    }
                    else if abs(icon_type - 3.) < 0.1 { // Error
                        let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                        df.circle(5., 5., 4.);
                        df.fill_keep(#c00);
                        df.stroke(#be, 0.5);
                        let sz = 1.;
                        df.move_to(5. - sz, 5. - sz);
                        df.line_to(5. + sz, 5. + sz);
                        df.move_to(5. - sz, 5. + sz);
                        df.line_to(5. + sz, 5. - sz);
                        df.stroke(#0, 0.8);
                        return df.result;
                    }
                    else if abs(icon_type - 2.) < 0.1 { // Warning
                        let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                        df.move_to(5., 1.);
                        df.line_to(9., 9.);
                        df.line_to(1., 9.);
                        df.close_path();
                        df.fill_keep(vec4(253.0 / 255.0, 205.0 / 255.0, 59.0 / 255.0, 1.0));
                        df.stroke(#be, 0.5);
                        df.move_to(5., 3.5);
                        df.line_to(5., 5.25);
                        df.stroke(#0, 0.8);
                        df.move_to(5., 7.25);
                        df.line_to(5., 7.5);
                        df.stroke(#0, 0.8);
                        return df.result;
                    }
                    else { // Panic
                        let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                        df.move_to(5., 1.);
                        df.line_to(9., 9.);
                        df.line_to(1., 9.);
                        df.close_path();
                        df.fill_keep(#c00);
                        df.stroke(#be, 0.5);
                        let sz = 1.;
                        df.move_to(5. - sz, 6.25 - sz);
                        df.line_to(5. + sz, 6.25 + sz);
                        df.move_to(5. - sz, 6.25 + sz);
                        df.line_to(5. + sz, 6.25 - sz);
                        df.stroke(#f, 0.8);
                        
                        return df.result;
                    }
                }
            }
            
        "#)
    }
    
    pub fn draw_icon(&mut self, cx: &mut Cx, icon_type: CodeIconType) {
        self.code_icon.icon_type = icon_type.shader_float();
        self.code_icon.draw_quad_walk(cx, live_walk!(cx, self::walk));
    }
    
}
