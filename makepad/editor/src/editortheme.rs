use render::*;

theme_layout!(LayoutCodeEditor);
theme_text_style!(TextStyleCodeEditorText);
theme_walk!(WalkCodeIcon);

theme_color!(Color_code_bg);
theme_color!(Color_code_gutter_bg);
theme_color!(Color_code_indent_line_unknown);
theme_color!(Color_code_indent_line_fn);
theme_color!(Color_code_indent_line_typedef);
theme_color!(Color_code_indent_line_looping);
theme_color!(Color_code_indent_line_flow);
theme_color!(Color_code_selection);
theme_color!(Color_code_selection_defocus);
theme_color!(Color_code_highlight);
theme_color!(Color_code_cursor);
theme_color!(Color_code_cursor_row);
theme_color!(Color_code_paren_pair_match);
theme_color!(Color_code_paren_pair_fail);
theme_color!(Color_code_marker_error);
theme_color!(Color_code_marker_warning);
theme_color!(Color_code_marker_log);
theme_color!(Color_code_line_number_normal);
theme_color!(Color_code_line_number_highlight);
theme_color!(Color_code_whitespace);
theme_color!(Color_code_keyword);
theme_color!(Color_code_flow);
theme_color!(Color_code_looping);
theme_color!(Color_code_identifier);
theme_color!(Color_code_call);
theme_color!(Color_code_type_name);
theme_color!(Color_code_string);
theme_color!(Color_code_number);
theme_color!(Color_code_comment);
theme_color!(Color_code_doc_comment);
theme_color!(Color_code_paren_d1);
theme_color!(Color_code_paren_d2);
theme_color!(Color_code_operator);
theme_color!(Color_code_delimiter);
theme_color!(Color_code_unexpected);
theme_color!(Color_code_warning);
theme_color!(Color_code_error);
theme_color!(Color_code_defocus);



pub fn set_editor_theme_values(cx: &mut Cx) {
    
    TextStyleCodeEditorText::set(cx, TextStyle{
        font_path: "resources/LiberationMono-Regular.ttf".to_string(),
        brightness: 1.1,
        line_spacing: 1.8,
        top_drop: 1.3,
        ..TextStyle::default()
    });
    
    LayoutCodeEditor::set(cx, Layout {
        walk: Walk::wh(Width::Fill, Height::Fill),
        padding: Padding {l: 4.0, t: 4.0, r: 4.0, b: 4.0},
        ..Layout::default()
    });
    
    WalkCodeIcon::set(cx, Walk{
        width: Width::Fix(14.0),
        height: Height::Fix(14.0),
        margin: Margin {l: 0., t: 0.5, r: 4., b: 0.},
    });
}

pub fn set_dark_editor_theme(cx: &mut Cx) {
    set_editor_theme_values(cx);


    Color_code_bg::set(cx, color256(30, 30, 30));
    Color_code_gutter_bg::set(cx, color256(30, 30, 30));
    Color_code_indent_line_unknown::set(cx, color("#5"));
    Color_code_indent_line_fn::set(cx, color256(220, 220, 174));
    Color_code_indent_line_typedef::set(cx, color256(91, 155, 211));
    Color_code_indent_line_looping::set(cx, color("darkorange"));
    Color_code_indent_line_flow::set(cx, color256(196, 133, 190));
    Color_code_selection::set(cx, color256(42, 78, 117));
    Color_code_selection_defocus::set(cx, color256(75, 75, 75));
    Color_code_highlight::set(cx, color256a(75, 75, 95, 128));
    Color_code_cursor::set(cx, color256(176, 176, 176));
    Color_code_cursor_row::set(cx, color256(45, 45, 45));
    
    Color_code_paren_pair_match::set(cx, color256(255, 255, 255));
    Color_code_paren_pair_fail::set(cx, color256(255, 0, 0));
    
    Color_code_marker_error::set(cx, color256(200, 0, 0));
    Color_code_marker_warning::set(cx, color256(0, 200, 0));
    Color_code_marker_log::set(cx, color256(200, 200, 200));
    Color_code_line_number_normal::set(cx, color256(136, 136, 136));
    Color_code_line_number_highlight::set(cx, color256(212, 212, 212));
    
    Color_code_whitespace::set(cx, color256(110, 110, 110));
    
    Color_code_keyword::set(cx, color256(91, 155, 211));
    Color_code_flow::set(cx, color256(196, 133, 190));
    Color_code_looping::set(cx, color("darkorange"));
    Color_code_identifier::set(cx, color256(212, 212, 212));
    Color_code_call::set(cx, color256(220, 220, 174));
    Color_code_type_name::set(cx, color256(86, 201, 177));
    
    Color_code_string::set(cx, color256(204, 145, 123));
    Color_code_number::set(cx, color256(182, 206, 170));
    
    Color_code_comment::set(cx, color256(99, 141, 84));
    Color_code_doc_comment::set(cx, color256(120, 171, 104));
    Color_code_paren_d1::set(cx, color256(212, 212, 212));
    Color_code_paren_d2::set(cx, color256(212, 212, 212));
    Color_code_operator::set(cx, color256(212, 212, 212));
    Color_code_delimiter::set(cx, color256(212, 212, 212));
    Color_code_unexpected::set(cx, color256(255, 0, 0));
    
    Color_code_warning::set(cx, color256(225, 229, 112));
    Color_code_error::set(cx, color256(254, 0, 0));
    Color_code_defocus::set(cx, color256(128, 128, 128));
}



