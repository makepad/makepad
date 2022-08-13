pub mod frame;
pub mod frame_traits;
pub use frame::*;
pub use frame_traits::*;
use crate::makepad_platform::*;

live_register!{
    import crate::frame::frame::*;
    import crate::scroll_bars::ScrollBars;
    
    Solid: Frame {bg: {shape: Solid}}
    Rect: Frame {bg: {shape: Rect}}
    Box: Frame {bg: {shape: Box}}
    BoxX: Frame {bg: {shape: BoxX}}
    BoxY: Frame {bg: {shape: BoxY}}
    BoxAll: Frame {bg: {shape: BoxAll}}
    GradientY: Frame {bg: {shape: GradientY}}
    Circle: Frame {bg: {shape: Circle}}
    Hexagon: Frame {bg: {shape: Hexagon}}
    GradientX: Frame {bg: {shape: Solid, fill: GradientX}}
    GradientY: Frame {bg: {shape: Solid, fill: GradientY}}
    Image: Frame {bg: {shape: Solid, fill: Image}}
    UserDraw: Frame {user_draw: true}
    ScrollXY: Frame {scroll_bars: ScrollBars{show_scroll_x:true, show_scroll_y:true}}
    ScrollX: Frame {scroll_bars: ScrollBars{show_scroll_x:true, show_scroll_y:false}}
    ScrollY: Frame {scroll_bars: ScrollBars{show_scroll_x:false, show_scroll_y:true}}
}

