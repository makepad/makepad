use makepad_render::*;
use makepad_tinyserde::*;
use crate::normalbutton::*;
use crate::tab::*;
use crate::desktopwindow::*;
use crate::windowmenu::*;
use crate::tabclose::*;
use crate::texteditor::*;
use crate::textinput::*;
use crate::scrollbar::*;
use crate::scrollshadow::*;
use crate::desktopbutton::*;
use crate::splitter::*;
use crate::tabcontrol::*;

#[derive(Debug, Copy, Clone, SerRon, DeRon, PartialEq)]
pub struct StyleOptions {
    pub scale: f32,
    pub dark: bool
}

impl Default for StyleOptions{
    fn default()->Self{
        Self{
            scale:1.0,
            dark:true,
        }
    }
}

pub struct Theme {}
impl Theme {
    pub fn text_style_unscaled() -> TextStyleId {uid!()}
    pub fn text_style_normal() -> TextStyleId {uid!()}
    pub fn text_style_fixed() -> TextStyleId {uid!()}
    
    pub fn color_bg_splitter() -> ColorId {uid!()}
    pub fn color_bg_splitter_over() -> ColorId {uid!()}
    pub fn color_bg_splitter_peak() -> ColorId {uid!()}
    pub fn color_bg_splitter_drag() -> ColorId {uid!()}
    
    pub fn color_scrollbar_base() -> ColorId {uid!()}
    pub fn color_scrollbar_over() -> ColorId {uid!()}
    pub fn color_scrollbar_down() -> ColorId {uid!()}
    
    pub fn color_bg_normal() -> ColorId {uid!()}
    pub fn color_bg_selected() -> ColorId {uid!()}
    pub fn color_bg_odd() -> ColorId {uid!()}
    pub fn color_bg_selected_over() -> ColorId {uid!()}
    pub fn color_bg_odd_over() -> ColorId {uid!()}
    pub fn color_bg_marked() -> ColorId {uid!()}
    pub fn color_bg_marked_over() -> ColorId {uid!()}
    pub fn color_over_border() -> ColorId {uid!()}
    pub fn color_icon() -> ColorId {uid!()}
    pub fn color_drop_quad() -> ColorId {uid!()}
    
    pub fn color_text_focus() -> ColorId {uid!()}
    pub fn color_text_defocus() -> ColorId {uid!()}
    pub fn color_text_selected_focus() -> ColorId {uid!()}
    pub fn color_text_deselected_focus() -> ColorId {uid!()}
    pub fn color_text_selected_defocus() -> ColorId {uid!()}
    pub fn color_text_deselected_defocus() -> ColorId {uid!()}
}

