pub use makepad_component;
pub use makepad_component::makepad_platform;
pub mod imgui;
pub mod button;

use makepad_platform::*;

pub use crate::{
    button::{ButtonImGUI},
    imgui::{ImGUI, ImGUIRef, ImGUIRun, ImGuiEventExt},
};

pub fn live_register(cx: &mut Cx) {
    makepad_platform::live_cx::live_register(cx);
    makepad_component::live_register(cx);
}
