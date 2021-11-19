use makepad_render::*;
mod buttonlogic;
pub use crate::buttonlogic::*;
mod normalbutton;
pub use crate::normalbutton::*;
mod desktopbutton;
pub use crate::desktopbutton::*;

pub fn live_register(cx:&mut Cx){
    crate::normalbutton::live_register(cx);
    crate::desktopbutton::live_register(cx);
}
