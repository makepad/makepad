use std::{cell::{Cell, RefCell}, fs::File, io::Read, os::{fd::{AsFd, AsRawFd, FromRawFd}, unix::fs::FileExt}, rc::Rc};
use crate::{libc_sys::{self, munmap}, makepad_math::{dvec2, DVec2}, wayland::{wayland_type, xkb_sys}, Area, KeyEvent, KeyModifiers, MouseDownEvent, MouseMoveEvent, MouseUpEvent, TextInputEvent, WindowClosedEvent};

use wayland_client::{delegate_noop, protocol::{wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer::{self, ButtonState}, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface}, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::{wp::{cursor_shape::v1::client::{wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1::{self, WpCursorShapeManagerV1}}, fractional_scale::v1::client::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1}, text_input::zv3::client::{zwp_text_input_manager_v3, zwp_text_input_v3}, viewporter::client::{wp_viewport, wp_viewporter}}, xdg::{self, decoration::zv1::client::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1}, shell::client::{xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base}}};

use crate::{cx_native::EventFlow, event::WindowGeom, select_timer::SelectTimers, wayland::wayland_app::WaylandApp, x11::xlib_event::XlibEvent, WindowCloseRequestedEvent, WindowGeomChangeEvent, WindowId, WindowMovedEvent};

use super::opengl_wayland::WaylandWindow;

pub(crate) struct WaylandState {
    pub(crate) compositor: Option<wl_compositor::WlCompositor>,
    pub(crate) wm_base: Option<xdg_wm_base::XdgWmBase>,
    pub(crate) seat: Option<wl_seat::WlSeat>,
    pub(crate) cursor_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    pub(crate) cursor_shape: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    pub(crate) pointer: Option<wl_pointer::WlPointer>,
    pub(crate) last_mouse_pos: DVec2,
    pub(crate) pointer_serial: Option<u32>,
    pub(crate) decoration_manager: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    pub(crate) windows: Vec<WaylandWindow>,
    pub(crate) current_window: Option<WindowId>,
    pub(crate) modifiers: KeyModifiers,
    pub(crate) timers: SelectTimers,
    pub(crate) scale_manager: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    pub(crate) viewporter: Option<wp_viewporter::WpViewporter>,
    pub(crate) xkb_state: Option<xkb_sys::XkbState>,
    pub(crate) xkb_cx: xkb_sys::XkbContext,
    pub(crate) text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    pub(crate) text_input_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    event_callback: Option<Box<dyn FnMut(&mut WaylandState, XlibEvent)>>,

    pub(crate) event_flow: EventFlow,
    pub(crate) event_loop_running: bool,
}

