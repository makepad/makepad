use wayland_client::protocol::wl_keyboard;
use wayland_protocols::xdg::{decoration::zv1::client::zxdg_toplevel_decoration_v1, shell::client::xdg_toplevel};

use crate::{TimerEvent, WindowId};

#[derive(Debug)]
pub enum WaylandEvent {
    Toplevel(xdg_toplevel::Event, WindowId),
    Keyboard(wl_keyboard::Event),
    Paint,
    Timer(TimerEvent),
}
