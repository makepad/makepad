use makepad_render::*;
use makepad_widget::*;

#[derive(Clone)]
pub struct CodeIcon {
    pub quad: Quad,
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
            quad: Quad {
                shader: cx.add_shader(Self::def_code_icon_shader(), "CodeIcon"),
                ..Quad::new(cx)
            }
        }
    }
    
    pub fn walk()->WalkId{uid!()}
    
    pub fn style(cx:&mut Cx, opt:&StyleOptions){
        Self::walk().set(cx, Walk{
            width: Width::Fix(14.0*opt.scale),
            height: Height::Fix(14.0*opt.scale),
            margin: Margin {l: 0., t: 0.5, r: 4., b: 0.},
        })
    }
    
    pub fn icon_id()->FloatId{uid!()}
    
    pub fn def_code_icon_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader!{"
            instance icon_id: Self::icon_id();
            
            fn pixel() -> vec4 {
                let col = color;
                if abs(icon_id - 5.) < 0.1 { //Wait
                    let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                    df.circle(5., 5., 4.);
                    df.fill_keep(pick!(orange));
                    df.stroke(pick!(gray), 0.5);
                    df.move_to(3., 5.);
                    df.line_to(3., 5.);
                    df.move_to(5., 5.);
                    df.line_to(5., 5.);
                    df.move_to(7., 5.);
                    df.line_to(7., 5.);
                    df.stroke(pick!(black), 0.8);
                    return df.result;
                }
                if abs(icon_id - 4.) < 0.1 { //OK
                    let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                    df.circle(5., 5., 4.);
                    df.fill_keep(pick!(#555));
                    df.stroke(pick!(#555), 0.5);
                    let sz = 1.;
                    df.move_to(5., 5.);
                    df.line_to(5., 5.);
                    df.stroke(pick!(#aaa), 0.8);
                    return df.result;
                }
                else if abs(icon_id - 3.) < 0.1 { // Error
                    let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                    df.circle(5., 5., 4.);
                    df.fill_keep(pick!(#c00));
                    df.stroke(pick!(gray), 0.5);
                    let sz = 1.;
                    df.move_to(5. - sz, 5. - sz);
                    df.line_to(5. + sz, 5. + sz);
                    df.move_to(5. - sz, 5. + sz);
                    df.line_to(5. + sz, 5. - sz);
                    df.stroke(pick!(black), 0.8);
                    return df.result;
                }
                else if abs(icon_id - 2.) < 0.1 { // Warning
                    let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                    df.move_to(5., 1.);
                    df.line_to(9., 9.);
                    df.line_to(1., 9.);
                    df.close_path();
                    df.fill_keep(vec4(253.0 / 255.0, 205.0 / 255.0, 59.0 / 255.0, 1.0));
                    df.stroke(pick!(gray), 0.5);
                    df.move_to(5., 3.5);
                    df.line_to(5., 5.25);
                    df.stroke(pick!(black), 0.8);
                    df.move_to(5., 7.25);
                    df.line_to(5., 7.5);
                    df.stroke(pick!(black), 0.8);
                    return df.result;
                }
                else { // Panic
                    let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                    df.move_to(5., 1.);
                    df.line_to(9., 9.);
                    df.line_to(1., 9.);
                    df.close_path();
                    df.fill_keep(pick!(#c00));
                    df.stroke(pick!(gray), 0.5);
                    let sz = 1.;
                    df.move_to(5. - sz, 6.25 - sz);
                    df.line_to(5. + sz, 6.25 + sz);
                    df.move_to(5. - sz, 6.25 + sz);
                    df.line_to(5. + sz, 6.25 - sz);
                    df.stroke(pick!(white), 0.8);

                    return df.result;
                }
            }
        "})
    }
    /*
    pub fn draw_icon_abs(&mut self, cx: &mut Cx, x: f32, y: f32, icon_type: CodeIconType) -> InstanceArea {
        let inst = self.quad.draw_quad_abs(cx, Rect {x: x, y: y, w: self.width, h: self.height});
        inst.push_float(cx, icon_type.shader_float());
        inst
    }*/
    
    pub fn draw_icon(&mut self, cx: &mut Cx, icon_type: CodeIconType) -> InstanceArea {
        
        let inst = self.quad.draw_quad(cx, Self::walk().get(cx));
        inst.push_float(cx, icon_type.shader_float());
        inst
    }
    
}
