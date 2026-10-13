//! One window of the shell: a view backed by its `CAMetalLayer` (the root view of the UIWindow,
//! a full-screen view above it, or a popup's view over its parent), the state its callbacks
//! need, and what became of each touch (the `RawEvent` it makes). Moved from
//! os/apple/ios/ios_app.rs (geometry, touches, long press). Everything here runs on the main
//! thread.

use super::app::app;
use super::Shared;
#[cfg(target_os = "ios")]
use crate::buffers::Wire;
use crate::types::*;
use makepad_apple_sys::*;
use std::cell::Cell;
#[cfg(target_os = "ios")]
use std::cell::RefCell;
use std::os::raw::c_void;

pub struct Win {
    pub sid: Cell<u32>,
    pub owner_sid: u32,
    pub id: Cell<Id>,
    pub owner_id: Id,
    pub popup: bool,
    pub parent: Option<&'static Win>,
    pub view: ObjcId,
    pub retired: Cell<bool>,
    /// the display link of the window and whether the app asked for a frame
    pub link: Cell<ObjcId>,
    pub want_frame: Cell<bool>,
    /// what the app last heard: x, y, w, h, dpi, safe-area top, right, bottom, left, refresh
    last_geom: Cell<Option<[f64; 10]>>,
    focused: Cell<Option<bool>>,
    /// the touches on the view: the UITouch (UIKit keeps it for the whole gesture) and the
    /// point the app knows it by
    #[cfg(target_os = "ios")]
    touches: RefCell<Vec<(usize, TouchPoint)>>,
    #[cfg(target_os = "ios")]
    next_touch: Cell<u32>,
}

