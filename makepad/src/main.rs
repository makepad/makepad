pub mod keyboard;
pub use crate::keyboard::*;
mod fileeditor;
pub use crate::fileeditor::*;
mod filepanel;
pub use crate::filepanel::*;
mod homepage;
pub use crate::homepage::*;
mod loglist;
pub use crate::loglist::*;
mod logitem; 
pub use crate::logitem::*;
mod app;
pub use crate::app::*;
mod appwindow;
pub use crate::appwindow::*;
mod appstorage;
pub use crate::appstorage::*;
mod filetree;
pub use crate::filetree::*;
mod buildmanager;
pub use crate::buildmanager::*;
mod makepadstyle;
pub use crate::makepadstyle::*;

pub mod codeicon;
pub use crate::codeicon::*;
mod rusteditor;
pub use crate::rusteditor::*;
mod jseditor;
pub use crate::jseditor::*;
mod plaineditor;
pub use crate::plaineditor::*;


//mod rustcompiler;
//pub use crate::rustcompiler::*;
use render::*;
mod builder;

main_app!(App);
