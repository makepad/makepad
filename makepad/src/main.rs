mod fileeditor;
pub use crate::fileeditor::*;
mod app;
pub use crate::app::*;
mod appwindow;
pub use crate::appwindow::*;
mod rustcompiler;
pub use crate::rustcompiler::*;

use render::*;
main_app!(App);
