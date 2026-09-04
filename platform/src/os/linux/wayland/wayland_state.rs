#![allow(unused_imports, unused_variables)]
use crate::{
    libc_sys::{self, munmap},
    makepad_math::{dvec2, Vec2d},
    wayland::{wayland_type, xkb_sys},
    Area, DragEvent, DragItem, DragResponse, DropEvent, KeyEvent, KeyModifiers, MouseButton,
    MouseCursor, MouseDownEvent, MouseMoveEvent, MouseUpEvent, TextClipboardEvent, TextInputEvent,
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
        wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager,
        wl_data_offer, wl_data_source, wl_keyboard, wl_output,
        wl_pointer::{self, ButtonState},
        wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface,
        wl_surface,
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
    event::{
        PopupDismissReason, PopupDismissedEvent, ScrollEvent, ScrollPhase, WindowGeom,
        TAP_COUNT_DISTANCE, TAP_COUNT_TIME,
    },
    select_timer::SelectTimers,
    wayland::wayland_app::WaylandApp,
    x11::xlib_event::XlibEvent,
    KeyCode, WindowCloseRequestedEvent, WindowGeomChangeEvent, WindowId, WindowMovedEvent,
};

use super::super::windowing_backend::PIXELS_PER_WHEEL_DETENT;
use super::opengl_wayland::{WaylandPopupWindow, WaylandWindow};

/// Reserved timer ID for keyboard repeat. Uses a high value to avoid conflicts with app timers.
const KEY_REPEAT_TIMER_ID: u64 = u64::MAX - 1;

fn is_caption_double_click(
    previous: Option<(WindowId, Vec2d, u32)>,
    window_id: WindowId,
    pos: Vec2d,
    time: u32,
) -> bool {
    previous.is_some_and(|(last_window_id, last_pos, last_time)| {
        last_window_id == window_id
            && time.wrapping_sub(last_time) <= (TAP_COUNT_TIME * 1000.0) as u32
            && (pos - last_pos).length() < TAP_COUNT_DISTANCE
    })
}

#[derive(Clone, Copy, Debug)]
struct CaptionPress {
    window_id: WindowId,
    pos: Vec2d,
    time: u32,
    serial: u32,
    drag_started: bool,
}

impl CaptionPress {
    fn start_drag_if_needed(&mut self, window_id: WindowId, pos: Vec2d) -> Option<(WindowId, u32)> {
        if self.drag_started {
            return None;
        }
        if self.window_id != window_id {
            self.drag_started = true;
            return None;
        }
        if (pos - self.pos).length() < TAP_COUNT_DISTANCE {
            return None;
        }
        self.drag_started = true;
        Some((self.window_id, self.serial))
    }

    fn completed_click(self, window_id: WindowId, pos: Vec2d) -> Option<(WindowId, Vec2d, u32)> {
        (self.window_id == window_id
            && !self.drag_started
            && (pos - self.pos).length() < TAP_COUNT_DISTANCE)
            .then_some((self.window_id, self.pos, self.time))
    }
}

const RESIZE_EDGE_LEFT: u8 = 1 << 0;
const RESIZE_EDGE_RIGHT: u8 = 1 << 1;
const RESIZE_EDGE_TOP: u8 = 1 << 2;
const RESIZE_EDGE_BOTTOM: u8 = 1 << 3;

fn xdg_toplevel_edge_mask(states: &[u8], first_state: u32) -> u8 {
    [
        (first_state, RESIZE_EDGE_LEFT),
        (first_state + 1, RESIZE_EDGE_RIGHT),
        (first_state + 2, RESIZE_EDGE_TOP),
        (first_state + 3, RESIZE_EDGE_BOTTOM),
    ]
    .into_iter()
    .filter_map(|(state, edge)| WaylandState::xdg_toplevel_has_state(states, state).then_some(edge))
    .fold(0, |mask, edge| mask | edge)
}

fn resize_edge_mask(edge: xdg_toplevel::ResizeEdge) -> u8 {
    match edge {
        xdg_toplevel::ResizeEdge::Top => RESIZE_EDGE_TOP,
        xdg_toplevel::ResizeEdge::Bottom => RESIZE_EDGE_BOTTOM,
        xdg_toplevel::ResizeEdge::Left => RESIZE_EDGE_LEFT,
        xdg_toplevel::ResizeEdge::TopLeft => RESIZE_EDGE_TOP | RESIZE_EDGE_LEFT,
        xdg_toplevel::ResizeEdge::BottomLeft => RESIZE_EDGE_BOTTOM | RESIZE_EDGE_LEFT,
        xdg_toplevel::ResizeEdge::Right => RESIZE_EDGE_RIGHT,
        xdg_toplevel::ResizeEdge::TopRight => RESIZE_EDGE_TOP | RESIZE_EDGE_RIGHT,
        xdg_toplevel::ResizeEdge::BottomRight => RESIZE_EDGE_BOTTOM | RESIZE_EDGE_RIGHT,
        _ => 0,
    }
}

fn resize_edge_from_mask(mask: u8) -> Option<xdg_toplevel::ResizeEdge> {
    use xdg_toplevel::ResizeEdge;
    Some(
        match (
            mask & (RESIZE_EDGE_LEFT | RESIZE_EDGE_RIGHT),
            mask & (RESIZE_EDGE_TOP | RESIZE_EDGE_BOTTOM),
        ) {
            (RESIZE_EDGE_LEFT, RESIZE_EDGE_TOP) => ResizeEdge::TopLeft,
            (RESIZE_EDGE_LEFT, RESIZE_EDGE_BOTTOM) => ResizeEdge::BottomLeft,
            (RESIZE_EDGE_RIGHT, RESIZE_EDGE_TOP) => ResizeEdge::TopRight,
            (RESIZE_EDGE_RIGHT, RESIZE_EDGE_BOTTOM) => ResizeEdge::BottomRight,
            (RESIZE_EDGE_LEFT, 0) => ResizeEdge::Left,
            (RESIZE_EDGE_RIGHT, 0) => ResizeEdge::Right,
            (0, RESIZE_EDGE_TOP) => ResizeEdge::Top,
            (0, RESIZE_EDGE_BOTTOM) => ResizeEdge::Bottom,
            _ => return None,
        },
    )
}

