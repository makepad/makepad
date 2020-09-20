use makepad_render::*;
use crate::normalbutton::*;
use crate::tab::*;
use crate::desktopwindow::*;
use crate::tabclose::*;
use crate::texteditor::*;
use crate::textinput::*;
use crate::scrollbar::*;
use crate::scrollshadow::*;
use crate::desktopbutton::*;
use crate::splitter::*;
use crate::tabcontrol::*;
use crate::xrcontrol::*;

pub fn set_widget_style(cx: &mut Cx) {
    
    live!(cx, r#"
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

    TabClose::style(cx);
    DesktopWindow::style(cx);
    NormalButton::style(cx);
    Tab::style(cx);
    TextEditor::style(cx);
    TextInput::style(cx);
    ScrollBar::style(cx);
    ScrollShadow::style(cx);
    DesktopButton::style(cx);
    Splitter::style(cx);
    TabControl::style(cx);
    XRControl::style(cx);
}
