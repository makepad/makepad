use std::os::fd::AsFd;

use wayland_client::{delegate_noop, protocol::{wl_buffer, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface}, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::xdg::{self, shell::client::{xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base}, decoration::zv1::client::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1}};

use crate::{cx_native::EventFlow, wayland::wayland_event::WaylandEvent, WindowId};

use super::opengl_wayland::WaylandWindow;

pub(crate) struct WaylandState {
    pub(crate) compositor: Option<wl_compositor::WlCompositor>,
    pub(crate) wm_base: Option<xdg_wm_base::XdgWmBase>,
    pub(crate) seat: Option<wl_seat::WlSeat>,
    pub(crate) decoration_manager: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    pub(crate) windows: Vec<WaylandWindow>,
    event_callback: Option<Box<dyn FnMut(&mut WaylandState, WaylandEvent)>>
}

impl WaylandState {
    pub fn new(event_callback: Box<dyn FnMut(&mut WaylandState, WaylandEvent)>) -> Self {
        Self {
            compositor: None,
            wm_base: None,
            seat: None,
            decoration_manager: None,
            windows: Vec::new(),
            event_callback: Some(event_callback)
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()>  for WaylandState {
    fn event(
        state: &mut Self,
        wl_registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => {
                    let compositor = wl_registry.bind::<wl_compositor::WlCompositor, _, _>(name, 1, qhandle, ());
                    state.compositor = Some(compositor);
                }
                "xdg_wm_base" => {
                    let wm_base = wl_registry.bind::<xdg_wm_base::XdgWmBase, _, _>(name, 1, qhandle, ());
                    state.wm_base = Some(wm_base);
                }
                "wl_seat" => {
                    let seat = wl_registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qhandle, ());
                    state.seat = Some(seat);
                },
                "zxdg_decoration_manager_v1" => {
                    let decoration_manager = wl_registry.bind::<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _, _>(name, 1, qhandle, ());
                    state.decoration_manager = Some(decoration_manager);
                },
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WaylandState {
    fn event(
        state: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_wm_base::Event::Ping { serial } => wm_base.pong(serial),
            _ => {}
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, WindowId> for WaylandState {
    fn event(
            state: &mut Self,
            xdg_surface: &xdg_toplevel::XdgToplevel,
            event: xdg_toplevel::Event,
            window_id: &WindowId,
            conn: &Connection,
            qhandle: &QueueHandle<Self>,
        ) {
            state.do_callback(WaylandEvent::Toplevel(event, *window_id));
        }
}
impl Dispatch<xdg_surface::XdgSurface, WindowId> for WaylandState {
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        window_id: &WindowId,
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial, .. } = event {
            let window = state.windows.iter().find(|window| window.window_id == *window_id).unwrap();

            xdg_surface.ack_configure(serial);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for WaylandState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event {
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qhandle, ());
            }
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for WaylandState {
    fn event(
        state: &mut Self,
        keyboard: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        state.do_callback(WaylandEvent::Keyboard(event));
    }
}

delegate_noop!(WaylandState: ignore wl_compositor::WlCompositor);
delegate_noop!(WaylandState: ignore wl_surface::WlSurface);
delegate_noop!(WaylandState: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(WaylandState: ignore zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1);
// delegate_noop!(WaylandState: ignore xdg_positioner::XdgPositioner);

impl WaylandState {
    pub(crate) fn available(&self) -> bool {
        self.compositor.is_some() && self.wm_base.is_some() && self.seat.is_some() && self.decoration_manager.is_some()
    }
    fn do_callback(&mut self, event: WaylandEvent) {
        if let Some(mut callback) = self.event_callback.take() {
            callback(self, event);
            self.event_callback = Some(callback);
        }
    }
}
