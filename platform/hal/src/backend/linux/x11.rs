//! Xlib shell, ported from xlib_app/xlib_window. Vulkan uses this Display after XInitThreads;
//! all window requests, XIM callbacks and input dispatch remain on the HAL main thread.
mod selection;
mod sys;
mod touch;
mod xim;

use super::{input, io, lock, log_enabled, Key, Op, OsCmd, Protocol, Shared, Shell};
use crate::types::{
    input as raw, open_flags, pointer_lock, window_kind, window_state, window_style,
};
use std::collections::HashMap;
use std::ffi::{c_int, c_long, c_ulong, c_void, CStr};
use std::ptr;
use std::time::{Duration, Instant};
use sys as x;

const PROPERTY_CHANGE: c_long = 1 << 22;
const WM_MASK: c_long = (x::SubstructureRedirectMask | x::SubstructureNotifyMask) as c_long;

struct Window {
    owner: Key,
    recipient: Key,
    token: u64,
    native: x::Window,
    parent: Option<Key>,
    colormap: x::Colormap,
    ic: Option<xim::XimInputContext>,
    geom: [f64; 4],
    state: u32,
    dpi: f32,
    closed: bool,
    retired: bool,
    mapped: bool,
    popup: bool,
    kind: u32,
    topmost: bool,
    buttons: bool,
    chromeless_max: bool,
    frame: Option<Instant>,
    last_frame: Instant,
    pos: [f64; 2],
    lock: u32,
    lock_anchor: [f64; 2],
    cursor: u32,
    ime: bool,
    pending_press: Option<x::XButtonEvent>,
    deferred: Vec<x::XEvent>,
}

pub(super) struct App {
    shared: &'static Shared,
    display: *mut x::Display,
    root: x::Window,
    screen: c_int,
    xim: x::XIM,
    font: x::XFontSet,
    windows: HashMap<x::Window, Window>,
    atoms: HashMap<String, x::Atom>,
    cursors: HashMap<u32, x::Cursor>,
    keyboard: input::Keyboard,
    touch: Option<touch::Touch>,
    focus: Option<x::Window>,
    selection: selection::Selections,
    dpi: f32,
    /// every window's visual and depth: 32-bit TrueColor where the server has one
    visual: (*mut x::Visual, c_int),
}

