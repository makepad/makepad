use crate::cx::*;
use crate::drawquad::DrawQuad;

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawColor {
    #[default_shader(self::shader)]
    pub base: DrawQuad,
    pub color: Vec4,
}

impl DrawColor{
    
    pub fn style(cx:&mut Cx){
        Self::register_draw_input(cx);
        live_body!(cx, r#"
            self::shader: Shader {
                use crate::drawquad::shader::*;
                draw_input: self::DrawColor;
                fn pixel() -> vec4 {
                    return vec4(color.rgb*color.a, color.a);
                }
            }
        "#);
    }
}
