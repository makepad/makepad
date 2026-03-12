#![allow(unused_imports, unused_variables)]
use crate::{
    libc_sys::{self, munmap},
    makepad_math::{dvec2, Vec2d},
    wayland::{wayland_type, xkb_sys},
    Area, DragEvent, DragItem, DragResponse, DropEvent, KeyEvent, KeyModifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, TextClipboardEvent, TextInputEvent,
    WindowClosedEvent, WindowDragQueryEvent, WindowDragQueryResponse,
};
use std::{
    cell::{Cell, RefCell},
    os::fd::{AsFd, AsRawFd, FromRawFd},
    rc::Rc,
    sync::Arc,
    sync::Mutex,
};

use wayland_client::{
    delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
        wl_data_source, wl_keyboard, wl_output,
        wl_pointer::{self, ButtonState},
        wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
    },
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::{
    wp::{
        cursor_shape::v1::client::{
            wp_cursor_shape_device_v1,
            wp_cursor_shape_manager_v1::{self, WpCursorShapeManagerV1},
        },
        fractional_scale::v1::client::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1},
        primary_selection::zv1::client::{
            zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
            zwp_primary_selection_offer_v1, zwp_primary_selection_source_v1,
        },
        text_input::zv3::client::{zwp_text_input_manager_v3, zwp_text_input_v3},
        viewporter::client::{wp_viewport, wp_viewporter},
    },
    xdg::{
        self,
        decoration::zv1::client::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1},
        shell::client::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base},
        toplevel_icon::v1::client::{xdg_toplevel_icon_manager_v1, xdg_toplevel_icon_v1},
    },
};

use crate::{
    cx_native::EventFlow,
    event::{PopupDismissReason, PopupDismissedEvent, ScrollEvent, WindowGeom},
    select_timer::SelectTimers,
    wayland::wayland_app::WaylandApp,
    x11::xlib_event::XlibEvent,
    KeyCode, WindowCloseRequestedEvent, WindowGeomChangeEvent, WindowId, WindowMovedEvent,
};

use super::opengl_wayland::{WaylandPopupWindow, WaylandWindow};

pub(crate) struct ClipboardOffer {
    offer: wl_data_offer::WlDataOffer,
    mime_types: Vec<String>,
}

struct PendingClipboardRead {
    fd: std::os::fd::OwnedFd,
    bytes: Vec<u8>,
}

pub(crate) struct WaylandState {
    pub(crate) compositor: Option<wl_compositor::WlCompositor>,
    pub(crate) wm_base: Option<xdg_wm_base::XdgWmBase>,
    pub(crate) seat: Option<wl_seat::WlSeat>,
    pub(crate) shm: Option<wl_shm::WlShm>,
    pub(crate) data_device_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    pub(crate) data_device: Option<wl_data_device::WlDataDevice>,
    pub(crate) clipboard_source: Option<wl_data_source::WlDataSource>,
    pub(crate) clipboard_offer: Option<ClipboardOffer>,
    pub(crate) data_offers: Vec<ClipboardOffer>,
    pending_clipboard_read: Option<PendingClipboardRead>,
    pending_paste_text_input: Option<String>,
    /// Queued clipboard copy content waiting for a serial from keyboard/pointer.
    pub(crate) pending_clipboard_copy: Option<String>,
    pub(crate) internal_drag_items: Option<Arc<Vec<DragItem>>>,
    pub(crate) clipboard_text: String,
    pub(crate) cursor_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    pub(crate) cursor_shape: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    pub(crate) pointer: Option<wl_pointer::WlPointer>,
    pub(crate) last_mouse_pos: Vec2d,
    pub(crate) pointer_serial: Option<u32>,
    pub(crate) keyboard_serial: Option<u32>,
    pub(crate) decoration_manager: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    pub(crate) icon_manager: Option<xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1>,
    pub(crate) windows: Vec<WaylandWindow>,
    pub(crate) popups: Vec<WaylandPopupWindow>,
    pub(crate) pointer_window: Option<WindowId>,
    pub(crate) keyboard_window: Option<WindowId>,
    pub(crate) modifiers: KeyModifiers,
    pub(crate) timers: SelectTimers,
    pub(crate) scale_manager: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    pub(crate) viewporter: Option<wp_viewporter::WpViewporter>,
    pub(crate) xkb_state: Option<xkb_sys::XkbState>,
    pub(crate) xkb_cx: xkb_sys::XkbContext,
    pub(crate) text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    pub(crate) text_input_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    pub(crate) primary_selection_manager:
        Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    pub(crate) primary_selection_device:
        Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    pub(crate) primary_selection_source:
        Option<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1>,
    pub(crate) primary_selection_text: String,
    pub(crate) last_resize_edge: Option<xdg_toplevel::ResizeEdge>,
    event_callback: Option<Box<dyn FnMut(&mut WaylandState, XlibEvent)>>,

    pub(crate) scroll_accumulator: Vec2d,
    pub(crate) scroll_is_wheel: bool,
    pub(crate) last_scroll_time: f64,
    pub(crate) event_flow: EventFlow,
    pub(crate) event_loop_running: bool,
}

