use render::*;
use crate::codeeditor::*;
use crate::codeicon::*;

pub fn set_editor_theme_values(cx: &mut Cx) {
    CodeEditor::theme(cx);
    CodeIcon::theme(cx);
}

pub fn set_dark_editor_theme(cx: &mut Cx) {
    set_editor_theme_values(cx);
    
    CodeEditor::color_bg().set_base(cx, color256(30, 30, 30));
    CodeEditor::color_gutter_bg().set_base(cx, color256(30, 30, 30));
    CodeEditor::color_indent_line_unknown().set_base(cx, color("#5"));
    CodeEditor::color_indent_line_fn().set_base(cx, color256(220, 220, 174));
    CodeEditor::color_indent_line_typedef().set_base(cx, color256(91, 155, 211));
    CodeEditor::color_indent_line_looping().set_base(cx, color("darkorange"));
    CodeEditor::color_indent_line_flow().set_base(cx, color256(196, 133, 190));
    CodeEditor::color_selection().set_base(cx, color256(42, 78, 117));
    CodeEditor::color_selection_defocus().set_base(cx, color256(75, 75, 75));
    CodeEditor::color_highlight().set_base(cx, color256a(75, 75, 95, 128));
    CodeEditor::color_cursor().set_base(cx, color256(176, 176, 176));
    CodeEditor::color_cursor_row().set_base(cx, color256(45, 45, 45));
    
    CodeEditor::color_paren_pair_match().set_base(cx, color256(255, 255, 255));
    CodeEditor::color_paren_pair_fail().set_base(cx, color256(255, 0, 0));
    
    CodeEditor::color_marker_error().set_base(cx, color256(200, 0, 0));
    CodeEditor::color_marker_warning().set_base(cx, color256(0, 200, 0));
    CodeEditor::color_marker_log().set_base(cx, color256(200, 200, 200));
    CodeEditor::color_line_number_normal().set_base(cx, color256(136, 136, 136));
    CodeEditor::color_line_number_highlight().set_base(cx, color256(212, 212, 212));
    
    CodeEditor::color_whitespace().set_base(cx, color256(110, 110, 110));
    
    CodeEditor::color_keyword().set_base(cx, color256(91, 155, 211));
    CodeEditor::color_flow().set_base(cx, color256(196, 133, 190));
    CodeEditor::color_looping().set_base(cx, color("darkorange"));
    CodeEditor::color_identifier().set_base(cx, color256(212, 212, 212));
    CodeEditor::color_call().set_base(cx, color256(220, 220, 174));
    CodeEditor::color_type_name().set_base(cx, color256(86, 201, 177));
    CodeEditor::color_theme_name().set_base(cx, color256(204, 145, 123));
    
    CodeEditor::color_string().set_base(cx, color256(204, 145, 123));
    CodeEditor::color_number().set_base(cx, color256(182, 206, 170));
    
    CodeEditor::color_comment().set_base(cx, color256(99, 141, 84));
    CodeEditor::color_doc_comment().set_base(cx, color256(120, 171, 104));
    CodeEditor::color_paren_d1().set_base(cx, color256(212, 212, 212));
    CodeEditor::color_paren_d2().set_base(cx, color256(212, 212, 212));
    CodeEditor::color_operator().set_base(cx, color256(212, 212, 212));
    CodeEditor::color_delimiter().set_base(cx, color256(212, 212, 212));
    CodeEditor::color_unexpected().set_base(cx, color256(255, 0, 0));
    
    CodeEditor::color_warning().set_base(cx, color256(225, 229, 112));
    CodeEditor::color_error().set_base(cx, color256(254, 0, 0));
    CodeEditor::color_defocus().set_base(cx, color256(128, 128, 128));
}



