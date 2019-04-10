use render::*;

pub fn set_dark_style(cx:&mut Cx){
    
    cx.set_font("normal_font", "resources/ubuntu_regular_256.font");
    cx.set_font("mono_font", "resources/liberation_mono_regular_256.font");
    
    //cx.set_font("mono_font", "resources/source_code_pro_medium_256.font");
    //cx.set_font("mono_font", "resources/monofur_55_256.font");
    
    //cx.set_font("mono_font", "resources/inconsolata_regular_256.font");
    cx.set_font("icon_font", "resources/fontawesome.font");
    cx.set_size("font_size", 11.0);

    cx.set_color("bg_split", color256(25,25,25));

    cx.set_color("bg_selected", color256(40,40,40));
    cx.set_color("bg_odd", color256(37,37,37));

    cx.set_color("bg_normal", color256(52,52,52));

    cx.set_color("bg_selected_over", color256(61,61,61));
    cx.set_color("bg_odd_over", color256(56,56,56));

    cx.set_color("bg_marked", color256(17,70,110));
    cx.set_color("bg_marked_over", color256(17,70,110));
    cx.set_color("over_border", color256(255,255,255));

    cx.set_color("icon_color", color256(127,127,127));

    cx.set_color("text_selected_focus", color256(255,255,255));
    cx.set_color("text_deselected_focus", color256(157,157,157));
    cx.set_color("text_selected_defocus", color256(157,157,157));
    cx.set_color("text_deselected_defocus", color256(130,130,130));

    //cx.set_color("text_select",color("Purple900"));
    //cx.set_color("accent_normal", color("Purple900"));
    //cx.set_color("accent_down", color("Purple500"));
    //cx.set_color("accent_gray", color("Grey700"));

    //cx.set_color("bg_top", color("Grey900"));
    //cx.set_color("bg_normal", color("Grey850"));
    //cx.set_color("bg_hi", color("Grey800"));

    //cx.set_color("text_normal", color("Grey300"));
    //cx.set_color("text_accent", color("Grey400"));
    //cx.set_color("text_med", color("Grey500"));
    //cx.set_color("text_hi", color("Grey300"));
    //cx.set_color("text_lo", color("Grey700"));

    cx.set_color("code_bg", color("Grey900"));
    cx.set_color("code_class", color("Pink300"));
    cx.set_color("code_object", color("Indigo200"));
    cx.set_color("code_paren", color("BlueGrey400"));
    cx.set_color("code_array", color("Cyan300"));
    cx.set_color("code_function", color("Amber300"));
    cx.set_color("code_call", color("Yellow300"));
    cx.set_color("code_if", color("LightGreen300"));
    cx.set_color("code_loop", color("DeepOrange300"));
    cx.set_color("code_comment", color("Blue700"));
    cx.set_color("code_exception", color("Red400"));
    cx.set_color("code_var", color("BlueGrey200"));
    cx.set_color("code_let", color("BlueGrey100"));
    cx.set_color("code_const", color("BlueGrey400"));
    cx.set_color("code_global", color("YellowA100"));
    cx.set_color("code_arg", color("BlueGrey500"));
    cx.set_color("code_unknown", color("White"));
    cx.set_color("code_operator", color("Amber300"));
    cx.set_color("code_number", color("IndigoA100"));
    cx.set_color("code_boolean", color("Red400"));
    cx.set_color("code_string", color("GreenA200"));
    cx.set_color("code_tok_exception", color("red"));
    cx.set_color("code_log", color("yellow"));
}