/// The CAMetalLayer of a view as a window needs it: the device, BGRA, triple buffered.
pub(super) fn init_view(view: ObjcId, opaque: bool) {
    unsafe {
        let layer: ObjcId = msg_send![view, layer];
        let screen: ObjcId = msg_send![class!(UIScreen), mainScreen];
        let scale: f64 = msg_send![screen, scale];
        let () = msg_send![view, setContentScaleFactor: scale];
        let () = msg_send![view, setOpaque: if opaque { YES } else { NO }];
        let () = msg_send![view, setMultipleTouchEnabled: YES];
        let () = msg_send![view, setUserInteractionEnabled: YES];
        let () = msg_send![layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
        let () = msg_send![layer, setPresentsWithTransaction: NO];
        let () = msg_send![layer, setMaximumDrawableCount: 3u64];
        let () = msg_send![layer, setAllowsNextDrawableTimeout: YES];
        let () = msg_send![layer, setOpaque: if opaque { YES } else { NO }];
        let () = msg_send![layer, setContentsScale: scale];
        if !opaque {
            let clear: ObjcId = msg_send![class!(UIColor), clearColor];
            let () = msg_send![view, setBackgroundColor: clear];
        }
    }
}

/// The long press of a view (iOS): it begins once the finger rested long enough; the touches
/// keep reaching the view.
pub(super) fn add_gestures(view: ObjcId) {
    #[cfg(target_os = "ios")]
    unsafe {
        let recognizer: ObjcId = msg_send![class!(UILongPressGestureRecognizer), alloc];
        let recognizer: ObjcId = msg_send![recognizer, initWithTarget: app().pulse.get() action: sel!(longPress:)];
        let () = msg_send![recognizer, setCancelsTouchesInView: NO];
        let () = msg_send![view, addGestureRecognizer: recognizer];
        let () = msg_send![recognizer, release];
    }
    #[cfg(not(target_os = "ios"))]
    let _ = view;
}

impl Win {
    /// The window's view: the root view for the first normal window (while no other window has
    /// it), else a new view: full screen over the root view, or for a popup `geom` (x, y relative
    /// to the parent's view, w, h in points) over its parent.
    pub fn create(sid: u32, id: Id, kind: u32, parent: Option<&'static Win>, geom: [f32; 4]) -> &'static Win {
        let a = app();
        let popup = kind & 0xff == window_kind::POPUP && parent.is_some();
        let shared = Shared::global();
        let view = unsafe {
            if !popup && !a.root_taken() {
                a.root.get()
            } else {
                let host: ObjcId = if popup { parent.map_or(a.root.get(), |p| p.view) } else { a.root.get() };
                let frame = if popup {
                    NSRect { origin: NSPoint { x: geom[0] as f64, y: geom[1] as f64 }, size: NSSize { width: geom[2].max(1.0) as f64, height: geom[3].max(1.0) as f64 } }
                } else {
                    msg_send![host, bounds]
                };
                let view: ObjcId = msg_send![a.classes().view, alloc];
                let view: ObjcId = msg_send![view, initWithFrame: frame];
                init_view(view, !popup);
                if !popup {
                    // UIViewAutoresizingFlexibleWidth | FlexibleHeight: follows rotation
                    let () = msg_send![view, setAutoresizingMask: 18u64];
                }
                add_gestures(view);
                let () = msg_send![host, addSubview: view];
                // the superview keeps it
                let () = msg_send![view, release];
                view
            }
        };
        let layer: ObjcId = unsafe { msg_send![view, layer] };
        unsafe {
            let () = msg_send![layer, setDevice: shared.device()];
            // host windows keep a pristine base for lending and overlays
            let retain = sid == crate::session::HOST || shared.flags() & open_flags::RETAIN_WINDOWS != 0;
            let () = msg_send![layer, setFramebufferOnly: if retain { NO } else { YES }];
        }
        let win: &'static Win = Box::leak(Box::new(Win {
            sid: Cell::new(sid),
            owner_sid: sid,
            id: Cell::new(id),
            owner_id: id,
            popup,
            parent,
            view,
            retired: Cell::new(false),
            link: Cell::new(nil),
            want_frame: Cell::new(false),
            last_geom: Cell::new(None),
            focused: Cell::new(None),
            #[cfg(target_os = "ios")]
            touches: RefCell::new(Vec::new()),
            #[cfg(target_os = "ios")]
            next_touch: Cell::new(1),
        }));
        unsafe {
            (*view).set_ivar::<*mut c_void>("hal_win", win as *const Win as *mut c_void);
            let () = msg_send![view, setHidden: NO];
        }
        shared.register_layer(sid, id, RcObjcId::from_unowned(std::ptr::NonNull::new(layer).expect("layer")));
        a.add_window(win);
        if super::log_enabled() {
            let bounds: NSRect = unsafe { msg_send![view, bounds] };
            eprintln!("[uikit] window {}:{} made{}: {}x{} points", sid, id.index, if popup { " (popup)" } else { "" }, bounds.size.width, bounds.size.height);
        }
        win
    }

    pub fn lend(&self, sid: u32, id: Id) {
        self.sid.set(sid);
        self.id.set(id);
        self.want_frame.set(false);
        self.last_geom.set(None);
        self.focused.set(None);
        #[cfg(target_os = "ios")]
        self.touches.borrow_mut().clear();
        self.send_geom();
        self.focus(app().active());
    }

    pub(super) fn push(&self, f: impl FnOnce(&mut crate::buffers::EventQueue)) -> u64 {
        if self.retired.get() {
            return 0;
        }
        Shared::global().push(self.sid.get(), f)
    }

    pub fn dpi(&self) -> f64 {
        unsafe { msg_send![self.view, contentScaleFactor] }
    }

    /// The window's place (a popup's, in its parent's view), size, scale and safe area, as
    /// `SafeArea` / `Geom` / `Display` events; only what changed since the last report.
    pub fn send_geom(&self) {
        if self.retired.get() {
            return;
        }
        let (frame, bounds, insets, refresh): (NSRect, NSRect, UIEdgeInsets, isize) = unsafe {
            let screen: ObjcId = msg_send![class!(UIScreen), mainScreen];
            (msg_send![self.view, frame], msg_send![self.view, bounds], msg_send![self.view, safeAreaInsets], msg_send![screen, maximumFramesPerSecond])
        };
        let dpi = self.dpi();
        let (x, y) = if self.popup { (frame.origin.x, frame.origin.y) } else { (0.0, 0.0) };
        // a popup lies within its parent: the parent's safe area is the parent's business
        let insets = if self.popup { [0.0; 4] } else { [insets.top, insets.right, insets.bottom, insets.left] };
        let now = [x, y, bounds.size.width, bounds.size.height, dpi, insets[0], insets[1], insets[2], insets[3], refresh as f64];
        let before = self.last_geom.replace(Some(now));
        if before == Some(now) {
            return;
        }
        let id = self.id.get();
        if before.is_none_or(|b| b[4] != dpi || b[9] != now[9]) {
            self.push(|q| q.display(id, dpi as f32, refresh as f32));
        }
        if !self.popup {
            // ahead of the geometry, which then carries it
            self.push(|q| q.safe_area(id, insets[0], insets[1], insets[2], insets[3]));
        }
        let state = if self.popup { window_state::RESTORE } else { window_state::FULLSCREEN };
        self.push(|q| q.geom(id, x, y, now[2], now[3], dpi as f32, state));
    }

    /// The app became active or not: the window has the focus while it is.
    pub fn focus(&self, focused: bool) {
        if self.popup || self.focused.replace(Some(focused)) == Some(focused) {
            return;
        }
        let id = self.id.get();
        self.push(|q| q.focus(id, focused));
    }

    /// A popup moves and resizes within its parent; a full-screen window keeps the screen.
    pub fn set_geom(&self, geom: [f32; 4]) {
        if !self.popup {
            return;
        }
        let frame = NSRect { origin: NSPoint { x: geom[0] as f64, y: geom[1] as f64 }, size: NSSize { width: geom[2].max(1.0) as f64, height: geom[3].max(1.0) as f64 } };
        unsafe {
            let () = msg_send![self.view, setFrame: frame];
        }
    }

    pub fn set_hidden(&self, hidden: bool) {
        unsafe {
            let () = msg_send![self.view, setHidden: if hidden { YES } else { NO }];
        }
    }

    /// The app closes the window: it hears `Closed` once, the view goes (the root view stays
    /// in the window controller, free for the next window).
    pub fn close(&'static self) {
        if self.retired.get() {
            return;
        }
        let id = self.id.get();
        Shared::global().push(self.sid.get(), |q| q.closed(id));
        self.close_silently();
    }

    /// Closes the window for a finished session: nothing is told to anyone.
    pub fn close_silently(&'static self) {
        if self.retired.replace(true) {
            return;
        }
        Shared::global().unregister_layer(self.owner_sid, self.owner_id);
        unsafe {
            let link = self.link.replace(nil);
            if link != nil {
                let () = msg_send![link, invalidate];
                let () = msg_send![link, release];
            }
            (*self.view).set_ivar::<*mut c_void>("hal_win", std::ptr::null_mut());
            if self.view != app().root.get() {
                let () = msg_send![self.view, removeFromSuperview];
            }
        }
        self.want_frame.set(false);
        // the popups over it go with it
        for child in app().live_windows().into_iter().filter(|w| w.parent.is_some_and(|p| std::ptr::eq(p, self))) {
            child.close();
        }
    }

    /// An external drag the shell cannot start ended at once, with no drop: the app's gesture
    /// guards recover.
    pub fn drag_ended(&self) {
        let id = self.id.get();
        self.push(|q| q.drag_drop(id, -100000.0, -100000.0, false, 0, &[]));
    }

    // ---- touches (iOS)

    /// The touches of `set` changed (`phase` start / move / stop): the app hears every touch on
    /// the view, the unchanged ones as stable, and lifted ones are forgotten after.
    #[cfg(target_os = "ios")]
    pub fn touches(&self, set: ObjcId, phase: u32) {
        if phase == input::touch::START {
            self.dismiss_popups_outside();
        }
        {
            let mut touches = self.touches.borrow_mut();
            for touch in ns_set_objects(set) {
                let (at, force): (NSPoint, f64) = unsafe { (msg_send![touch, locationInView: self.view], msg_send![touch, force]) };
                let key = touch as usize;
                let point = match touches.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, point)) => point,
                    None => {
                        let id = self.next_touch.get();
                        self.next_touch.set(id.wrapping_add(1).max(1));
                        touches.push((key, TouchPoint { id, phase, x: 0.0, y: 0.0, force: 0.0 }));
                        &mut touches.last_mut().expect("pushed").1
                    }
                };
                point.phase = phase;
                point.x = at.x as f32;
                point.y = at.y as f32;
                point.force = force as f32;
            }
        }
        let mut touches = self.touches.borrow_mut();
        let mut bytes = Vec::with_capacity(touches.len() * TouchPoint::SIZE);
        for (_, point) in touches.iter() {
            point.write(&mut bytes);
        }
        let count = touches.len() as u32;
        let id = self.id.get();
        self.push(|q| q.touch(id, count, &bytes));
        touches.retain_mut(|(_, point)| {
            if point.phase == input::touch::STOP {
                return false;
            }
            point.phase = input::touch::STABLE;
            true
        });
    }

    /// The system took the touches of `set` away (a gesture, a call): the app hears them
    /// cancelled, never lifted.
    #[cfg(target_os = "ios")]
    pub fn touches_cancelled(&self, set: ObjcId) {
        let keys: Vec<usize> = ns_set_objects(set).into_iter().map(|t| t as usize).collect();
        let mut touches = self.touches.borrow_mut();
        let mut bytes = Vec::new();
        let mut count = 0u32;
        touches.retain(|(key, point)| {
            if keys.contains(key) {
                point.write(&mut bytes);
                count += 1;
                false
            } else {
                true
            }
        });
        if count > 0 {
            let id = self.id.get();
            self.push(|q| q.touch_cancel(id, count, &bytes));
        }
    }

    /// The recognizer gives no touch: the long press names the touch nearest to it, so widgets
    /// match it to the touch they captured.
    #[cfg(target_os = "ios")]
    pub fn long_press(&self, x: f64, y: f64) {
        let pointer = self
            .touches
            .borrow()
            .iter()
            .map(|(_, p)| p)
            .min_by(|a, b| {
                let da = (a.x as f64 - x).powi(2) + (a.y as f64 - y).powi(2);
                let db = (b.x as f64 - x).powi(2) + (b.y as f64 - y).powi(2);
                da.total_cmp(&db)
            })
            .map_or(0, |p| p.id);
        let id = self.id.get();
        self.push(|q| q.long_press(id, x, y, pointer));
    }

    /// A touch began on this window: the popups it is not in (nor over) are dismissed.
    #[cfg(target_os = "ios")]
    fn dismiss_popups_outside(&self) {
        let mut keep: Vec<*const Win> = Vec::new();
        let mut at: Option<&Win> = Some(self);
        while let Some(w) = at {
            keep.push(w as *const Win);
            at = w.parent;
        }
        for popup in app().live_windows().into_iter().filter(|w| w.popup && !keep.contains(&(*w as *const Win))) {
            let id = popup.id.get();
            popup.push(|q| q.popup_dismissed(id));
        }
    }
}

/// The objects of an NSSet.
#[cfg(target_os = "ios")]
fn ns_set_objects(set: ObjcId) -> Vec<ObjcId> {
    let mut out = Vec::new();
    unsafe {
        let enumerator: ObjcId = msg_send![set, objectEnumerator];
        loop {
            let object: ObjcId = msg_send![enumerator, nextObject];
            if object == nil {
                break;
            }
            out.push(object);
        }
    }
    out
}