/// Narrows `edge` to the components the compositor still allows. A tiled window shares
/// its inner borders with a neighbour and cannot resize them, but its outer ones stay
/// free; dropping the whole corner in that case would cost the user a grab they still
/// have, so a corner degrades to whichever of its two edges survives.
pub(crate) fn available_resize_edge(
    edge: xdg_toplevel::ResizeEdge,
    unavailable: u8,
) -> Option<xdg_toplevel::ResizeEdge> {
    resize_edge_from_mask(resize_edge_mask(edge) & !unavailable)
}

pub(crate) fn resize_edge_cursor(
    edge: xdg_toplevel::ResizeEdge,
) -> wp_cursor_shape_device_v1::Shape {
    use wp_cursor_shape_device_v1::Shape;
    match edge {
        xdg_toplevel::ResizeEdge::Top => Shape::NResize,
        xdg_toplevel::ResizeEdge::Bottom => Shape::SResize,
        xdg_toplevel::ResizeEdge::Left => Shape::WResize,
        xdg_toplevel::ResizeEdge::Right => Shape::EResize,
        xdg_toplevel::ResizeEdge::TopLeft => Shape::NwResize,
        xdg_toplevel::ResizeEdge::TopRight => Shape::NeResize,
        xdg_toplevel::ResizeEdge::BottomLeft => Shape::SwResize,
        xdg_toplevel::ResizeEdge::BottomRight => Shape::SeResize,
        _ => Shape::Default,
    }
}

/// State for tracking keyboard key repeat.
struct KeyRepeatState {
    key_code: KeyCode,
    text: String,
    /// True while waiting for the initial delay; false during steady-state repeat.
    in_initial_delay: bool,
}

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
    pub(crate) subcompositor: Option<wl_subcompositor::WlSubcompositor>,
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
    pub(crate) pointer_enter_serial: Option<u32>,
    pub(crate) requested_cursor: MouseCursor,
    pub(crate) keyboard_serial: Option<u32>,
    pub(crate) decoration_manager: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    pub(crate) icon_manager: Option<xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1>,
    pub(crate) windows: Vec<WaylandWindow>,
    pub(crate) popups: Vec<WaylandPopupWindow>,
    pub(crate) pointer_window: Option<WindowId>,
    /// Set while the pointer is over a window's shadow gutter rather than the window
    /// itself, together with the edge a press there would resize. Kept apart from
    /// [`Self::pointer_window`] because the gutter is outside the window: the app must
    /// not see hover or clicks at coordinates that fall outside its own surface.
    pub(crate) pointer_shadow: Option<(WindowId, xdg_toplevel::ResizeEdge)>,
    /// The latest un-dispatched pointer motion `(window_id, pos)`, coalesced across a whole
    /// `dispatch_pending` batch. A high-Hz mouse queues many `wl_pointer` motion+frame pairs between
    /// paints; dispatching each as a `MouseMove` runs a redundant hover hit-test across the whole
    /// widget tree, stealing frame budget during a fling. We keep only the latest and flush it once
    /// after the queue is drained (and before any intervening button/leave, to preserve ordering),
    /// mirroring the Windows `coalesce_mouse_move`. See [`Self::flush_pending_motion`].
    pub(crate) pending_motion: Option<(WindowId, Vec2d)>,
    pub(crate) keyboard_window: Option<WindowId>,
    pub(crate) modifiers: KeyModifiers,
    pub(crate) timers: SelectTimers,
    pub(crate) scale_manager: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    pub(crate) viewporter: Option<wp_viewporter::WpViewporter>,
    pub(crate) xkb_state: Option<xkb_sys::XkbState>,
    pub(crate) xkb_cx: xkb_sys::XkbContext,
    pub(crate) text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    pub(crate) text_input_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    /// zwp_text_input_v3 double-buffers preedit/commit; these accumulate the
    /// pending IME state until the matching `Done` event applies it.
    text_input_pending_preedit: Option<String>,
    text_input_pending_commit: Option<String>,
    /// Last composition preview forwarded to the widget, to skip redundant updates.
    text_input_last_preedit: String,
    pub(crate) primary_selection_manager:
        Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    pub(crate) primary_selection_device:
        Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    pub(crate) primary_selection_source:
        Option<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1>,
    pub(crate) primary_selection_text: String,
    pub(crate) last_resize_edge: Option<xdg_toplevel::ResizeEdge>,
    caption_press: Option<CaptionPress>,
    last_caption_click: Option<(WindowId, Vec2d, u32)>,
    consumed_pointer_buttons: MouseButton,
    event_callback: Option<Box<dyn FnMut(&mut WaylandState, XlibEvent)>>,

    pub(crate) scroll_accumulator: Vec2d,
    /// Wheel detents accumulated over the current pointer frame, from `AxisValue120`
    /// (fractional detents on high-resolution wheels) or `AxisDiscrete` on pre-v8
    /// compositors. Same sign convention as `scroll_accumulator`.
    pub(crate) scroll_detents: Vec2d,
    pub(crate) scroll_is_wheel: bool,
    /// Set when `wl_pointer::AxisStop` arrives in the current pointer frame: the fingers
    /// lifted off the touchpad. The frame's Scroll event is then sent with
    /// `ScrollPhase::Ended` (even if its delta is zero) so widgets can start their own
    /// fling — Wayland compositors do not synthesize momentum scrolling for clients.
    pub(crate) scroll_stopped: bool,
    /// Windows whose last presented frame's `wl_surface::frame` callback has not fired
    /// yet. While a window is listed here the compositor is not ready for a new frame
    /// on that surface, so presenting it is skipped (its pass stays dirty). See the
    /// frame-callback pacing in `linux_wayland.rs`.
    frame_callbacks_pending: Vec<WindowId>,
    pub(crate) event_flow: EventFlow,
    pub(crate) event_loop_running: bool,

    /// Keyboard repeat rate in keys per second (0 = disabled).
    key_repeat_rate: i32,
    /// Keyboard repeat delay in milliseconds before repeat starts.
    key_repeat_delay: i32,
    /// Currently repeating key state, if any.
    key_repeat: Option<KeyRepeatState>,
}

