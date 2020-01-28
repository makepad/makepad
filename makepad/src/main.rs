pub mod keyboard;
mod fileeditor;
mod filepanel;
mod homepage;
mod loglist;
mod app;
pub use crate::app::*;
mod appwindow;
mod appstorage;
mod filetree;
mod buildmanager;
mod makepadstyle;
mod searchindex;
mod searchresults;

pub mod codeicon;
mod rusteditor;
mod jseditor;
mod plaineditor;
mod itemdisplay;

//mod rustcompiler;
//pub use crate::rustcompiler::*;
use makepad_render::*;
mod builder;

main_app!(App);
