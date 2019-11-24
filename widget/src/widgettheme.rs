use render::*;
use crate::normalbutton::*;
use crate::tab::*;
use crate::desktopwindow::*;
use crate::windowmenu::*;
use crate::tabclose::*;
use crate::codeeditor::*;
use crate::codeicon::*;

pub struct Theme{}
impl Theme{
    pub fn text_style_normal()->TextStyleId{uid!()}
    pub fn text_style_fixed()->TextStyleId{uid!()}

    pub fn color_bg_splitter()->ColorId{uid!()}
    pub fn color_bg_splitter_over()->ColorId{uid!()}
    pub fn color_bg_splitter_peak()->ColorId{uid!()}
    pub fn color_bg_splitter_drag()->ColorId{uid!()}
    
    pub fn color_scrollbar_base()->ColorId{uid!()}
    pub fn color_scrollbar_over()->ColorId{uid!()}
    pub fn color_scrollbar_down()->ColorId{uid!()}
    
    pub fn color_bg_normal()->ColorId{uid!()}
    pub fn color_bg_selected()->ColorId{uid!()}
    pub fn color_bg_odd()->ColorId{uid!()}
    pub fn color_bg_selected_over()->ColorId{uid!()}
    pub fn color_bg_odd_over()->ColorId{uid!()}
    pub fn color_bg_marked()->ColorId{uid!()}
    pub fn color_bg_marked_over()->ColorId{uid!()}
    pub fn color_over_border()->ColorId{uid!()}
    pub fn color_icon()->ColorId{uid!()}
    pub fn color_drop_quad()->ColorId{uid!()}
    
    pub fn color_text_focus()->ColorId{uid!()}
    pub fn color_text_defocus()->ColorId{uid!()}
    pub fn color_text_selected_focus()->ColorId{uid!()}
    pub fn color_text_deselected_focus()->ColorId{uid!()}
    pub fn color_text_selected_defocus()->ColorId{uid!()}
    pub fn color_text_deselected_defocus()->ColorId{uid!()}
}

pub fn set_widget_theme_values(cx: &mut Cx) {
    let font = cx.load_font("resources/Ubuntu-R.ttf");
    Theme::text_style_normal().set_base(cx, TextStyle {
        font: font,
        font_size: 8.0,
        brightness: 1.0,
        curve: 0.7,
        line_spacing: 1.4,
        top_drop: 1.1,
        height_factor: 1.3,
    });

    let font = cx.load_font("resources/LiberationMono-Regular.ttf");
    Theme::text_style_fixed().set_base(cx, TextStyle{
        font: font,
        brightness: 1.1,
        line_spacing: 1.8,
        top_drop: 1.3,
        ..TextStyle::default()
    });
    

    TabClose::theme(cx);
    DesktopWindow::theme(cx);
    NormalButton::theme(cx);
    Tab::theme(cx);
    MenuItemDraw::theme(cx);
    CodeEditor::theme(cx);
    CodeIcon::theme(cx);
}


pub fn set_dark_widget_theme(cx: &mut Cx) {
    
    Theme::color_bg_splitter().set_base(cx, color256(25, 25, 25));
    Theme::color_bg_splitter_over().set_base(cx, color("#5"));
    Theme::color_bg_splitter_peak().set_base(cx, color("#f"));
    Theme::color_bg_splitter_drag().set_base(cx, color("#6"));
    Theme::color_scrollbar_base().set_base(cx, color("#5"));
    Theme::color_scrollbar_over().set_base(cx, color("#7"));
    Theme::color_scrollbar_down().set_base(cx, color("#9"));
    Theme::color_bg_normal().set_base(cx, color256(52, 52, 52));
    Theme::color_bg_selected().set_base(cx, color256(40, 40, 40));
    Theme::color_bg_odd().set_base(cx, color256(37, 37, 37));
    Theme::color_bg_selected_over().set_base(cx, color256(61, 61, 61));
    Theme::color_bg_odd_over().set_base(cx, color256(56, 56, 56));
    Theme::color_bg_marked().set_base(cx, color256(17, 70, 110));
    Theme::color_bg_marked_over().set_base(cx, color256(17, 70, 110));
    Theme::color_over_border().set_base(cx, color256(255, 255, 255));
    Theme::color_drop_quad().set_base(cx, color("#a"));
    Theme::color_text_defocus().set_base(cx, color("#9"));
    Theme::color_text_focus().set_base(cx, color("#b"));
    Theme::color_icon().set_base(cx, color256(127, 127, 127));
    
    Theme::color_text_selected_focus().set_base(cx, color256(255, 255, 255));
    Theme::color_text_deselected_focus().set_base(cx, color256(157, 157, 157));
    Theme::color_text_selected_defocus().set_base(cx, color256(157, 157, 157));
    Theme::color_text_deselected_defocus().set_base(cx, color256(130, 130, 130));

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
    
    set_widget_theme_values(cx);
}