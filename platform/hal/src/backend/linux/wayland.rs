//! Wayland desktop client: xdg-shell roles and compositor pacing, independent of Cx and EGL.
mod constraints;
mod cursor;
mod input;
mod selection;
mod shadow;
use constraints::{LockedPointer, PointerConstraints, RelativeManager, RelativePointer};

use super::{input as common, io, lock, log_enabled, Key, Op, OsCmd, Protocol, Shared, Shell};
use crate::types::{input as raw, open_flags, pointer_lock, window_kind, window_state};
use std::collections::{HashMap, HashSet};
use std::os::fd::{AsFd, AsRawFd};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wayland_client::{
    delegate_noop, protocol::*, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::{
    wp::{
        cursor_shape::v1::client::{wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1},
        fractional_scale::v1::client::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1},
        pointer_gestures::zv1::client::{zwp_pointer_gesture_pinch_v1, zwp_pointer_gestures_v1},
        primary_selection::zv1::client::{
            zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
            zwp_primary_selection_offer_v1, zwp_primary_selection_source_v1,
        },
        text_input::zv3::client::{zwp_text_input_manager_v3, zwp_text_input_v3},
        viewporter::client::{wp_viewport, wp_viewporter},
    },
    xdg::{
        activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1},
        decoration::zv1::client::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1},
        shell::client::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base},
        toplevel_icon::v1::client::{xdg_toplevel_icon_manager_v1, xdg_toplevel_icon_v1},
    },
};

struct Window {
    owner: Key,
    recipient: Key,
    token: u64,
    parent: Option<Key>,
    surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    top: Option<xdg_toplevel::XdgToplevel>,
    popup: Option<xdg_popup::XdgPopup>,
    decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    viewport: Option<wp_viewport::WpViewport>,
    shadow: Option<shadow::WaylandCsdShadow>,
    want_frame: bool,
    last_frame: Instant,
    locked: Option<LockedPointer>,
    locked_active: bool,
    pin: bool,
    pin_restore: [f64; 2],
    outputs: HashSet<u32>,
    fractional_dpi: Option<f32>,
    dpi: f32,
    refresh: f32,
    geom: [f64; 4],
    floating: [f64; 2],
    pending_size: [i32; 2],
    state: u32,
    pending_state: u32,
    tiled: bool,
    pending_tiled: bool,
    active: bool,
    pending_active: bool,
    configured: bool,
    closed: bool,
    retired: bool,
    csd: bool,
    pending_csd: bool,
    buttons: bool,
    chromeless_max: bool,
    resizable: bool,
    pending_edges: u8,
    edges: u8,
}
struct Output {
    proxy: wl_output::WlOutput,
    position: [i32; 2],
    pixels: [i32; 2],
    scale: i32,
    refresh: i32,
}

struct State {
    shared: &'static Shared,
    conn: Connection,
    qh: QueueHandle<State>,
    compositor: Option<wl_compositor::WlCompositor>,
    wm: Option<xdg_wm_base::XdgWmBase>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    shm: Option<wl_shm::WlShm>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    fractional: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    decorations: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    cursor_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    cursor_shape: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    cursor_theme: Option<cursor::Theme>,
    cursor_theme_attempted: bool,
    gestures: Option<zwp_pointer_gestures_v1::ZwpPointerGesturesV1>,
    pinch: Option<zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1>,
    constraints: Option<PointerConstraints>,
    relative_manager: Option<RelativeManager>,
    relative: Option<RelativePointer>,
    activation: Option<xdg_activation_v1::XdgActivationV1>,
    icons: Option<xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard_proxy: Option<wl_keyboard::WlKeyboard>,
    touch_proxy: Option<wl_touch::WlTouch>,
    text_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    text: Option<zwp_text_input_v3::ZwpTextInputV3>,
    windows: HashMap<Key, Window>,
    /// Only requested frames before the first successful Vulkan present use a timer.
    frame_timers: HashMap<Key, Instant>,
    outputs: HashMap<u32, Output>,
    input: input::Input,
    selection: selection::Selection,
}

