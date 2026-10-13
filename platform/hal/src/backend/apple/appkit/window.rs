//! One window of the shell: the NSWindow, its content view and its CAMetalLayer, the state the
//! delegates need, and what became of each input event (the `RawEvent` it makes). Moved from
//! os/apple/macos/macos_window.rs (`MacosWindow`) and os/apple/macos/macos.rs (`MetalWindow::new`).
//! Everything here runs on the main thread.

use super::app::{self, app};
use super::screen::{self, Rect, Screen};
use super::Shared;
use crate::buffers::pack_drag_items;
use crate::types::*;
use makepad_apple_sys::*;
use std::cell::Cell;
use std::os::raw::c_void;

/// The window's NSEvent kinds a window drag may start from.
const DRAG_START_EVENTS: [NSEventType; 6] = [
    NSEventType::NSLeftMouseDown,
    NSEventType::NSLeftMouseDragged,
    NSEventType::NSRightMouseDown,
    NSEventType::NSRightMouseDragged,
    NSEventType::NSOtherMouseDown,
    NSEventType::NSOtherMouseDragged,
];

pub struct Win {
    pub sid: Cell<u32>,
    pub owner_sid: u32,
    pub id: Cell<Id>,
    pub owner_id: Id,
    pub popup: bool,
    /// `window_style` bits the window was made with
    pub style: u32,
    pub window: ObjcId,
    pub view: ObjcId,
    pub delegate: ObjcId,
    pub layer: ObjcId,
    /// the caret line in view points, y down from the view's top: x, y, w, h
    pub ime_rect: Cell<[f64; 4]>,
    /// key events go to the input context only while a text field has the focus
    pub ime_active: Cell<bool>,
    pub fullscreen: Cell<bool>,
    /// AppKit owns the titlebar layout until the fullscreen exit animation completes.
    pub exiting_fullscreen: Cell<bool>,
    last_mouse: Cell<[f64; 2]>,
    /// buttons this window delivered a down for and no up yet
    buttons_down: Cell<u32>,
    /// momentum scroll stream time (secs): while it is live macOS swallows tap-to-click
    last_momentum: Cell<f64>,
    touch_began: Cell<f64>,
    touch_disqualified: Cell<bool>,
    pub retired: Cell<bool>,
    effect_view: Cell<ObjcId>,
    /// the display link of the window and whether the app asked for a frame
    pub link: Cell<ObjcId>,
    pub want_frame: Cell<bool>,
    pub fallback: Cell<ObjcId>,
    last_geom: Cell<Option<[f64; 12]>>,
    dpi_override: Cell<Option<f64>>,
    default_window_buttons: Cell<Option<DefaultWindowButtons>>,
    /// Horizontal inset and vertical offset from AppKit's original button layout.
    window_buttons_offset: Cell<[f64; 2]>,
    /// Requested traffic-light center, in points below the window's top edge.
    buttons_center_y: Cell<Option<f64>>,
    /// the newest mouse-down / drag NSEvent (retained): a file drag the app starts later needs it
    last_press: Cell<ObjcId>,
    /// the press whose caption answer is pending, and its click count
    caption_event: Cell<ObjcId>,
    /// what the app said about the drag over the window the last time (`drag_response`)
    pub drag_answer: Cell<u32>,
    /// a drop was announced (the app accepts it iff it answered with an operation)
    pub transparent: Cell<bool>,
}

/// AppKit's traffic-light layout, captured before zooming or entering fullscreen.
#[derive(Clone, Copy)]
struct DefaultWindowButtons {
    titlebar_width: f64,
    titlebar_height: f64,
    /// The caption height that centers the native buttons: twice their top inset + height.
    caption_height: f64,
    origins: [NSPoint; 3],
}

