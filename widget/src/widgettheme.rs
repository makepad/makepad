use render::*;

theme_text_style!(TextStyle_normal);
theme_text_style!(TextStyle_tab_title);
theme_text_style!(TextStyle_button_title);
theme_text_style!(TextStyle_window_caption);
theme_text_style!(TextStyle_window_menu);

theme_layout!(TabLayout_bg);
theme_layout!(Layout_window_menu);

theme_walk!(Walk_tab_close);

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


// NormalButton styles


theme_layout!(NormalButton_layout_bg);
theme_anim!(NormalButton_anim_default);
theme_anim!(NormalButton_anim_over);
theme_anim!(NormalButton_anim_down);
theme_shader!(NormalButton_shader_bg);
instance_color!(NormalButton_border_color);
instance_float!(NormalButton_glow_size);

pub fn set_normal_button_values(cx:&mut Cx){
    NormalButton_layout_bg::set(cx, Layout {
        align: Align::center(),
        walk: Walk {
            width: Width::Compute,
            height: Height::Compute,
            margin: Margin::all(1.0),
        },
        padding: Padding {l: 16.0, t: 14.0, r: 16.0, b: 14.0},
        ..Default::default()
    });
    
    NormalButton_anim_default::set(cx, Anim::new(Play::Cut {duration: 0.5}, vec![
        Track::color_id(Quad_color::id(), Ease::Lin, vec![(1., Color_bg_normal::id())]),
        Track::float(NormalButton_glow_size::id(), Ease::Lin, vec![(1., 0.0)]),
        Track::color(NormalButton_border_color::id(), Ease::Lin, vec![(1., color("#6"))]),
    ]));
    
    NormalButton_anim_over::set(cx, Anim::new(Play::Cut {duration: 0.05}, vec![
        Track::color(Quad_color::id(), Ease::Lin, vec![(1., color("#999"))]),
        Track::float(NormalButton_glow_size::id(), Ease::Lin, vec![(1., 1.0)]),
        Track::color(NormalButton_border_color::id(), Ease::Lin, vec![(1., color("white"))]),
    ]));
    
    NormalButton_anim_down::set(cx, Anim::new(Play::Cut {duration: 0.2}, vec![
        Track::color(Quad_color::id(), Ease::Lin, vec![(0.0, color("#f")), (1.0, color("#6"))]),
        Track::float(NormalButton_glow_size::id(), Ease::Lin, vec![(0.0, 1.0), (1.0, 1.0)]),
        Track::color(NormalButton_border_color::id(), Ease::Lin, vec![(0.0, color("white")), (1.0, color("white"))]),
    ]));
    
    // lets define the shader
    
}





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
    
    TextStyle_window_caption::set(cx, default_text.clone());
    TextStyle_window_menu::set(cx, default_text.clone());
    TextStyle_button_title::set(cx, default_text.clone());
    TextStyle_normal::set(cx, default_text.clone());
    TextStyle_tab_title::set(cx, default_text.clone());
    
    
    TabLayout_bg::set(cx, Layout {
        align: Align::left_center(),
        walk: Walk::wh(Width::Compute, Height::Fix(40.)),
        padding: Padding {l: 16.0, t: 1.0, r: 16.0, b: 0.0},
        ..Default::default()
    });
    
    Walk_tab_close::set(cx, Walk {
        width: Width::Fix(10.),
        height: Height::Fix(10.),
        margin: Margin {l: -4., t: 0., r: 4., b: 0.}
    });
    
    Layout_window_menu::set(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fix(20.)),
        padding: Padding {l: 2., t: 3., b: 2., r: 0.},
        line_wrap: LineWrap::None,
        ..Default::default()
    });

    set_normal_button_values(cx);

}

pub fn set_dark_widget_theme(cx: &mut Cx) {
    set_widget_theme_values(cx);
    Color_bg_splitter::set(cx, color256(25, 25, 25));
    Color_bg_splitter_over::set(cx, color("#5"));
    Color_bg_splitter_peak::set(cx, color("#f"));
    Color_bg_splitter_drag::set(cx, color("#6"));
    Color_scrollbar_base::set(cx, color("#5"));
    Color_scrollbar_over::set(cx, color("#7"));
    Color_scrollbar_down::set(cx, color("#9"));
    Color_bg_normal::set(cx, color256(52, 52, 52));
    Color_bg_selected::set(cx, color256(40, 40, 40));
    Color_bg_odd::set(cx, color256(37, 37, 37));
    Color_bg_selected_over::set(cx, color256(61, 61, 61));
    Color_bg_odd_over::set(cx, color256(56, 56, 56));
    Color_bg_marked::set(cx, color256(17, 70, 110));
    Color_bg_marked_over::set(cx, color256(17, 70, 110));
    Color_over_border::set(cx, color256(255, 255, 255));
    Color_drop_quad::set(cx, color("#a"));
    Color_text_defocus::set(cx, color("#9"));
    Color_text_focus::set(cx, color("#b"));
    Color_icon::set(cx, color256(127, 127, 127));
    
    Color_text_selected_focus::set(cx, color256(255, 255, 255));
    Color_text_deselected_focus::set(cx, color256(157, 157, 157));
    Color_text_selected_defocus::set(cx, color256(157, 157, 157));
    Color_text_deselected_defocus::set(cx, color256(130, 130, 130));
}