impl WaylandState {
    pub fn new(event_callback: Box<dyn FnMut(&mut WaylandState, XlibEvent)>) -> Self {
        Self {
            compositor: None,
            wm_base: None,
            seat: None,
            cursor_manager: None,
            cursor_shape: None,
            pointer: None,
            decoration_manager: None,
            scale_manager: None,
            viewporter: None,
            windows: Vec::new(),
            current_window: None,
            pointer_serial: None,
            modifiers: KeyModifiers::default(),
            xkb_state: None,
            xkb_cx: xkb_sys::XkbContext::new().unwrap(),
            text_input: None,
            text_input_manager: None,
            last_mouse_pos: dvec2(0., 0.),
            timers: SelectTimers::new(),
            event_callback: Some(event_callback),
            event_flow: EventFlow::Wait,
            event_loop_running: true,
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()>  for WaylandState {
    fn event(
        state: &mut Self,
        wl_registry: &wl_registry::WlRegistry,
        event: wl_registry::Event, _: &(),
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
                "wp_cursor_shape_manager_v1" => {
                    let cursor = wl_registry.bind::<WpCursorShapeManagerV1, _, _>(name, 1, qhandle, ());
                    state.cursor_manager = Some(cursor);
                },
                "wp_fractional_scale_manager_v1" => {
                    let scale_manager = wl_registry.bind::<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(name, 1, qhandle, ());
                    state.scale_manager = Some(scale_manager);
                },
                "wp_viewporter" => {
                    let viewporter = wl_registry.bind::<wp_viewporter::WpViewporter, _, _>(name, 1, qhandle, ());
                    state.viewporter = Some(viewporter);
                },
                "zwp_text_input_manager_v3" => {
                    let text_input_manager = wl_registry.bind::<zwp_text_input_manager_v3::ZwpTextInputManagerV3, _, _>(name, 1, qhandle, ());
                    state.text_input_manager = Some(text_input_manager);
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
                if let Some(window) = state.windows.iter_mut().find(|win| win.window_id == *window_id) {
                    println!("preffered scale: {}", scale as f64 / 120.);
                    let old_geom = window.window_geom.clone();
                    let mut new_geom = window.window_geom.clone();
                    new_geom.dpi_factor = scale as f64 / 120.;
                    state.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom,
                        new_geom,
                    }));
                }
            },
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
            xdg_toplevel::Event::Configure { width, height, states } => {
                if width <= 0 || height <= 0 {
                    return;
                }
                if let Some(window) = state.windows.iter().find(|win| win.window_id == *window_id) {
                    state.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent{
                        window_id: *window_id,
                        old_geom: window.window_geom.clone(),
                        new_geom: WindowGeom {
                            dpi_factor: window.window_geom.dpi_factor,
                            can_fullscreen: false,
                            xr_is_presenting: false,
                            is_fullscreen: false,
                            is_topmost: false,
                            position: dvec2(0., 0.),
                            inner_size: dvec2(width as f64, height as f64),
                            outer_size: dvec2(width as f64, height as f64),
                        },
                    }));
                }
            }
            xdg_toplevel::Event::Close => {
                state.do_callback(XlibEvent::WindowClosed(WindowClosedEvent{window_id: *window_id}))
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
        if let Some(input_manager) = state.text_input_manager.as_ref() {
            state.text_input = Some(input_manager.get_text_input(&seat, qhandle, ()));
        }
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event {
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
            zwp_text_input_v3::Event::Enter { surface } => {
            },
            zwp_text_input_v3::Event::Leave { surface } => {
            },
            zwp_text_input_v3::Event::PreeditString { text, cursor_begin, cursor_end } => {
            },
            zwp_text_input_v3::Event::CommitString { text } => {
                if let Some(text_str) = text {
                    state.do_callback(XlibEvent::TextInput(TextInputEvent{ input: text_str, replace_last: false, was_paste: false }));
                }
            },
            zwp_text_input_v3::Event::DeleteSurroundingText { before_length, after_length } => {},
            zwp_text_input_v3::Event::Done { serial } => {
            },
            _ => {},
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
            wl_keyboard::Event::Enter { serial, surface, keys } => {
                // state.do_callback(XlibEvent::AppGotFocus);
            },
            wl_keyboard::Event::Leave { serial, surface } => {
                // state.do_callback(XlibEvent::AppLostFocus);
            },
            wl_keyboard::Event::Key { serial: _, time: _, key, state: key_state } => {
                if let Some(xkb_state) = state.xkb_state.as_mut() {
                        if let WEnum::Value(key_state) = key_state {
                            match key_state {
                                wl_keyboard::KeyState::Pressed => {
                                    let key_code = xkb_state.keycode_to_makepad_keycode(key + 8);
                                    let text_str = xkb_state.key_get_utf8(key + 8);

                                    // todo(drindr): distinguish `block_text`
                                    state.do_callback(XlibEvent::TextInput(TextInputEvent{ input: text_str, replace_last: false, was_paste: false }));
                                    state.do_callback(XlibEvent::KeyDown(KeyEvent{
                                        key_code: key_code,
                                        is_repeat: false,
                                        modifiers: state.modifiers,
                                        time: state.time_now(),
                                    }));
                                },
                                wl_keyboard::KeyState::Released => {
                                    let key_code = xkb_state.keycode_to_makepad_keycode(key + 8);
                                    state.do_callback(XlibEvent::KeyUp(KeyEvent{
                                        key_code: key_code,
                                        is_repeat: false,
                                        modifiers: state.modifiers,
                                        time: state.time_now(),
                                    }))
                                },
                                _ => {}
                            };
                        }

                }
            },
            // wl_keyboard::Event::RepeatInfo { rate, delay } => {},
            wl_keyboard::Event::Modifiers { serial: _, mods_depressed, mods_latched, mods_locked, group } => {
                if let Some(xkb_state) = state.xkb_state.as_mut() {
                    xkb_state.update_mask(
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        0,
                        0,
                        group
                    );
                    state.modifiers = xkb_state.get_key_modifiers();
                }
            }
            wl_keyboard::Event::Keymap { format, fd, size } => {
                match format {
                    WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) => {
                        let map_str = unsafe {
                            libc_sys::mmap(std::ptr::null_mut(), size as libc_sys::size_t, libc_sys::PROT_READ, libc_sys::MAP_SHARED, fd.as_raw_fd(), 0)
                        };
                        let keymap = xkb_sys::XkbKeymap::from_cstr(&state.xkb_cx, map_str).unwrap();
                        unsafe {
                            munmap(map_str, size as libc_sys::size_t);
                        }
                        state.xkb_state = xkb_sys::XkbState::new(&keymap);
                    },
                    _ => {}
                }
            }
            _ => {},
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
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                state.pointer_serial = Some(serial);
                let mut window_id = None;
                state.windows.iter().for_each(|win| {
                    if win.base_surface.id() == surface.id() {
                        window_id = Some(win.window_id);
                        state.current_window = window_id;
                    }
                });
                state.do_callback(XlibEvent::AppGotFocus);
            },
            wl_pointer::Event::Leave { serial, surface: _ } => {
                state.pointer_serial = Some(serial);
                state.do_callback(XlibEvent::AppLostFocus);
            },
            wl_pointer::Event::Motion { time, surface_x, surface_y } => {
                if let Some(window_id) = state.current_window {
                    let pos = dvec2(surface_x as f64, surface_y as f64);
                    state.last_mouse_pos = pos;
                    state.do_callback(XlibEvent::MouseMove(MouseMoveEvent{
                        abs: pos,
                        window_id: window_id,
                        modifiers: state.modifiers,
                        time: state.time_now(),
                        handled: Cell::new(Area::Empty),
                    }));
                }
            },
            wl_pointer::Event::Button { serial, time, button, state: key_state } => {
                if let Some(btn) = wayland_type::from_mouse(button){
                    if let Some(window_id) = state.current_window {
                        match key_state {
                            WEnum::Value(ButtonState::Pressed) => {
                                state.do_callback(XlibEvent::MouseDown(MouseDownEvent{
                                    abs: state.last_mouse_pos,
                                    button: btn,
                                    window_id: window_id,
                                    modifiers: state.modifiers,
                                    handled: Cell::new(Area::Empty),
                                    time: state.time_now(),
                                }))
                            },
                            WEnum::Value(ButtonState::Released) => {
                                state.do_callback(XlibEvent::MouseUp(MouseUpEvent{
                                    abs: state.last_mouse_pos,
                                    button: btn,
                                    window_id,
                                    modifiers: state.modifiers,
                                    time: state.time_now(),
                                }))
                            },
                            WEnum::Unknown(_) | WEnum::Value(_) => {}
                        }
                    }
                }
            },
            wl_pointer::Event::Axis { time, axis, value } => {},
            wl_pointer::Event::Frame => {},
            wl_pointer::Event::AxisSource { axis_source } => {},
            wl_pointer::Event::AxisStop { time, axis } => {},
            wl_pointer::Event::AxisDiscrete { axis, discrete } => {},
            wl_pointer::Event::AxisValue120 { axis, value120 } => {},
            wl_pointer::Event::AxisRelativeDirection { axis, direction } => {},
            _ => {},
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
// delegate_noop!(WaylandState: ignore xdg_positioner::XdgPositioner);

impl WaylandState {
    pub(crate) fn available(&self) -> bool {
        self.compositor.is_some() && self.wm_base.is_some() && self.seat.is_some() && self.decoration_manager.is_some()
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
