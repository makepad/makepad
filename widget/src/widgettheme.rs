use render::*;

theme_text_style!(TextStyleNormal);
theme_text_style!(TextStyleTab);
theme_text_style!(TextStyleNormalButton);
theme_text_style!(TextStyleDesktopWindowCaption);
theme_text_style!(TextStyleWindowMenu);

theme_layout!(LayoutNormalButton);
theme_layout!(LayoutTab);
theme_layout!(LayoutWindowMenu);

theme_walk!(WalkTabClose);

theme_color!(ColorBgSplitter);
theme_color!(ColorBgSplitterOver);
theme_color!(ColorBgSplitterPeak);
theme_color!(ColorBgSplitterDrag);

theme_color!(ColorBgNormal);
theme_color!(ColorBgSelected);
theme_color!(ColorBgOdd);
theme_color!(ColorBgSelectedOver);
theme_color!(ColorBgOddOver);
theme_color!(ColorBgMarked);
theme_color!(ColorBgMarkedOver);
theme_color!(ColorOverBorder);
theme_color!(ColorIcon);
theme_color!(ColorDropQuad);

theme_color!(ColorTextFocus);
theme_color!(ColorTextDefocus);
theme_color!(ColorTextSelectedFocus);
theme_color!(ColorTextDeselectedFocus);
theme_color!(ColorTextSelectedDefocus);
theme_color!(ColorTextDeselectedDefocus);

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
    
    TextStyleDesktopWindowCaption::set(cx, default_text.clone());
    TextStyleWindowMenu::set(cx, default_text.clone());
    TextStyleNormalButton::set(cx, default_text.clone());
    TextStyleNormal::set(cx, default_text.clone());
    
    LayoutNormalButton::set(cx, Layout {
        align: Align::center(),
        walk: Walk {
            width: Width::Compute,
            height: Height::Compute,
            margin: Margin::all(1.0),
        },
        padding: Padding {l: 16.0, t: 14.0, r: 16.0, b: 14.0},
        ..Default::default()
    });
    
    TextStyleTab::set(cx, default_text.clone());
    LayoutTab::set(cx, Layout {
        align: Align::left_center(),
        walk: Walk::wh(Width::Compute, Height::Fix(40.)),
        padding: Padding {l: 16.0, t: 1.0, r: 16.0, b: 0.0},
        ..Default::default()
    });
    
    WalkTabClose::set(cx, Walk {
        width: Width::Fix(10.),
        height: Height::Fix(10.),
        margin: Margin {l: -4., t: 0., r: 4., b: 0.}
    });
    
    LayoutWindowMenu::set(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fix(20.)),
        padding: Padding {l: 2., t: 3., b: 2., r: 0.},
        line_wrap: LineWrap::None,
        ..Default::default()
    });
}

pub fn set_dark_widget_theme(cx: &mut Cx) {
    set_widget_theme_values(cx);
    ColorBgSplitter::set(cx, color256(25, 25, 25));
    ColorBgSplitterOver::set(cx, color256(25, 25, 25));
    
    ColorBgNormal::set(cx, color256(52, 52, 52));
    ColorBgSelected::set(cx, color256(40, 40, 40));
    ColorBgOdd::set(cx, color256(37, 37, 37));
    ColorBgSelectedOver::set(cx, color256(61, 61, 61));
    ColorBgOddOver::set(cx, color256(56, 56, 56));
    ColorBgMarked::set(cx, color256(17, 70, 110));
    ColorBgMarkedOver::set(cx, color256(17, 70, 110));
    ColorOverBorder::set(cx, color256(255, 255, 255));
    ColorDropQuad::set(cx, color("#a"));
    ColorTextDefocus::set(cx, color("#9"));
    ColorTextFocus::set(cx, color("#b"));
    ColorIcon::set(cx, color256(127, 127, 127));
    
    ColorTextSelectedFocus::set(cx, color256(255, 255, 255));
    ColorTextDeselectedFocus::set(cx, color256(157, 157, 157));
    ColorTextSelectedDefocus::set(cx, color256(157, 157, 157));
    ColorTextDeselectedDefocus::set(cx, color256(130, 130, 130));
}
