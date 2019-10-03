pub mod keyboard;
pub use crate::keyboard::*;
mod fileeditor;
pub use crate::fileeditor::*;
mod app;
pub use crate::app::*;
mod appwindow;
pub use crate::appwindow::*;
//mod rustcompiler;
//pub use crate::rustcompiler::*;
mod hubui;
pub use crate::hubui::*;
use render::*;

main_app!(App);