impl Win {
    /// Makes the window, its view and its layer. `geom` is x, y, w, h (points; a NaN position
    /// asks for the centre of the main screen); a popup's position is relative to its parent's
    /// content origin, top left.
    pub fn create(sid: u32, id: Id, kind: u32, parent: Option<&Win>, geom: [f32; 4]) -> &'static Win {
        let classes = app().classes();
        let style = kind & !0xff;
        let kind = kind & 0xff;
        let popup = kind == window_kind::POPUP;
        let size = NSSize { width: geom[2].max(1.0) as f64, height: geom[3].max(1.0) as f64 };
        let win = unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
            let window_class = if style & window_style::PANEL != 0 { classes.panel } else { classes.window };
            let window: ObjcId = msg_send![window_class, alloc];
            let delegate: ObjcId = msg_send![classes.window_delegate, new];
            let view: ObjcId = msg_send![classes.view, alloc];
            let layer: ObjcId = msg_send![class!(CAMetalLayer), new];
            let win: &'static Win = Box::leak(Box::new(Win {
                sid: Cell::new(sid),
                id: Cell::new(id),
                owner_sid: sid,
                owner_id: id,
                popup,
                style,
                window,
                view,
                delegate,
                layer,
                ime_rect: Cell::new([0.0; 4]),
                ime_active: Cell::new(false),
                fullscreen: Cell::new(false),
                exiting_fullscreen: Cell::new(false),
                last_mouse: Cell::new([0.0; 2]),
                buttons_down: Cell::new(0),
                last_momentum: Cell::new(0.0),
                touch_began: Cell::new(0.0),
                touch_disqualified: Cell::new(false),
                retired: Cell::new(false),
                effect_view: Cell::new(nil),
                link: Cell::new(nil),
                want_frame: Cell::new(false),
                fallback: Cell::new(nil),
                last_geom: Cell::new(None),
                dpi_override: Cell::new(None),
                default_window_buttons: Cell::new(None),
                window_buttons_offset: Cell::new([0.0; 2]),
                buttons_center_y: Cell::new(None),
                last_press: Cell::new(nil),
                caption_event: Cell::new(nil),
                drag_answer: Cell::new(input::drag_response::NONE),
                transparent: Cell::new(false),
            }));
            let ptr = win as *const Win as *mut c_void;
            (*delegate).set_ivar("macos_window_ptr", ptr);
            let () = msg_send![view, initWithPtr: ptr];
            // raw trackpad touches (NSTouchTypeMaskIndirect), which widgets use to stop kinetic
            // scrolling the instant a finger contacts the pad
            let () = msg_send![view, setAllowedTouchTypes: 2u64];

            if popup {
                win.init_popup(size, geom, parent);
            } else {
                win.init_window(kind, size, geom);
            }

            // the metal layer, as the old MetalWindow set it up
            let shared = Shared::global();
            let () = msg_send![layer, setDevice: shared.device()];
            let () = msg_send![layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![layer, setPresentsWithTransaction: NO];
            let () = msg_send![layer, setMaximumDrawableCount: 3u64];
            let () = msg_send![layer, setDisplaySyncEnabled: YES];
            let () = msg_send![layer, setNeedsDisplayOnBoundsChange: YES];
            let () = msg_send![layer, setAutoresizingMask: (1u64 << 4) | (1u64 << 1)];
            let () = msg_send![layer, setAllowsNextDrawableTimeout: YES];
            // Host windows retain a pristine base for lending and overlays even
            // when ordinary application windows did not request snapshots.
            let retain = sid == crate::session::HOST || shared.flags() & open_flags::RETAIN_WINDOWS != 0;
            let () = msg_send![layer, setFramebufferOnly: if retain { NO } else { YES }];
            let () = msg_send![layer, setDelegate: view];
            set_layer_background(layer, 1.0);
            let () = msg_send![view, setWantsBestResolutionOpenGLSurface: YES];
            let () = msg_send![view, setWantsLayer: YES];
            let () = msg_send![view, setLayerContentsPlacement: 11i64];
            let () = msg_send![view, setLayer: layer];
            let scale: f64 = msg_send![window, backingScaleFactor];
            let () = msg_send![layer, setContentsScale: scale];
            let frame: NSRect = msg_send![view, frame];
            let () = msg_send![layer, setDrawableSize: CGSize { width: frame.size.width * scale, height: frame.size.height * scale }];
            shared.register_layer(sid, id, RcObjcId::from_unowned(std::ptr::NonNull::new(layer).expect("layer")));
            // `NSView.layer` owns the layer now; balance CAMetalLayer `new`
            let () = msg_send![layer, release];
            let () = msg_send![pool, drain];
            win
        };
        app().add_window(win);
        if super::log_enabled() {
            let frame: NSRect = unsafe { msg_send![win.view, frame] };
            eprintln!("[appkit] window {}:{} made: {}x{} points at scale {}", sid, id.index, frame.size.width, frame.size.height, win.dpi());
        }
        win
    }

    /// A titled / chromeless window (`MacosWindow::init`).
    unsafe fn init_window(&self, kind: u32, size: NSSize, geom: [f32; 4]) {
        let style = self.style;
        let borderless = kind == window_kind::CHROMELESS || style & window_style::BORDERLESS != 0;
        let mut mask = NSWindowStyleMask::NSFullSizeContentViewWindowMask as u64;
        if borderless {
            mask |= NSWindowStyleMask::NSBorderlessWindowMask as u64;
        } else {
            mask |= NSWindowStyleMask::NSTitledWindowMask as u64;
            if style & window_style::NOT_CLOSABLE == 0 {
                mask |= NSWindowStyleMask::NSClosableWindowMask as u64;
            }
            if style & window_style::NOT_MINIATURIZABLE == 0 {
                mask |= NSWindowStyleMask::NSMiniaturizableWindowMask as u64;
            }
            if style & window_style::NOT_RESIZABLE == 0 {
                mask |= NSWindowStyleMask::NSResizableWindowMask as u64;
            }
        }
        if self.nonactivating_panel() {
            mask |= NSWindowStyleMask::NSNonactivatingPanelWindowMask as u64;
        }
        let style_mask = mask;
        let centred = geom[0].is_nan() || geom[1].is_nan();
        let origin = if centred {
            NSPoint { x: 0.0, y: 0.0 }
        } else {
            // a restored position can name a display that is gone: pin it before the window is
            // built, `fit_to_screens` corrects the finished frame
            let (x, y) = screen::clamp_point_to_screens(&screens(), geom[0] as f64, geom[1] as f64);
            NSPoint { x, y }
        };
        let frame = NSRect { origin, size };
        let () = msg_send![self.window, initWithContentRect: frame styleMask: style_mask backing: NSBackingStoreType::NSBackingStoreBuffered as u64 defer: NO];
        let () = msg_send![self.window, setDelegate: self.delegate];
        let () = msg_send![self.window, setReleasedWhenClosed: NO];
        let () = msg_send![self.window, setTitleVisibility: NSWindowTitleVisibility::NSWindowTitleHidden];
        let () = msg_send![self.window, setTitlebarAppearsTransparent: YES];
        // Only AppKit's buttons intercept input above the app-drawn caption.
        self.defang_titlebar_container();
        let mut behavior = 0u64;
        if style & window_style::JOIN_ALL_SPACES != 0 {
            behavior |= NSWindowCollectionBehaviorCanJoinAllSpaces;
        }
        if style & window_style::FULLSCREEN_AUXILIARY != 0 {
            behavior |= NSWindowCollectionBehaviorFullScreenAuxiliary;
        }
        let () = msg_send![self.window, setCollectionBehavior: behavior];
        let level = if style & window_style::LEVEL_STATUS != 0 {
            NSStatusWindowLevel
        } else if style & window_style::LEVEL_FLOATING != 0 {
            NSFloatingWindowLevel
        } else {
            NSNormalWindowLevel
        };
        let () = msg_send![self.window, setLevel: level];
        if style & window_style::PANEL != 0 {
            let () = msg_send![self.window, setHidesOnDeactivate: NO];
            let () = msg_send![self.window, setBecomesKeyOnlyIfNeeded: if style & window_style::KEY_ONLY_IF_NEEDED != 0 { YES } else { NO }];
        }
        let () = msg_send![self.window, setAcceptsMouseMovedEvents: YES];
        let () = msg_send![self.view, setLayerContentsRedrawPolicy: 2i64];
        let () = msg_send![self.window, setContentView: self.view];
        let () = msg_send![self.window, makeFirstResponder: self.view];
        self.add_tracking(size);
        if centred {
            let () = msg_send![self.window, center];
        }
        // a restored size and position are only as good as the display arrangement they were
        // saved on; an exact capture size is not fitted (it may exceed any display)
        if style & window_style::EXACT_SIZE == 0 {
            self.fit_to_screens();
        }
        let hidden_until_present = Shared::global().flags() & open_flags::HIDDEN_UNTIL_PRESENT != 0 && !Shared::global().revealed();
        if hidden_until_present {
            // on screen but transparent and click-through until the first frame is on glass: a
            // window off screen gets no display link and no vsync
            let () = msg_send![self.window, setAlphaValue: 0.0f64];
            let () = msg_send![self.window, setIgnoresMouseEvents: YES];
        }
        if self.nonactivating_panel() || !focus_allowed() {
            // visible but unfocused: never key, the app never activates
            let () = msg_send![self.window, orderFront: nil];
        } else {
            let () = msg_send![self.window, makeKeyAndOrderFront: nil];
            if std::env::args().any(|arg| arg == "--focus") {
                let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                let () = msg_send![ns_app, activateIgnoringOtherApps: YES];
            }
        }
        let input_context: ObjcId = msg_send![self.view, inputContext];
        let () = msg_send![input_context, invalidateCharacterCoordinates];
        let _ = self.capture_default_window_buttons();
    }

    /// A borderless panel at the popup level, placed relative to its parent (`init_popup`).
    unsafe fn init_popup(&self, size: NSSize, geom: [f32; 4], parent: Option<&Win>) {
        let (px, py) = (geom[0] as f64, geom[1] as f64);
        let origin = match parent {
            Some(parent) => {
                let parent_frame: NSRect = msg_send![parent.window, frame];
                let parent_content: NSRect = msg_send![parent.window, contentLayoutRect];
                let x = parent_frame.origin.x + parent_content.origin.x + px;
                let top = parent_frame.origin.y + parent_frame.size.height - parent_content.origin.y;
                NSPoint { x, y: top - py - size.height }
            }
            None => NSPoint { x: px, y: py },
        };
        let frame = NSRect { origin, size };
        let style = NSWindowStyleMask::NSBorderlessWindowMask as u64 | NSWindowStyleMask::NSFullSizeContentViewWindowMask as u64;
        let () = msg_send![self.window, initWithContentRect: frame styleMask: style backing: NSBackingStoreType::NSBackingStoreBuffered as u64 defer: NO];
        let () = msg_send![self.window, setDelegate: self.delegate];
        let () = msg_send![self.window, setReleasedWhenClosed: NO];
        let () = msg_send![self.window, setLevel: NSPopUpMenuWindowLevel];
        let () = msg_send![self.window, setHasShadow: YES];
        let () = msg_send![self.window, setAcceptsMouseMovedEvents: YES];
        let () = msg_send![self.view, setLayerContentsRedrawPolicy: 2i64];
        let () = msg_send![self.window, setContentView: self.view];
        let () = msg_send![self.window, makeFirstResponder: self.view];
        // orderFront, then makeKey so the delegate gets resignKey on focus loss
        let () = msg_send![self.window, orderFront: nil];
        let () = msg_send![self.window, makeKeyWindow];
        self.add_tracking(size);
        let input_context: ObjcId = msg_send![self.view, inputContext];
        let () = msg_send![input_context, invalidateCharacterCoordinates];
    }

    /// A panel that does not activate the app.
    pub fn nonactivating_panel(&self) -> bool {
        self.style & window_style::PANEL != 0 && self.style & window_style::NON_ACTIVATING != 0
    }

    /// A panel that becomes key only when a click needs a text field.
    pub fn needs_panel_to_become_key(&self) -> bool {
        self.style & window_style::PANEL != 0 && self.style & window_style::KEY_ONLY_IF_NEEDED != 0
    }

    unsafe fn add_tracking(&self, size: NSSize) {
        let rect = NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size };
        let track: ObjcId = msg_send![class!(NSTrackingArea), alloc];
        let track: ObjcId = msg_send![
            track,
            initWithRect: rect
            options: NSTrackignActiveAlways | NSTrackingInVisibleRect | NSTrackingMouseEnteredAndExited | NSTrackingMouseMoved | NSTrackingCursorUpdate
            owner: self.view
            userInfo: nil
        ];
        let () = msg_send![self.view, addTrackingArea: track];
    }

    // ---- events to the session

    /// Moves input and window commands to the borrower without replacing AppKit objects.
    pub fn lend(&self, sid: u32, id: Id) {
        self.sid.set(sid);
        self.id.set(id);
        self.want_frame.set(false);
        self.ime_active.set(false);
        self.dpi_override.set(None);
        self.buttons_center_y.set(None);
        self.last_geom.set(None);
        self.send_geom();
    }

    fn push(&self, f: impl FnOnce(&mut crate::buffers::EventQueue)) {
        if !self.retired.get() {
            Shared::global().push(self.sid.get(), f);
        }
    }

    pub fn dpi(&self) -> f64 {
        unsafe { msg_send![self.window, backingScaleFactor] }
    }

    fn state(&self) -> u32 {
        if self.fullscreen.get() {
            return window_state::FULLSCREEN;
        }
        let miniaturized: bool = unsafe { msg_send![self.window, isMiniaturized] };
        if miniaturized {
            window_state::MINIMIZE
        } else {
            window_state::RESTORE
        }
    }

    /// The window's position (Cocoa global points), inner size and scale, as `Geom` / `Chrome`
    /// events; only when something changed since the last report.
    pub fn send_geom(&self) {
        if self.retired.get() {
            return;
        }
        // A relayout can reset the buttons without changing the window's outer geometry.
        self.position_window_buttons();
        let (frame, view): (NSRect, NSRect) = unsafe { (msg_send![self.window, frame], msg_send![self.view, frame]) };
        let dpi = self.dpi();
        let state = self.state();
        let chrome = self.traffic_lights().unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        let level: i64 = unsafe { msg_send![self.window, level] };
        let topmost = level > NSNormalWindowLevel;
        let now = [
            frame.origin.x, frame.origin.y, view.size.width, view.size.height, dpi,
            state as f64, self.fullscreen.get() as u8 as f64,
            chrome.x, chrome.y, chrome.w, chrome.h, topmost as u8 as f64,
        ];
        if self.last_geom.get() == Some(now) {
            return;
        }
        self.last_geom.set(Some(now));
        // Publish together: Cx converts the native button bounds with the following dpi.
        self.push(|q| {
            q.chrome(self.id.get(), chrome.x, chrome.y, chrome.w, chrome.h, topmost);
            q.geom(self.id.get(), now[0], now[1], now[2], now[3], dpi as f32, state);
        });
    }

    /// The bounding box of the three window buttons in window points, top left origin.
    fn traffic_lights(&self) -> Option<Rect> {
        unsafe {
            let close: ObjcId = msg_send![self.window, standardWindowButton: 0u64];
            let miniaturize: ObjcId = msg_send![self.window, standardWindowButton: 1u64];
            let zoom: ObjcId = msg_send![self.window, standardWindowButton: 2u64];
            if close.is_null() || miniaturize.is_null() || zoom.is_null() {
                return None;
            }
            let content_view: ObjcId = msg_send![self.window, contentView];
            let content_frame: NSRect = msg_send![content_view, frame];
            let h = content_frame.size.height;
            let boxed = |btn: ObjcId| -> (f64, f64, f64, f64) {
                let superview: ObjcId = msg_send![btn, superview];
                let ns_frame: NSRect = msg_send![btn, frame];
                let ns_frame: NSRect = msg_send![content_view, convertRect: ns_frame fromView: superview];
                let top = h - (ns_frame.origin.y + ns_frame.size.height);
                (top, ns_frame.origin.x, ns_frame.origin.x + ns_frame.size.width, h - ns_frame.origin.y)
            };
            let (t0, l0, r0, b0) = boxed(close);
            let (t1, l1, r1, b1) = boxed(miniaturize);
            let (t2, l2, r2, b2) = boxed(zoom);
            let (top, left, right, bottom) = (t0.min(t1).min(t2), l0.min(l1).min(l2), r0.max(r1).max(r2), b0.max(b1).max(b2));
            // during a fullscreen transition the content view resizes before the buttons'
            // superview repositions: the buttons are always near the top
            if top > h * 0.5 {
                return None;
            }
            Some(Rect::new(left, top, right - left, bottom - top))
        }
    }

    pub fn got_focus(&self) {
        self.push(|q| q.focus(self.id.get(), true));
    }

    pub fn lost_focus(&self) {
        if self.popup {
            self.push(|q| q.popup_dismissed(self.id.get()));
        } else {
            self.push(|q| q.focus(self.id.get(), false));
        }
    }

    /// `windowShouldClose:`. The app decides: the window stays until it sends `WindowClose`.
    pub fn close_requested(&self) -> bool {
        app::note_exit_requested();
        self.push(|q| q.close_requested(self.id.get()));
        false
    }

    /// `windowWillClose:`: the window is going; the app hears of it once.
    pub fn will_close(&self) {
        if self.retired.replace(true) {
            return;
        }
        Shared::global().unregister_layer(self.owner_sid, self.owner_id);
        app().window_gone(self);
        // retired windows send nothing: the one `Closed` is the last event of the window
        Shared::global().push(self.sid.get(), |q| q.closed(self.id.get()));
        unsafe {
            let () = msg_send![self.window, setAcceptsMouseMovedEvents: NO];
            let () = msg_send![self.window, setDelegate: nil];
        }
    }

    /// Closes the window for a finished session: nothing is told to anyone.
    pub fn close_silently(&self) {
        if self.retired.replace(true) {
            return;
        }
        Shared::global().unregister_layer(self.owner_sid, self.owner_id);
        app().window_gone(self);
        unsafe {
            let () = msg_send![self.window, setAcceptsMouseMovedEvents: NO];
            let () = msg_send![self.window, setDelegate: nil];
            let () = msg_send![self.window, close];
        }
    }

    // ---- pointer

    fn pointer(&self) -> [f64; 2] {
        self.last_mouse.get()
    }

    pub fn mouse_down(&self, event: ObjcId, button: u32, mods: u32) {
        if self.retired.get() {
            return;
        }
        // a real press arrived: the current touch needs no synthesized tap
        self.touch_disqualified.set(true);
        activate_on_pointer_down(self.window);
        let () = unsafe { msg_send![self.window, makeFirstResponder: self.view] };
        self.buttons_down.set(self.buttons_down.get() | button);
        let [x, y] = self.pointer();
        if button == 1 {
            self.remember_press(event);
            // the caption question goes to the app; its answer (`caption_answer`) starts the
            // window drag, or the zoom on a double click
            self.remember_caption(event);
            self.push(|q| q.drag_query(self.id.get(), x, y));
        }
        self.push(|q| q.mouse_down(self.id.get(), x, y, button, mods));
    }

    pub fn mouse_up(&self, button: u32, mods: u32) {
        if self.retired.get() {
            return;
        }
        // the physical button-up ends a scrub pin at the platform layer: the cursor goes back to
        // where the press was even when the app drops the up
        if button == 1 {
            app().end_pin_on_release();
        }
        self.buttons_down.set(self.buttons_down.get() & !button);
        let [x, y] = self.pointer();
        self.push(|q| q.mouse_up(self.id.get(), x, y, button, mods));
    }

    /// A buttonless `mouseMoved:` while a button still counts down: the up never reached the
    /// view (a full-screen transition, a Space switch, a window drag took it). Delivered first.
    pub fn release_lost_buttons(&self, mods: u32) {
        let down = self.buttons_down.get();
        if self.retired.get() || down == 0 {
            return;
        }
        for bit in 0..32 {
            if down & (1 << bit) != 0 {
                self.mouse_up(1 << bit, mods);
            }
        }
        self.buttons_down.set(0);
    }

    pub fn mouse_moved(&self, event: ObjcId, pos: [f64; 2], mods: u32) {
        if self.retired.get() {
            return;
        }
        let [dx, dy]: [f64; 2] = unsafe { [msg_send![event, deltaX], msg_send![event, deltaY]] };
        // pointer lock: the hardware cursor is frozen, `abs` stays pinned at the lock point and
        // the true motion travels as the lock delta
        let (pos, delta) = app().locked_mouse_transform(pos, [dx, dy], self.last_mouse.get());
        self.last_mouse.set(pos);
        if !self.popup && !self.nonactivating_panel() {
            app().startup_focus_hack();
        }
        if event_is_press_motion(event) {
            self.remember_press(event);
        }
        self.push(|q| q.mouse_move(self.id.get(), pos[0], pos[1], delta[0], delta[1], mods));
    }

    /// The pointer position the next event reports, from the view.
    pub fn set_pointer(&self, pos: [f64; 2]) {
        self.last_mouse.set(pos);
    }

    pub fn scroll(&self, delta: [f64; 2], mods: u32, precise: bool, phase: u32) {
        if self.retired.get() {
            return;
        }
        match phase {
            // only live momentum arms the tap synthesizer: after the stream ends macOS delivers
            // taps itself and a synthesized one would double it
            input::scroll::MOMENTUM => self.last_momentum.set(app::time_now()),
            // a scroll gesture means the current touch is not a tap
            input::scroll::BEGAN | input::scroll::CHANGED | input::scroll::ENDED => self.touch_disqualified.set(true),
            _ => {}
        }
        let [x, y] = self.pointer();
        self.push(|q| q.scroll(self.id.get(), x, y, delta[0], delta[1], mods, precise, phase));
    }

    pub fn pinch(&self, scale: f64, phase: u32, _mods: u32) {
        if self.retired.get() {
            return;
        }
        self.touch_disqualified.set(true);
        let [x, y] = self.pointer();
        self.push(|q| q.pinch(self.id.get(), x, y, scale, phase));
    }

    /// A raw trackpad touch began; `single` whether exactly one finger touches.
    pub fn touches_began(&self, single: bool) {
        if single {
            let now = app::time_now();
            self.touch_began.set(now);
            // only a touch that starts while the OS momentum stream is live can have its tap
            // suppressed by macOS; anywhere else the OS delivers the click itself
            self.touch_disqualified.set(now - self.last_momentum.get() > 0.25);
        } else {
            self.touch_began.set(0.0);
            self.touch_disqualified.set(true);
        }
    }

    /// A short single-finger gesture-free touch during the momentum stream: macOS suppressed its
    /// tap-to-click, the tap is delivered here.
    pub fn touches_ended(&self, mods: u32) {
        let began = self.touch_began.replace(0.0);
        if began > 0.0 && !self.touch_disqualified.get() && app::time_now() - began < 0.3 {
            self.mouse_down(nil, 1, mods);
            self.mouse_up(1, mods);
        }
    }

    // ---- caption drags and the file drag the app starts

    fn remember_press(&self, event: ObjcId) {
        if event == nil {
            return;
        }
        unsafe {
            let () = msg_send![event, retain];
            let old = self.last_press.replace(event);
            if old != nil {
                let () = msg_send![old, release];
            }
        }
    }

    fn remember_caption(&self, event: ObjcId) {
        if event == nil {
            return;
        }
        unsafe {
            let () = msg_send![event, retain];
            let old = self.caption_event.replace(event);
            if old != nil {
                let () = msg_send![old, release];
            }
        }
    }

    /// The app's answer to the drag query of the last press. A caption starts the window drag
    /// while the button is still down (the app has seen the press, the shell ends it with a
    /// synthesized up); a double click on the caption does what Desktop & Dock says.
    pub fn caption_answer(&self, answer: u32) {
        let event = self.caption_event.replace(nil);
        if event == nil {
            return;
        }
        unsafe {
            if answer == input::drag_query::CAPTION {
                let clicks: i64 = msg_send![event, clickCount];
                if clicks == 2 {
                    let defaults: ObjcId = msg_send![class!(NSUserDefaults), standardUserDefaults];
                    let action: ObjcId = msg_send![defaults, stringForKey: str_to_nsstring("AppleActionOnDoubleClick")];
                    let action = if action == nil { String::new() } else { nsstring_to_string(action) };
                    // Fill is the Window menu's action; AppKit exposes no public equivalent.
                    let can_fill: bool = msg_send![self.window, respondsToSelector: sel!(_zoomFill:)];
                    match action.as_str() {
                        "Minimize" => {
                            let () = msg_send![self.window, performMiniaturize: nil];
                        }
                        "None" => {}
                        "Fill" if can_fill => {
                            let () = msg_send![self.window, _zoomFill: nil];
                        }
                        _ => {
                            let () = msg_send![self.window, performZoom: nil];
                        }
                    }
                    self.send_geom();
                } else {
                    let pressed: u64 = msg_send![class!(NSEvent), pressedMouseButtons];
                    if pressed & 1 != 0 {
                        let () = msg_send![self.window, performWindowDragWithEvent: event];
                        // the drag loop swallowed the up
                        self.release_lost_buttons(0);
                    }
                }
            }
            let () = msg_send![event, release];
        }
    }

    /// Starts a Finder-compatible file drag from this window (`start_external_dragging`).
    pub fn start_external_drag(&self, items: &[(u32, String)]) -> bool {
        if self.retired.get() || items.is_empty() {
            return false;
        }
        let paths: Option<Vec<&str>> = items.iter().map(|(kind, s)| (*kind == input::drag_item::FILE).then_some(s.as_str())).collect();
        let Some(paths) = paths else {
            eprintln!("[hal] an external drag accepts only public file paths");
            return false;
        };
        if paths.iter().any(|p| {
            let path = std::path::Path::new(p);
            !path.is_absolute() || !path.is_file()
        }) {
            eprintln!("[hal] an external drag requires absolute paths to existing files");
            return false;
        }
        unsafe {
            let event = self.last_press.get();
            if event == nil {
                return false;
            }
            let pressed: u64 = msg_send![class!(NSEvent), pressedMouseButtons];
            if pressed == 0 {
                return false;
            }
            let window_point: NSPoint = msg_send![event, locationInWindow];
            let view_point: NSPoint = msg_send![self.view, convertPoint: window_point fromView: nil];
            let workspace: ObjcId = msg_send![class!(NSWorkspace), sharedWorkspace];
            let mut dragging_items = Vec::with_capacity(paths.len());
            for (index, path) in paths.iter().enumerate() {
                let ns_path = str_to_nsstring(path);
                let file_url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: ns_path isDirectory: NO];
                if file_url == nil {
                    continue;
                }
                let item: ObjcId = msg_send![class!(NSDraggingItem), alloc];
                let item: ObjcId = msg_send![item, initWithPasteboardWriter: file_url];
                if item == nil {
                    continue;
                }
                let icon: ObjcId = msg_send![workspace, iconForFile: ns_path];
                let icon: ObjcId = if icon == nil { nil } else { msg_send![icon, copy] };
                let offset = index as f64 * 3.0;
                let frame = NSRect {
                    origin: NSPoint { x: view_point.x - 24.0 + offset, y: view_point.y - 24.0 - offset },
                    size: NSSize { width: 48.0, height: 48.0 },
                };
                let _: () = msg_send![item, setDraggingFrame: frame contents: icon];
                if icon != nil {
                    let _: () = msg_send![icon, release];
                }
                dragging_items.push(item);
            }
            if dragging_items.is_empty() {
                return false;
            }
            let array: ObjcId = msg_send![class!(NSArray), arrayWithObjects: dragging_items.as_ptr() count: dragging_items.len()];
            let session: ObjcId = msg_send![self.view, beginDraggingSessionWithItems: array event: event source: self.view];
            if session != nil {
                let _: () = msg_send![session, setAnimatesToStartingPositionsOnCancelOrFail: NO];
            }
            for item in dragging_items {
                let _: () = msg_send![item, release];
            }
            session != nil
        }
    }

    /// Dragged items entering / moving over the window; `phase` 0 enter, 1 over.
    pub fn drag_over(&self, phase: u32, pos: [f64; 2], items: &[(u32, String)]) {
        let list: Vec<(u32, &str)> = items.iter().map(|(k, s)| (*k, s.as_str())).collect();
        let payload = pack_drag_items(&list);
        self.push(|q| q.drag_over(self.id.get(), pos[0], pos[1], phase, items.len() as u32, &payload));
    }

    pub fn drag_drop(&self, pos: [f64; 2], dropped: bool, items: &[(u32, String)]) {
        let list: Vec<(u32, &str)> = items.iter().map(|(k, s)| (*k, s.as_str())).collect();
        let payload = pack_drag_items(&list);
        self.push(|q| q.drag_drop(self.id.get(), pos[0], pos[1], dropped, items.len() as u32, &payload));
    }

    // ---- text

    /// Typed text; `replace_last` replaces the previous composition preview / range.
    pub fn text_commit(&self, text: &str, replace: bool) {
        if replace {
            self.push(|q| q.ime(self.id.get(), input::ime::COMMIT, 0, 1, text.as_bytes()));
        } else {
            self.push(|q| q.text(self.id.get(), input::text::TYPED, text.as_bytes()));
        }
    }

    /// The composition preview changed (`start`: the first change of a composition).
    pub fn composition(&self, text: &str, start: bool) {
        let kind = if start { input::ime::START } else { input::ime::UPDATE };
        self.push(|q| q.ime(self.id.get(), kind, 0, 0, text.as_bytes()));
    }

    pub fn composition_cancelled(&self) {
        self.push(|q| q.ime(self.id.get(), input::ime::CANCEL, 0, 0, b""));
    }

    pub fn paste(&self, text: &str) {
        self.push(|q| q.text(self.id.get(), input::text::PASTE, text.as_bytes()));
    }

    pub fn key(&self, key: u32, repeat: bool, mods: u32, down: bool) {
        if down {
            self.push(|q| q.key_down(self.id.get(), key, repeat, mods));
        } else {
            self.push(|q| q.key_up(self.id.get(), key, mods));
        }
    }

    pub fn set_ime(&self, active: bool, rect: [f64; 4]) {
        self.ime_active.set(active);
        if self.ime_rect.get() != rect {
            self.ime_rect.set(rect);
            // the caret line moved: the IME re-asks where to put its candidate window
            unsafe {
                let input_context: ObjcId = msg_send![self.view, inputContext];
                if input_context != nil {
                    let () = msg_send![input_context, invalidateCharacterCoordinates];
                }
            }
        }
        if !active {
            unsafe {
                let has_marked_text: BOOL = msg_send![self.view, hasMarkedText];
                if has_marked_text != NO {
                    // the widget keeps its preview as text on blur: only AppKit's state is cleared
                    super::classes::clear_marked_text(&*self.view);
                }
            }
        }
    }

    // ---- window commands

    pub fn set_title(&self, title: &str) {
        unsafe {
            let () = msg_send![self.window, setTitle: str_to_nsstring(title)];
        }
        self.send_geom();
    }

    pub fn set_state(&self, state: u32) {
        unsafe {
            match state {
                window_state::MINIMIZE => {
                    let () = msg_send![self.window, miniaturize: nil];
                }
                window_state::MAXIMIZE | window_state::FULLSCREEN => {
                    // the toggle: into fullscreen only when not in it
                    if !self.fullscreen.get() {
                        let () = msg_send![self.window, toggleFullScreen: nil];
                    }
                }
                window_state::HIDE => {
                    let () = msg_send![self.window, orderOut: nil];
                }
                window_state::SHOW => {
                    let () = msg_send![self.window, orderFront: nil];
                }
                _ => {
                    // restore: out of fullscreen, out of the Dock
                    if self.fullscreen.get() {
                        let () = msg_send![self.window, toggleFullScreen: nil];
                    }
                    let miniaturized: bool = msg_send![self.window, isMiniaturized];
                    if miniaturized {
                        let () = msg_send![self.window, deminiaturize: nil];
                    }
                }
            }
        }
        self.send_geom();
    }

    /// Position and outer size of the window.
    pub fn set_geom(&self, geom: [f32; 4]) {
        let [x, y, w, h] = geom.map(|v| v as f64);
        unsafe {
            let mut frame: NSRect = msg_send![self.window, frame];
            if x.is_finite() && y.is_finite() {
                frame.origin = NSPoint { x, y };
            }
            if w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0 {
                frame.size = NSSize { width: w, height: h };
            }
            // a caller placing the window cannot know the display arrangement it places into
            let all = if self.style & window_style::EXACT_SIZE != 0 { Vec::new() } else { screens() };
            let fitted = screen::fit_window_rect_to_screens(&all, Rect::new(frame.origin.x, frame.origin.y, frame.size.width, frame.size.height));
            let () = msg_send![self.window, setFrame: NSRect { origin: NSPoint { x: fitted.x, y: fitted.y }, size: NSSize { width: fitted.w, height: fitted.h } } display: YES];
        }
    }

    /// Moves and resizes the window so it sits entirely within one display's visible frame.
    pub fn fit_to_screens(&self) {
        let screens = screens();
        if screens.is_empty() {
            return;
        }
        unsafe {
            let frame: NSRect = msg_send![self.window, frame];
            let current = Rect::new(frame.origin.x, frame.origin.y, frame.size.width, frame.size.height);
            let fitted = screen::fit_window_rect_to_screens(&screens, current);
            if fitted == current {
                return;
            }
            let () = msg_send![self.window, setFrame: NSRect { origin: NSPoint { x: fitted.x, y: fitted.y }, size: NSSize { width: fitted.w, height: fitted.h } } display: YES];
        }
    }

    pub fn set_topmost(&self, on: bool) {
        unsafe {
            let () = msg_send![self.window, setLevel: if on { NSFloatingWindowLevel } else { NSNormalWindowLevel }];
        }
        self.send_geom();
    }

    pub fn set_buttons_visible(&self, visible: bool) {
        unsafe {
            let hidden = if visible { NO } else { YES };
            for which in 0..3u64 {
                let button: ObjcId = msg_send![self.window, standardWindowButton: which];
                if button != nil {
                    let () = msg_send![button, setHidden: hidden];
                }
            }
        }
        self.send_geom();
    }

    pub fn center_window_buttons(&self, y_center: f64) {
        self.buttons_center_y.set(Some(y_center));
        self.send_geom();
    }

    pub fn set_dpi_override(&self, dpi_override: Option<f64>) {
        self.dpi_override.set(dpi_override);
        self.send_geom();
    }

    /// Native buttons keep their size; the caption expands only when the UI zooms in.
    pub fn window_buttons_zoom(&self) -> f64 {
        self.dpi_override.get().map_or(1.0, |dpi| dpi / self.dpi()).max(1.0)
    }

    /// Reapply after AppKit lays out its titlebar, using its original origins, never a delta
    /// from an already zoomed layout. An explicit center overrides only the vertical position.
    pub(super) fn position_window_buttons(&self) {
        const RIGHT_TO_LEFT: i64 = 1;

        if self.retired.get() || self.fullscreen.get() || self.exiting_fullscreen.get() {
            return;
        }
        let zoom = self.window_buttons_zoom();
        if zoom == 1.0 && self.window_buttons_offset.get() == [0.0; 2] && self.buttons_center_y.get().is_none() {
            return;
        }
        let tab_group: ObjcId = unsafe { msg_send![self.window, tabGroup] };
        if tab_group != nil && unsafe { msg_send![tab_group, isTabBarVisible] } {
            return;
        }
        let container = self.titlebar_container();
        if container == nil {
            return;
        }
        let Some(buttons) = self.window_buttons() else { return };
        let Some(default) = self.capture_default_window_buttons() else { return };
        // Whole backing pixels keep the native controls sharp.
        let dpi = self.dpi();
        let offset = (default.caption_height * (zoom - 1.0) / 2.0 * dpi).round() / dpi;
        let y_offset = self.buttons_center_y.get().map_or(offset, |center| center - default.caption_height / 2.0);
        unsafe {
            let theme_frame: ObjcId = msg_send![container, superview];
            if theme_frame == nil {
                return;
            }
            let theme_bounds: NSRect = msg_send![theme_frame, bounds];
            let mut titlebar_frame: NSRect = msg_send![container, frame];
            let height = default.titlebar_height + 2.0 * y_offset;
            let y = theme_bounds.size.height - height;
            if titlebar_frame.size.height != height || titlebar_frame.origin.y != y {
                titlebar_frame.size.height = height;
                titlebar_frame.origin.y = y;
                let () = msg_send![container, setFrame: titlebar_frame];
            }
            // RTL buttons stay anchored to the right edge as the window width changes.
            let direction: i64 = msg_send![self.window, windowTitlebarLayoutDirection];
            let x_offset = if direction == RIGHT_TO_LEFT { titlebar_frame.size.width - default.titlebar_width - offset } else { offset };
            for (button, origin) in buttons.into_iter().zip(default.origins) {
                let origin = NSPoint { x: origin.x + x_offset, y: origin.y + y_offset };
                let frame: NSRect = msg_send![button, frame];
                if frame.origin.x != origin.x || frame.origin.y != origin.y {
                    let () = msg_send![button, setFrameOrigin: origin];
                }
            }
        }
        self.window_buttons_offset.set([offset, y_offset]);
    }

    fn window_buttons(&self) -> Option<[ObjcId; 3]> {
        let buttons = [0u64, 1, 2].map(|kind| -> ObjcId {
            unsafe { msg_send![self.window, standardWindowButton: kind] }
        });
        (!buttons.contains(&nil)).then_some(buttons)
    }

    fn capture_default_window_buttons(&self) -> Option<DefaultWindowButtons> {
        if self.default_window_buttons.get().is_none() {
            let buttons = self.window_buttons()?;
            let lights = self.traffic_lights()?;
            let container = self.titlebar_container();
            if container == nil {
                return None;
            }
            let titlebar_frame: NSRect = unsafe { msg_send![container, frame] };
            self.default_window_buttons.set(Some(DefaultWindowButtons {
                titlebar_width: titlebar_frame.size.width,
                titlebar_height: titlebar_frame.size.height,
                caption_height: lights.y * 2.0 + lights.h,
                origins: buttons.map(|button| {
                    let frame: NSRect = unsafe { msg_send![button, frame] };
                    frame.origin
                }),
            }));
        }
        self.default_window_buttons.get()
    }

    /// Keep only the standard buttons hittable in the native titlebar container.
    unsafe fn defang_titlebar_container(&self) {
        let subclass = app().classes().titlebar_container;
        if subclass.is_null() {
            return;
        }
        let close: ObjcId = msg_send![self.window, standardWindowButton: 0u64];
        let titlebar: ObjcId = msg_send![close, superview];
        let container: ObjcId = msg_send![titlebar, superview];
        let Some(class) = Class::get("NSTitlebarContainerView") else { return };
        let is_container: bool = msg_send![container, isKindOfClass: class];
        if is_container {
            object_setClass(container, subclass as ObjcId);
        }
    }

    /// Only the container whose hit testing we control can grow over the app's caption.
    fn titlebar_container(&self) -> ObjcId {
        let class = app().classes().titlebar_container;
        if class.is_null() {
            return nil;
        }
        unsafe {
            let close: ObjcId = msg_send![self.window, standardWindowButton: 0u64];
            let titlebar: ObjcId = msg_send![close, superview];
            let container: ObjcId = msg_send![titlebar, superview];
            let is_ours: bool = msg_send![container, isKindOfClass: &*class];
            if is_ours { container } else { nil }
        }
    }

    pub fn set_titlebar_visible(&self, visible: bool) {
        unsafe {
            let () = msg_send![self.titlebar_container(), setHidden: if visible { NO } else { YES }];
        }
    }

    /// `visuals`: bit 0 transparent, bits 8.. the backdrop (`WindowBackdrop`: None 0, Auto 1,
    /// Mica 2, Acrylic 3, Vibrancy 4, Blur 5), `intensity` the backdrop's alpha.
    pub fn set_visuals(&self, visuals: u32, intensity: f32) {
        const NS_VIEW_WIDTH_SIZABLE: i64 = 1 << 1;
        const NS_VIEW_HEIGHT_SIZABLE: i64 = 1 << 4;
        const MATERIAL_HUD_WINDOW: i64 = 1;
        const MATERIAL_UNDER_WINDOW_BACKGROUND: i64 = 12;
        let transparent = visuals & 1 != 0;
        let backdrop = (visuals >> 8) & 0xff;
        unsafe {
            self.transparent.set(transparent);
            let () = msg_send![self.window, setOpaque: if transparent { NO } else { YES }];
            let bg_color: ObjcId = if transparent { msg_send![class!(NSColor), clearColor] } else { msg_send![class!(NSColor), windowBackgroundColor] };
            let () = msg_send![self.window, setBackgroundColor: bg_color];
            let () = msg_send![self.layer, setOpaque: if transparent { NO } else { YES }];
            set_layer_background(self.layer, if transparent { 0.0 } else { 1.0 });
            if backdrop != 0 {
                let effect_view = if self.effect_view.get() == nil {
                    let effect_view: ObjcId = msg_send![class!(NSVisualEffectView), alloc];
                    let bounds: NSRect = msg_send![self.view, bounds];
                    let effect_view: ObjcId = msg_send![effect_view, initWithFrame: bounds];
                    let () = msg_send![effect_view, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE];
                    let () = msg_send![self.view, addSubview: effect_view positioned: 0i64 relativeTo: nil];
                    self.effect_view.set(effect_view);
                    effect_view
                } else {
                    self.effect_view.get()
                };
                let material = if backdrop == 5 { MATERIAL_HUD_WINDOW } else { MATERIAL_UNDER_WINDOW_BACKGROUND };
                let () = msg_send![effect_view, setMaterial: material];
                let () = msg_send![effect_view, setBlendingMode: 0i64];
                let () = msg_send![effect_view, setState: 1i64];
                let () = msg_send![effect_view, setAlphaValue: intensity.clamp(0.0, 1.0) as f64];
            } else if self.effect_view.get() != nil {
                let () = msg_send![self.effect_view.get(), removeFromSuperview];
                self.effect_view.set(nil);
            }
        }
        self.send_geom();
    }

    /// The window's pointer, in view points with the origin at the top left.
    pub fn view_point(&self, window_point: NSPoint) -> [f64; 2] {
        unsafe {
            let p: NSPoint = msg_send![self.view, convertPoint: window_point fromView: nil];
            let frame: NSRect = msg_send![self.view, frame];
            [p.x, frame.size.height - p.y]
        }
    }
}