impl WaylandState {
    pub fn new(event_callback: Box<dyn FnMut(&mut WaylandState, XlibEvent)>) -> Self {
        Self {
            compositor: None,
            wm_base: None,
            seat: None,
            shm: None,
            data_device_manager: None,
            data_device: None,
            clipboard_source: None,
            clipboard_offer: None,
            data_offers: Vec::new(),
            pending_clipboard_read: None,
            pending_paste_text_input: None,
            pending_clipboard_copy: None,
            internal_drag_items: None,
            clipboard_text: String::new(),
            cursor_manager: None,
            cursor_shape: None,
            pointer: None,
            decoration_manager: None,
            icon_manager: None,
            scale_manager: None,
            viewporter: None,
            windows: Vec::new(),
            popups: Vec::new(),
            pointer_window: None,
            keyboard_window: None,
            pointer_serial: None,
            keyboard_serial: None,
            modifiers: KeyModifiers::default(),
            xkb_state: None,
            xkb_cx: xkb_sys::XkbContext::new().unwrap(),
            text_input: None,
            text_input_manager: None,
            primary_selection_manager: None,
            primary_selection_device: None,
            primary_selection_source: None,
            primary_selection_text: String::new(),
            last_mouse_pos: dvec2(0., 0.),
            last_resize_edge: None,
            timers: SelectTimers::new(),
            event_callback: Some(event_callback),
            scroll_accumulator: dvec2(0.0, 0.0),
            scroll_is_wheel: false,
            last_scroll_time: 0.0,
            event_flow: EventFlow::Wait,
            event_loop_running: true,
        }
    }

    fn window_id_for_surface(&self, surface: &wl_surface::WlSurface) -> Option<WindowId> {
        let surface_id = surface.id();
        self.windows
            .iter()
            .find(|win| win.base_surface.id() == surface_id)
            .map(|win| win.window_id)
            .or_else(|| {
                self.popups
                    .iter()
                    .find(|win| win.base_surface.id() == surface_id)
                    .map(|win| win.window_id)
            })
    }

