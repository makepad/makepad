use render::*;

theme_text_style!(TextStyleTab);
theme_text_style!(TextStyleNormalButton);
theme_text_style!(TextStyleDesktopWindowCaption);
theme_text_style!(TextStyleWindowMenu);

theme_layout!(LayoutNormalButton);
theme_layout!(LayoutTab);
theme_layout!(LayoutWindowMenu);

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

theme_color!(ColorTextSelectedFocus);
theme_color!(ColorTextDeselectedFocus);
theme_color!(ColorTextSelectedDefocus);
theme_color!(ColorTextDeselectedDefocus);

//theme_text!(ThemeNormalText);
//theme_text!(ThemeMonoText);

pub fn set_theme_defaults(cx: &mut Cx) {
    let default_text = TextStyle::default();
    
    TextStyleDesktopWindowCaption::set(cx, default_text.clone());
    TextStyleWindowMenu::set(cx, default_text.clone());
    TextStyleNormalButton::set(cx, default_text.clone());
    LayoutNormalButton::set(cx, Layout {
        align: Align::center(),
        width: Width::Compute,
        height: Height::Compute,
        margin: Margin::all(1.0),
        padding: Padding {l: 16.0, t: 14.0, r: 16.0, b: 14.0},
        ..Default::default()
    });
    
    TextStyleTab::set(cx, default_text.clone());
    LayoutTab::set(cx, Layout {
        align: Align::left_center(),
        width: Width::Compute,
        height: Height::Fix(40.),
        margin: Margin::all(0.),
        padding: Padding {l: 16.0, t: 1.0, r: 16.0, b: 0.0},
        ..Default::default()
    });
    
    LayoutWindowMenu::set(cx, Layout {
        width: Width::Fill,
        height: Height::Fix(20.),
        padding: Padding {l: 2., t: 3., b: 2., r: 0.},
        line_wrap: LineWrap::None,
        ..Default::default()
    });
    // set text_styles
    //cx.set_font("normal_font", "resources/Ubunutu-R.ttf");
    //cx.set_font("mono_font", "resources/LiberationMono-Regular.ttf");
    // set layouts
    // set loose values
    
}

pub fn set_dark_theme(cx: &mut Cx) {
    set_theme_defaults(cx);
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
    ColorTextSelectedFocus::set(cx, color256(255, 255, 255));
    ColorTextDeselectedFocus::set(cx, color256(157, 157, 157));
    ColorTextSelectedDefocus::set(cx, color256(157, 157, 157));
    ColorTextDeselectedDefocus::set(cx, color256(130, 130, 130));
    
    /* cx.set_color("icon_color", color256(127, 127, 127));
    
    cx.set_color("text_selected_focus", color256(255, 255, 255));
    cx.set_color("text_deselected_focus", color256(157, 157, 157));
    cx.set_color("text_selected_defocus", color256(157, 157, 157));
    cx.set_color("text_deselected_defocus", color256(130, 130, 130));*/
}
