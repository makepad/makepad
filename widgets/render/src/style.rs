use std::collections::BTreeMap;
use crate::cx::*;

#[derive(Clone)]
pub struct StyleSheet{
    pub normal_font:String,
    pub icon_font:String,
    pub font_size:f32,

    pub text_select:Vec4,

    pub accent_normal:Vec4,
    pub accent_down:Vec4,
    pub accent_gray:Vec4,

    pub bg_top:Vec4,
    pub bg_normal:Vec4,
    pub bg_hi:Vec4,

    pub text_normal:Vec4,
    pub text_accent:Vec4,
    pub text_med:Vec4,
    pub text_hi:Vec4,
    pub text_lo:Vec4,

    pub code_bg:Vec4,
    pub code_class:Vec4,
    pub code_object:Vec4,
    pub code_paren:Vec4,
    pub code_array:Vec4,
    pub code_function:Vec4,
    pub code_call:Vec4,
    pub code_if:Vec4,
    pub code_loop:Vec4,
    pub code_comment:Vec4,
    pub code_exception:Vec4,
    pub code_var:Vec4,
    pub code_let:Vec4,
    pub code_const:Vec4,
    pub code_global:Vec4,
    pub code_arg:Vec4,
    pub code_unknown:Vec4,
    pub code_operator:Vec4,
    pub code_number:Vec4,
    pub code_boolean:Vec4,
    pub code_string:Vec4,
    pub code_tok_exception:Vec4,
    pub code_log:Vec4,
    
    pub color1:Vec4,
    pub color2:Vec4,
    pub color3:Vec4,
    pub color4:Vec4,
    pub color5:Vec4,
    pub color6:Vec4,
    pub color7:Vec4,
    pub color8:Vec4,
    pub color9:Vec4,
    pub color10:Vec4,
    
    pub color:BTreeMap<String,Vec4>,
    pub font:BTreeMap<String,String>,
    pub size:BTreeMap<String,f64>
}

impl Default for StyleSheet{
    fn default()->StyleSheet{
        StyleSheet{
            normal_font:"resources/ubuntu_regular_256.font".to_string(),
            icon_font:"resources/fontawesome.font".to_string(),
            font_size:11.0,

            text_select:color("Purple900"),

            accent_normal:color("Purple900"),
            accent_down:color("Purple500"),
            accent_gray:color("Grey700"),

            bg_top:color("Grey900"),
            bg_normal:color("Grey850"),
            bg_hi:color("Grey800"),

            text_normal:color("Grey300"),
            text_accent:color("Grey400"),
            text_med:color("Grey500"),
            text_hi:color("Grey300"),
            text_lo:color("Grey700"),

            code_bg:color("Grey900"),
            code_class:color("Pink300"),
            code_object:color("Indigo200"),
            code_paren:color("BlueGrey400"),
            code_array:color("Cyan300"),
            code_function:color("Amber300"),
            code_call:color("Yellow300"),
            code_if:color("LightGreen300"),
            code_loop:color("DeepOrange300"),
            code_comment:color("Blue700"),
            code_exception:color("Red400"),
            code_var:color("BlueGrey200"),
            code_let:color("BlueGrey100"),
            code_const:color("BlueGrey400"),
            code_global:color("YellowA100"),
            code_arg:color("BlueGrey500"),
            code_unknown:color("White"),
            code_operator:color("Amber300"),
            code_number:color("IndigoA100"),
            code_boolean:color("Red400"),
            code_string:color("GreenA200"),
            code_tok_exception:color("red"),
            code_log:color("yellow"),
            color1:color("red"),
            color2:color("red"),
            color3:color("red"),
            color4:color("red"),
            color5:color("red"),
            color6:color("red"),
            color7:color("red"),
            color8:color("red"),
            color9:color("red"),
            color10:color("red"),
            color:BTreeMap::new(),
            font:BTreeMap::new(),
            size:BTreeMap::new(),
        }
    }
}