    pub(crate) fn xdg_surface_for_window(
        &self,
        window_id: WindowId,
    ) -> Option<xdg_surface::XdgSurface> {
        self.windows
            .iter()
            .find(|win| win.window_id == window_id)
            .map(|win| win.xdg_surface.clone())
            .or_else(|| {
                self.popups
                    .iter()
                    .find(|win| win.window_id == window_id)
                    .map(|win| win.xdg_surface.clone())
            })
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        wl_registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    let compositor =
                        wl_registry.bind::<wl_compositor::WlCompositor, _, _>(name, 1, qhandle, ());
                    state.compositor = Some(compositor);
                }
                "xdg_wm_base" => {
                    let wm_base =
                        wl_registry.bind::<xdg_wm_base::XdgWmBase, _, _>(name, 1, qhandle, ());
                    state.wm_base = Some(wm_base);
                }
                "wl_seat" => {
                    let seat = wl_registry.bind::<wl_seat::WlSeat, _, _>(
                        name,
                        version.min(5),
                        qhandle,
                        (),
                    );
                    state.seat = Some(seat);
                    state.ensure_data_device(qhandle);
                }
                "wl_data_device_manager" => {
                    let data_device_manager = wl_registry
                        .bind::<wl_data_device_manager::WlDataDeviceManager, _, _>(
                        name,
                        version.min(3),
                        qhandle,
                        (),
                    );
                    state.data_device_manager = Some(data_device_manager);
                    state.ensure_data_device(qhandle);
                }
                "zxdg_decoration_manager_v1" => {
                    let decoration_manager = wl_registry
                        .bind::<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _, _>(
                        name,
                        1,
                        qhandle,
                        (),
                    );
                    state.decoration_manager = Some(decoration_manager);
                }
                "wp_cursor_shape_manager_v1" => {
                    let cursor =
                        wl_registry.bind::<WpCursorShapeManagerV1, _, _>(name, 1, qhandle, ());
                    state.cursor_manager = Some(cursor);
                }
                "wp_fractional_scale_manager_v1" => {
                    let scale_manager = wl_registry
                        .bind::<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(
                        name,
                        1,
                        qhandle,
                        (),
                    );
                    state.scale_manager = Some(scale_manager);
                }
                "wp_viewporter" => {
                    let viewporter =
                        wl_registry.bind::<wp_viewporter::WpViewporter, _, _>(name, 1, qhandle, ());
                    state.viewporter = Some(viewporter);
                }
                "wl_shm" => {
                    let shm = wl_registry.bind::<wl_shm::WlShm, _, _>(name, 1, qhandle, ());
                    state.shm = Some(shm);
                }
                "xdg_toplevel_icon_manager_v1" => {
                    let icon_manager = wl_registry
                        .bind::<xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1, _, _>(
                        name,
                        1,
                        qhandle,
                        (),
                    );
                    state.icon_manager = Some(icon_manager);
                }
                "zwp_text_input_manager_v3" => {
                    let text_input_manager = wl_registry
                        .bind::<zwp_text_input_manager_v3::ZwpTextInputManagerV3, _, _>(
                        name,
                        1,
                        qhandle,
                        (),
                    );
                    state.text_input_manager = Some(text_input_manager);
                }
                "zwp_primary_selection_device_manager_v1" => {
                    let manager = wl_registry
                        .bind::<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1, _, _>(
                        name,
                        1,
                        qhandle,
                        (),
                    );
                    state.primary_selection_manager = Some(manager);
                    state.ensure_primary_selection_device(qhandle);
                }
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

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, WindowId> for WaylandState {
    fn event(
        state: &mut Self,
        fractional_scale: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        window_id: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_fractional_scale_v1::Event::PreferredScale { scale } => {
                if let Some(window) = state
                    .windows
                    .iter_mut()
                    .find(|win| win.window_id == *window_id)
                {
                    let old_geom = window.window_geom.clone();
                    let mut new_geom = window.window_geom.clone();
                    new_geom.dpi_factor = scale as f64 / 120.;
                    state.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom,
                        new_geom,
                    }));
                } else if let Some(window) = state
                    .popups
                    .iter_mut()
                    .find(|win| win.window_id == *window_id)
                {
                    let old_geom = window.window_geom.clone();
                    let mut new_geom = window.window_geom.clone();
                    new_geom.dpi_factor = scale as f64 / 120.;
                    state.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom,
                        new_geom,
                    }));
                }
            }
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
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                if let Some(window) = state.windows.iter().find(|win| win.window_id == *window_id) {
                    let inner_size = if width > 0 && height > 0 {
                        dvec2(width as f64, height as f64)
                    } else {
                        window.window_geom.inner_size
                    };
                    let is_maximized =
                        WaylandState::xdg_toplevel_has_state(&states, 1 /* maximized */);
                    let is_fullscreen =
                        WaylandState::xdg_toplevel_has_state(&states, 2 /* fullscreen */);
                    state.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom: window.window_geom.clone(),
                        new_geom: WindowGeom {
                            dpi_factor: window.window_geom.dpi_factor,
                            can_fullscreen: false,
                            xr_is_presenting: false,
                            is_fullscreen: is_fullscreen || is_maximized,
                            is_topmost: false,
                            position: dvec2(0., 0.),
                            inner_size,
                            outer_size: inner_size,
                        },
                    }));
                }
            }
            xdg_toplevel::Event::Close => {
                state.do_callback(XlibEvent::WindowClosed(WindowClosedEvent {
                    window_id: *window_id,
                }))
            }
            _ => {}
        }
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
            xdg_surface.ack_configure(serial);
            let mut first_configure_event = None;
            if let Some(window) = state
                .windows
                .iter_mut()
                .find(|win| win.window_id == *window_id)
            {
                if !window.configured {
                    let mut old_geom = window.window_geom.clone();
                    old_geom.inner_size = dvec2(0., 0.);
                    old_geom.outer_size = dvec2(0., 0.);
                    first_configure_event = Some(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom,
                        new_geom: window.window_geom.clone(),
                    });
                }
                window.configured = true;
            } else if let Some(window) = state
                .popups
                .iter_mut()
                .find(|win| win.window_id == *window_id)
            {
                if !window.configured {
                    let mut old_geom = window.window_geom.clone();
                    old_geom.inner_size = dvec2(0., 0.);
                    old_geom.outer_size = dvec2(0., 0.);
                    first_configure_event = Some(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom,
                        new_geom: window.window_geom.clone(),
                    });
                }
                window.configured = true;
            }
            if let Some(event) = first_configure_event {
                state.do_callback(XlibEvent::WindowGeomChange(event));
            }
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, WindowId> for WaylandState {
    fn event(
        state: &mut Self,
        _xdg_popup: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        window_id: &WindowId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                let mut geom_change = None;
                if let Some(popup) = state
                    .popups
                    .iter_mut()
                    .find(|popup| popup.window_id == *window_id)
                {
                    let old_geom = popup.window_geom.clone();
                    popup.window_geom.position = dvec2(x as f64, y as f64);
                    if width > 0 && height > 0 {
                        popup.window_geom.inner_size = dvec2(width as f64, height as f64);
                        popup.window_geom.outer_size = popup.window_geom.inner_size;
                    }
                    if popup.window_geom != old_geom {
                        geom_change = Some(WindowGeomChangeEvent {
                            window_id: *window_id,
                            old_geom,
                            new_geom: popup.window_geom.clone(),
                        });
                    }
                }
                if let Some(event) = geom_change {
                    state.do_callback(XlibEvent::WindowGeomChange(event));
                }
            }
            xdg_popup::Event::PopupDone => {
                // WindowClosed must fire before PopupDismissed so the
                // platform can access the CxWindow pool entry (valid
                // generation) before the app drops its WindowHandle
                // which frees the pool slot.
                state.do_callback(XlibEvent::WindowClosed(WindowClosedEvent {
                    window_id: *window_id,
                }));
                state.do_callback(XlibEvent::PopupDismissed(PopupDismissedEvent {
                    window_id: *window_id,
                    reason: PopupDismissReason::Compositor,
                }));
            }
            _ => {}
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
        state.ensure_data_device(qhandle);
        if let Some(input_manager) = state.text_input_manager.as_ref() {
            state.text_input = Some(input_manager.get_text_input(&seat, qhandle, ()));
        }
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qhandle, ());
            }
            if capabilities.contains(wl_seat::Capability::Pointer) {
                let pointer = seat.get_pointer(qhandle, ());
                if let Some(manager) = state.cursor_manager.as_ref() {
                    state.cursor_shape = Some(manager.get_pointer(&pointer, qhandle, ()));
                }
                state.pointer = Some(pointer);
            }
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                if state.data_offers.iter().all(|entry| entry.offer != id) {
                    state.data_offers.push(ClipboardOffer {
                        offer: id,
                        mime_types: Vec::new(),
                    });
                }
            }
            wl_data_device::Event::Selection { id } => {
                state.clipboard_offer = id.map(|offer| {
                    if let Some(index) = state
                        .data_offers
                        .iter()
                        .position(|entry| entry.offer == offer)
                    {
                        state.data_offers.swap_remove(index)
                    } else {
                        ClipboardOffer {
                            offer,
                            mime_types: Vec::new(),
                        }
                    }
                });
                state.data_offers.clear();
            }
            _ => {}
        }
    }

    fn event_created_child(
        opcode: u16,
        qhandle: &QueueHandle<Self>,
    ) -> Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            wl_data_device::EVT_DATA_OFFER_OPCODE => {
                qhandle.make_data::<wl_data_offer::WlDataOffer, ()>(())
            }
            _ => unreachable!("wl_data_device created unknown child for opcode {}", opcode),
        }
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_offer::Event::Offer { mime_type } => {
                if let Some(active_offer) = state.clipboard_offer.as_mut() {
                    if active_offer.offer == *proxy
                        && !active_offer.mime_types.iter().any(|m| m == &mime_type)
                    {
                        active_offer.mime_types.push(mime_type.clone());
                    }
                }
                if let Some(offer) = state
                    .data_offers
                    .iter_mut()
                    .find(|entry| entry.offer == *proxy)
                {
                    if !offer.mime_types.iter().any(|m| m == &mime_type) {
                        offer.mime_types.push(mime_type);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                if Self::is_text_mime_type(&mime_type) {
                    let raw_fd = fd.as_raw_fd();
                    unsafe {
                        let flags = libc_sys::fcntl(raw_fd, libc_sys::F_GETFL, 0);
                        if flags >= 0 {
                            let _ = libc_sys::fcntl(
                                raw_fd,
                                libc_sys::F_SETFL,
                                flags | libc_sys::O_NONBLOCK,
                            );
                        }
                        let bytes = state.clipboard_text.as_bytes();
                        let _ = libc_sys::write(
                            raw_fd,
                            bytes.as_ptr() as *const std::os::raw::c_void,
                            bytes.len(),
                        );
                    }
                }
            }
            wl_data_source::Event::Cancelled => {
                if state
                    .clipboard_source
                    .as_ref()
                    .is_some_and(|source| source == proxy)
                {
                    state.clipboard_source = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_data_device_manager::WlDataDeviceManager,
        _event: wl_data_device_manager::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &zwp_text_input_v3::ZwpTextInputV3,
        event: <zwp_text_input_v3::ZwpTextInputV3 as Proxy>::Event,
        data: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zwp_text_input_v3::Event::Enter { surface } => {}
            zwp_text_input_v3::Event::Leave { surface } => {}
            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {}
            zwp_text_input_v3::Event::CommitString { text } => {
                if let Some(text_str) = text.filter(|text| !text.chars().all(char::is_control)) {
                    state.do_callback(XlibEvent::TextInput(TextInputEvent {
                        input: text_str,
                        replace_last: false,
                        was_paste: false,
                        ..Default::default()
                    }));
                }
            }
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => {}
            zwp_text_input_v3::Event::Done { serial } => {}
            _ => {}
        }
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        _event: <zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        // We only set primary selection, not read it.
    }

    fn event_created_child(
        opcode: u16,
        qhandle: &QueueHandle<Self>,
    ) -> Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => {
                qhandle
                    .make_data::<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()>(())
            }
            _ => unreachable!(
                "zwp_primary_selection_device_v1 created unknown child for opcode {}",
                opcode
            ),
        }
    }
}

impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        event: <zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_source_v1::Event::Send { mime_type: _, fd } => {
                use std::io::Write;
                let mut file = std::fs::File::from(fd);
                let _ = file.write_all(state.primary_selection_text.as_bytes());
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                state.primary_selection_source = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_text_input_manager_v3::ZwpTextInputManagerV3, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        event: <zwp_text_input_manager_v3::ZwpTextInputManagerV3 as Proxy>::Event,
        data: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        if let Some(seat) = state.seat.as_ref() {
            state.text_input = Some(proxy.get_text_input(seat, qhandle, ()));
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
        match event {
            wl_keyboard::Event::Enter {
                serial,
                surface,
                keys: _,
            } => {
                state.keyboard_serial = Some(serial);
                state.flush_pending_clipboard_copy(qhandle, serial);
                if let Some(window_id) = state.window_id_for_surface(&surface) {
                    if state.keyboard_window != Some(window_id) {
                        if let Some(prev) = state.keyboard_window {
                            state.do_callback(XlibEvent::WindowLostFocus(prev));
                        }
                        state.keyboard_window = Some(window_id);
                        state.do_callback(XlibEvent::WindowGotFocus(window_id));
                    }
                }
            }
            wl_keyboard::Event::Leave { serial, surface } => {
                state.keyboard_serial = Some(serial);
                state.flush_pending_clipboard_copy(qhandle, serial);
                if let Some(window_id) = state.window_id_for_surface(&surface) {
                    if state.keyboard_window == Some(window_id) {
                        state.keyboard_window = None;
                        state.do_callback(XlibEvent::WindowLostFocus(window_id));
                    }
                }
                {
                    let popup_ids: Vec<_> =
                        state.popups.iter().rev().map(|p| p.window_id).collect();
                    for window_id in popup_ids {
                        state.do_callback(XlibEvent::PopupDismissed(PopupDismissedEvent {
                            window_id,
                            reason: PopupDismissReason::FocusLost,
                        }));
                    }
                }
            }
            wl_keyboard::Event::Key {
                serial,
                time: _,
                key,
                state: key_state,
            } => {
                if let WEnum::Value(key_state) = key_state {
                    match key_state {
                        wl_keyboard::KeyState::Pressed => {
                            state.keyboard_serial = Some(serial);
                            state.flush_pending_clipboard_copy(qhandle, serial);
                            let (key_code, text_str) =
                                if let Some(xkb_state) = state.xkb_state.as_mut() {
                                    (
                                        xkb_state.keycode_to_makepad_keycode(key + 8),
                                        xkb_state.key_get_utf8(key + 8),
                                    )
                                } else {
                                    return;
                                };

                            let primary_mod = state.modifiers.control || state.modifiers.logo;
                            if primary_mod {
                                match key_code {
                                    KeyCode::KeyV => state.request_clipboard_paste(conn),
                                    KeyCode::KeyC => {
                                        let response = Rc::new(RefCell::new(None));
                                        state.do_callback(XlibEvent::TextCopy(
                                            TextClipboardEvent {
                                                response: response.clone(),
                                            },
                                        ));
                                        let content = response.borrow().clone();
                                        if let Some(content) = content {
                                            state.set_clipboard_text(qhandle, serial, content);
                                        }
                                    }
                                    KeyCode::KeyX => {
                                        let response = Rc::new(RefCell::new(None));
                                        state.do_callback(XlibEvent::TextCut(TextClipboardEvent {
                                            response: response.clone(),
                                        }));
                                        let content = response.borrow().clone();
                                        if let Some(content) = content {
                                            state.set_clipboard_text(qhandle, serial, content);
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            let block_text = primary_mod || state.modifiers.alt;
                            state.do_callback(XlibEvent::KeyDown(KeyEvent {
                                key_code,
                                is_repeat: false,
                                modifiers: state.modifiers,
                                time: state.time_now(),
                            }));

                            if !block_text && text_str.chars().any(|ch| !ch.is_control()) {
                                state.do_callback(XlibEvent::TextInput(TextInputEvent {
                                    input: text_str,
                                    replace_last: false,
                                    was_paste: false,
                                    ..Default::default()
                                }));
                            }
                        }
                        wl_keyboard::KeyState::Released => {
                            if let Some(xkb_state) = state.xkb_state.as_mut() {
                                let key_code = xkb_state.keycode_to_makepad_keycode(key + 8);
                                state.do_callback(XlibEvent::KeyUp(KeyEvent {
                                    key_code,
                                    is_repeat: false,
                                    modifiers: state.modifiers,
                                    time: state.time_now(),
                                }));
                            }
                        }
                        _ => {}
                    };
                }
            }
            // wl_keyboard::Event::RepeatInfo { rate, delay } => {},
            wl_keyboard::Event::Modifiers {
                serial: _,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                if let Some(xkb_state) = state.xkb_state.as_mut() {
                    xkb_state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                    state.modifiers = xkb_state.get_key_modifiers();
                }
            }
            wl_keyboard::Event::Keymap { format, fd, size } => match format {
                WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) => {
                    let map_str = unsafe {
                        libc_sys::mmap(
                            std::ptr::null_mut(),
                            size as libc_sys::size_t,
                            libc_sys::PROT_READ,
                            libc_sys::MAP_SHARED,
                            fd.as_raw_fd(),
                            0,
                        )
                    };
                    let keymap = xkb_sys::XkbKeymap::from_cstr(&state.xkb_cx, map_str).unwrap();
                    unsafe {
                        munmap(map_str, size as libc_sys::size_t);
                    }
                    state.xkb_state = xkb_sys::XkbState::new(&keymap);
                }
                _ => {}
            },
            _ => {}
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, ()> for WaylandState {
    fn event(
        state: &mut Self,
        pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x: _,
                surface_y: _,
            } => {
                state.pointer_serial = Some(serial);
                state.flush_pending_clipboard_copy(qhandle, serial);
                state.pointer_window = state.window_id_for_surface(&surface);
            }
            wl_pointer::Event::Leave { serial, surface: _ } => {
                state.pointer_serial = Some(serial);
                state.flush_pending_clipboard_copy(qhandle, serial);
                state.pointer_window = None;
                state.last_resize_edge = None;
            }
            wl_pointer::Event::Motion {
                time,
                surface_x,
                surface_y,
            } => {
                if let Some(window_id) = state.pointer_window {
                    let pos = dvec2(surface_x as f64, surface_y as f64);
                    state.last_mouse_pos = pos;

                    // Edge-resize detection (matches X11 backend thresholds)
                    let window_size = state
                        .windows
                        .iter()
                        .find(|w| w.window_id == window_id)
                        .map(|w| w.window_geom.inner_size);
                    if let Some(ws) = window_size {
                        let edge = if pos.x < 10.0 && pos.y < 10.0 {
                            Some((
                                xdg_toplevel::ResizeEdge::TopLeft,
                                wp_cursor_shape_device_v1::Shape::NwResize,
                            ))
                        } else if pos.x < 10.0 && pos.y >= ws.y - 10.0 {
                            Some((
                                xdg_toplevel::ResizeEdge::BottomLeft,
                                wp_cursor_shape_device_v1::Shape::SwResize,
                            ))
                        } else if pos.x < 5.0 {
                            Some((
                                xdg_toplevel::ResizeEdge::Left,
                                wp_cursor_shape_device_v1::Shape::WResize,
                            ))
                        } else if pos.x >= ws.x - 10.0 && pos.y < 10.0 {
                            Some((
                                xdg_toplevel::ResizeEdge::TopRight,
                                wp_cursor_shape_device_v1::Shape::NeResize,
                            ))
                        } else if pos.x >= ws.x - 10.0 && pos.y >= ws.y - 10.0 {
                            Some((
                                xdg_toplevel::ResizeEdge::BottomRight,
                                wp_cursor_shape_device_v1::Shape::SeResize,
                            ))
                        } else if pos.x >= ws.x - 5.0 {
                            Some((
                                xdg_toplevel::ResizeEdge::Right,
                                wp_cursor_shape_device_v1::Shape::EResize,
                            ))
                        } else if pos.y < 5.0 {
                            Some((
                                xdg_toplevel::ResizeEdge::Top,
                                wp_cursor_shape_device_v1::Shape::NResize,
                            ))
                        } else if pos.y >= ws.y - 5.0 {
                            Some((
                                xdg_toplevel::ResizeEdge::Bottom,
                                wp_cursor_shape_device_v1::Shape::SResize,
                            ))
                        } else {
                            None
                        };
                        if let Some((resize_edge, cursor_shape)) = edge {
                            state.last_resize_edge = Some(resize_edge);
                            if let (Some(cursor_dev), Some(serial)) =
                                (state.cursor_shape.as_ref(), state.pointer_serial)
                            {
                                cursor_dev.set_shape(serial, cursor_shape);
                            }
                        } else {
                            if state.last_resize_edge.is_some() {
                                if let (Some(cursor_dev), Some(serial)) =
                                    (state.cursor_shape.as_ref(), state.pointer_serial)
                                {
                                    cursor_dev.set_shape(
                                        serial,
                                        wp_cursor_shape_device_v1::Shape::Default,
                                    );
                                }
                            }
                            state.last_resize_edge = None;
                        }
                    }

                    state.do_callback(XlibEvent::MouseMove(MouseMoveEvent {
                        abs: pos,
                        window_id: window_id,
                        modifiers: state.modifiers,
                        time: state.time_now(),
                        handled: Cell::new(Area::Empty),
                    }));
                    if let Some(items) = state.internal_drag_items.as_ref() {
                        state.do_callback(XlibEvent::Drag(DragEvent {
                            modifiers: state.modifiers,
                            handled: Arc::new(Mutex::new(false)),
                            abs: pos,
                            items: items.clone(),
                            response: Arc::new(Mutex::new(DragResponse::None)),
                        }));
                    }
                }
            }
            wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: key_state,
            } => {
                state.pointer_serial = Some(serial);
                state.flush_pending_clipboard_copy(qhandle, serial);
                // Outside-click popup dismissal: if press lands on a
                // regular window while popups are open, fire dismiss.
                if let WEnum::Value(ButtonState::Pressed) = key_state {
                    if let Some(win_id) = state.pointer_window {
                        if state.windows.iter().any(|w| w.window_id == win_id)
                            && !state.popups.is_empty()
                        {
                            let popup_ids: Vec<_> =
                                state.popups.iter().rev().map(|p| p.window_id).collect();
                            for popup_wid in popup_ids {
                                state.do_callback(XlibEvent::PopupDismissed(PopupDismissedEvent {
                                    window_id: popup_wid,
                                    reason: PopupDismissReason::OutsideClick,
                                }));
                            }
                        }
                    }
                }
                if let Some(btn) = wayland_type::from_mouse(button) {
                    if let Some(window_id) = state.pointer_window {
                        match key_state {
                            WEnum::Value(ButtonState::Pressed) => {
                                if btn == MouseButton::PRIMARY {
                                    if state.windows.iter().any(|win| win.window_id == window_id) {
                                        // Edge resize takes priority
                                        if let Some(resize_edge) = state.last_resize_edge.take() {
                                            if let (Some(seat), Some(window)) = (
                                                state.seat.as_ref(),
                                                state
                                                    .windows
                                                    .iter()
                                                    .find(|win| win.window_id == window_id),
                                            ) {
                                                window.toplevel.resize(seat, serial, resize_edge);
                                                return;
                                            }
                                        }

                                        let response =
                                            Rc::new(Cell::new(WindowDragQueryResponse::NoAnswer));
                                        state.do_callback(XlibEvent::WindowDragQuery(
                                            WindowDragQueryEvent {
                                                window_id,
                                                abs: state.last_mouse_pos,
                                                response: response.clone(),
                                            },
                                        ));
                                        if matches!(
                                            response.get(),
                                            WindowDragQueryResponse::Caption
                                        ) {
                                            if let (Some(seat), Some(window)) = (
                                                state.seat.as_ref(),
                                                state
                                                    .windows
                                                    .iter()
                                                    .find(|win| win.window_id == window_id),
                                            ) {
                                                window.toplevel._move(seat, serial);
                                                return;
                                            }
                                        }
                                    }
                                }
                                state.do_callback(XlibEvent::MouseDown(MouseDownEvent {
                                    abs: state.last_mouse_pos,
                                    button: btn,
                                    window_id: window_id,
                                    modifiers: state.modifiers,
                                    handled: Cell::new(Area::Empty),
                                    time: state.time_now(),
                                }))
                            }
                            WEnum::Value(ButtonState::Released) => {
                                state.do_callback(XlibEvent::MouseUp(MouseUpEvent {
                                    abs: state.last_mouse_pos,
                                    button: btn,
                                    window_id,
                                    modifiers: state.modifiers,
                                    time: state.time_now(),
                                }));
                                if btn == MouseButton::PRIMARY {
                                    if let Some(items) = state.internal_drag_items.take() {
                                        state.do_callback(XlibEvent::Drop(DropEvent {
                                            modifiers: state.modifiers,
                                            handled: Arc::new(Mutex::new(false)),
                                            abs: state.last_mouse_pos,
                                            items,
                                        }));
                                        state.do_callback(XlibEvent::DragEnd);
                                    }
                                }
                            }
                            WEnum::Unknown(_) | WEnum::Value(_) => {}
                        }
                    }
                }
            }
            // Wayland axis values use motion-event coordinates: positive
            // vertical = downward on screen = content slides down = viewport
            // moves UP. Makepad's internal convention is positive = viewport
            // moves DOWN (matching X11 button mapping and macOS after its
            // negation of scrollingDeltaY). Negate to align conventions,
            // same as winit does for the same reason.
            wl_pointer::Event::Axis {
                time: _,
                axis,
                value,
            } => match axis {
                WEnum::Value(wl_pointer::Axis::VerticalScroll) => {
                    state.scroll_accumulator.y -= value;
                }
                WEnum::Value(wl_pointer::Axis::HorizontalScroll) => {
                    state.scroll_accumulator.x -= value;
                }
                _ => {}
            },
            wl_pointer::Event::AxisSource { axis_source } => {
                state.scroll_is_wheel = axis_source == WEnum::Value(wl_pointer::AxisSource::Wheel);
            }
            wl_pointer::Event::Frame => {
                let acc = state.scroll_accumulator;
                if acc.x != 0.0 || acc.y != 0.0 {
                    if let Some(window_id) = state.pointer_window {
                        let time_now = state.time_now();
                        let scroll = if state.scroll_is_wheel {
                            let last = state.last_scroll_time;
                            state.last_scroll_time = time_now;
                            let speed = 1200.0 * (0.2 - 2.0 * (time_now - last)).max(0.01);
                            dvec2(acc.x.signum() * speed, acc.y.signum() * speed)
                        } else {
                            acc
                        };
                        state.do_callback(XlibEvent::Scroll(ScrollEvent {
                            window_id,
                            scroll,
                            abs: state.last_mouse_pos,
                            modifiers: state.modifiers,
                            is_mouse: state.scroll_is_wheel,
                            handled_x: Cell::new(false),
                            handled_y: Cell::new(false),
                            time: time_now,
                        }));
                    }
                }
                state.scroll_accumulator = dvec2(0.0, 0.0);
                state.scroll_is_wheel = false;
            }
            wl_pointer::Event::AxisStop { time: _, axis: _ } => {}
            wl_pointer::Event::AxisDiscrete {
                axis: _,
                discrete: _,
            } => {}
            wl_pointer::Event::AxisValue120 {
                axis: _,
                value120: _,
            } => {}
            wl_pointer::Event::AxisRelativeDirection {
                axis: _,
                direction: _,
            } => {}
            _ => {}
        }
    }
}

impl Dispatch<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        cursor_shape_manager: &wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
        event: wp_cursor_shape_manager_v1::Event,
        _: &(),
        conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        if let Some(pointer) = state.pointer.as_ref() {
            state.cursor_shape = Some(cursor_shape_manager.get_pointer(pointer, qhandle, ()));
        }
    }
}

delegate_noop!(WaylandState: ignore wp_viewport::WpViewport);
delegate_noop!(WaylandState: ignore wp_viewporter::WpViewporter);
delegate_noop!(WaylandState: ignore wl_surface::WlSurface);
delegate_noop!(WaylandState: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
delegate_noop!(WaylandState: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(WaylandState: ignore wl_compositor::WlCompositor);
delegate_noop!(WaylandState: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(WaylandState: ignore zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1);
delegate_noop!(WaylandState: ignore xdg_toplevel_icon_v1::XdgToplevelIconV1);
delegate_noop!(WaylandState: ignore wl_shm::WlShm);
delegate_noop!(WaylandState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(WaylandState: ignore wl_buffer::WlBuffer);
delegate_noop!(WaylandState: ignore xdg_positioner::XdgPositioner);
delegate_noop!(WaylandState: ignore zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1);
delegate_noop!(WaylandState: ignore zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1);

impl Dispatch<xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1,
        _event: xdg_toplevel_icon_manager_v1::Event,
        _: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        // icon_size events are informational; we ignore them for now
    }
}

impl WaylandState {
    fn ensure_data_device(&mut self, qhandle: &QueueHandle<Self>) {
        if self.data_device.is_none() {
            if let (Some(data_device_manager), Some(seat)) =
                (self.data_device_manager.as_ref(), self.seat.as_ref())
            {
                self.data_device = Some(data_device_manager.get_data_device(seat, qhandle, ()));
            }
        }
    }

    fn ensure_primary_selection_device(&mut self, qhandle: &QueueHandle<Self>) {
        if self.primary_selection_device.is_none() {
            if let (Some(manager), Some(seat)) =
                (self.primary_selection_manager.as_ref(), self.seat.as_ref())
            {
                self.primary_selection_device = Some(manager.get_device(seat, qhandle, ()));
            }
        }
    }

    pub(crate) fn set_primary_selection_text(
        &mut self,
        qhandle: &QueueHandle<Self>,
        serial: u32,
        text: String,
    ) {
        self.primary_selection_text = text;
        if let Some(device) = self.primary_selection_device.as_ref() {
            if let Some(manager) = self.primary_selection_manager.as_ref() {
                let source = manager.create_source(qhandle, ());
                source.offer("text/plain;charset=utf-8".to_string());
                source.offer("text/plain".to_string());
                source.offer("UTF8_STRING".to_string());
                source.offer("STRING".to_string());
                source.offer("TEXT".to_string());
                device.set_selection(Some(&source), serial);
                self.primary_selection_source = Some(source);
            }
        }
    }

    fn is_text_mime_type(mime_type: &str) -> bool {
        matches!(
            mime_type,
            "text/plain;charset=utf-8" | "text/plain" | "UTF8_STRING" | "STRING" | "TEXT"
        )
    }

    fn preferred_clipboard_mime_type(offer: &ClipboardOffer) -> Option<&str> {
        for preferred in [
            "text/plain;charset=utf-8",
            "text/plain",
            "UTF8_STRING",
            "STRING",
            "TEXT",
        ] {
            if let Some(mime_type) = offer.mime_types.iter().find(|m| m.as_str() == preferred) {
                return Some(mime_type.as_str());
            }
        }
        offer.mime_types.first().map(String::as_str)
    }

    pub(crate) fn set_clipboard_text(
        &mut self,
        qhandle: &QueueHandle<Self>,
        serial: u32,
        text: String,
    ) {
        self.ensure_data_device(qhandle);
        if let (Some(data_device_manager), Some(data_device)) =
            (self.data_device_manager.as_ref(), self.data_device.as_ref())
        {
            let source = data_device_manager.create_data_source(qhandle, ());
            source.offer("text/plain;charset=utf-8".to_string());
            source.offer("text/plain".to_string());
            source.offer("UTF8_STRING".to_string());
            source.offer("STRING".to_string());
            source.offer("TEXT".to_string());
            data_device.set_selection(Some(&source), serial);
            self.clipboard_source = Some(source);
            self.clipboard_text = text;
        }
    }

    /// Flush a pending clipboard copy now that a serial is available.
    pub(crate) fn flush_pending_clipboard_copy(
        &mut self,
        qhandle: &QueueHandle<Self>,
        serial: u32,
    ) {
        if let Some(text) = self.pending_clipboard_copy.take() {
            self.set_clipboard_text(qhandle, serial, text);
        }
    }

    pub(crate) fn start_internal_drag(&mut self, items: Vec<DragItem>) {
        self.internal_drag_items = Some(Arc::new(items));
    }

    fn dispatch_paste_bytes(&mut self, mut bytes: Vec<u8>) {
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        let input = String::from_utf8_lossy(&bytes).into_owned();
        if !input.is_empty() {
            self.pending_paste_text_input = Some(input);
        }
    }

    pub(crate) fn take_pending_paste_text_input(&mut self) -> Option<String> {
        self.pending_paste_text_input.take()
    }

    pub(crate) fn pump_pending_clipboard_read(&mut self) {
        let mut pending = match self.pending_clipboard_read.take() {
            Some(pending) => pending,
            None => return,
        };

        let read_raw_fd = pending.fd.as_raw_fd();
        let mut readfds = unsafe { std::mem::zeroed::<libc_sys::fd_set>() };
        let mut timeout = libc_sys::timeval {
            tv_sec: 0,
            tv_usec: 0,
        };
        unsafe {
            libc_sys::FD_ZERO(&mut readfds);
            libc_sys::FD_SET(read_raw_fd, &mut readfds);
        }
        let ready = unsafe {
            libc_sys::select(
                read_raw_fd + 1,
                &mut readfds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut timeout,
            )
        };
        if ready <= 0 {
            self.pending_clipboard_read = Some(pending);
            return;
        }

        loop {
            let mut chunk = [0u8; 4096];
            let count = unsafe {
                libc_sys::read(
                    read_raw_fd,
                    chunk.as_mut_ptr() as *mut std::os::raw::c_void,
                    chunk.len(),
                )
            };
            if count > 0 {
                pending.bytes.extend_from_slice(&chunk[..count as usize]);
                continue;
            }

            if pending.bytes.is_empty() {
                self.pending_clipboard_read = Some(pending);
            } else {
                self.dispatch_paste_bytes(pending.bytes);
            }
            return;
        }
    }

    fn request_clipboard_paste(&mut self, conn: &Connection) {
        if let Some(offer) = self.clipboard_offer.as_ref() {
            if let Some(mime_type) = Self::preferred_clipboard_mime_type(offer) {
                let mut pipe_fds = [0; 2];
                if unsafe { libc_sys::pipe(pipe_fds.as_mut_ptr()) } != 0 {
                    return;
                }
                let read_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(pipe_fds[0]) };
                let read_raw_fd = read_fd.as_raw_fd();
                let write_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(pipe_fds[1]) };
                offer.offer.receive(mime_type.to_string(), write_fd.as_fd());
                drop(write_fd);
                let _ = conn.flush();

                unsafe {
                    let flags = libc_sys::fcntl(read_raw_fd, libc_sys::F_GETFL, 0);
                    if flags >= 0 {
                        let _ = libc_sys::fcntl(
                            read_raw_fd,
                            libc_sys::F_SETFL,
                            flags | libc_sys::O_NONBLOCK,
                        );
                    }
                }
                self.pending_clipboard_read = Some(PendingClipboardRead {
                    fd: read_fd,
                    bytes: Vec::new(),
                });
                self.pump_pending_clipboard_read();
            }
        } else if !self.clipboard_text.is_empty() {
            self.do_callback(XlibEvent::TextInput(TextInputEvent {
                input: self.clipboard_text.clone(),
                replace_last: false,
                was_paste: true,
                ..Default::default()
            }));
        }
    }

    pub(crate) fn available(&self) -> bool {
        self.compositor.is_some() && self.wm_base.is_some()
    }

    fn xdg_toplevel_has_state(states: &[u8], needle: u32) -> bool {
        states
            .chunks_exact(4)
            .any(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) == needle)
    }

    fn do_callback(&mut self, event: XlibEvent) {
        if let Some(mut callback) = self.event_callback.take() {
            callback(self, event);
            self.event_callback = Some(callback);
        }
    }

    pub fn start_timer(&mut self, id: u64, timeout: f64, repeats: bool) {
        self.timers.start_timer(id, timeout, repeats);
    }

    pub fn stop_timer(&mut self, id: u64) {
        self.timers.stop_timer(id);
    }
    pub fn time_now(&self) -> f64 {
        self.timers.time_now()
    }
}
