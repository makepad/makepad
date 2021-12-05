use makepad_render::*;
mod buttonlogic;
mod button;
mod desktopbutton;
mod desktopwindow;
mod scrollbar;
mod scrollview;
mod frame;
mod windowmenu;

pub mod dock;
pub mod file_tree;
pub mod splitter;
pub mod tab;
pub mod tab_bar;
pub mod tab_close_button;
pub mod genid;
pub mod barewindow;

pub use crate::{
    genid::{GenId, GenIdMap, GenIdAllocator},
    buttonlogic::{ButtonLogic, ButtonAction},
    button::{Button},
    desktopwindow::{DesktopWindow},
    scrollview::{ScrollView},
    frame::{Frame, FrameActions}
};

pub fn live_register(cx:&mut Cx){
    crate::button::live_register(cx);
    crate::desktopbutton::live_register(cx);
    crate::desktopwindow::live_register(cx);
    crate::barewindow::live_register(cx);
    crate::windowmenu::live_register(cx);
    crate::frame::live_register(cx);
    crate::scrollview::live_register(cx);
    crate::scrollbar::live_register(cx);
    crate::file_tree::live_register(cx);
    crate::splitter::live_register(cx);
    crate::tab_close_button::live_register(cx);
    crate::tab::live_register(cx);
    crate::tab_bar::live_register(cx);
    crate::dock::live_register(cx);
}