pub(super) struct App {
    state: State,
    queue: EventQueue<State>,
    conn: Connection,
}
impl App {
    pub fn connect(shared: &'static Shared) -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("connect_to_env: {e}"))?;
        if log_enabled() {
            eprintln!("[hal-wayland] display connected; discovering registry globals");
        }
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut state = State {
            shared,
            conn: conn.clone(),
            qh: qh.clone(),
            compositor: None,
            wm: None,
            subcompositor: None,
            shm: None,
            viewporter: None,
            fractional: None,
            decorations: None,
            cursor_manager: None,
            cursor_shape: None,
            cursor_theme: None,
            cursor_theme_attempted: false,
            gestures: None,
            pinch: None,
            constraints: None,
            relative_manager: None,
            relative: None,
            activation: None,
            icons: None,
            seat: None,
            pointer: None,
            keyboard_proxy: None,
            touch_proxy: None,
            text_manager: None,
            text: None,
            windows: HashMap::new(),
            frame_timers: HashMap::new(),
            outputs: HashMap::new(),
            input: input::Input::new().map_err(|e| format!("input initialization: {e}"))?,
            selection: selection::Selection::default(),
        };
        conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut state).map_err(|e| format!("registry roundtrip: {e}"))?;
        if state.compositor.is_none() || state.wm.is_none() {
            return Err(format!("required globals unavailable: wl_compositor={} xdg_wm_base={}",
                state.compositor.is_some(), state.wm.is_some()));
        }
        queue.roundtrip(&mut state).map_err(|e| format!("bound globals roundtrip: {e}"))?;
        if log_enabled() {
            eprintln!("[hal-wayland] globals ready: compositor=v{} xdg_wm_base=v{} outputs={} wl_seat={} xdg_decoration={} wl_shm={} subcompositor={} viewporter={}",
                state.compositor.as_ref().unwrap().version(), state.wm.as_ref().unwrap().version(),
                state.outputs.len(), state.seat.is_some(), state.decorations.is_some(), state.shm.is_some(),
                state.subcompositor.is_some(), state.viewporter.is_some());
            if state.seat.is_none() {
                eprintln!("[hal-wayland] wl_seat absent; continuing without input devices");
            }
            if state.decorations.is_none() {
                eprintln!("[hal-wayland] xdg-decoration absent; using client-side decorations");
            }
        }
        shared
            .client_chrome
            .store(true, std::sync::atomic::Ordering::Release);
        state.publish_outputs();
        *lock(&shared.egl_display) = Some(crate::backend::gl::surface::Display {
            platform: crate::backend::gl::surface::PLATFORM_WAYLAND, native: conn.backend().display_ptr() as usize, visual: 0,
        });
        Ok(Self { state, queue, conn })
    }
}
impl State {
    fn resolve(&self, key: Key) -> Option<Key> {
        self.windows
            .values()
            .find(|w| !w.closed && (w.owner == key || w.recipient == key))
            .map(|w| w.owner)
    }
    fn surface_key(&self, surface: &wl_surface::WlSurface) -> Option<Key> {
        self.windows
            .values()
            .find(|w| {
                !w.closed
                    && (&w.surface == surface
                        || w.shadow.as_ref().is_some_and(|s| s.edge(surface).is_some()))
            })
            .map(|w| w.owner)
    }
    fn create(&mut self, key: Key, kind: u32, parent: crate::Id, geom: [f32; 4]) {
        if log_enabled() {
            eprintln!("[hal-wayland] window create key={key:?} kind={kind:#x} parent={parent:?} geom={geom:?}");
        }
        let Some(compositor) = self.compositor.as_ref() else {
            self.shared.window_failed(key, "wl_compositor unavailable");
            return;
        };
        let Some(wm) = self.wm.as_ref() else {
            self.shared.window_failed(key, "xdg_wm_base unavailable");
            return;
        };
        let surface = compositor.create_surface(&self.qh, key);
        if !surface.is_alive() {
            self.shared.window_failed(key, "wl_compositor.create_surface returned a dead proxy");
            return;
        }
        let xdg = wm.get_xdg_surface(&surface, &self.qh, key);
        if !xdg.is_alive() {
            surface.destroy();
            self.shared.window_failed(key, "xdg_wm_base.get_xdg_surface returned a dead proxy");
            return;
        }
        let mut geom = geom.map(|n| n as f64);
        for n in &mut geom[..2] {
            if !n.is_finite() {
                *n = 0.0;
            }
        }
        geom[2] = geom[2].max(1.0);
        geom[3] = geom[3].max(1.0);
        let parent_key = (!parent.is_none())
            .then(|| self.resolve((key.0, parent)))
            .flatten();
        let mut top = None;
        let mut popup = None;
        let mut decoration = None;
        let preference = |value: &str| match value {
            "client" | "client-side" => Some(true),
            "server" | "server-side" => Some(false),
            _ => None,
        };
        let borderless =
            kind & 0xff == window_kind::CHROMELESS || kind & crate::window_style::BORDERLESS != 0;
        let client = borderless
            || std::env::args()
                .find_map(|a| a.strip_prefix("--wayland-decoration=").and_then(preference))
                .or_else(|| {
                    std::env::var("MAKEPAD_WAYLAND_DECORATION")
                        .ok()
                        .and_then(|v| preference(&v))
                })
                .unwrap_or(false);
        let csd = kind & 0xff != window_kind::POPUP && (self.decorations.is_none() || client);
        if kind & 0xff == window_kind::POPUP {
            let Some(parent) = parent_key.and_then(|p| self.windows.get(&p)) else {
                xdg.destroy();
                surface.destroy();
                self.shared.window_failed(key, &format!("Wayland popup parent {parent:?} is not an open window"));
                return;
            };
            let position = wm.create_positioner(&self.qh, ());
            if !position.is_alive() {
                xdg.destroy();
                surface.destroy();
                self.shared.window_failed(key, "xdg_wm_base.create_positioner returned a dead proxy");
                return;
            }
            position.set_size(geom[2] as i32, geom[3] as i32);
            position.set_anchor_rect(geom[0] as i32, geom[1] as i32, 1, 1);
            position.set_anchor(xdg_positioner::Anchor::TopLeft);
            position.set_gravity(xdg_positioner::Gravity::BottomRight);
            position.set_constraint_adjustment(
                xdg_positioner::ConstraintAdjustment::SlideX
                    | xdg_positioner::ConstraintAdjustment::SlideY
                    | xdg_positioner::ConstraintAdjustment::FlipX
                    | xdg_positioner::ConstraintAdjustment::FlipY,
            );
            let role = xdg.get_popup(Some(&parent.xdg), &position, &self.qh, key);
            position.destroy();
            if !role.is_alive() {
                xdg.destroy();
                surface.destroy();
                self.shared.window_failed(key, "xdg_surface.get_popup returned a dead proxy");
                return;
            }
            popup = Some(role);
        } else {
            let toplevel = xdg.get_toplevel(&self.qh, key);
            if !toplevel.is_alive() {
                xdg.destroy();
                surface.destroy();
                self.shared.window_failed(key, "xdg_surface.get_toplevel returned a dead proxy");
                return;
            }
            let name = std::env::current_exe()
                .ok()
                .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "makepad".into());
            toplevel.set_app_id(name.clone());
            toplevel.set_title(name);
            if let Some(parent) = parent_key
                .and_then(|p| self.windows.get(&p))
                .and_then(|w| w.top.as_ref())
            {
                toplevel.set_parent(Some(parent));
            }
            if kind & crate::types::window_style::NOT_RESIZABLE != 0 {
                toplevel.set_min_size(geom[2] as i32, geom[3] as i32);
                toplevel.set_max_size(geom[2] as i32, geom[3] as i32);
            }
            if let Some(manager) = self.decorations.as_ref() {
                let object = manager.get_toplevel_decoration(&toplevel, &self.qh, key);
                object.set_mode(if client {
                    zxdg_toplevel_decoration_v1::Mode::ClientSide
                } else {
                    zxdg_toplevel_decoration_v1::Mode::ServerSide
                });
                decoration = Some(object);
            }
            top = Some(toplevel);
        }
        let fractional = if self.viewporter.is_some() {
            self.fractional
                .as_ref()
                .map(|m| m.get_fractional_scale(&surface, &self.qh, key))
        } else {
            None
        };
        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(&surface, &self.qh, ()));
        let shadow = if top.is_some() {
            shadow::WaylandCsdShadow::new(
                compositor,
                self.subcompositor.as_ref(),
                self.shm.as_ref(),
                self.viewporter.as_ref(),
                &surface,
                &self.qh,
            )
        } else {
            None
        };
        let token = self.shared.register(
            key,
            Protocol::Wayland,
            self.conn.backend().display_ptr() as usize,
            surface.id().as_ptr() as usize,
        );
        self.windows.insert(
            key,
            Window {
                owner: key,
                recipient: key,
                token,
                parent: parent_key,
                surface,
                xdg,
                top,
                popup,
                decoration,
                fractional,
                viewport,
                shadow,
                want_frame: false,
                last_frame: Instant::now(),
                locked: None,
                locked_active: false,
                pin: false,
                pin_restore: [0.0; 2],
                outputs: HashSet::new(),
                fractional_dpi: None,
                dpi: 1.0,
                refresh: 60.0,
                geom,
                floating: [geom[2], geom[3]],
                pending_size: [0; 2],
                state: window_state::RESTORE,
                pending_state: window_state::RESTORE,
                tiled: false,
                pending_tiled: false,
                active: false,
                pending_active: false,
                configured: false,
                closed: false,
                retired: false,
                csd,
                pending_csd: csd,
                buttons: !borderless,
                chromeless_max: false,
                resizable: kind & crate::types::window_style::NOT_RESIZABLE == 0,
                pending_edges: 0,
                edges: 0,
            },
        );
        self.windows[&key].surface.commit();
        if log_enabled() {
            eprintln!("[hal-wayland] window created key={key:?} token={token} client_decorations={csd}; initial empty commit sent, waiting for xdg_surface.configure");
        }
    }
    fn publish(&mut self, key: Key) {
        let Some(w) = self.windows.get_mut(&key).filter(|w| !w.closed) else {
            return;
        };
        let output = w
            .outputs
            .iter()
            .filter_map(|id| self.outputs.get(id))
            .max_by_key(|o| o.scale)
            .or_else(|| self.outputs.values().next());
        let integer = output.map_or(1, |o| o.scale.max(1));
        let dpi = w.fractional_dpi.unwrap_or(integer as f32);
        let refresh = output.map_or(60.0, |o| {
            if o.refresh > 0 {
                o.refresh as f32 / 1000.0
            } else {
                60.0
            }
        });
        let display_changed = w.dpi != dpi || w.refresh != refresh;
        w.dpi = dpi;
        w.refresh = refresh;
        if w.fractional.is_some() && w.viewport.is_some() {
            if w.surface.version() >= 3 {
                w.surface.set_buffer_scale(1);
            }
            w.viewport
                .as_ref()
                .unwrap()
                .set_destination(w.geom[2] as i32, w.geom[3] as i32);
        } else {
            if w.surface.version() >= 3 {
                w.surface.set_buffer_scale(integer);
            }
        }
        if w.configured {
            w.xdg
                .set_window_geometry(0, 0, w.geom[2] as i32, w.geom[3] as i32);
        }
        if let Some(shadow) = w.shadow.as_mut() {
            shadow.update(
                w.geom[2] as i32,
                w.geom[3] as i32,
                w.csd && !w.tiled && w.state == window_state::RESTORE,
                w.active,
            );
        }
        self.shared.geometry(
            w.owner,
            [
                (w.geom[2] * dpi as f64).ceil() as u32,
                (w.geom[3] * dpi as f64).ceil() as u32,
            ],
            w.configured,
        );
        // CSD buttons remain app-drawn in fullscreen too; their hit rectangle must survive
        // a configure so Cx can keep it at the top right when the UI zooms.
        let buttons = w.csd && w.buttons && !(w.chromeless_max && w.state == window_state::MAXIMIZE);
        self.shared.push(w.recipient, |q, id| {
            if display_changed {
                q.display(id, dpi, refresh);
            }
            q.chrome(
                id,
                if buttons {
                    (w.geom[2] - 138.0).max(0.0)
                } else {
                    0.0
                },
                0.0,
                if buttons { 138.0 } else { 0.0 },
                if buttons { 29.0 } else { 0.0 },
                false,
            );
            q.geom(id, w.geom[0], w.geom[1], w.geom[2], w.geom[3], dpi, w.state);
        });
    }
    fn frame_ready(&mut self, key: Key, next_ns: u64, reason: &str) {
        self.frame_timers.remove(&key);
        let Some(w) = self.windows.get_mut(&key).filter(|w| w.configured && !w.closed)
        else {
            return;
        };
        w.want_frame = false;
        w.last_frame = Instant::now();
        if log_enabled() {
            eprintln!("[hal-wayland] FrameReady key={key:?} recipient={:?} reason={reason} next_ns={next_ns}; queued to session", w.recipient);
        }
        self.shared.push(w.recipient, |q, id| q.frame_ready(id, next_ns));
    }
    fn frame(&mut self, key: Key) {
        let Some(w) = self.windows.get_mut(&key).filter(|w| !w.closed) else {
            return;
        };
        w.want_frame = true;
        if !w.configured {
            if log_enabled() {
                eprintln!("[hal-wayland] FrameRequest key={key:?}: waiting for first configure");
            }
            return;
        }
        // Frames are paced by a refresh timer from the last FrameReady. A wl_surface.frame
        // request committed without a new buffer carries no damage, so compositors (weston)
        // may hold its done event until something else repaints the output; the swapchain's
        // FIFO present already waits on the compositor, so the timer never runs ahead of it.
        let due = *self.frame_timers.entry(key).or_insert_with(|| {
            (w.last_frame + Duration::from_secs_f64(1.0 / w.refresh as f64)).max(Instant::now())
        });
        if log_enabled() {
            eprintln!("[hal-wayland] FrameRequest key={key:?}: FrameReady in {:?}", due.saturating_duration_since(Instant::now()));
        }
    }
    fn dispatch_frame_timers(&mut self) {
        if self.frame_timers.is_empty() {
            return;
        }
        let now = Instant::now();
        let due: Vec<_> = self.frame_timers
            .iter()
            .filter_map(|(key, when)| (*when <= now).then_some(*key))
            .collect();
        for key in due {
            let next_ns = self.windows.get(&key).map_or(0, |w| crate::clock_ns() + (1e9 / w.refresh as f64) as u64);
            self.frame_ready(key, next_ns, "refresh timer");
        }
    }
    fn close(&mut self, key: Key, notify: bool, reason: &str) {
        self.frame_timers.remove(&key);
        let children: Vec<_> = self
            .windows
            .values()
            .filter(|w| !w.closed && w.parent == Some(key))
            .map(|w| w.owner)
            .collect();
        for child in children {
            self.close(child, notify, reason);
        }
        if let Some(w) = self.windows.get_mut(&key).filter(|w| !w.closed) {
            if log_enabled() {
                eprintln!("[hal-wayland] window close key={key:?} recipient={:?} token={} configured={} reason={reason}; emit_closed={notify}", w.recipient, w.token, w.configured);
            }
            w.closed = true;
            w.want_frame = false;
            if let Some(p) = w.locked.take() {
                p.destroy();
            }
            self.shared.unregister(key);
            if notify {
                self.shared.push(w.recipient, |q, id| q.closed(id));
            }
            if self.input.focus == Some(key) {
                self.input.focus = None;
                self.input.keyboard.reset();
            }
            if self.input.pointer == Some(key) {
                self.input.pointer = None;
            }
            // Unmapping/destroying wl_surface before the last WSI lease would race present.
            // The retire op performs both after Vulkan's presentation worker releases it.
        }
    }
    fn retire(&mut self, token: u64) {
        if let Some(w) = self.windows.values_mut().find(|w| w.token == token) {
            w.retired = true;
        }
        // xdg_popup parents outlive their descendants, even if presentation completes
        // out of order. A ready parent is rechecked after each child is destroyed.
        while let Some(key) = self
            .windows
            .values()
            .find(|w| {
                w.retired
                    && !self
                        .windows
                        .values()
                        .any(|child| child.parent == Some(w.owner))
            })
            .map(|w| w.owner)
        {
            let mut w = self.windows.remove(&key).unwrap();
            if log_enabled() {
                eprintln!("[hal-wayland] window destroy key={key:?} token={} (all native leases retired)", w.token);
            }
            if let Some(shadow) = w.shadow.take() {
                shadow.destroy();
            }
            if let Some(d) = w.decoration.take() {
                d.destroy();
            }
            if let Some(p) = w.popup.take() {
                p.destroy();
            }
            if let Some(t) = w.top.take() {
                t.destroy();
            }
            if let Some(v) = w.viewport.take() {
                v.destroy();
            }
            if let Some(f) = w.fractional.take() {
                f.destroy();
            }
            w.xdg.destroy();
            w.surface.destroy();
        }
    }
    fn publish_outputs(&mut self) {
        let mut outputs: Vec<_> = self.outputs.iter().collect();
        outputs.sort_by_key(|(id, _)| **id);
        *lock(&self.shared.screens) = outputs
            .iter()
            .enumerate()
            .map(|(index, (_, o))| {
                let scale = o.scale.max(1) as f64;
                let bounds = [
                    o.position[0] as f64,
                    o.position[1] as f64,
                    o.pixels[0] as f64 / scale,
                    o.pixels[1] as f64 / scale,
                ];
                crate::ScreenInfo {
                    bounds,
                    work_area: bounds,
                    is_primary: index == 0,
                }
            })
            .collect();
        let dpi = outputs.first().map_or(1.0, |(_, o)| o.scale.max(1) as f32);
        self.shared
            .dpi
            .store(dpi.to_bits(), std::sync::atomic::Ordering::Release);
        for key in self.windows.keys().copied().collect::<Vec<_>>() {
            self.publish(key);
        }
    }
    fn devices(&mut self) {
        let Some(seat) = self.seat.as_ref() else {
            return;
        };
        if self.text.is_none() {
            if let Some(m) = self.text_manager.as_ref() {
                self.text = Some(m.get_text_input(seat, &self.qh, ()));
            }
        }
        if self.selection.device.is_none() {
            if let Some(m) = self.selection.manager.as_ref() {
                self.selection.device = Some(m.get_data_device(seat, &self.qh, ()));
            }
        }
        if self.selection.primary_device.is_none() {
            if let Some(m) = self.selection.primary_manager.as_ref() {
                self.selection.primary_device = Some(m.get_device(seat, &self.qh, ()));
            }
        }
        if let Some(pointer) = self.pointer.as_ref() {
            if self.cursor_shape.is_none() {
                if let Some(m) = self.cursor_manager.as_ref() {
                    self.cursor_shape = Some(m.get_pointer(pointer, &self.qh, ()));
                }
            }
            if self.pinch.is_none() {
                if let Some(m) = self.gestures.as_ref() {
                    self.pinch = Some(m.get_pinch_gesture(pointer, &self.qh, ()));
                }
            }
            if self.relative.is_none() {
                if let Some(m) = self.relative_manager.as_ref() {
                    self.relative = Some(m.get_relative_pointer(pointer, &self.qh, ()));
                }
            }
        }
    }
    fn command(&mut self, sid: u32, cmd: OsCmd) {
        match cmd {
            OsCmd::WindowCreate {
                id,
                kind,
                parent,
                geom,
            } => self.create((sid, id), kind, parent, geom),
            OsCmd::WindowClose { id } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.close(k, true, "OsCmd::WindowClose");
                }
            }
            OsCmd::WindowGeom { id, geom } => {
                if let Some(k) = self.resolve((sid, id)) {
                    let w = self.windows.get_mut(&k).unwrap();
                    if let (Some(popup), Some(wm)) = (w.popup.as_ref(), self.wm.as_ref()) {
                        if popup.version() >= 3 {
                            let position = wm.create_positioner(&self.qh, ());
                            position.set_size(geom[2].max(1.0) as i32, geom[3].max(1.0) as i32);
                            position.set_anchor_rect(geom[0] as i32, geom[1] as i32, 1, 1);
                            position.set_anchor(xdg_positioner::Anchor::TopLeft);
                            position.set_gravity(xdg_positioner::Gravity::BottomRight);
                            position.set_constraint_adjustment(
                                xdg_positioner::ConstraintAdjustment::SlideX
                                    | xdg_positioner::ConstraintAdjustment::SlideY
                                    | xdg_positioner::ConstraintAdjustment::FlipX
                                    | xdg_positioner::ConstraintAdjustment::FlipY,
                            );
                            popup.reposition(&position, 0);
                            position.destroy();
                        } else {
                            eprintln!("[hal-wayland] this compositor's xdg_popup version has no reposition request");
                        }
                    } else {
                        w.geom[2] = geom[2].max(1.0) as f64;
                        w.geom[3] = geom[3].max(1.0) as f64;
                        w.floating = [w.geom[2], w.geom[3]];
                        self.publish(k);
                    }
                }
            }
            OsCmd::WindowTitle { id, title } => {
                if let Some(w) = self.resolve((sid, id)).and_then(|k| self.windows.get(&k)) {
                    if let Some(t) = w.top.as_ref() {
                        t.set_title(title);
                    }
                }
            }
            OsCmd::WindowState { id, state } => {
                if let Some(k) = self.resolve((sid, id)) {
                    let w = self.windows.get_mut(&k).unwrap();
                    if let Some(t) = w.top.as_ref() {
                        match state {
                            window_state::MAXIMIZE => t.set_maximized(),
                            window_state::FULLSCREEN => t.set_fullscreen(None),
                            window_state::MINIMIZE => t.set_minimized(),
                            window_state::RESTORE => {
                                t.unset_fullscreen();
                                t.unset_maximized();
                            }
                            // xdg-shell has no hide/show request. Minimise is the compositor's supported
                            // hiding mechanism; destroying the live WSI surface would be unsafe.
                            window_state::HIDE => t.set_minimized(),
                            window_state::SHOW => {
                                self.activate(k);
                            }
                            _ => {}
                        }
                    }
                }
            }
            OsCmd::WindowButtons { id, show } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.windows.get_mut(&k).unwrap().buttons = show;
                    self.publish(k);
                }
            }
            OsCmd::ChromelessWhenMax { id, on } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.windows.get_mut(&k).unwrap().chromeless_max = on;
                    self.publish(k);
                }
            }
            OsCmd::Cursor { id, cursor } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.input.cursors.insert(k, cursor);
                    if self.input.pointer == Some(k) {
                        self.cursor(cursor);
                    }
                }
            }
            OsCmd::PointerLock { id, mode } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.pointer_lock(k, mode);
                }
            }
            OsCmd::ImeShow { id, area, config } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.ime_show(k, area, config);
                }
            }
            OsCmd::ImeHide { id } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.ime_hide(k);
                }
            }
            OsCmd::ImeSync {
                id,
                selection,
                composition: _,
                text,
            } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.ime_sync(k, selection, text);
                }
            }
            OsCmd::ClipSet { text } => self.set_clipboard(text, false),
            OsCmd::SelectionSet { text } => self.set_clipboard(text, true),
            OsCmd::ClipRead { req, primary } => {
                self.read_clipboard(selection::Purpose::Reply(sid, req), primary)
            }
            OsCmd::DragStart { id, items } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.start_drag(k, items);
                }
            }
            OsCmd::DragReply { id, to, answer } => {
                if let Some(k) = self.resolve((sid, id)) {
                    if to == raw::reply::QUERY {
                        self.caption_answer(k, answer);
                    } else {
                        self.drag_reply(k, answer);
                    }
                }
            }
            OsCmd::FrameRequest { id } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.frame(k);
                } else {
                    self.shared.push((sid, id), |q, id| q.frame_ready(id, 0));
                }
            }
            OsCmd::ActivateApp => {
                if let Some(k) = self
                    .windows
                    .values()
                    .find(|w| !w.closed && w.recipient.0 == sid)
                    .map(|w| w.owner)
                {
                    self.activate(k);
                }
            }
            OsCmd::WindowVisuals { id, visuals, .. } => {
                if let Some(k) = self.resolve((sid, id)) {
                    self.shared
                        .visuals(k, visuals & 1 != 0 || visuals >> 8 != 0);
                    if let (Some(w), Some(comp)) = (self.windows.get(&k), self.compositor.as_ref())
                    {
                        if visuals & 1 != 0 || visuals >> 8 != 0 {
                            w.surface.set_opaque_region(None);
                        } else {
                            let region = comp.create_region(&self.qh, ());
                            region.add(0, 0, w.geom[2] as i32, w.geom[3] as i32);
                            w.surface.set_opaque_region(Some(&region));
                            region.destroy();
                        }
                        w.surface.commit();
                    }
                }
                if visuals >> 8 != 0 {
                    eprintln!("[hal-wayland] compositor blur is not exposed by xdg-shell");
                }
            }
            OsCmd::WindowTopmost { .. } => {
                eprintln!("[hal-wayland] xdg-shell does not expose always-on-top")
            }
            OsCmd::ShowInDock { .. } => {
                eprintln!("[hal-wayland] xdg-shell does not expose task-list visibility")
            }
            OsCmd::FileDialog { req, .. } => self
                .shared
                .push((sid, crate::Id::NONE), |q, _| q.dialog_result(req, 0, &[])),
            OsCmd::WindowButtonsCenter { .. }
            | OsCmd::WindowDpiOverride { .. }
            | OsCmd::DirectPointerSpeed { .. }
            | OsCmd::DirectNaturalScroll { .. }
            | OsCmd::DirectTapToClick { .. }
            | OsCmd::MenuSet { .. }
            | OsCmd::ClipboardActions { .. }
            | OsCmd::ShareText { .. }
            | OsCmd::MobileUi { .. }
            | OsCmd::BackUnhandled
            | OsCmd::SelectionHandles { .. } => {}
            OsCmd::DirectDisplay { req, command } => self.shared.push((sid, crate::Id::NONE), |q, _| {
                q.direct_display_result(req, command.tag() as u32, false, b"Wayland does not own direct displays");
            }),
            OsCmd::AppIcon { w, h, rgba } => {
                if let (Some(manager), Some(shm)) = (self.icons.as_ref(), self.shm.as_ref()) {
                    if let Some(buffer) = shadow::rgba_buffer(shm, &self.qh, w, h, &rgba) {
                        let icon = manager.create_icon(&self.qh, ());
                        icon.add_buffer(&buffer, 1);
                        for window in self
                            .windows
                            .values()
                            .filter(|w| w.recipient.0 == sid && !w.closed)
                        {
                            if let Some(top) = window.top.as_ref() {
                                manager.set_icon(top, Some(&icon));
                            }
                        }
                        icon.destroy();
                        buffer.destroy();
                    }
                }
            }
        }
    }
    fn activate(&mut self, key: Key) {
        if self.shared.flags() & open_flags::NO_FOCUS != 0 {
            return;
        }
        if let (Some(manager), Some(seat), Some(w)) = (
            self.activation.as_ref(),
            self.seat.as_ref(),
            self.windows.get(&key),
        ) {
            let token = manager.get_activation_token(&self.qh, key);
            token.set_serial(self.input.serial, seat);
            token.set_surface(&w.surface);
            token.commit();
        }
    }
}

