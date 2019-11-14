use render::*;

// the default values are dark theme and with defaults
theme_color!(ThemeBgSplit, color256(25, 25, 25));
theme_color!(ThemeBgNormal, color256(52, 52, 52));

//theme_text!(ThemeNormalText);
//theme_text!(ThemeMonoText);

pub fn set_theme_defaults(cx: &mut Cx) {
    // set text_styles
    cx.set_font("normal_font", "resources/Ubunutu-R.ttf");
    cx.set_font("mono_font", "resources/LiberationMono-Regular.ttf");
    // set layouts
    // set loose values
    
}

pub fn set_theme_values(cx: &mut Cx){
    // do button layout and 
}

pub fn set_dark_theme(cx: &mut Cx) {
    cx.set_color::<ThemeBgSplit>(color256(25, 25, 25));
    
    cx.set_color("bg_split", color256(25, 25, 25));
    theme_color!(ThemeBgSelected, color256(40, 40, 40));
    cx.set_color("bg_odd", color256(37, 37, 37));
    
    cx.set_color("bg_normal", color256(52, 52, 52));
    
    cx.set_color("bg_selected_over", color256(61, 61, 61));
    cx.set_color("bg_odd_over", color256(56, 56, 56));
    
    cx.set_color("bg_marked", color256(17, 70, 110));
    cx.set_color("bg_marked_over", color256(17, 70, 110));
    cx.set_color("over_border", color256(255, 255, 255));
    
    cx.set_color("icon_color", color256(127, 127, 127));
    
    cx.set_color("text_selected_focus", color256(255, 255, 255));
    cx.set_color("text_deselected_focus", color256(157, 157, 157));
    cx.set_color("text_selected_defocus", color256(157, 157, 157));
    cx.set_color("text_deselected_defocus", color256(130, 130, 130));
}