pub fn set_widget_style(cx: &mut Cx, opt: &StyleOptions) {
    
    //if opt.dark {
        Theme::color_bg_splitter().set(cx, color256(25, 25, 25));
        Theme::color_bg_splitter_over().set(cx, color("#5"));
        Theme::color_bg_splitter_peak().set(cx, color("#f"));
        Theme::color_bg_splitter_drag().set(cx, color("#6"));
        Theme::color_scrollbar_base().set(cx, color("#5"));
        Theme::color_scrollbar_over().set(cx, color("#7"));
        Theme::color_scrollbar_down().set(cx, color("#9"));
        Theme::color_bg_normal().set(cx, color256(52, 52, 52));
        Theme::color_bg_selected().set(cx, color256(40, 40, 40));
        Theme::color_bg_odd().set(cx, color256(37, 37, 37));
        Theme::color_bg_selected_over().set(cx, color256(61, 61, 61));
        Theme::color_bg_odd_over().set(cx, color256(56, 56, 56));
        Theme::color_bg_marked().set(cx, color256(17, 70, 110));
        Theme::color_bg_marked_over().set(cx, color256(17, 70, 110));
        Theme::color_over_border().set(cx, color256(255, 255, 255));
        Theme::color_drop_quad().set(cx, color("#a"));
        Theme::color_text_defocus().set(cx, color("#9"));
        Theme::color_text_focus().set(cx, color("#b"));
        Theme::color_icon().set(cx, color256(127, 127, 127));
        
        Theme::color_text_selected_focus().set(cx, color256(255, 255, 255));
        Theme::color_text_deselected_focus().set(cx, color256(157, 157, 157));
        Theme::color_text_selected_defocus().set(cx, color256(157, 157, 157));
        Theme::color_text_deselected_defocus().set(cx, color256(130, 130, 130));
        
        TextEditor::color_bg().set(cx, color256(30, 30, 30));
        TextEditor::color_gutter_bg().set(cx, color256(30, 30, 30));
        TextEditor::color_indent_line_unknown().set(cx, color("#5"));
        TextEditor::color_indent_line_fn().set(cx, color256(220, 220, 174));
        TextEditor::color_indent_line_typedef().set(cx, color256(91, 155, 211));
        TextEditor::color_indent_line_looping().set(cx, color("darkorange"));
        TextEditor::color_indent_line_flow().set(cx, color256(196, 133, 190));
        TextEditor::color_selection().set(cx, color256(42, 78, 117));
        TextEditor::color_selection_defocus().set(cx, color256(75, 75, 75));
        TextEditor::color_highlight().set(cx, color256a(75, 75, 95, 128));
        TextEditor::color_cursor().set(cx, color256(176, 176, 176));
        TextEditor::color_cursor_row().set(cx, color256(45, 45, 45));
        
        TextEditor::color_paren_pair_match().set(cx, color256(255, 255, 255));
        TextEditor::color_paren_pair_fail().set(cx, color256(255, 0, 0));
        
        TextEditor::color_message_marker_error().set(cx, color256(200, 0, 0));
        TextEditor::color_message_marker_warning().set(cx, color256(0, 200, 0));
        TextEditor::color_message_marker_log().set(cx, color256(200, 200, 200));

        TextEditor::color_search_marker().set(cx, color256(128, 64, 0));

        TextEditor::color_line_number_normal().set(cx, color256(136, 136, 136));
        TextEditor::color_line_number_highlight().set(cx, color256(212, 212, 212));
        
        TextEditor::color_whitespace().set(cx, color256(110, 110, 110));
        
        TextEditor::color_keyword().set(cx, color256(91, 155, 211));
        TextEditor::color_flow().set(cx, color256(196, 133, 190));
        TextEditor::color_looping().set(cx, color("darkorange"));
        TextEditor::color_identifier().set(cx, color256(212, 212, 212));
        TextEditor::color_call().set(cx, color256(220, 220, 174));
        TextEditor::color_type_name().set(cx, color256(86, 201, 177));
        TextEditor::color_theme_name().set(cx, color256(204, 145, 123));
        
        TextEditor::color_string().set(cx, color256(204, 145, 123));
        TextEditor::color_number().set(cx, color256(182, 206, 170));
        
        TextEditor::color_comment().set(cx, color256(99, 141, 84));
        TextEditor::color_doc_comment().set(cx, color256(120, 171, 104));
        TextEditor::color_paren_d1().set(cx, color256(212, 212, 212));
        TextEditor::color_paren_d2().set(cx, color256(212, 212, 212));
        TextEditor::color_operator().set(cx, color256(212, 212, 212));
        TextEditor::color_delimiter().set(cx, color256(212, 212, 212));
        TextEditor::color_unexpected().set(cx, color256(255, 0, 0));
        
        TextEditor::color_warning().set(cx, color256(225, 229, 112));
        TextEditor::color_error().set(cx, color256(254, 0, 0));
        TextEditor::color_defocus().set(cx, color256(128, 128, 128));
    //}
    
    let font = cx.load_font("resources/Ubuntu-R.ttf");
    Theme::text_style_unscaled().set(cx, TextStyle {
        font: font,
        font_size: 8.0,
        brightness: 1.0,
        curve: 0.6,
        line_spacing: 1.4,
        top_drop: 1.2,
        height_factor: 1.3,
    });
    
    Theme::text_style_normal().set(cx, TextStyle {
        font_size: 8.0 * opt.scale,
        ..Theme::text_style_unscaled().get(cx)
    });
    
    let font = cx.load_font("resources/LiberationMono-Regular.ttf");
    Theme::text_style_fixed().set(cx, TextStyle {
        font: font,
        brightness: 1.1,
        font_size: 8.0 * opt.scale, 
        line_spacing: 1.8,
        top_drop: 1.3,
        ..Theme::text_style_unscaled().get(cx)
    });
    
    TabClose::style(cx, opt);
    DesktopWindow::style(cx, opt);
    NormalButton::style(cx, opt);
    Tab::style(cx, opt);
    MenuItemDraw::style(cx, opt);
    TextEditor::style(cx, opt);
    TextInput::style(cx, opt);
    ScrollBar::style(cx, opt);
    ScrollShadow::style(cx, opt);
    DesktopButton::style(cx, opt);
    Splitter::style(cx, opt);
    TabControl::style(cx, opt);
}