impl App {
    pub fn connect(shared: &'static Shared) -> Result<Self, String> {
        unsafe {
            if x::XInitThreads() == 0 {
                return Err("XInitThreads failed".into());
            }
            let display = x::XOpenDisplay(ptr::null());
            if display.is_null() {
                return Err("XOpenDisplay failed (DISPLAY)".into());
            }
            if log_enabled() {
                eprintln!("[hal-x11] XInitThreads and XOpenDisplay succeeded display={display:p}");
            }
            x::XSetErrorHandler(Some(error_handler));
            x::XrmInitialize();
            x::setlocale(0, c"".as_ptr());
            x::XSetLocaleModifiers(c"".as_ptr());
            let mut xim = x::XOpenIM(display, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            if xim.is_null() {
                x::XSetLocaleModifiers(c"@im=none".as_ptr());
                xim = x::XOpenIM(display, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            }
            let screen = x::XDefaultScreen(display);
            let root = x::XRootWindow(display, screen);
            let font = xim::create_xim_font_set(display);
            let keyboard = match input::Keyboard::new() {
                Ok(k) => k,
                Err(e) => {
                    if !font.is_null() {
                        x::XFreeFontSet(display, font);
                    }
                    if !xim.is_null() {
                        x::XCloseIM(xim);
                    }
                    x::XCloseDisplay(display);
                    return Err(format!("X11 keyboard initialization: {e}"));
                }
            };
            let selection = selection::Selections::new(display, root);
            let mut visual = (x::XDefaultVisual(display, screen), x::XDefaultDepth(display, screen));
            let mut argb: x::XVisualInfo = std::mem::zeroed();
            if x::XMatchVisualInfo(display, screen, 32, 4, &mut argb) != 0 {
                visual = (argb.visual, argb.depth);
            }
            // the GL backend's EGL configs must render to that visual
            *lock(&shared.egl_display) = Some(crate::backend::gl::surface::Display {
                platform: crate::backend::gl::surface::PLATFORM_X11, native: display as usize, visual: (*visual.0).visualid as u32,
            });
            let mut app = Self {
                shared,
                display,
                root,
                screen,
                xim,
                font,
                windows: HashMap::new(),
                atoms: HashMap::new(),
                cursors: HashMap::new(),
                keyboard,
                focus: None,
                touch: touch::Touch::new(display),
                selection,
                dpi: 1.0,
                visual,
            };
            x::XSelectInput(
                display,
                root,
                PROPERTY_CHANGE | x::StructureNotifyMask as c_long,
            );
            app.update_display();
            Ok(app)
        }
    }
    fn atom(&mut self, name: &str) -> x::Atom {
        *self.atoms.entry(name.into()).or_insert_with(|| unsafe {
            x::XInternAtom(self.display, input::cstring(name).as_ptr(), 0)
        })
    }
    fn find(&self, key: Key) -> Option<x::Window> {
        self.windows
            .values()
            .find(|w| !w.closed && (w.recipient == key || w.owner == key))
            .map(|w| w.native)
    }
    fn publish(&self, native: x::Window) {
        let Some(w) = self.windows.get(&native).filter(|w| !w.closed) else {
            return;
        };
        let [px, py, width, height] = w.geom;
        self.shared.geometry(
            w.owner,
            [
                (width * w.dpi as f64).round() as u32,
                (height * w.dpi as f64).round() as u32,
            ],
            w.mapped,
        );
        self.shared.push(w.recipient, |q, id| {
            q.chrome(id, 0.0, 0.0, 0.0, 0.0, w.topmost);
            q.geom(id, px, py, width, height, w.dpi, w.state);
        });
    }
    fn fit(&self, mut geom: [f64; 4]) -> [f64; 4] {
        // The native X11 path exposes one root-screen work area. Preserve already-visible
        // rectangles, and rescue saved placements outside that area (xlib_window::fit_create_geom).
        let screens = lock(&self.shared.screens);
        if let Some(s) = screens.first() {
            let a = s.work_area;
            if a[2] > 0.0
                && a[3] > 0.0
                && !(geom[0] >= a[0]
                    && geom[1] >= a[1]
                    && geom[0] + geom[2] <= a[0] + a[2]
                    && geom[1] + geom[3] <= a[1] + a[3])
            {
                geom[2] = geom[2].clamp((200.0 / self.dpi as f64).min(a[2]), a[2]);
                geom[3] = geom[3].clamp((120.0 / self.dpi as f64).min(a[3]), a[3]);
                geom[0] = geom[0].clamp(a[0], a[0] + a[2] - geom[2]);
                geom[1] = geom[1].clamp(a[1], a[1] + a[3] - geom[3]);
            }
        }
        geom
    }
    fn create(&mut self, key: Key, kind: u32, parent: crate::Id, geom: [f32; 4]) {
        if log_enabled() {
            eprintln!("[hal-x11] window create key={key:?} kind={kind:#x} parent={parent:?} geom={geom:?}");
        }
        unsafe {
            let (visual, depth) = self.visual;
            let colormap = x::XCreateColormap(self.display, self.root, visual, 0);
            let popup = kind & 0xff == window_kind::POPUP;
            let parent_key = (!parent.is_none())
                .then(|| {
                    self.find((key.0, parent))
                        .and_then(|n| self.windows.get(&n))
                        .map(|w| w.owner)
                })
                .flatten();
            let mut geom = geom.map(|v| v as f64);
            let placed = geom[0].is_finite() && geom[1].is_finite();
            if !geom[0].is_finite() {
                geom[0] = 0.0;
            }
            if !geom[1].is_finite() {
                geom[1] = 0.0;
            }
            geom[2] = geom[2].max(1.0);
            geom[3] = geom[3].max(1.0);
            if placed && !popup && kind & window_style::EXACT_SIZE == 0 {
                geom = self.fit(geom);
            }
            if popup {
                if let Some(p) = parent_key
                    .and_then(|p| self.find(p))
                    .and_then(|p| self.windows.get(&p))
                {
                    geom[0] += p.geom[0];
                    geom[1] += p.geom[1];
                }
            }
            let mut attrs: x::XSetWindowAttributes = std::mem::zeroed();
            attrs.colormap = colormap;
            attrs.override_redirect = popup as c_int;
            attrs.event_mask = (x::ExposureMask
                | x::StructureNotifyMask
                | x::PointerMotionMask
                | x::ButtonPressMask
                | x::ButtonReleaseMask
                | x::KeyPressMask
                | x::KeyReleaseMask
                | x::FocusChangeMask
                | x::EnterWindowMask
                | x::LeaveWindowMask) as c_long
                | PROPERTY_CHANGE;
            let dpi = self.dpi as f64;
            let native = x::XCreateWindow(
                self.display,
                self.root,
                coord(geom[0] * dpi),
                coord(geom[1] * dpi),
                extent(geom[2] * dpi),
                extent(geom[3] * dpi),
                0,
                depth,
                x::InputOutput,
                visual,
                (x::CWColormap | x::CWEventMask | x::CWBorderPixel | x::CWOverrideRedirect)
                    as c_ulong,
                &mut attrs,
            );
            if native == 0 {
                x::XFreeColormap(self.display, colormap);
                self.shared.window_failed(key, "XCreateWindow returned zero");
                return;
            }
            // XCreateWindow allocates an XID before the server validates the request.
            // Confirm it exists before publishing a Vulkan surface for that XID.
            let mut actual: x::XWindowAttributes = std::mem::zeroed();
            if x::XGetWindowAttributes(self.display, native, &mut actual) == 0 {
                x::XFreeColormap(self.display, colormap);
                self.shared.window_failed(key, "XCreateWindow was rejected by the server (see X error above)");
                return;
            }
            if log_enabled() {
                eprintln!("[hal-x11] XCreateWindow succeeded key={key:?} native={native:#x} depth={depth} pixels={}x{}", actual.width, actual.height);
            }
            let mut protocols = [self.atom("WM_DELETE_WINDOW"), self.atom("_NET_WM_PING")];
            x::XSetWMProtocols(
                self.display,
                native,
                protocols.as_mut_ptr(),
                protocols.len() as c_int,
            );
            let name = std::env::var("RESOURCE_NAME").ok().unwrap_or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "makepad".into())
            });
            let mut class = name.as_bytes().to_vec();
            class.push(0);
            class.extend_from_slice(name.as_bytes());
            class.push(0);
            self.property(native, "WM_CLASS", "STRING", 8, &class);
            self.property32(
                native,
                "_NET_WM_PID",
                "CARDINAL",
                &[std::process::id() as c_ulong],
            );
            self.property32(native, "XdndAware", "ATOM", &[5]);
            let normal = self.atom(if popup {
                "_NET_WM_WINDOW_TYPE_POPUP_MENU"
            } else {
                "_NET_WM_WINDOW_TYPE_NORMAL"
            });
            self.property32(native, "_NET_WM_WINDOW_TYPE", "ATOM", &[normal]);
            if let Some(parent) = parent_key.and_then(|k| self.find(k)) {
                self.property32(native, "WM_TRANSIENT_FOR", "WINDOW", &[parent]);
            }
            let mut hints: x::XWMHints = std::mem::zeroed();
            hints.flags = 1;
            hints.input = (self.shared.flags() & open_flags::NO_FOCUS == 0
                && kind & window_style::NON_ACTIVATING == 0) as c_int;
            x::XSetWMHints(self.display, native, &mut hints);
            let ic = xim::create_xim_input_context(self.font, self.xim, native);
            let token =
                self.shared
                    .register(key, Protocol::X11, self.display as usize, native as usize);
            self.windows.insert(
                native,
                Window {
                    owner: key,
                    recipient: key,
                    token,
                    native,
                    parent: parent_key,
                    colormap,
                    ic,
                    geom,
                    state: window_state::RESTORE,
                    dpi: self.dpi,
                    closed: false,
                    retired: false,
                    mapped: true,
                    popup,
                    kind,
                    topmost: false,
                    buttons: true,
                    chromeless_max: false,
                    frame: None,
                    last_frame: Instant::now(),
                    pos: [0.0; 2],
                    lock: pointer_lock::UNLOCK,
                    lock_anchor: [0.0; 2],
                    cursor: 1,
                    ime: false,
                    pending_press: None,
                    deferred: Vec::new(),
                },
            );
            self.decorations(native);
            if self.shared.flags() & open_flags::HIDDEN_UNTIL_PRESENT != 0
                && !self
                    .shared
                    .revealed
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                self.property32(native, "_NET_WM_WINDOW_OPACITY", "CARDINAL", &[0]);
            }
            if let Some(touch) = self.touch.as_ref() {
                touch.select(self.display, native);
            }
            x::XMapWindow(self.display, native);
            if popup && self.shared.flags() & open_flags::NO_FOCUS == 0 {
                x::XGrabPointer(
                    self.display,
                    native,
                    1,
                    (x::ButtonPressMask | x::ButtonReleaseMask | x::PointerMotionMask) as u32,
                    x::GrabModeAsync,
                    x::GrabModeAsync,
                    0,
                    0,
                    0,
                );
            }
            self.publish(native);
            self.shared.push(key, |q, id| {
                q.display(id, self.dpi, 60.0);
                q.frame_ready(id, 0);
            });
            x::XFlush(self.display);
            if log_enabled() {
                eprintln!("[hal-x11] window mapped key={key:?} token={token}; initial geometry and FrameReady queued");
            }
        }
    }
    fn close(&mut self, native: x::Window, notify: bool, reason: &str) {
        let Some(owner) = self
            .windows
            .get(&native)
            .filter(|w| !w.closed)
            .map(|w| w.owner)
        else {
            return;
        };
        if self
            .windows
            .get(&native)
            .is_some_and(|w| w.lock != pointer_lock::UNLOCK)
        {
            self.pointer_lock(native, pointer_lock::UNLOCK);
        }
        let children: Vec<_> = self
            .windows
            .values()
            .filter(|w| !w.closed && w.parent == Some(owner))
            .map(|w| w.native)
            .collect();
        for child in children {
            self.close(child, notify, reason);
        }
        self.cancel_touches(native);
        if let Some(w) = self.windows.get_mut(&native).filter(|w| !w.closed) {
            if log_enabled() {
                eprintln!("[hal-x11] window close key={:?} recipient={:?} token={} reason={reason}; emit_closed={notify}", w.owner, w.recipient, w.token);
            }
            w.closed = true;
            w.frame = None;
            unsafe {
                x::XUnmapWindow(self.display, native);
                if w.popup || w.lock != pointer_lock::UNLOCK {
                    x::XUngrabPointer(self.display, 0);
                }
            }
            if self.focus == Some(native) {
                self.focus = None;
                self.keyboard.reset();
            }
            self.shared.unregister(w.owner);
            if notify {
                self.shared.push(w.recipient, |q, id| q.closed(id));
            }
        }
    }
    fn retire(&mut self, token: u64) {
        if let Some(w) = self.windows.values_mut().find(|w| w.token == token) {
            w.retired = true;
        }
        // A parent may finish presenting before a popup. Preserve the whole native
        // hierarchy until each child's own Vulkan lease has retired.
        while let Some(native) = self
            .windows
            .values()
            .find(|w| {
                w.retired
                    && !self
                        .windows
                        .values()
                        .any(|child| child.parent == Some(w.owner))
            })
            .map(|w| w.native)
        {
            let w = self.windows.remove(&native).unwrap();
            if log_enabled() {
                eprintln!("[hal-x11] window destroy key={:?} token={} (all native leases retired)", w.owner, w.token);
            }
            unsafe {
                if let Some(ic) = w.ic {
                    x::XDestroyIC(ic.xic);
                }
                x::XDestroyWindow(self.display, w.native);
                x::XFreeColormap(self.display, w.colormap);
            }
        }
    }
    fn property(&mut self, window: x::Window, name: &str, kind: &str, format: i32, bytes: &[u8]) {
        let (name, kind) = (self.atom(name), self.atom(kind));
        unsafe {
            x::XChangeProperty(
                self.display,
                window,
                name,
                kind,
                format,
                0,
                bytes.as_ptr(),
                bytes.len() as c_int,
            );
        }
    }
    fn property32(&mut self, window: x::Window, name: &str, kind: &str, words: &[c_ulong]) {
        let (name, kind) = (self.atom(name), self.atom(kind));
        unsafe {
            x::XChangeProperty(
                self.display,
                window,
                name,
                kind,
                32,
                0,
                words.as_ptr().cast(),
                words.len() as c_int,
            );
        }
    }
    fn message(&mut self, window: x::Window, name: &str, data: [c_long; 5]) {
        let atom = self.atom(name);
        send_message(self.display, self.root, window, atom, WM_MASK, data);
    }
    fn net_state(&mut self, window: x::Window, action: c_long, a: &str, b: Option<&str>) {
        let a = self.atom(a);
        let b = b.map_or(0, |b| self.atom(b));
        self.message(
            window,
            "_NET_WM_STATE",
            [action, a as c_long, b as c_long, 1, 0],
        );
    }
    fn decorations(&mut self, native: x::Window) {
        if let Some(w) = self.windows.get(&native) {
            let decorated = w.kind & 0xff != window_kind::CHROMELESS
                && w.kind & window_style::BORDERLESS == 0
                && !(w.chromeless_max && w.state == window_state::MAXIMIZE);
            // Motif functions: resize, move, minimise, maximise, close. No ALL bit: excluded
            // functions stay excluded. The WM may still override client hints.
            let mut functions = 4;
            if w.kind & window_style::NOT_RESIZABLE == 0 {
                functions |= 2 | 16;
            }
            if w.buttons && w.kind & window_style::NOT_CLOSABLE == 0 {
                functions |= 32;
            }
            if w.buttons && w.kind & window_style::NOT_MINIATURIZABLE == 0 {
                functions |= 8;
            }
            self.property32(
                native,
                "_MOTIF_WM_HINTS",
                "_MOTIF_WM_HINTS",
                &[3, functions, decorated as c_ulong, 0, 0],
            );
        }
    }
    fn state(&mut self, native: x::Window, state: u32) {
        match state {
            window_state::MAXIMIZE => self.net_state(
                native,
                1,
                "_NET_WM_STATE_MAXIMIZED_HORZ",
                Some("_NET_WM_STATE_MAXIMIZED_VERT"),
            ),
            window_state::FULLSCREEN => self.net_state(native, 1, "_NET_WM_STATE_FULLSCREEN", None),
            window_state::RESTORE => {
                self.net_state(native, 0, "_NET_WM_STATE_FULLSCREEN", None);
                self.net_state(
                    native,
                    0,
                    "_NET_WM_STATE_MAXIMIZED_HORZ",
                    Some("_NET_WM_STATE_MAXIMIZED_VERT"),
                );
                unsafe {
                    x::XMapWindow(self.display, native);
                }
            }
            window_state::MINIMIZE => unsafe {
                x::XIconifyWindow(self.display, native, self.screen);
            },
            window_state::HIDE | window_state::SHOW => {
                let mapped = state == window_state::SHOW;
                if let Some(w) = self.windows.get_mut(&native) {
                    w.mapped = mapped;
                }
                unsafe {
                    if mapped {
                        x::XMapWindow(self.display, native);
                    } else {
                        x::XUnmapWindow(self.display, native);
                    }
                }
                self.publish(native);
            }
            _ => {}
        }
    }
    fn set_cursor(&mut self, native: x::Window, cursor: u32) {
        let cursor_handle = *self.cursors.entry(cursor).or_insert_with(|| unsafe {
            if cursor == 0 {
                let bitmap = x::XCreateBitmapFromData(self.display, self.root, c"".as_ptr(), 1, 1);
                let mut color: x::XColor = std::mem::zeroed();
                let value = x::XCreatePixmapCursor(
                    self.display,
                    bitmap,
                    bitmap,
                    &mut color,
                    &mut color,
                    0,
                    0,
                );
                x::XFreePixmap(self.display, bitmap);
                value
            } else {
                x::XcursorLibraryLoadCursor(self.display, input::cursor_name(cursor).as_ptr())
            }
        });
        unsafe {
            x::XDefineCursor(self.display, native, cursor_handle);
        }
    }
    fn pointer_lock(&mut self, native: x::Window, mode: u32) {
        let Some(w) = self.windows.get_mut(&native) else {
            return;
        };
        if mode == pointer_lock::REPIN {
            return;
        } // every real motion rewarps below
        if mode == pointer_lock::PIN && w.lock != pointer_lock::UNLOCK {
            return;
        }
        if mode == pointer_lock::UNPIN && w.lock != pointer_lock::PIN {
            return;
        }
        if mode == pointer_lock::UNLOCK && w.lock == pointer_lock::UNLOCK {
            return;
        }
        let unlocking = matches!(mode, pointer_lock::UNLOCK | pointer_lock::UNPIN);
        let cursor = if unlocking {
            unsafe {
                x::XUngrabPointer(self.display, 0);
            }
            if w.lock == pointer_lock::PIN {
                w.pos = w.lock_anchor;
                unsafe {
                    x::XWarpPointer(
                        self.display,
                        0,
                        native,
                        0,
                        0,
                        0,
                        0,
                        coord(w.pos[0] * w.dpi as f64),
                        coord(w.pos[1] * w.dpi as f64),
                    );
                }
            }
            w.lock = pointer_lock::UNLOCK;
            w.cursor
        } else {
            let status = unsafe {
                x::XGrabPointer(
                    self.display,
                    native,
                    0,
                    (x::ButtonPressMask | x::ButtonReleaseMask | x::PointerMotionMask) as u32,
                    1,
                    1,
                    native,
                    0,
                    0,
                )
            };
            if status != 0 {
                eprintln!("[hal-x11] pointer grab refused: {status}");
                return;
            }
            w.lock = mode;
            w.lock_anchor = if mode == pointer_lock::PIN {
                w.pos
            } else {
                [w.geom[2] * 0.5, w.geom[3] * 0.5]
            };
            unsafe {
                x::XWarpPointer(
                    self.display,
                    0,
                    native,
                    0,
                    0,
                    0,
                    0,
                    coord(w.lock_anchor[0] * w.dpi as f64),
                    coord(w.lock_anchor[1] * w.dpi as f64),
                );
            }
            0
        };
        self.set_cursor(native, cursor);
    }
    fn ime_area(&mut self, native: x::Window, rect: [f32; 4]) {
        let Some(w) = self.windows.get_mut(&native) else {
            return;
        };
        let Some(mut ic) = w.ic else {
            return;
        };
        let scale = w.dpi;
        let height = rect[3] * scale;
        let top = rect[1] * scale;
        let below = top + height + (height * 0.65).clamp(10.0, 18.0);
        let mut root: x::XWindowAttributes = unsafe { std::mem::zeroed() };
        let root_y = (w.geom[1] * scale as f64) as f32;
        unsafe {
            x::XGetWindowAttributes(self.display, self.root, &mut root);
        }
        let available = root.height as f32 - root_y - below;
        let estimate = (height * 7.0).clamp(124.0, 260.0);
        let anchor = if available < estimate && root_y + top > available {
            top
        } else {
            below
        };
        let spot = x::XPoint {
            x: (rect[0] * scale) as i16,
            y: anchor as i16,
        };
        let px = (height * 0.25).max(3.0);
        let py = (height * 1.25).max(20.0);
        let area = x::XRectangle {
            x: (rect[0] * scale - px).max(0.0) as i16,
            y: (top - py).max(0.0) as i16,
            width: (rect[2] * scale + 2.0 * px).max(1.0) as u16,
            height: (height + 2.0 * py).max(1.0) as u16,
        };
        unsafe {
            if ic.preedit_style == xim::XimPreeditStyle::Position
                && !ic.spot_initialized_at_creation
            {
                if let Some(new) = xim::create_xim_position_input_context_with_spot(
                    self.font,
                    self.xim,
                    native,
                    ic.status_style(),
                    spot,
                    area,
                ) {
                    x::XDestroyIC(ic.xic);
                    ic = new;
                    w.ic = Some(new);
                }
            }
            let attrs = x::XVaCreateNestedList(
                0,
                x::XNSpotLocation.as_ptr(),
                &spot,
                x::XNArea.as_ptr(),
                &area,
                ptr::null_mut::<c_void>(),
            );
            if !attrs.is_null() {
                x::XSetICValues(
                    ic.xic,
                    x::XNPreeditAttributes.as_ptr(),
                    attrs,
                    ptr::null_mut::<c_void>(),
                );
                x::XFree(attrs);
            }
            x::XSetICFocus(ic.xic);
        }
        w.ime = true;
    }
    fn update_display(&mut self) {
        let mut dpi = 1.0;
        let resources = self.atom("RESOURCE_MANAGER");
        let current =
            selection::property(self.display, self.root, resources, false).and_then(|p| {
                std::ffi::CString::new(
                    p.bytes
                        .into_iter()
                        .take_while(|b| *b != 0)
                        .collect::<Vec<_>>(),
                )
                .ok()
            });
        unsafe {
            let resources = current.as_ref().map_or_else(
                || x::XResourceManagerString(self.display).cast_const(),
                |s| s.as_ptr(),
            );
            if !resources.is_null() {
                let db = x::XrmGetStringDatabase(resources);
                if !db.is_null() {
                    let mut ty = ptr::null_mut();
                    let mut value: x::XrmValue = std::mem::zeroed();
                    if x::XrmGetResource(
                        db,
                        c"Xft.dpi".as_ptr(),
                        c"Xft.Dpi".as_ptr(),
                        &mut ty,
                        &mut value,
                    ) != 0
                        && !value.addr.is_null()
                    {
                        if let Ok(v) = CStr::from_ptr(value.addr).to_string_lossy().parse::<f32>() {
                            if v.is_finite() && v > 0.0 {
                                dpi = v / 96.0;
                            }
                        }
                    }
                    x::XrmDestroyDatabase(db);
                }
            }
        }
        self.dpi = dpi;
        self.shared
            .dpi
            .store(dpi.to_bits(), std::sync::atomic::Ordering::Release);
        let mut attr: x::XWindowAttributes = unsafe { std::mem::zeroed() };
        unsafe {
            x::XGetWindowAttributes(self.display, self.root, &mut attr);
        }
        let bounds = [
            0.0,
            0.0,
            attr.width as f64 / dpi as f64,
            attr.height as f64 / dpi as f64,
        ];
        let desktop = self.atom("_NET_CURRENT_DESKTOP");
        let desktop = selection::property(self.display, self.root, desktop, false)
            .map_or(0, |p| p.words.first().copied().unwrap_or(0)) as usize;
        let work = self.atom("_NET_WORKAREA");
        let work = selection::property(self.display, self.root, work, false)
            .and_then(|p| {
                p.words.get(desktop * 4..desktop * 4 + 4).map(|w| {
                    [
                        w[0] as f64 / dpi as f64,
                        w[1] as f64 / dpi as f64,
                        w[2] as f64 / dpi as f64,
                        w[3] as f64 / dpi as f64,
                    ]
                })
            })
            .unwrap_or(bounds);
        *lock(&self.shared.screens) = vec![crate::ScreenInfo {
            bounds,
            work_area: work,
            is_primary: true,
        }];
        for w in self.windows.values_mut().filter(|w| !w.closed) {
            if w.dpi != dpi {
                let ratio = w.dpi as f64 / dpi as f64;
                for value in &mut w.geom {
                    *value *= ratio;
                }
                for value in &mut w.pos {
                    *value *= ratio;
                }
                for value in &mut w.lock_anchor {
                    *value *= ratio;
                }
                w.dpi = dpi;
                self.shared
                    .push(w.recipient, |q, id| q.display(id, dpi, 60.0));
            }
        }
        for id in self.windows.keys().copied().collect::<Vec<_>>() {
            self.publish(id);
        }
    }
    fn read_state(&mut self, native: x::Window) {
        let state_atom = self.atom("_NET_WM_STATE");
        let Some(property) = selection::property(self.display, native, state_atom, false) else {
            return;
        };
        let state = if property
            .words
            .contains(&self.atom("_NET_WM_STATE_FULLSCREEN"))
        {
            window_state::FULLSCREEN
        } else if property
            .words
            .contains(&self.atom("_NET_WM_STATE_MAXIMIZED_VERT"))
            && property
                .words
                .contains(&self.atom("_NET_WM_STATE_MAXIMIZED_HORZ"))
        {
            window_state::MAXIMIZE
        } else if property.words.contains(&self.atom("_NET_WM_STATE_HIDDEN")) {
            window_state::MINIMIZE
        } else {
            window_state::RESTORE
        };
        let topmost = property.words.contains(&self.atom("_NET_WM_STATE_ABOVE"));
        if let Some(w) = self.windows.get_mut(&native) {
            w.state = state;
            w.topmost = topmost;
        }
        self.decorations(native);
        self.publish(native);
    }
    fn paste(&mut self, native: x::Window, primary: bool) {
        if let Some(w) = self.windows.get(&native) {
            self.selection
                .read(selection::Purpose::Paste(w.recipient), primary);
        }
    }
    fn key(&mut self, mut ev: x::XKeyEvent, down: bool, repeat: bool) {
        let Some(w) = self.windows.get(&ev.window) else {
            return;
        };
        let key = w.recipient;
        let modifiers = modifiers(ev.state);
        // The server knows the active XKB group and layout. Level zero keeps Shift punctuation
        // from changing the shared key identity; XIM supplies the composed/shifted text.
        let sym = unsafe {
            x::XkbKeycodeToKeysym(
                self.display,
                ev.keycode as u8,
                ((ev.state >> 13) & 3) as c_int,
                0,
            )
        } as u32;
        let code = input::keycode(sym);
        if ev.keycode != 0 {
            self.shared.push(key, |q, id| {
                if down {
                    q.key_down(id, code.to_index(), repeat, modifiers)
                } else {
                    q.key_up(id, code.to_index(), modifiers)
                }
            });
        }
        if !down {
            return;
        }
        if code == makepad_key_code::KeyCode::Escape && w.popup {
            self.shared.push(key, |q, id| q.popup_dismissed(id));
            return;
        }
        if input::shortcut(modifiers) {
            if code == makepad_key_code::KeyCode::KeyV {
                self.paste(ev.window, false);
            }
            return;
        }
        let text = if let Some(ic) = w.ic {
            let mut bytes = vec![0u8; 64];
            let mut symbol = 0;
            let mut status = 0;
            loop {
                let n = unsafe {
                    x::Xutf8LookupString(
                        ic.xic,
                        &mut ev,
                        bytes.as_mut_ptr().cast(),
                        bytes.len() as c_int,
                        &mut symbol,
                        &mut status,
                    )
                };
                if status == x::XBufferOverflow {
                    bytes.resize(n.max(0) as usize + 1, 0);
                    continue;
                }
                bytes.truncate(n.max(0) as usize);
                break;
            }
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            let shifted = unsafe {
                x::XkbKeycodeToKeysym(
                    self.display,
                    ev.keycode as u8,
                    ((ev.state >> 13) & 3) as c_int,
                    (ev.state & 1) as c_int,
                )
            } as u32;
            self.keyboard.text(shifted)
        };
        if input::printable(&text) {
            self.shared.push(key, |q, id| {
                q.ime(id, raw::ime::COMMIT, 0, 0, text.as_bytes())
            });
        }
    }
    fn button(&mut self, ev: x::XButtonEvent, down: bool) {
        let Some(w) = self.windows.get_mut(&ev.window) else {
            return;
        };
        let pos = if w.lock == pointer_lock::UNLOCK {
            [ev.x as f64 / w.dpi as f64, ev.y as f64 / w.dpi as f64]
        } else {
            w.pos
        };
        w.pos = pos;
        let mods = modifiers(ev.state);
        if (4..=7).contains(&ev.button) {
            if down {
                let (dx, dy) = match ev.button {
                    4 => (0.0, -60.0),
                    5 => (0.0, 60.0),
                    6 => (-60.0, 0.0),
                    _ => (60.0, 0.0),
                };
                self.shared.push(w.recipient, |q, id| {
                    q.scroll(id, pos[0], pos[1], dx, dy, mods, false, raw::scroll::NONE)
                });
            }
            return;
        }
        if down
            && w.popup
            && (pos[0] < 0.0 || pos[1] < 0.0 || pos[0] >= w.geom[2] || pos[1] >= w.geom[3])
        {
            self.shared.push(w.recipient, |q, id| q.popup_dismissed(id));
            return;
        }
        if down && !w.popup && matches!(ev.button, 1 | 3) {
            w.pending_press = Some(ev);
            self.shared
                .push(w.recipient, |q, id| q.drag_query(id, pos[0], pos[1]));
        } else {
            let button = input::button(ev.button);
            if button != 0 {
                self.shared.push(w.recipient, |q, id| {
                    if down {
                        q.mouse_down(id, pos[0], pos[1], button, mods)
                    } else {
                        q.mouse_up(id, pos[0], pos[1], button, mods)
                    }
                });
            }
            if down && ev.button == 2 {
                self.paste(ev.window, true);
            }
        }
        if !down
            && self
                .windows
                .get(&ev.window)
                .is_some_and(|w| w.lock == pointer_lock::PIN)
        {
            self.pointer_lock(ev.window, pointer_lock::UNPIN);
        }
    }
    fn query_answer(&mut self, native: x::Window, answer: u32) {
        let Some(w) = self.windows.get_mut(&native) else {
            return;
        };
        let Some(press) = w.pending_press.take() else {
            return;
        };
        let deferred = std::mem::take(&mut w.deferred);
        if answer == raw::drag_query::CAPTION && press.button == 1 {
            unsafe {
                x::XUngrabPointer(self.display, press.time);
            }
            self.message(
                native,
                "_NET_WM_MOVERESIZE",
                [press.x_root as c_long, press.y_root as c_long, 8, 1, 1],
            );
        } else if answer == raw::drag_query::SYS_MENU
            || (answer == raw::drag_query::CAPTION && press.button == 3)
        {
            self.message(
                native,
                "_GTK_SHOW_WINDOW_MENU",
                [0, press.x_root as c_long, press.y_root as c_long, 0, 0],
            );
        } else {
            let pos = [press.x as f64 / w.dpi as f64, press.y as f64 / w.dpi as f64];
            self.shared.push(w.recipient, |q, id| {
                q.mouse_down(
                    id,
                    pos[0],
                    pos[1],
                    input::button(press.button),
                    modifiers(press.state),
                )
            });
        }
        for ev in deferred {
            self.event(ev);
        }
    }
    fn touches(&self, native: x::Window, mut points: Vec<crate::TouchPoint>) {
        use crate::buffers::Wire;
        if let Some(w) = self.windows.get(&native) {
            let mut bytes = Vec::new();
            for p in &mut points {
                p.x /= w.dpi;
                p.y /= w.dpi;
                p.write(&mut bytes);
            }
            self.shared.push(w.recipient, |q, id| {
                q.touch(id, points.len() as u32, &bytes)
            });
        }
    }
    fn cancel_touches(&mut self, native: x::Window) {
        if let Some(t) = self.touch.as_mut() {
            let points = t.cancel(native);
            if !points.is_empty() {
                self.touches(native, points);
            }
        }
    }
    fn event(&mut self, mut ev: x::XEvent) {
        unsafe {
            if let Some((native, points)) = self
                .touch
                .as_mut()
                .and_then(|t| t.event(self.display, &mut ev))
            {
                self.touches(native, points);
                return;
            }
            if self.selection.event(&mut ev, self.shared) {
                return;
            }
            let kind = ev.type_ as u32;
            let native = ev.xany.window;
            if matches!(kind, x::ButtonPress | x::ButtonRelease | x::MotionNotify)
                && self.touch.as_ref().is_some_and(|t| t.active(native))
            {
                return;
            }
            if native == self.root {
                if kind == x::ConfigureNotify
                    || (kind == 28
                        && ["RESOURCE_MANAGER", "_NET_WORKAREA", "_NET_CURRENT_DESKTOP"]
                            .into_iter()
                            .any(|name| self.atom(name) == ev.xproperty.atom))
                {
                    self.update_display();
                }
                return;
            }
            if !self.windows.get(&native).is_some_and(|w| !w.closed) {
                return;
            }
            if matches!(kind, x::ButtonPress | x::ButtonRelease | x::MotionNotify)
                && self.windows[&native].pending_press.is_some()
            {
                self.windows.get_mut(&native).unwrap().deferred.push(ev);
                return;
            }
            let filtered = x::XFilterEvent(&mut ev, 0) != 0;
            if let Some(text) = xim::take_preedit() {
                if let Some(w) = self.focus.and_then(|f| self.windows.get(&f)) {
                    self.shared.push(w.recipient, |q, id| {
                        q.ime(
                            id,
                            if text.is_empty() {
                                raw::ime::CANCEL
                            } else {
                                raw::ime::UPDATE
                            },
                            0,
                            0,
                            text.as_bytes(),
                        )
                    });
                }
            }
            if filtered {
                return;
            }
            match kind {
                x::KeyPress => self.key(ev.xkey, true, false),
                x::KeyRelease => {
                    if x::XPending(self.display) > 0 {
                        let mut next: x::XEvent = std::mem::zeroed();
                        x::XPeekEvent(self.display, &mut next);
                        if next.type_ as u32 == x::KeyPress
                            && next.xkey.window == native
                            && next.xkey.keycode == ev.xkey.keycode
                            && next.xkey.time == ev.xkey.time
                        {
                            x::XNextEvent(self.display, &mut next);
                            self.key(next.xkey, true, true);
                            return;
                        }
                    }
                    self.key(ev.xkey, false, false);
                }
                x::ButtonPress | x::ButtonRelease => {
                    self.button(ev.xbutton, kind == x::ButtonPress)
                }
                x::MotionNotify => {
                    let w = self.windows.get_mut(&native).unwrap();
                    let pos = [
                        ev.xmotion.x as f64 / w.dpi as f64,
                        ev.xmotion.y as f64 / w.dpi as f64,
                    ];
                    let mut delta = [0.0; 2];
                    if w.lock != pointer_lock::UNLOCK {
                        let centre = w.lock_anchor;
                        delta = [pos[0] - centre[0], pos[1] - centre[1]];
                        if delta[0].abs() < 1.0 / w.dpi as f64
                            && delta[1].abs() < 1.0 / w.dpi as f64
                        {
                            return;
                        }
                        if w.lock == pointer_lock::PIN {
                            w.pos[0] += delta[0];
                            w.pos[1] += delta[1];
                        }
                        x::XWarpPointer(
                            self.display,
                            0,
                            native,
                            0,
                            0,
                            0,
                            0,
                            (centre[0] * w.dpi as f64) as c_int,
                            (centre[1] * w.dpi as f64) as c_int,
                        );
                    } else {
                        w.pos = pos;
                    }
                    self.shared.push(w.recipient, |q, id| {
                        q.mouse_move(
                            id,
                            w.pos[0],
                            w.pos[1],
                            delta[0],
                            delta[1],
                            modifiers(ev.xmotion.state),
                        )
                    });
                }
                x::LeaveNotify => {
                    let w = &self.windows[&native];
                    self.shared
                        .push(w.recipient, |q, id| q.mouse_leave(id, w.pos[0], w.pos[1]));
                }
                x::FocusIn | x::FocusOut => {
                    if matches!(ev.xfocus.mode, 1 | 2) || ev.xfocus.detail == 5 {
                        return;
                    }
                    if kind == x::FocusOut {
                        self.cancel_touches(native);
                        self.pointer_lock(native, pointer_lock::UNLOCK);
                    }
                    let w = &self.windows[&native];
                    let focused = kind == x::FocusIn;
                    self.focus = focused.then_some(native);
                    self.keyboard.reset();
                    if let Some(ic) = w.ic {
                        if focused && w.ime {
                            x::XSetICFocus(ic.xic);
                        } else {
                            x::XUnsetICFocus(ic.xic);
                        }
                    }
                    self.shared.push(w.recipient, |q, id| {
                        q.focus(id, focused);
                        if !focused {
                            q.ime(id, raw::ime::CANCEL, 0, 0, &[]);
                            if w.popup {
                                q.popup_dismissed(id);
                            }
                        }
                    });
                }
                x::ConfigureNotify => {
                    let e = ev.xconfigure;
                    let w = self.windows.get_mut(&native).unwrap();
                    let mut px = 0;
                    let mut py = 0;
                    let mut child = 0;
                    x::XTranslateCoordinates(
                        self.display,
                        native,
                        self.root,
                        0,
                        0,
                        &mut px,
                        &mut py,
                        &mut child,
                    );
                    w.geom = [
                        px as f64 / w.dpi as f64,
                        py as f64 / w.dpi as f64,
                        e.width as f64 / w.dpi as f64,
                        e.height as f64 / w.dpi as f64,
                    ];
                    self.publish(native);
                }
                19 | 18 => {
                    self.windows.get_mut(&native).unwrap().mapped = kind == 19;
                    self.publish(native);
                }
                28 => {
                    if ev.xproperty.atom == self.atom("_NET_WM_STATE") {
                        self.read_state(native);
                    }
                }
                x::Expose => {
                    let w = &self.windows[&native];
                    self.shared.push(w.recipient, |q, id| q.frame_ready(id, 0));
                }
                x::ClientMessage => {
                    let e = ev.xclient;
                    if e.message_type == self.atom("WM_PROTOCOLS") {
                        if e.data.l[0] as c_ulong == self.atom("WM_DELETE_WINDOW") {
                            let w = &self.windows[&native];
                            if log_enabled() {
                                eprintln!("[hal-x11] CloseRequested key={:?}: window manager sent WM_DELETE_WINDOW", w.recipient);
                            }
                            self.shared.push(w.recipient, |q, id| q.close_requested(id));
                        } else if e.data.l[0] as c_ulong == self.atom("_NET_WM_PING") {
                            ev.xclient.window = self.root;
                            x::XSendEvent(self.display, self.root, 0, WM_MASK, &mut ev);
                        }
                    } else {
                        let w = &self.windows[&native];
                        self.selection
                            .drag_message(e, w.recipient, w.dpi, self.shared);
                    }
                }
                x::DestroyNotify => self.close(native, true, "X server sent DestroyNotify"),
                _ => {}
            }
        }
    }
}

