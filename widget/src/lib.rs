use makepad_render::*;
mod buttonlogic;
pub use crate::buttonlogic::*;
mod button;
pub use crate::button::*;
mod desktopbutton;
pub use crate::desktopbutton::*;
mod desktopwindow;
pub use crate::desktopwindow::*;
mod windowmenu;
pub use crate::windowmenu::*;
mod frame;
pub use crate::frame::*;

pub fn live_register(cx:&mut Cx){
    crate::button::live_register(cx);
    crate::desktopbutton::live_register(cx);
    crate::desktopwindow::live_register(cx);
    crate::windowmenu::live_register(cx);
    crate::frame::live_register(cx);
}