impl WaylandState {
    pub fn new(event_callback: Box<dyn FnMut(&mut WaylandState, XlibEvent)>) -> Self {
        Self {
            compositor: None,
            subcompositor: None,
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
            pointer_shadow: None,
            pending_motion: None,
            keyboard_window: None,
            pointer_serial: None,
            pointer_enter_serial: None,
            requested_cursor: MouseCursor::Default,
            keyboard_serial: None,
            modifiers: KeyModifiers::default(),
            xkb_state: None,
            xkb_cx: xkb_sys::XkbContext::new().unwrap(),
            text_input: None,
            text_input_manager: None,
            text_input_pending_preedit: None,
            text_input_pending_commit: None,
            text_input_last_preedit: String::new(),
            primary_selection_manager: None,
            primary_selection_device: None,
            primary_selection_source: None,
            primary_selection_text: String::new(),
            last_mouse_pos: dvec2(0., 0.),
            last_resize_edge: None,
            caption_press: None,
            last_caption_click: None,
            consumed_pointer_buttons: MouseButton::empty(),
            timers: SelectTimers::new(),
            event_callback: Some(event_callback),
            scroll_accumulator: dvec2(0.0, 0.0),
            scroll_detents: dvec2(0.0, 0.0),
            scroll_is_wheel: false,
            scroll_stopped: false,
            frame_callbacks_pending: Vec::new(),
            event_flow: EventFlow::Wait,
            event_loop_running: true,
            key_repeat_rate: 25,
            key_repeat_delay: 600,
            key_repeat: None,
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

    fn clear_resize_edge(&mut self, force_cursor_update: bool) {
        if self.last_resize_edge.take().is_some() || force_cursor_update {
            if let (Some(cursor), Some(serial)) =
                (self.cursor_shape.as_ref(), self.pointer_enter_serial)
            {
                cursor.set_shape(serial, self.requested_cursor.into());
            }
        }
    }

    fn update_resize_edge(
        &mut self,
        window_id: WindowId,
        pos: Vec2d,
        force_cursor_update: bool,
    ) {
        self.last_mouse_pos = pos;
        let window_state = self
            .windows
            .iter()
            .find(|window| window.window_id == window_id)
            .filter(|window| {
                window.uses_client_side_decorations
                    && !window.is_maximized
                    && !window.is_fullscreen
                    // The gutter already owns the grabs, and hit-testing here as well would
                    // charge every pointer motion near an edge for a whole-widget-tree
                    // `WindowDragQuery` dispatch that cannot change the answer.
                    && !window.csd_shadow_gutter_active()
            })
            .map(|window| {
                (
                    window.window_geom.inner_size,
                    window.unavailable_resize_edges,
                )
            });
        // The gutter outside the window is the primary way to resize, so these interior
        // bands only need to cover the case where the shadow could not be created and
        // there is no gutter to aim at. They stay narrow because every pixel they claim
        // is a pixel the app's own widgets do not get.
        let mut edge = window_state.and_then(|(size, unavailable)| {
            let mut mask = 0;
            if pos.x < 10.0 {
                mask |= RESIZE_EDGE_LEFT;
            } else if pos.x >= size.x - 10.0 {
                mask |= RESIZE_EDGE_RIGHT;
            }
            if pos.y < 10.0 {
                mask |= RESIZE_EDGE_TOP;
            } else if pos.y >= size.y - 10.0 {
                mask |= RESIZE_EDGE_BOTTOM;
            }
            // Away from a corner the band narrows to 5 px, so a single-axis hit outside
            // that has to fall through to the app.
            if mask.count_ones() == 1
                && pos.x >= 5.0
                && pos.x < size.x - 5.0
                && pos.y >= 5.0
                && pos.y < size.y - 5.0
            {
                return None;
            }
            resize_edge_from_mask(mask & !unavailable)
        });
        if edge.is_some() {
            let response = Rc::new(Cell::new(WindowDragQueryResponse::NoAnswer));
            self.do_callback(XlibEvent::WindowDragQuery(WindowDragQueryEvent {
                window_id,
                abs: pos,
                response: response.clone(),
            }));
            if matches!(response.get(), WindowDragQueryResponse::Client) {
                edge = None;
            }
        }
        if let Some(resize_edge) = edge {
            self.last_resize_edge = Some(resize_edge);
            if let (Some(cursor), Some(serial)) =
                (self.cursor_shape.as_ref(), self.pointer_enter_serial)
            {
                cursor.set_shape(serial, resize_edge_cursor(resize_edge));
            }
        } else {
            self.clear_resize_edge(force_cursor_update);
        }
    }

    /// Handles the pointer entering one of a window's shadow surfaces: the pointer is in
    /// the gutter, outside the window proper, where the only gesture is a resize. Returns
    /// false when `surface` belongs to no shadow, leaving the caller's normal path intact.
    fn enter_shadow_gutter(&mut self, surface: &wl_surface::WlSurface) -> bool {
        let surface_id = surface.id();
        let Some((window_id, edge, shape)) = self.windows.iter().find_map(|window| {
            window
                .csd_shadow_resize_for_surface(&surface_id)
                .map(|(edge, shape)| (window.window_id, edge, shape))
        }) else {
            return false;
        };
        self.pointer_shadow = Some((window_id, edge));
        if let (Some(cursor), Some(serial)) =
            (self.cursor_shape.as_ref(), self.pointer_enter_serial)
        {
            cursor.set_shape(serial, shape);
        }
        true
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
                "wl_subcompositor" => {
                    let subcompositor = wl_registry
                        .bind::<wl_subcompositor::WlSubcompositor, _, _>(name, 1, qhandle, ());
                    state.subcompositor = Some(subcompositor);
                }
                "xdg_wm_base" => {
                    let wm_base =
                        wl_registry.bind::<xdg_wm_base::XdgWmBase, _, _>(
                            name,
                            version.min(7),
                            qhandle,
                            (),
                        );
                    state.wm_base = Some(wm_base);
                }
                "wl_seat" => {
                    // Version 8 adds wl_pointer::AxisValue120 for high-resolution wheel
                    // detents (replacing AxisDiscrete on v8+ compositors). Note the v7+
                    // requirement that keymap fds be mapped MAP_PRIVATE.
                    let seat = wl_registry.bind::<wl_seat::WlSeat, _, _>(
                        name,
                        version.min(9),
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
                let mut geom_change = None;
                let mut disable_client_resize = false;
                let mut refresh_client_resize = false;
                if let Some(window) = state
                    .windows
                    .iter_mut()
                    .find(|win| win.window_id == *window_id)
                {
                    let inner_size = if width > 0 && height > 0 {
                        dvec2(width as f64, height as f64)
                    } else {
                        window.window_geom.inner_size
                    };
                    let is_maximized =
                        WaylandState::xdg_toplevel_has_state(&states, 1 /* maximized */);
                    let is_fullscreen =
                        WaylandState::xdg_toplevel_has_state(&states, 2 /* fullscreen */);
                    let is_active =
                        WaylandState::xdg_toplevel_has_state(&states, 4 /* activated */);
                    let tiled_edges = xdg_toplevel_edge_mask(&states, 5 /* tiled_left */);
                    let constrained_edges =
                        xdg_toplevel_edge_mask(&states, 10 /* constrained_left */);
                    let unavailable_resize_edges = tiled_edges | constrained_edges;
                    let resize_was_disabled = window.is_maximized || window.is_fullscreen;
                    let resize_edges_changed =
                        window.unavailable_resize_edges != unavailable_resize_edges;
                    window.is_maximized = is_maximized;
                    window.is_fullscreen = is_fullscreen;
                    window.is_tiled = tiled_edges != 0;
                    window.is_active = is_active;
                    window.unavailable_resize_edges = unavailable_resize_edges;
                    disable_client_resize = is_maximized || is_fullscreen;
                    // A size change moves the right and bottom bands out from under a
                    // stationary pointer. Without re-running the hit test the window keeps
                    // a resize cursor it no longer has an edge for, and the next click is
                    // swallowed starting a resize from nowhere.
                    refresh_client_resize = resize_edges_changed
                        || resize_was_disabled != disable_client_resize
                        || window.window_geom.inner_size != inner_size;
                    geom_change = Some(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom: window.window_geom.clone(),
                        new_geom: WindowGeom {
                            dpi_factor: window.window_geom.dpi_factor,
                            can_fullscreen: false,
                            xr_is_presenting: false,
                            // Preserve the established Makepad API: on Wayland this
                            // flag has always represented maximized or fullscreen.
                            is_fullscreen: is_fullscreen || is_maximized,
                            is_topmost: false,
                            position: dvec2(0., 0.),
                            inner_size,
                            outer_size: inner_size,
                            ..Default::default()
                        },
                    });
                }
                if let Some(event) = geom_change {
                    state.do_callback(XlibEvent::WindowGeomChange(event));
                }
                if state.pointer_window == Some(*window_id) {
                    if disable_client_resize {
                        state.clear_resize_edge(false);
                    } else if refresh_client_resize {
                        state.update_resize_edge(*window_id, state.last_mouse_pos, false);
                    }
                }
            }
            xdg_toplevel::Event::Close => {
                let accept_close = Rc::new(Cell::new(true));
                state.do_callback(XlibEvent::WindowCloseRequested(WindowCloseRequestedEvent {
                    window_id: *window_id,
                    accept_close,
                }))
            }
            _ => {}
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, WindowId>
    for WaylandState
{
    fn event(
        state: &mut Self,
        _decoration: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        window_id: &WindowId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event else {
            return;
        };
        let uses_client_side_decorations = match mode {
            WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide) => false,
            WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ClientSide)
            | WEnum::Value(_)
            | WEnum::Unknown(_) => true,
        };
        if let Some(window) = state
            .windows
            .iter_mut()
            .find(|window| window.window_id == *window_id)
        {
            // Decoration state is double-buffered with xdg_surface state. Keep
            // only the latest mode and apply it at the matching surface configure.
            window.pending_client_side_decorations = Some(uses_client_side_decorations);
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
            let mut configure_event = None;
            let mut clear_resize_edge = false;
            let mut update_resize_edge = false;
            // Proxy clones are cheap handles and let us initialize CSD shadow
            // resources while mutably borrowing the matching window.
            let compositor = state.compositor.clone();
            let subcompositor = state.subcompositor.clone();
            let shm = state.shm.clone();
            let viewporter = state.viewporter.clone();
            if let Some(window) = state
                .windows
                .iter_mut()
                .find(|win| win.window_id == *window_id)
            {
                let decoration_changed =
                    if let Some(uses_csd) = window.pending_client_side_decorations.take() {
                        if uses_csd == window.uses_client_side_decorations {
                            false
                        } else {
                            if uses_csd {
                                if let Some(compositor) = compositor.as_ref() {
                                    window.ensure_csd_shadow(
                                        compositor,
                                        subcompositor.as_ref(),
                                        shm.as_ref(),
                                        viewporter.as_ref(),
                                        qhandle,
                                    );
                                }
                            }
                            clear_resize_edge = !uses_csd;
                            update_resize_edge = uses_csd;
                            window.uses_client_side_decorations = uses_csd;
                            true
                        }
                    } else {
                        false
                    };
                if !window.configured {
                    let mut old_geom = window.window_geom.clone();
                    old_geom.inner_size = dvec2(0., 0.);
                    old_geom.outer_size = dvec2(0., 0.);
                    configure_event = Some(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom,
                        new_geom: window.window_geom.clone(),
                    });
                } else if decoration_changed {
                    configure_event = Some(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom: window.window_geom.clone(),
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
                    configure_event = Some(WindowGeomChangeEvent {
                        window_id: *window_id,
                        old_geom,
                        new_geom: window.window_geom.clone(),
                    });
                }
                window.configured = true;
            }
            if let Some(event) = configure_event {
                state.do_callback(XlibEvent::WindowGeomChange(event));
            }
            if clear_resize_edge && state.pointer_window == Some(*window_id) {
                state.clear_resize_edge(false);
            } else if update_resize_edge && state.pointer_window == Some(*window_id) {
                state.update_resize_edge(*window_id, state.last_mouse_pos, false);
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
                cursor_begin: _,
                cursor_end: _,
            } => {
                // Double-buffered: stash the preedit (composition) text and apply
                // it on the matching `Done`. A `None`/absent preedit means the
                // composition preview should be cleared for this cycle.
                state.text_input_pending_preedit = text;
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                // Double-buffered: stash the committed text and apply on `Done`.
                state.text_input_pending_commit = text;
            }
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length: _,
                after_length: _,
            } => {}
            zwp_text_input_v3::Event::Done { serial: _ } => {
                // Apply the IME state accumulated since the previous `Done`, in the
                // protocol-mandated order: commit string first, then preedit. Per
                // spec the pending state resets each cycle, so a `Done` carrying no
                // preedit means the composition preview is cleared.
                if let Some(commit) =
                    state.text_input_pending_commit.take().filter(|t| {
                        !t.is_empty() && !t.chars().all(char::is_control)
                    })
                {
                    // `replace_last = false` commits: replaces any active
                    // composition preview with the text, then clears composition.
                    state.do_callback(XlibEvent::TextInput(TextInputEvent {
                        input: commit,
                        replace_last: false,
                        was_paste: false,
                        ..Default::default()
                    }));
                    // The widget's composition is now cleared by the commit above.
                    state.text_input_last_preedit.clear();
                }
                let preedit = state.text_input_pending_preedit.take().unwrap_or_default();
                if preedit != state.text_input_last_preedit {
                    // `replace_last = true` updates the inline composition preview;
                    // an empty string clears it.
                    state.do_callback(XlibEvent::TextInput(TextInputEvent {
                        input: preedit.clone(),
                        replace_last: true,
                        was_paste: false,
                        ..Default::default()
                    }));
                    state.text_input_last_preedit = preedit;
                }
            }
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
                // Cancel any active key repeat when keyboard focus is lost
                state.timers.stop_timer(KEY_REPEAT_TIMER_ID);
                state.key_repeat = None;

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
                            let (key_code, text_str, should_repeat) =
                                if let Some(xkb_state) = state.xkb_state.as_mut() {
                                    (
                                        xkb_state.keycode_to_makepad_keycode(key + 8),
                                        xkb_state.key_get_utf8(key + 8),
                                        xkb_state.key_repeats(key + 8),
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
                                    input: text_str.clone(),
                                    replace_last: false,
                                    was_paste: false,
                                    ..Default::default()
                                }));
                            }

                            // Start key repeat timer if the key supports it
                            if should_repeat && state.key_repeat_rate > 0 {
                                state.timers.stop_timer(KEY_REPEAT_TIMER_ID);
                                state.key_repeat = Some(KeyRepeatState {
                                    key_code,
                                    text: text_str,
                                    in_initial_delay: true,
                                });
                                let delay_secs = state.key_repeat_delay as f64 / 1000.0;
                                state
                                    .timers
                                    .start_timer(KEY_REPEAT_TIMER_ID, delay_secs, false);
                            }
                        }
                        wl_keyboard::KeyState::Released => {
                            if let Some(xkb_state) = state.xkb_state.as_mut() {
                                let key_code = xkb_state.keycode_to_makepad_keycode(key + 8);

                                // Stop key repeat if this is the key being repeated
                                if state
                                    .key_repeat
                                    .as_ref()
                                    .is_some_and(|r| r.key_code == key_code)
                                {
                                    state.timers.stop_timer(KEY_REPEAT_TIMER_ID);
                                    state.key_repeat = None;
                                }

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
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.key_repeat_rate = rate;
                state.key_repeat_delay = delay;
            }
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
                    // wl_seat v7+ requires keymap fds to be mapped MAP_PRIVATE; it is
                    // also valid on older versions since the map is read-only.
                    let map_str = unsafe {
                        libc_sys::mmap(
                            std::ptr::null_mut(),
                            size as libc_sys::size_t,
                            libc_sys::PROT_READ,
                            libc_sys::MAP_PRIVATE,
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
                surface_x,
                surface_y,
            } => {
                state.pointer_serial = Some(serial);
                state.pointer_enter_serial = Some(serial);
                state.flush_pending_clipboard_copy(qhandle, serial);
                state.clear_resize_edge(true);
                state.pointer_shadow = None;
                state.pointer_window = None;
                if state.enter_shadow_gutter(&surface) {
                    return;
                }
                state.pointer_window = state.window_id_for_surface(&surface);
                if let Some(window_id) = state.pointer_window {
                    let pos = dvec2(surface_x as f64, surface_y as f64);
                    state.last_mouse_pos = pos;
                    // Deliver the enter position through the normal coalesced motion path so
                    // stationary pointers establish hover state and the right app cursor.
                    state.pending_motion = Some((window_id, pos));
                }
            }
            wl_pointer::Event::Leave { serial, surface: _ } => {
                // Dispatch any buffered motion before the pointer leaves, so the final hover
                // position is delivered to the right window first.
                state.flush_pending_motion();
                state.pointer_serial = Some(serial);
                state.flush_pending_clipboard_copy(qhandle, serial);
                state.pointer_window = None;
                state.pointer_shadow = None;
                state.pointer_enter_serial = None;
                state.last_resize_edge = None;
                state.caption_press = None;
                state.last_caption_click = None;
            }
            wl_pointer::Event::Motion {
                time: _,
                surface_x,
                surface_y,
            } => {
                if let Some(window_id) = state.pointer_window {
                    let pos = dvec2(surface_x as f64, surface_y as f64);
                    state.last_mouse_pos = pos;
                    let drag_request = state
                        .caption_press
                        .as_mut()
                        .and_then(|press| press.start_drag_if_needed(window_id, pos));
                    if let Some((press_window, press_serial)) = drag_request {
                        state.last_caption_click = None;
                        if let (Some(seat), Some(window)) = (
                            state.seat.as_ref(),
                            state
                                .windows
                                .iter()
                                .find(|window| window.window_id == press_window),
                        ) {
                            window.toplevel._move(seat, press_serial);
                        }
                    }

                    // Buffer this motion instead of dispatching immediately; the latest one is
                    // flushed as a single MouseMove once the whole event batch is drained (or before
                    // an intervening button/leave). See `flush_pending_motion`.
                    state.pending_motion = Some((window_id, pos));
                }
            }
            wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: key_state,
            } => {
                // Dispatch any buffered motion first so a MouseMove precedes this button's
                // down/up (and the WindowDragQuery it triggers) at the correct hover position.
                state.flush_pending_motion();
                state.pointer_serial = Some(serial);
                state.flush_pending_clipboard_copy(qhandle, serial);
                // Outside-click popup dismissal: if press lands on a
                // regular window while popups are open, fire dismiss.
                if let WEnum::Value(ButtonState::Pressed) = key_state {
                    // A press in the shadow gutter is as much "outside" as one on the
                    // window, so it dismisses popups too.
                    if let Some(win_id) = state.pointer_window.or(state
                        .pointer_shadow
                        .map(|(window_id, _)| window_id))
                    {
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
                // In the gutter the only gesture is a resize, and the app is not told about
                // it: these coordinates are outside its surface, and the compositor takes
                // the pointer grab for the duration of the drag.
                if let Some((window_id, resize_edge)) = state.pointer_shadow {
                    if let (WEnum::Value(ButtonState::Pressed), Some(MouseButton::PRIMARY)) =
                        (key_state, wayland_type::from_mouse(button))
                    {
                        if let (Some(seat), Some(window)) = (
                            state.seat.as_ref(),
                            state.windows.iter().find(|win| win.window_id == window_id),
                        ) {
                            window.toplevel.resize(seat, serial, resize_edge);
                        }
                    }
                    return;
                }
                if let Some(btn) = wayland_type::from_mouse(button) {
                    if let Some(window_id) = state.pointer_window {
                        match key_state {
                            WEnum::Value(ButtonState::Pressed) => {
                                // A surface can disappear before delivering a release. Do not let
                                // that stale bit consume the next independent press/release pair.
                                state.consumed_pointer_buttons.remove(btn);
                                let previous_caption_click = if btn == MouseButton::PRIMARY {
                                    state.last_caption_click.take()
                                } else {
                                    state.last_caption_click = None;
                                    None
                                };
                                state.caption_press = None;
                                if btn == MouseButton::PRIMARY
                                    || btn == MouseButton::SECONDARY
                                {
                                    let uses_client_side_decorations = state
                                        .windows
                                        .iter()
                                        .find(|win| win.window_id == window_id)
                                        .is_some_and(|win| {
                                            win.uses_client_side_decorations && !win.is_fullscreen
                                        });
                                    if uses_client_side_decorations {
                                        let response =
                                            Rc::new(Cell::new(WindowDragQueryResponse::NoAnswer));
                                        state.do_callback(XlibEvent::WindowDragQuery(
                                            WindowDragQueryEvent {
                                                window_id,
                                                abs: state.last_mouse_pos,
                                                response: response.clone(),
                                            },
                                        ));
                                        let response = response.get();
                                        // The top resize zone overlaps the caption vertically, but
                                        // caption buttons must keep their full-height click target.
                                        if btn == MouseButton::PRIMARY
                                            && !matches!(
                                                response,
                                                WindowDragQueryResponse::Client
                                            )
                                        {
                                            if let Some(resize_edge) = state.last_resize_edge {
                                                if let (Some(seat), Some(window)) = (
                                                    state.seat.as_ref(),
                                                    state
                                                        .windows
                                                        .iter()
                                                        .find(|win| win.window_id == window_id),
                                                ) {
                                                    window
                                                        .toplevel
                                                        .resize(seat, serial, resize_edge);
                                                    state.consumed_pointer_buttons.insert(btn);
                                                    return;
                                                }
                                            }
                                        }
                                        if matches!(
                                            response,
                                            WindowDragQueryResponse::Caption
                                        ) {
                                            let is_double_click = is_caption_double_click(
                                                previous_caption_click,
                                                window_id,
                                                state.last_mouse_pos,
                                                time,
                                            );
                                            if let (Some(seat), Some(window)) = (
                                                state.seat.as_ref(),
                                                state
                                                    .windows
                                                    .iter()
                                                    .find(|win| win.window_id == window_id),
                                            ) {
                                                if btn == MouseButton::SECONDARY {
                                                    window.toplevel.show_window_menu(
                                                        seat,
                                                        serial,
                                                        state.last_mouse_pos.x as i32,
                                                        state.last_mouse_pos.y as i32,
                                                    );
                                                    state.consumed_pointer_buttons.insert(btn);
                                                    return;
                                                }
                                                if is_double_click {
                                                    if window.is_maximized {
                                                        window.toplevel.unset_maximized();
                                                    } else {
                                                        window.toplevel.set_maximized();
                                                    }
                                                    state.consumed_pointer_buttons.insert(btn);
                                                    return;
                                                }
                                                state.caption_press = Some(CaptionPress {
                                                    window_id,
                                                    pos: state.last_mouse_pos,
                                                    time,
                                                    serial,
                                                    drag_started: false,
                                                });
                                                state.consumed_pointer_buttons.insert(btn);
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
                                let consumed = state.consumed_pointer_buttons.contains(btn);
                                if consumed {
                                    state.consumed_pointer_buttons.remove(btn);
                                }
                                if btn == MouseButton::PRIMARY {
                                    if let Some(press) = state.caption_press.take() {
                                        state.last_caption_click =
                                            press.completed_click(window_id, state.last_mouse_pos);
                                        return;
                                    }
                                }
                                if consumed {
                                    return;
                                }
                                state.do_callback(XlibEvent::MouseUp(MouseUpEvent {
                                    abs: state.last_mouse_pos,
                                    button: btn,
                                    window_id,
                                    modifiers: state.modifiers,
                                    time: state.time_now(),
                                }));
                                if btn == MouseButton::PRIMARY {
                                    if let Some(items) = state.internal_drag_items.take() {
                                        state.do_callback(XlibEvent::Drop(
                                            window_id,
                                            DropEvent {
                                                modifiers: state.modifiers,
                                                handled: Arc::new(Mutex::new(false)),
                                                abs: state.last_mouse_pos,
                                                items,
                                            },
                                        ));
                                        state.do_callback(XlibEvent::DragEnd);
                                    }
                                }
                            }
                            WEnum::Unknown(_) | WEnum::Value(_) => {}
                        }
                    }
                }
            }
            // Wayland axis values already match Makepad's convention: positive vertical =
            // scroll down = viewport moves DOWN. The spec pins the sign in
            // wl_pointer::axis_relative_direction, whose `identical` case is fingers moving
            // down producing a "vertical_scroll down" axis event; libinput documents the
            // same ("the positive direction being down or right"). So pass the values
            // through untouched -- the compositor has already applied the user's
            // natural-scrolling preference to the sign, and negating here would invert both
            // settings. Toolkits that do negate (winit, SDL, Chromium) only do so because
            // their own convention is inverted; GTK, which shares Makepad's, does not.
            wl_pointer::Event::Axis {
                time: _,
                axis,
                value,
            } => match axis {
                WEnum::Value(wl_pointer::Axis::VerticalScroll) => {
                    state.scroll_accumulator.y += value;
                }
                WEnum::Value(wl_pointer::Axis::HorizontalScroll) => {
                    state.scroll_accumulator.x += value;
                }
                _ => {}
            },
            wl_pointer::Event::AxisSource { axis_source } => {
                state.scroll_is_wheel = axis_source == WEnum::Value(wl_pointer::AxisSource::Wheel);
            }
            wl_pointer::Event::Frame => {
                let acc = state.scroll_accumulator;
                let detents = state.scroll_detents;
                // Dispatch when there is a scroll delta, or when the touchpad gesture just
                // ended (AxisStop): the `Ended` event may carry a zero delta but is what lets
                // widgets start their fling animation at finger lift-off.
                if acc.x != 0.0
                    || acc.y != 0.0
                    || detents.x != 0.0
                    || detents.y != 0.0
                    || state.scroll_stopped
                {
                    if let Some(window_id) = state.pointer_window {
                        // Deliver any buffered motion first so the Scroll event's hover
                        // position is current (Button and Leave already do this).
                        state.flush_pending_motion();
                        let time_now = state.time_now();
                        let scroll = if state.scroll_is_wheel {
                            if detents.x != 0.0 || detents.y != 0.0 {
                                // Scale wheel detents to a fixed distance each so slow,
                                // deliberate clicks and fast spins both move proportionally.
                                dvec2(
                                    detents.x * PIXELS_PER_WHEEL_DETENT,
                                    detents.y * PIXELS_PER_WHEEL_DETENT,
                                )
                            } else {
                                // Some compositors send wheel frames without discrete or
                                // value120 information; the accumulated axis value is
                                // already a real distance in pixels.
                                acc
                            }
                        } else {
                            acc
                        };
                        // Wheels have no gesture phases. Finger-driven (touchpad) scrolling
                        // reports `Changed` per frame and `Ended` when the fingers lift
                        // (AxisStop), letting widgets run their own momentum fling —
                        // Wayland compositors do not synthesize momentum for clients.
                        let phase = if state.scroll_is_wheel {
                            ScrollPhase::None
                        } else if state.scroll_stopped {
                            ScrollPhase::Ended
                        } else {
                            ScrollPhase::Changed
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
                            phase,
                        }));
                    }
                }
                state.scroll_accumulator = dvec2(0.0, 0.0);
                state.scroll_detents = dvec2(0.0, 0.0);
                state.scroll_is_wheel = false;
                state.scroll_stopped = false;
            }
            wl_pointer::Event::AxisStop { time: _, axis: _ } => {
                // Fingers lifted off the touchpad: mark the gesture ended so this pointer
                // frame's Scroll event goes out with `ScrollPhase::Ended`.
                state.scroll_stopped = true;
            }
            // Wheel detent counts, carrying the same sign convention as the Axis event
            // above: the spec states each expresses its direction in terms of the positive
            // or negative direction of the same axis, never inverted relative to it.
            // AxisDiscrete is only sent by compositors below seat v8; v8+ compositors
            // send AxisValue120 instead (120 units per detent, fractional detents
            // allowed for high-resolution wheels), so the two never double-count.
            wl_pointer::Event::AxisDiscrete { axis, discrete } => match axis {
                WEnum::Value(wl_pointer::Axis::VerticalScroll) => {
                    state.scroll_detents.y += discrete as f64;
                }
                WEnum::Value(wl_pointer::Axis::HorizontalScroll) => {
                    state.scroll_detents.x += discrete as f64;
                }
                _ => {}
            },
            wl_pointer::Event::AxisValue120 { axis, value120 } => match axis {
                WEnum::Value(wl_pointer::Axis::VerticalScroll) => {
                    state.scroll_detents.y += value120 as f64 / 120.0;
                }
                WEnum::Value(wl_pointer::Axis::HorizontalScroll) => {
                    state.scroll_detents.x += value120 as f64 / 120.0;
                }
                _ => {}
            },
            // Purely informational: the physical direction of the entity that caused the
            // axis event. The axis value itself already reflects the user's natural-scrolling
            // setting, so scrolling content must ignore this. It exists for widgets that
            // should follow the physical wheel regardless of that setting -- the spec's
            // example is a volume slider -- which Makepad has no plumbing for, so drop it.
            wl_pointer::Event::AxisRelativeDirection {
                axis: _,
                direction: _,
            } => {}
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, WindowId> for WaylandState {
    fn event(
        state: &mut Self,
        _callback: &wl_callback::WlCallback,
        event: wl_callback::Event,
        window_id: &WindowId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        // The compositor is ready for a new frame on this window's surface. Clear the
        // pending flag; the Paint that follows event dispatch in the event loop presents
        // the window's pass if it is still dirty. The window may have been closed while
        // the callback was in flight, in which case there is nothing left to clear.
        if let wl_callback::Event::Done { .. } = event {
            state.clear_frame_callback_pending(*window_id);
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
delegate_noop!(WaylandState: ignore wl_region::WlRegion);
delegate_noop!(WaylandState: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(WaylandState: ignore wl_subsurface::WlSubsurface);
delegate_noop!(WaylandState: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
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

    /// Dispatch the latest coalesced pointer motion (if any) as a single `MouseMove` (plus a `Drag`
    /// while an internal drag is in flight), then clear it. Called once after the `wl_pointer` event
    /// batch is drained and before any intervening button/leave, so a high-Hz mouse produces one
    /// hover hit-test per frame instead of one per queued motion. See [`Self::pending_motion`].
    pub(crate) fn flush_pending_motion(&mut self) {
        let Some((window_id, pos)) = self.pending_motion.take() else {
            return;
        };
        // The window may have been closed by an event earlier in this batch;
        // dispatching a motion for a dead window would hit a stale or recycled
        // window pool slot downstream.
        if !self.windows.iter().any(|w| w.window_id == window_id)
            && !self.popups.iter().any(|w| w.window_id == window_id)
        {
            return;
        }
        self.update_resize_edge(window_id, pos, false);
        self.do_callback(XlibEvent::MouseMove(MouseMoveEvent {
                lock_delta: Default::default(),
            abs: pos,
            window_id,
            modifiers: self.modifiers,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
        if let Some(items) = self.internal_drag_items.as_ref() {
            self.do_callback(XlibEvent::Drag(
                window_id,
                DragEvent {
                    modifiers: self.modifiers,
                    handled: Arc::new(Mutex::new(false)),
                    abs: pos,
                    items: items.clone(),
                    response: Arc::new(Mutex::new(DragResponse::None)),
                },
            ));
        }
    }

    /// True while the given window's last presented frame awaits its `wl_surface::frame`
    /// callback, meaning the compositor is not ready for another frame on that surface.
    pub(crate) fn is_frame_callback_pending(&self, window_id: WindowId) -> bool {
        self.frame_callbacks_pending.contains(&window_id)
    }

    pub(crate) fn set_frame_callback_pending(&mut self, window_id: WindowId) {
        if !self.frame_callbacks_pending.contains(&window_id) {
            self.frame_callbacks_pending.push(window_id);
        }
    }

    /// Clear a window's pending frame callback. Called when the callback fires and when
    /// a window is closed, since the compositor never fires callbacks for a destroyed
    /// surface and a stale entry would keep the window's presents gated forever.
    pub(crate) fn clear_frame_callback_pending(&mut self, window_id: WindowId) {
        self.frame_callbacks_pending.retain(|id| *id != window_id);
    }

    pub(crate) fn any_frame_callback_pending(&self) -> bool {
        !self.frame_callbacks_pending.is_empty()
    }

    /// Called from the event loop when the key repeat timer fires.
    /// Returns true if the timer was handled (i.e., it was the key repeat timer).
    pub(crate) fn handle_key_repeat_timer(&mut self, timer_id: u64) -> bool {
        if timer_id != KEY_REPEAT_TIMER_ID {
            return false;
        }
        if let Some(repeat) = self.key_repeat.as_mut() {
            let key_code = repeat.key_code;
            let text = repeat.text.clone();
            let modifiers = self.modifiers;

            if repeat.in_initial_delay {
                // Initial delay has elapsed; switch to steady-state repeat interval.
                repeat.in_initial_delay = false;
                let interval_secs = 1.0 / self.key_repeat_rate as f64;
                self.timers
                    .start_timer(KEY_REPEAT_TIMER_ID, interval_secs, true);
            }

            self.do_callback(XlibEvent::KeyDown(KeyEvent {
                key_code,
                is_repeat: true,
                modifiers,
                time: self.time_now(),
            }));

            let block_text = modifiers.control || modifiers.logo || modifiers.alt;
            if !block_text && text.chars().any(|ch| !ch.is_control()) {
                self.do_callback(XlibEvent::TextInput(TextInputEvent {
                    input: text,
                    replace_last: false,
                    was_paste: false,
                    ..Default::default()
                }));
            }
        }
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_states(states: &[u32]) -> Vec<u8> {
        states
            .iter()
            .flat_map(|state| state.to_ne_bytes())
            .collect()
    }

    #[test]
    fn caption_double_click_requires_matching_window_time_and_position() {
        let window = WindowId(2, 1);
        let previous = Some((window, dvec2(10.0, 10.0), u32::MAX - 100));
        assert!(is_caption_double_click(
            previous,
            window,
            dvec2(12.0, 11.0),
            50
        ));
        assert!(!is_caption_double_click(
            previous,
            WindowId(3, 1),
            dvec2(12.0, 11.0),
            50
        ));
        assert!(!is_caption_double_click(
            previous,
            window,
            dvec2(20.0, 10.0),
            50
        ));
        assert!(!is_caption_double_click(
            previous,
            window,
            dvec2(12.0, 11.0),
            600
        ));
    }

    #[test]
    fn caption_press_waits_for_drag_threshold_before_requesting_move() {
        let window = WindowId(2, 1);
        let mut press = CaptionPress {
            window_id: window,
            pos: dvec2(10.0, 10.0),
            time: 100,
            serial: 77,
            drag_started: false,
        };

        assert_eq!(
            press.start_drag_if_needed(window, dvec2(13.0, 13.0)),
            None
        );
        assert!(!press.drag_started);
        assert_eq!(
            press.completed_click(window, dvec2(13.0, 13.0)),
            Some((window, dvec2(10.0, 10.0), 100))
        );
    }

    #[test]
    fn caption_drag_requests_move_once_and_cannot_complete_as_click() {
        let window = WindowId(2, 1);
        let mut press = CaptionPress {
            window_id: window,
            pos: dvec2(10.0, 10.0),
            time: 100,
            serial: 77,
            drag_started: false,
        };

        assert_eq!(
            press.start_drag_if_needed(window, dvec2(15.0, 10.0)),
            Some((window, 77))
        );
        assert!(press.drag_started);
        assert_eq!(
            press.start_drag_if_needed(window, dvec2(20.0, 10.0)),
            None
        );
        assert_eq!(press.completed_click(window, dvec2(10.0, 10.0)), None);
    }

    #[test]
    fn tiled_and_constrained_states_disable_only_their_resize_edges() {
        let states = encoded_states(&[5, 7, 11, 13]);
        let tiled = xdg_toplevel_edge_mask(&states, 5);
        let constrained = xdg_toplevel_edge_mask(&states, 10);
        assert_eq!(tiled, RESIZE_EDGE_LEFT | RESIZE_EDGE_TOP);
        assert_eq!(constrained, RESIZE_EDGE_RIGHT | RESIZE_EDGE_BOTTOM);
        // A corner whose edges are both free is kept whole.
        assert_eq!(
            available_resize_edge(xdg_toplevel::ResizeEdge::BottomRight, tiled),
            Some(xdg_toplevel::ResizeEdge::BottomRight)
        );
        // A corner with one tiled edge degrades to the edge that is still free,
        // rather than losing the grab entirely.
        assert_eq!(
            available_resize_edge(xdg_toplevel::ResizeEdge::TopLeft, constrained),
            Some(xdg_toplevel::ResizeEdge::TopLeft)
        );
        assert_eq!(
            available_resize_edge(xdg_toplevel::ResizeEdge::TopRight, tiled),
            Some(xdg_toplevel::ResizeEdge::Right)
        );
        assert_eq!(
            available_resize_edge(xdg_toplevel::ResizeEdge::BottomLeft, constrained),
            Some(xdg_toplevel::ResizeEdge::Left)
        );
        // Both components gone means no grab at all.
        assert_eq!(
            available_resize_edge(xdg_toplevel::ResizeEdge::TopLeft, tiled),
            None
        );
        assert_eq!(
            available_resize_edge(xdg_toplevel::ResizeEdge::Top, tiled),
            None
        );
    }

    #[test]
    fn resize_edge_cursor_matches_every_edge() {
        use wp_cursor_shape_device_v1::Shape;
        use xdg_toplevel::ResizeEdge;
        for (edge, shape) in [
            (ResizeEdge::Top, Shape::NResize),
            (ResizeEdge::Bottom, Shape::SResize),
            (ResizeEdge::Left, Shape::WResize),
            (ResizeEdge::Right, Shape::EResize),
            (ResizeEdge::TopLeft, Shape::NwResize),
            (ResizeEdge::TopRight, Shape::NeResize),
            (ResizeEdge::BottomLeft, Shape::SwResize),
            (ResizeEdge::BottomRight, Shape::SeResize),
        ] {
            assert_eq!(resize_edge_cursor(edge), shape);
            // Every edge must survive a round trip through the mask it degrades with.
            assert_eq!(available_resize_edge(edge, 0), Some(edge));
        }
    }
}
