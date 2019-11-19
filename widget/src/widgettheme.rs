use render::*;
use crate::normalbutton::*;
use crate::tab::*;
use crate::desktopwindow::*;
use crate::windowmenu::*;
use crate::tabclose::*;

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
    
    set_widget_theme_values(cx);
}