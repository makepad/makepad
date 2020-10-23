use makepad_render::*;

pub fn set_widget_style(cx: &mut Cx) {
    
    live_body!(cx, r#"
        self::text_style_unscaled: TextStyle{
            font: "resources/Ubuntu-R.ttf",
            font_size: 8.0,
            brightness: 1.0,
            curve: 0.6,
            line_spacing: 1.4,
            top_drop: 1.2,
            height_factor: 1.3,
        }
        
        self::text_style_normal: TextStyle{
            font_size: 8.0,
            ..self::text_style_unscaled
        }
        
        self::text_style_fixed: TextStyle{
            font: "resources/LiberationMono-Regular.ttf",
            brightness: 1.1,
            font_size: 8.0, 
            line_spacing: 1.8,
            top_drop: 1.3,
            ..self::text_style_unscaled
        }
        
        self::color_drop_quad: #a;
        
    "#);
    crate::foldcaption::FoldCaption::style(cx);
    crate::tabclose::TabClose::style(cx);
    crate::desktopwindow::DesktopWindow::style(cx);
    crate::normalbutton::NormalButton::style(cx);
    crate::tab::Tab::style(cx);
    crate::texteditor::TextEditor::style(cx);
    crate::textinput::TextInput::style(cx);
    crate::scrollbar::ScrollBar::style(cx);
    crate::scrollshadow::ScrollShadow::style(cx);
    crate::desktopbutton::DesktopButton::style(cx);
    crate::splitter::Splitter::style(cx);
    crate::tabcontrol::TabControl::style(cx);
    crate::xrcontrol::XRControl::style(cx);
    crate::viewport3d::Viewport3D::style(cx);
}

  /*
        Theme::color_bg_normal().set(cx, pick!(52, 52, 52).get(cx));
        Theme::color_bg_selected().set(cx, pick!(40, 40, 40).get(cx));
        Theme::color_bg_odd().set(cx, pick!(37, 37, 37).get(cx));
        Theme::color_bg_selected_over().set(cx, pick!(61, 61, 61).get(cx));
        Theme::color_bg_odd_over().set(cx, pick!(56, 56, 56).get(cx));
        Theme::color_bg_marked().set(cx, pick!(17, 70, 110).get(cx));
        Theme::color_bg_marked_over().set(cx, pick!(17, 70, 110).get(cx));
        Theme::color_over_border().set(cx, pick!(255, 255, 255).get(cx));
        Theme::color_drop_quad().set(cx, pick!(#a).get(cx));
        Theme::color_text_defocus().set(cx, pick!(#9).get(cx));
        Theme::color_text_focus().set(cx, pick!(#b).get(cx));
        Theme::color_icon().set(cx, pick!(127, 127, 127).get(cx));
        
        Theme::color_text_selected_focus().set(cx, pick!(255, 255, 255).get(cx));
        Theme::color_text_deselected_focus().set(cx, pick!(157, 157, 157).get(cx));
        Theme::color_text_selected_defocus().set(cx, pick!(157, 157, 157).get(cx));
        Theme::color_text_deselected_defocus().set(cx, pick!(130, 130, 130).get(cx));
        
        TextEditor::color_bg().set(cx, pick!(30, 30, 30).get(cx));
        TextEditor::color_gutter_bg().set(cx, pick!(30, 30, 30).get(cx));
        TextEditor::color_indent_line_unknown().set(cx, pick!(#5).get(cx));
        TextEditor::color_indent_line_fn().set(cx, pick!(220, 220, 174).get(cx));
        TextEditor::color_indent_line_typedef().set(cx, pick!(91, 155, 211).get(cx));
        TextEditor::color_indent_line_looping().set(cx, pick!(darkorange).get(cx));
        TextEditor::color_indent_line_flow().set(cx, pick!(196, 133, 190).get(cx));
        TextEditor::color_selection().set(cx, pick!(42, 78, 117).get(cx));
        TextEditor::color_selection_defocus().set(cx, pick!(75, 75, 75).get(cx));
        TextEditor::color_highlight().set(cx, pick!(75, 75, 95, 128).get(cx));
        TextEditor::color_cursor().set(cx, pick!(176, 176, 176).get(cx));
        TextEditor::color_cursor_row().set(cx, pick!(45, 45, 45).get(cx));
        
        TextEditor::color_paren_pair_match().set(cx, pick!(255, 255, 255).get(cx));
        TextEditor::color_paren_pair_fail().set(cx, pick!(255, 0, 0).get(cx));
        
        TextEditor::color_message_marker_error().set(cx, pick!(200, 0, 0).get(cx));
        TextEditor::color_message_marker_warning().set(cx, pick!(0, 200, 0).get(cx));
        TextEditor::color_message_marker_log().set(cx, pick!(200, 200, 200).get(cx));

        TextEditor::color_search_marker().set(cx, pick!(128, 64, 0).get(cx));

        TextEditor::color_line_number_normal().set(cx, pick!(136, 136, 136).get(cx));
        TextEditor::color_line_number_highlight().set(cx, pick!(212, 212, 212).get(cx));
        
        TextEditor::color_whitespace().set(cx, pick!(110, 110, 110).get(cx));
        
        TextEditor::color_keyword().set(cx, pick!(91, 155, 211).get(cx));
        TextEditor::color_flow().set(cx, pick!(196, 133, 190).get(cx));
        TextEditor::color_looping().set(cx, pick!(darkorange).get(cx));
        TextEditor::color_identifier().set(cx, pick!(212, 212, 212).get(cx));
        TextEditor::color_call().set(cx, pick!(220, 220, 174).get(cx));
        TextEditor::color_type_name().set(cx, pick!(86, 201, 177).get(cx));
        TextEditor::color_theme_name().set(cx, pick!(204, 145, 123).get(cx));
        
        TextEditor::color_string().set(cx, pick!(204, 145, 123).get(cx));
        TextEditor::color_number().set(cx, pick!(182, 206, 170).get(cx));
        
        TextEditor::color_comment().set(cx, pick!(99, 141, 84).get(cx));
        TextEditor::color_doc_comment().set(cx, pick!(120, 171, 104).get(cx));
        TextEditor::color_paren_d1().set(cx, pick!(212, 212, 212).get(cx));
        TextEditor::color_paren_d2().set(cx, pick!(212, 212, 212).get(cx));
        TextEditor::color_operator().set(cx, pick!(212, 212, 212).get(cx));
        TextEditor::color_delimiter().set(cx, pick!(212, 212, 212).get(cx));
        TextEditor::color_unexpected().set(cx, pick!(255, 0, 0).get(cx));
        
        TextEditor::color_warning().set(cx, pick!(225, 229, 112).get(cx));
        TextEditor::color_error().set(cx, pick!(254, 0, 0).get(cx));
        TextEditor::color_defocus().set(cx, pick!(128, 128, 128).get(cx));
        */
