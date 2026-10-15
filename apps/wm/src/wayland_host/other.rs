//! No Wayland server off Linux: nothing maps, so no tile ever has content.
use super::*;

pub fn layers(_: ClientId) -> Vec<Layer> { Vec::new() }
pub fn main_surface(_: ClientId) -> Option<Surf> { None }
pub fn send(_: ViewCmd) {}
pub fn frames_presented(_: ClientId) {}
pub fn owns(_: ClientId) -> bool { false }
pub fn icon_file(_: &str) -> Option<std::path::PathBuf> { None }
pub fn menu_apps() -> Vec<(String, String)> { Vec::new() }
pub fn cursor_shape(_: ClientId) -> u32 { 0 }

impl crate::App {
    pub(crate) fn wayland_poll(&mut self, _cx: &mut makepad_widgets::Cx) {}
    pub(crate) fn wayland_launch(&mut self, _cx: &mut makepad_widgets::Cx, _app_id: &str) -> bool { false }
    pub(crate) fn wayland_close(&mut self, _client: ClientId) -> bool { false }
}