impl Shell for App {
    fn protocol(&self) -> Protocol {
        Protocol::X11
    }
    fn command(&mut self, op: Op) {
        match op {
            Op::Retire(token) => self.retire(token),
            Op::CloseSession(sid) => {
                let ids: Vec<_> = self
                    .windows
                    .values()
                    .filter(|w| w.owner.0 == sid)
                    .map(|w| w.native)
                    .collect();
                for id in ids {
                    self.close(id, false, "session closed");
                }
                self.selection.close_session(sid);
                let returned: Vec<_> = self
                    .windows
                    .values_mut()
                    .filter(|w| w.owner.0 == crate::session::HOST && w.recipient.0 == sid)
                    .map(|w| {
                        w.recipient = w.owner;
                        w.pending_press = None;
                        w.deferred.clear();
                        w.native
                    })
                    .collect();
                for n in returned {
                    self.publish(n);
                    let key = self.windows[&n].recipient;
                    self.shared.push(key, |q, id| q.frame_ready(id, 0));
                }
                lock(&self.shared.inbox).remove(&sid);
            }
            Op::Lend {
                host,
                session,
                alias,
            } => {
                if let Some(native) = self.find((crate::session::HOST, host)) {
                    let w = self.windows.get_mut(&native).unwrap();
                    w.recipient = (session, alias);
                    self.publish(native);
                    self.shared
                        .push((session, alias), |q, id| q.frame_ready(id, 0));
                }
            }
            Op::Reveal => {
                let atom = self.atom("_NET_WM_WINDOW_OPACITY");
                for w in self.windows.values().filter(|w| !w.closed) {
                    unsafe {
                        x::XDeleteProperty(self.display, w.native, atom);
                    }
                }
            }
            Op::Cmd(sid, cmd) => match cmd {
                OsCmd::WindowCreate {
                    id,
                    kind,
                    parent,
                    geom,
                } => self.create((sid, id), kind, parent, geom),
                OsCmd::WindowClose { id } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.close(n, true, "OsCmd::WindowClose");
                    }
                }
                OsCmd::WindowGeom { id, geom } => {
                    if let Some(n) = self.find((sid, id)) {
                        let dpi = self.windows[&n].dpi;
                        let mut geom = geom.map(|v| v as f64);
                        let old = self.windows[&n].geom;
                        for (i, value) in geom.iter_mut().enumerate() {
                            if !value.is_finite() || (i >= 2 && *value <= 0.0) {
                                *value = old[i];
                            }
                        }
                        if !self.windows[&n].popup
                            && self.windows[&n].kind & window_style::EXACT_SIZE == 0
                        {
                            geom = self.fit(geom);
                        }
                        unsafe {
                            x::XMoveWindow(
                                self.display,
                                n,
                                coord(geom[0] * dpi as f64),
                                coord(geom[1] * dpi as f64),
                            );
                            x::XResizeWindow(
                                self.display,
                                n,
                                extent(geom[2] * dpi as f64),
                                extent(geom[3] * dpi as f64),
                            );
                        }
                    }
                }
                OsCmd::WindowState { id, state } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.state(n, state);
                    }
                }
                OsCmd::WindowTitle { id, title } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.property(n, "_NET_WM_NAME", "UTF8_STRING", 8, title.as_bytes());
                        self.property(n, "WM_NAME", "STRING", 8, title.as_bytes());
                    }
                }
                OsCmd::WindowTopmost { id, on } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.net_state(n, on as c_long, "_NET_WM_STATE_ABOVE", None);
                    }
                }
                OsCmd::WindowButtons { id, show } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.windows.get_mut(&n).unwrap().buttons = show;
                        self.decorations(n);
                    }
                }
                OsCmd::ChromelessWhenMax { id, on } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.windows.get_mut(&n).unwrap().chromeless_max = on;
                        self.decorations(n);
                    }
                }
                OsCmd::Cursor { id, cursor } => {
                    if let Some(n) = self.find((sid, id)) {
                        let w = self.windows.get_mut(&n).unwrap();
                        w.cursor = cursor;
                        if w.lock == pointer_lock::UNLOCK {
                            self.set_cursor(n, cursor);
                        }
                    }
                }
                OsCmd::PointerLock { id, mode } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.pointer_lock(n, mode);
                    }
                }
                OsCmd::ImeShow { id, area, .. } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.ime_area(n, area);
                    }
                }
                OsCmd::ImeHide { id } => {
                    if let Some(n) = self.find((sid, id)) {
                        let w = self.windows.get_mut(&n).unwrap();
                        w.ime = false;
                        if let Some(ic) = w.ic {
                            unsafe {
                                x::XUnsetICFocus(ic.xic);
                            }
                        }
                    }
                }
                OsCmd::ClipSet { text } => self.selection.set(text, false),
                OsCmd::SelectionSet { text } => self.selection.set(text, true),
                OsCmd::ClipRead { req, primary } => self
                    .selection
                    .read(selection::Purpose::Reply(sid, req), primary),
                OsCmd::DragStart { id, items } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.selection.start_drag(n, (sid, id), items, self.shared);
                    }
                }
                OsCmd::DragReply { id, to, answer } => {
                    if let Some(n) = self.find((sid, id)) {
                        if to == raw::reply::QUERY {
                            self.query_answer(n, answer);
                        } else {
                            self.selection.drag_reply(n, answer, self.shared);
                        }
                    }
                }
                OsCmd::FrameRequest { id } => {
                    if let Some(n) = self.find((sid, id)) {
                        let w = self.windows.get_mut(&n).unwrap();
                        if w.frame.is_none() {
                            w.frame = Some(
                                (w.last_frame + Duration::from_nanos(16_666_667))
                                    .max(Instant::now()),
                            );
                        }
                    } else {
                        self.shared.push((sid, id), |q, id| q.frame_ready(id, 0));
                    }
                }
                OsCmd::ActivateApp => {
                    if self.shared.flags() & open_flags::NO_FOCUS == 0 {
                        if let Some(n) = self.focus.or_else(|| {
                            self.windows
                                .values()
                                .find(|w| !w.closed && w.owner.0 == sid)
                                .map(|w| w.native)
                        }) {
                            self.message(n, "_NET_ACTIVE_WINDOW", [1, 0, 0, 0, 0]);
                        }
                    }
                }
                OsCmd::ShowInDock { on } => {
                    for n in self
                        .windows
                        .values()
                        .filter(|w| w.owner.0 == sid)
                        .map(|w| w.native)
                        .collect::<Vec<_>>()
                    {
                        self.net_state(n, (!on) as c_long, "_NET_WM_STATE_SKIP_TASKBAR", None);
                    }
                }
                OsCmd::AppIcon { w, h, rgba } => {
                    if rgba.len() == w as usize * h as usize * 4 {
                        let mut pixels = vec![w as c_ulong, h as c_ulong];
                        pixels.extend(rgba.chunks_exact(4).map(|p| {
                            ((p[3] as c_ulong) << 24)
                                | ((p[0] as c_ulong) << 16)
                                | ((p[1] as c_ulong) << 8)
                                | p[2] as c_ulong
                        }));
                        for n in self
                            .windows
                            .values()
                            .filter(|w| w.owner.0 == sid)
                            .map(|w| w.native)
                            .collect::<Vec<_>>()
                        {
                            self.property32(n, "_NET_WM_ICON", "CARDINAL", &pixels);
                        }
                    }
                }
                OsCmd::FileDialog { req, .. } => self
                    .shared
                    .push((sid, crate::Id::NONE), |q, _| q.dialog_result(req, 0, &[])),
                // XIM does not expose surrounding-text updates; desktop menus/clipboard actions
                // are app UI. Native button centering and zoom are AppKit-only.
                OsCmd::ImeSync { .. }
                | OsCmd::DirectPointerSpeed { .. }
                | OsCmd::DirectNaturalScroll { .. }
                | OsCmd::DirectTapToClick { .. }
                | OsCmd::WindowButtonsCenter { .. }
                | OsCmd::WindowDpiOverride { .. }
                | OsCmd::MenuSet { .. }
                | OsCmd::ClipboardActions { .. }
                | OsCmd::ShareText { .. }
                | OsCmd::MobileUi { .. }
                | OsCmd::BackUnhandled
                | OsCmd::SelectionHandles { .. } => {}
                OsCmd::DirectDisplay { req, command } => self.shared.push((sid, crate::Id::NONE), |q, _| {
                    q.direct_display_result(req, command.tag() as u32, false, b"X11 does not own direct displays");
                }),
                OsCmd::WindowVisuals { id, visuals, .. } => {
                    if let Some(n) = self.find((sid, id)) {
                        self.shared.visuals(
                            self.windows[&n].owner,
                            visuals & 1 != 0 || visuals >> 8 != 0,
                        );
                    }
                    if visuals >> 8 != 0 {
                        eprintln!("[hal-x11] window blur/material is unavailable");
                    }
                }
            },
        }
    }
    fn dispatch(&mut self) -> Result<(), String> {
        // A snapshot of the backlog keeps a flood from starving command/timer service, without
        // throwing away events or imposing a workload-dependent failure limit.
        let count = unsafe { x::XPending(self.display) };
        let mut pending_motion: Option<x::XEvent> = None;
        for _ in 0..count {
            if unsafe { x::XPending(self.display) } == 0 {
                break;
            }
            let mut ev: x::XEvent = unsafe { std::mem::zeroed() };
            unsafe {
                x::XNextEvent(self.display, &mut ev);
            }
            let motion = unsafe { ev.type_ as u32 == x::MotionNotify };
            if let Some(old) = pending_motion.take() {
                if !motion || unsafe { old.xmotion.window != ev.xmotion.window } {
                    self.event(old);
                }
            }
            if motion {
                pending_motion = Some(ev);
            } else {
                self.event(ev);
            }
        }
        if let Some(ev) = pending_motion {
            self.event(ev);
        }
        self.selection.tick(self.shared);
        let now = Instant::now();
        for w in self.windows.values_mut().filter(|w| !w.closed && w.mapped) {
            if w.frame.is_some_and(|due| due <= now) {
                w.frame = None;
                w.last_frame = now;
                self.shared.push(w.recipient, |q, id| {
                    q.frame_ready(id, crate::clock_ns() + 16_666_667)
                });
            }
        }
        unsafe {
            x::XFlush(self.display);
        }
        Ok(())
    }
    fn wait(&mut self, wake: i32) -> Result<(), String> {
        if unsafe { x::XPending(self.display) } > 0 {
            return Ok(());
        }
        let deadline = self
            .windows
            .values()
            .filter(|w| !w.closed && w.mapped)
            .filter_map(|w| w.frame)
            .min();
        io::wait(
            wake,
            &mut [io::PollFd {
                fd: unsafe { x::XConnectionNumber(self.display) },
                events: io::READ,
                revents: 0,
            }],
            deadline,
        )
    }
    fn shutdown(&mut self) {
        for n in self.windows.keys().copied().collect::<Vec<_>>() {
            self.close(n, false, "main loop stopping");
        }
    }
    fn empty(&self) -> bool {
        self.windows.is_empty()
    }
}
impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            self.selection.destroy();
            for (_, cursor) in self.cursors.drain() {
                x::XFreeCursor(self.display, cursor);
            }
            if !self.font.is_null() {
                x::XFreeFontSet(self.display, self.font);
            }
            if !self.xim.is_null() {
                x::XCloseIM(self.xim);
            }
            x::XCloseDisplay(self.display);
        }
    }
}
fn modifiers(state: u32) -> u32 {
    (if state & 1 != 0 { raw::MOD_SHIFT } else { 0 })
        | (if state & 4 != 0 { raw::MOD_CONTROL } else { 0 })
        | (if state & 8 != 0 { raw::MOD_ALT } else { 0 })
        | (if state & 64 != 0 { raw::MOD_LOGO } else { 0 })
}
// X11 wire coordinates are INT16 and extents CARD16; wrapping them changes placement.
fn coord(value: f64) -> c_int {
    value.clamp(-32768.0, 32767.0) as c_int
}
fn extent(value: f64) -> u32 {
    value.round().clamp(1.0, 65535.0) as u32
}
unsafe extern "C" fn error_handler(_: *mut x::Display, event: *mut x::XErrorEvent) -> c_int {
    if let Some(e) = event.as_ref() {
        eprintln!(
            "[hal-x11] X error {} request {}.{} resource {:#x} serial {}",
            e.error_code, e.request_code, e.minor_code, e.resourceid, e.serial
        );
    }
    0
}
fn send_message(
    display: *mut x::Display,
    destination: x::Window,
    window: x::Window,
    atom: x::Atom,
    mask: c_long,
    words: [c_long; 5],
) {
    unsafe {
        let mut event: x::XEvent = std::mem::zeroed();
        event.xclient = x::XClientMessageEvent {
            type_: x::ClientMessage as c_int,
            serial: 0,
            send_event: 1,
            display,
            window,
            message_type: atom,
            format: 32,
            data: x::XClientMessageEvent__bindgen_ty_1 { l: words },
        };
        x::XSendEvent(display, destination, 0, mask, &mut event);
    }
}