fn event_is_press_motion(event: ObjcId) -> bool {
    let kind: NSEventType = unsafe { msg_send![event, type] };
    DRAG_START_EVENTS.iter().any(|k| *k == kind)
}

/// Whether this process may activate itself or make a window key. An agent-driven or test
/// instance (`MAKEPAD_NO_FOCUS`, the `NO_FOCUS` open flag the client sets for `--remote`) shows
/// its windows but never takes the user's focus; `MAKEPAD_FOCUS` restores activation.
pub fn focus_allowed() -> bool {
    static ALLOWED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALLOWED.get_or_init(|| {
        if std::env::var_os("MAKEPAD_FOCUS").is_some() {
            return true;
        }
        if std::env::var_os("MAKEPAD_NO_FOCUS").is_some() {
            return false;
        }
        Shared::global().flags() & open_flags::NO_FOCUS == 0
    })
}

/// Activates the app and makes the window key on a pointer press (also for a press the remote
/// bridge injects), unless this is a no-focus process.
pub fn activate_on_pointer_down(window: ObjcId) -> bool {
    if window == nil || !focus_allowed() {
        return false;
    }
    unsafe {
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let active: bool = msg_send![ns_app, isActive];
        if !active {
            let () = msg_send![ns_app, activateIgnoringOtherApps: YES];
        }
        let key: bool = msg_send![window, isKeyWindow];
        if !key {
            let () = msg_send![window, makeKeyAndOrderFront: nil];
        }
        !active || !key
    }
}

pub fn set_layer_background(layer: ObjcId, alpha: f64) {
    unsafe {
        let color = CGColorCreateGenericRGB(0.0, 0.0, 0.0, alpha);
        let () = msg_send![layer, setBackgroundColor: color];
        CFRelease(color as *const c_void);
    }
}

/// The displays currently attached, in Cocoa's global point space (bottom-left origin).
pub fn screens() -> Vec<Screen> {
    unsafe {
        let screens: ObjcId = msg_send![class!(NSScreen), screens];
        let count: usize = msg_send![screens, count];
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            let s: ObjcId = msg_send![screens, objectAtIndex: index];
            if s == nil {
                continue;
            }
            let frame: NSRect = msg_send![s, frame];
            let visible: NSRect = msg_send![s, visibleFrame];
            out.push(Screen {
                bounds: Rect::new(frame.origin.x, frame.origin.y, frame.size.width, frame.size.height),
                work_area: Rect::new(visible.origin.x, visible.origin.y, visible.size.width, visible.size.height),
                // element zero of `NSScreen.screens` holds the menu bar: the one Cocoa places
                // windows against
                is_primary: index == 0,
            });
        }
        out
    }
}
