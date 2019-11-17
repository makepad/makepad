use render::*;
use crate::normalbutton::*;

theme_text_style!(TextStyle_normal);
theme_text_style!(TextStyle_tab_title);
theme_text_style!(TextStyle_button_title);
theme_text_style!(TextStyle_window_caption);
theme_text_style!(TextStyle_window_menu);

theme_color!(Color_bg_splitter);
theme_color!(Color_bg_splitter_over);
theme_color!(Color_bg_splitter_peak);
theme_color!(Color_bg_splitter_drag);

theme_color!(Color_scrollbar_base);
theme_color!(Color_scrollbar_over);
theme_color!(Color_scrollbar_down);

theme_color!(Color_bg_normal);
theme_color!(Color_bg_selected);
theme_color!(Color_bg_odd);
theme_color!(Color_bg_selected_over);
theme_color!(Color_bg_odd_over);
theme_color!(Color_bg_marked);
theme_color!(Color_bg_marked_over);
theme_color!(Color_over_border);
theme_color!(Color_icon);
theme_color!(Color_drop_quad);

theme_color!(Color_text_focus);
theme_color!(Color_text_defocus);
theme_color!(Color_text_selected_focus);
theme_color!(Color_text_deselected_focus);
theme_color!(Color_text_selected_defocus);
theme_color!(Color_text_deselected_defocus);


theme_layout!(Tab_layout_bg);

theme_walk!(TabClose_walk);

theme_layout!(WindowMenu_layout);


pub fn set_widget_theme_values(cx: &mut Cx) {
    
    let default_text = TextStyle {
        font_path: "resources/Ubuntu-R.ttf".to_string(),
        font_id: None,
        font_size: 8.0,
        brightness: 1.0,
        curve: 0.7,
        line_spacing: 1.4,
        top_drop: 1.1,
        height_factor: 1.3,
    };
    
    TextStyle_window_caption::set_base(cx, default_text.clone());
    TextStyle_window_menu::set_base(cx, default_text.clone());
    TextStyle_button_title::set_base(cx, default_text.clone());
    TextStyle_normal::set_base(cx, default_text.clone());
    TextStyle_tab_title::set_base(cx, default_text.clone());
    
    
    Tab_layout_bg::set_base(cx, Layout {
        align: Align::left_center(),
        walk: Walk::wh(Width::Compute, Height::Fix(40.)),
        padding: Padding {l: 16.0, t: 1.0, r: 16.0, b: 0.0},
        ..Default::default()
    });
    
    TabClose_walk::set_base(cx, Walk {
        width: Width::Fix(10.),
        height: Height::Fix(10.),
        margin: Margin {l: -4., t: 0., r: 4., b: 0.}
    });
    
    WindowMenu_layout::set_base(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fix(20.)),
        padding: Padding {l: 2., t: 3., b: 2., r: 0.},
        line_wrap: LineWrap::None,
        ..Default::default()
    });
    
    NormalButton::def_theme(cx);
    
}

pub fn set_dark_widget_theme(cx: &mut Cx) {
    Color_bg_splitter::set_base(cx, color256(25, 25, 25));
    Color_bg_splitter_over::set_base(cx, color("#5"));
    Color_bg_splitter_peak::set_base(cx, color("#f"));
    Color_bg_splitter_drag::set_base(cx, color("#6"));
    Color_scrollbar_base::set_base(cx, color("#5"));
    Color_scrollbar_over::set_base(cx, color("#7"));
    Color_scrollbar_down::set_base(cx, color("#9"));
    Color_bg_normal::set_base(cx, color256(52, 52, 52));
    Color_bg_selected::set_base(cx, color256(40, 40, 40));
    Color_bg_odd::set_base(cx, color256(37, 37, 37));
    Color_bg_selected_over::set_base(cx, color256(61, 61, 61));
    Color_bg_odd_over::set_base(cx, color256(56, 56, 56));
    Color_bg_marked::set_base(cx, color256(17, 70, 110));
    Color_bg_marked_over::set_base(cx, color256(17, 70, 110));
    Color_over_border::set_base(cx, color256(255, 255, 255));
    Color_drop_quad::set_base(cx, color("#a"));
    Color_text_defocus::set_base(cx, color("#9"));
    Color_text_focus::set_base(cx, color("#b"));
    Color_icon::set_base(cx, color256(127, 127, 127));
    
    Color_text_selected_focus::set_base(cx, color256(255, 255, 255));
    Color_text_deselected_focus::set_base(cx, color256(157, 157, 157));
    Color_text_selected_defocus::set_base(cx, color256(157, 157, 157));
    Color_text_deselected_defocus::set_base(cx, color256(130, 130, 130));
    
    set_widget_theme_values(cx);
}