impl Shell for App {
    fn protocol(&self) -> Protocol {
        Protocol::Wayland
    }
    fn command(&mut self, op: Op) {
        match op {
            Op::Cmd(sid, cmd) => self.state.command(sid, cmd),
            Op::Retire(token) => self.state.retire(token),
            Op::CloseSession(sid) => {
                for key in self
                    .state
                    .windows
                    .keys()
                    .filter(|k| k.0 == sid)
                    .copied()
                    .collect::<Vec<_>>()
                {
                    self.state.close(key, false, "session closed");
                }
                self.state.selection.close_session(sid);
                let returned: Vec<_> = self
                    .state
                    .windows
                    .values_mut()
                    .filter(|w| w.owner.0 == crate::session::HOST && w.recipient.0 == sid)
                    .map(|w| {
                        w.recipient = w.owner;
                        w.owner
                    })
                    .collect();
                for key in returned {
                    self.state.publish(key);
                    self.state.frame(key);
                }
                lock(&self.state.shared.inbox).remove(&sid);
            }
            Op::Lend {
                host,
                session,
                alias,
            } => {
                let key = (crate::session::HOST, host);
                if let Some(w) = self.state.windows.get_mut(&key) {
                    w.recipient = (session, alias);
                    self.state.publish(key);
                    self.state.frame(key);
                }
            }
            Op::Reveal => {}
        }
    }
    fn dispatch(&mut self) -> Result<(), String> {
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| format!("dispatch_pending: {e}"))?;
        self.state.dispatch_frame_timers();
        self.state.flush_motion();
        self.state.transfers();
        if let Some(key) = self.state.input.keyboard.tick() {
            self.state.key(key, true, true);
        }
        Ok(())
    }
    fn wait(&mut self, wake: i32) -> Result<(), String> {
        let write = match self.conn.flush() {
            Ok(()) => false,
            Err(wayland_client::backend::WaylandError::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                true
            }
            Err(e) => return Err(format!("display flush: {e}")),
        };
        let Some(guard) = self.queue.prepare_read() else {
            return Ok(());
        };
        let mut fds = vec![io::PollFd {
            fd: guard.connection_fd().as_raw_fd(),
            events: io::READ | if write { io::WRITE } else { 0 },
            revents: 0,
        }];
        self.state.selection.fds(&mut fds);
        io::wait(
            wake,
            &mut fds,
            self.state
                .frame_timers
                .values()
                .copied()
                .chain(self.state.input.keyboard.repeat.map(|(_, when)| when))
                .min(),
        )?;
        if fds[0].revents & 32 != 0 {
            return Err("Wayland display poll: invalid file descriptor (POLLNVAL)".into());
        }
        if fds[0].revents & (io::READ | 8 | 16) != 0 {
            // Vulkan WSI also reads this display on its own event queue. Readiness can be
            // consumed there between poll and read; EAGAIN/EINTR are not display loss.
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(format!("display read: {e}")),
            }
        } else {
            drop(guard);
        }
        Ok(())
    }
    fn shutdown(&mut self) {
        for key in self.state.windows.keys().copied().collect::<Vec<_>>() {
            self.state.close(key, false, "main loop stopping");
        }
    }
    fn empty(&self) -> bool {
        self.state.windows.is_empty()
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        s: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if log_enabled() {
                    eprintln!("[hal-wayland] global name={name} interface={interface} version={version}");
                }
                macro_rules! bind {
                    ($field:expr,$ty:ty,$v:expr) => {{
                        let bound = version.min($v).min(<$ty as Proxy>::interface().version);
                        $field = Some(registry.bind::<$ty, _, _>(name, bound, q, ()));
                    }};
                }
                match interface.as_str() {
                    "wl_compositor" => bind!(s.compositor, wl_compositor::WlCompositor, 6),
                    "xdg_wm_base" => bind!(s.wm, xdg_wm_base::XdgWmBase, 7),
                    "wl_subcompositor" => {
                        bind!(s.subcompositor, wl_subcompositor::WlSubcompositor, 1)
                    }
                    "wl_shm" => bind!(s.shm, wl_shm::WlShm, 1),
                    "wp_viewporter" => bind!(s.viewporter, wp_viewporter::WpViewporter, 1),
                    "wp_fractional_scale_manager_v1" => bind!(
                        s.fractional,
                        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
                        1
                    ),
                    "zxdg_decoration_manager_v1" => bind!(
                        s.decorations,
                        zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
                        1
                    ),
                    "wp_cursor_shape_manager_v1" => bind!(
                        s.cursor_manager,
                        wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
                        1
                    ),
                    "zwp_pointer_gestures_v1" => {
                        bind!(s.gestures, zwp_pointer_gestures_v1::ZwpPointerGesturesV1, 1)
                    }
                    "zwp_pointer_constraints_v1" => bind!(s.constraints, PointerConstraints, 1),
                    "zwp_relative_pointer_manager_v1" => {
                        bind!(s.relative_manager, RelativeManager, 1)
                    }
                    "xdg_activation_v1" => {
                        bind!(s.activation, xdg_activation_v1::XdgActivationV1, 1)
                    }
                    "xdg_toplevel_icon_manager_v1" => bind!(
                        s.icons,
                        xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1,
                        1
                    ),
                    "wl_seat" => {
                        if s.seat.is_none() {
                            bind!(s.seat, wl_seat::WlSeat, 9);
                        }
                    }
                    "zwp_text_input_manager_v3" => bind!(
                        s.text_manager,
                        zwp_text_input_manager_v3::ZwpTextInputManagerV3,
                        1
                    ),
                    "wl_data_device_manager" => bind!(
                        s.selection.manager,
                        wl_data_device_manager::WlDataDeviceManager,
                        3
                    ),
                    "zwp_primary_selection_device_manager_v1" => bind!(
                        s.selection.primary_manager,
                        zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
                        1
                    ),
                    "wl_output" => {
                        let proxy = registry.bind::<wl_output::WlOutput, _, _>(
                            name,
                            version.min(4),
                            q,
                            name,
                        );
                        s.outputs.insert(
                            name,
                            Output {
                                proxy,
                                position: [0; 2],
                                pixels: [0; 2],
                                scale: 1,
                                refresh: 60000,
                            },
                        );
                    }
                    _ => {}
                }
                s.devices();
            }
            wl_registry::Event::GlobalRemove { name } => {
                if log_enabled() {
                    eprintln!("[hal-wayland] global removed name={name}");
                }
                if let Some(output) = s.outputs.remove(&name) {
                    if output.proxy.version() >= 3 {
                        output.proxy.release();
                    }
                    for w in s.windows.values_mut() {
                        w.outputs.remove(&name);
                    }
                    s.publish_outputs();
                }
            }
            _ => {}
        }
    }
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        p: &xdg_wm_base::XdgWmBase,
        e: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = e {
            p.pong(serial);
        }
    }
}
impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        s: &mut Self,
        _: &wl_output::WlOutput,
        e: wl_output::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(o) = s.outputs.get_mut(name) else {
            return;
        };
        match e {
            wl_output::Event::Geometry { x, y, .. } => o.position = [x, y],
            wl_output::Event::Scale { factor } => o.scale = factor,
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                refresh,
            } if flags.contains(wl_output::Mode::Current) => {
                o.pixels = [width, height];
                o.refresh = refresh;
            }
            wl_output::Event::Done => s.publish_outputs(),
            _ => {}
        }
    }
}
impl Dispatch<wl_surface::WlSurface, Key> for State {
    fn event(
        s: &mut Self,
        _: &wl_surface::WlSurface,
        e: wl_surface::Event,
        key: &Key,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_surface::Event::Enter { output } => {
                let name = s
                    .outputs
                    .iter()
                    .find(|(_, o)| o.proxy == output)
                    .map(|(n, _)| *n);
                if let (Some(name), Some(w)) = (name, s.windows.get_mut(key)) {
                    w.outputs.insert(name);
                }
                s.publish(*key);
            }
            wl_surface::Event::Leave { output } => {
                let name = s
                    .outputs
                    .iter()
                    .find(|(_, o)| o.proxy == output)
                    .map(|(n, _)| *n);
                if let (Some(name), Some(w)) = (name, s.windows.get_mut(key)) {
                    w.outputs.remove(&name);
                }
                s.publish(*key);
            }
            _ => {}
        }
    }
}
impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, Key> for State {
    fn event(
        s: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        e: wp_fractional_scale_v1::Event,
        key: &Key,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = e {
            if scale > 0 {
                if let Some(w) = s.windows.get_mut(key) {
                    w.fractional_dpi = Some(scale as f32 / 120.0);
                }
                s.publish(*key);
            }
        }
    }
}
impl Dispatch<xdg_toplevel::XdgToplevel, Key> for State {
    fn event(
        s: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        e: xdg_toplevel::Event,
        key: &Key,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(w) = s.windows.get_mut(key) else {
            return;
        };
        match e {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let states: Vec<_> = states
                    .chunks_exact(4)
                    .map(|b| u32::from_ne_bytes(b.try_into().unwrap()))
                    .collect();
                w.pending_size = [width, height];
                w.pending_state = if states.contains(&2) {
                    window_state::FULLSCREEN
                } else if states.contains(&1) {
                    window_state::MAXIMIZE
                } else {
                    window_state::RESTORE
                };
                w.pending_active = states.contains(&4);
                w.pending_tiled = states.iter().any(|v| (5..=8).contains(v));
                w.pending_edges = 0;
                for (flag, bit) in [
                    (5, 1),
                    (6, 2),
                    (7, 4),
                    (8, 8),
                    (10, 1),
                    (11, 2),
                    (12, 4),
                    (13, 8),
                ] {
                    if states.contains(&flag) {
                        w.pending_edges |= bit;
                    }
                }
            }
            xdg_toplevel::Event::Close => {
                if log_enabled() {
                    eprintln!("[hal-wayland] CloseRequested key={key:?} recipient={:?}: compositor sent xdg_toplevel.close", w.recipient);
                }
                s.shared.push(w.recipient, |q, id| q.close_requested(id));
            }
            _ => {}
        }
    }
}
impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, Key> for State {
    fn event(
        s: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        e: zxdg_toplevel_decoration_v1::Event,
        key: &Key,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = e {
            if let Some(w) = s.windows.get_mut(key) {
                w.pending_csd = mode != WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide);
            }
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, Key> for State {
    fn event(
        s: &mut Self,
        p: &xdg_surface::XdgSurface,
        e: xdg_surface::Event,
        key: &Key,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = e {
            p.ack_configure(serial);
            let Some(w) = s.windows.get_mut(key) else {
                return;
            };
            let first = !w.configured;
            if w.state == window_state::RESTORE && w.pending_state != window_state::RESTORE {
                w.floating = [w.geom[2], w.geom[3]];
            }
            w.state = w.pending_state;
            w.active = w.pending_active;
            w.tiled = w.pending_tiled;
            w.edges = w.pending_edges;
            w.csd = w.pending_csd;
            for axis in 0..2 {
                w.geom[axis + 2] = if w.pending_size[axis] > 0 {
                    w.pending_size[axis] as f64
                } else {
                    w.floating[axis]
                };
            }
            w.configured = true;
            if log_enabled() {
                eprintln!("[hal-wayland] xdg_surface.configure key={key:?} serial={serial} first={first} geom={:?} client_decorations={} (acknowledged)", w.geom, w.csd);
            }
            let want = w.want_frame;
            s.publish(*key);
            if first {
                s.frame_ready(*key, 0, "first configure; no frame callback required");
            } else if want {
                s.frame(*key);
            }
        }
    }
}
impl Dispatch<xdg_popup::XdgPopup, Key> for State {
    fn event(
        s: &mut Self,
        _: &xdg_popup::XdgPopup,
        e: xdg_popup::Event,
        key: &Key,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(w) = s.windows.get_mut(key) {
            match e {
                xdg_popup::Event::Configure {
                    x,
                    y,
                    width,
                    height,
                } => {
                    w.geom = [x as f64, y as f64, width as f64, height as f64];
                    w.pending_size = [width, height];
                }
                xdg_popup::Event::PopupDone => {
                    if log_enabled() {
                        eprintln!("[hal-wayland] PopupDismissed key={key:?}: compositor sent xdg_popup.popup_done");
                    }
                    s.shared.push(w.recipient, |q, id| q.popup_dismissed(id))
                }
                _ => {}
            }
        }
    }
}
impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, Key> for State {
    fn event(
        s: &mut Self,
        p: &xdg_activation_token_v1::XdgActivationTokenV1,
        e: xdg_activation_token_v1::Event,
        key: &Key,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = e {
            if let (Some(m), Some(w)) = (s.activation.as_ref(), s.windows.get(key)) {
                m.activate(token, &w.surface);
            }
            p.destroy();
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_region::WlRegion);
delegate_noop!(State: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(State: ignore wl_subsurface::WlSubsurface);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore wp_viewporter::WpViewporter);
delegate_noop!(State: ignore wp_viewport::WpViewport);
delegate_noop!(State: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(State: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(State: ignore xdg_positioner::XdgPositioner);
delegate_noop!(State: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(State: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
delegate_noop!(State: ignore zwp_pointer_gestures_v1::ZwpPointerGesturesV1);
delegate_noop!(State: ignore RelativeManager);
delegate_noop!(State: ignore PointerConstraints);
delegate_noop!(State: ignore xdg_activation_v1::XdgActivationV1);
delegate_noop!(State: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);

delegate_noop!(State: ignore xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1);
delegate_noop!(State: ignore xdg_toplevel_icon_v1::XdgToplevelIconV1);
