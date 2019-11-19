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
mod makepadtheme;
pub use crate::makepadtheme::*;

//mod rustcompiler;
//pub use crate::rustcompiler::*;
use render::*;
#[path = "../../workspace/src/main.rs"]
mod workspace_main;

main_app!(App);
