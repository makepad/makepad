//! The application side of the shell: `UIApplicationMain` on the main thread, the one `UIWindow`
//! and its root view controller, the ops sessions send, the app's lifecycle and the per-window
//! display links. Moved from os/apple/ios/ios_app.rs (`IosApp`) and os/apple/tvos/tvos_app.rs.

use super::classes::Classes;
use super::window::Win;
use super::{lock, Op, Shared};
use crate::backend::exec::OsCmd;
use crate::types::*;
use makepad_apple_sys::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::os::raw::c_void;
use std::sync::atomic::{AtomicU8, Ordering};

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopPerformBlock(rl: *mut c_void, mode: ObjcId, block: *const c_void);
    fn CFRunLoopWakeUp(rl: *mut c_void);
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CAFrameRateRange {
    minimum: f32,
    maximum: f32,
    preferred: f32,
}

unsafe impl Encode for CAFrameRateRange {
    fn encode() -> Encoding {
        unsafe { Encoding::from_str("{CAFrameRateRange=fff}") }
    }
}

/// The system appearance as the main thread last read it: 0 unknown, 1 light, 2 dark.
static APPEARANCE: AtomicU8 = AtomicU8::new(0);

/// Whether the system is in dark mode, as of the last look the main thread took (at launch and
/// whenever the trait collection changed); `None` before the window system is up.
pub fn host_dark() -> Option<bool> {
    match APPEARANCE.load(Ordering::Acquire) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

/// Reads `userInterfaceStyle` (UIUserInterfaceStyleLight 1, Dark 2) of the main screen's traits.
pub(super) fn read_appearance() {
    unsafe {
        let screen: ObjcId = msg_send![class!(UIScreen), mainScreen];
        let traits: ObjcId = msg_send![screen, traitCollection];
        if traits == nil {
            return;
        }
        let style: i64 = msg_send![traits, userInterfaceStyle];
        if style == 1 || style == 2 {
            APPEARANCE.store(style as u8, Ordering::Release);
        }
    }
}

/// What only the main thread touches.
pub struct App {
    classes: OnceCell<Classes>,
    /// every window ever made, in order; closed ones stay allocated (their views keep a pointer
    /// to them) and are flagged retired
    windows: RefCell<Vec<&'static Win>>,
    /// the one UIWindow, its root view controller and the root view (made at launch, before any
    /// session; the first normal window of a session takes the root view)
    pub window: Cell<ObjcId>,
    pub controller: Cell<ObjcId>,
    pub root: Cell<ObjcId>,
    /// the target of display links and gesture recognizers
    pub pulse: Cell<ObjcId>,
    /// the app is active (between `didBecomeActive` and `willResignActive`): the GPU may be used
    active: Cell<bool>,
}

/// SAFETY: the app is only ever touched on the main thread.
struct MainOnly(App);
unsafe impl Sync for MainOnly {}
unsafe impl Send for MainOnly {}

static APP: std::sync::OnceLock<MainOnly> = std::sync::OnceLock::new();

pub fn app() -> &'static App {
    &APP.get_or_init(|| {
        MainOnly(App {
            classes: OnceCell::new(),
            windows: RefCell::new(Vec::new()),
            window: Cell::new(nil),
            controller: Cell::new(nil),
            root: Cell::new(nil),
            pulse: Cell::new(nil),
            active: Cell::new(false),
        })
    })
    .0
}

// ------------------------------------------------------------------ the loop

/// Gives the process's main thread to UIKit: `UIApplicationMain` runs at once (iOS ends an app
/// that does not bring up its UI) and never returns; the process ends with [`stop_main_loop`]
/// once the app is done, or when the OS ends it.
pub fn run_main_loop() {
    unsafe {
        let classes = Classes::new();
        let delegate = classes.app_delegate;
        let _ = app().classes.set(classes);
        let class_name = NSStringFromClass(delegate as ObjcId);
        let mut arg0 = *b"makepad\0";
        let mut argv = [arg0.as_mut_ptr() as *mut i8, std::ptr::null_mut()];
        UIApplicationMain(1, argv.as_mut_ptr(), nil, class_name);
    }
}

/// The app is done (any thread): the process ends on the main thread, after the guards that
/// keep the loop serving native teardown are gone. A UIKit app has no loop to return from.
pub fn stop_main_loop() {
    let shared = Shared::global();
    lock(&shared.run).stop = true;
    shared.run_cv.notify_all();
    wake_event_loop();
}

fn stopping() -> bool {
    let run = lock(&Shared::global().run);
    run.stop && run.keep_alive == 0
}

/// The main thread looks again at whether it should end the process.
pub(super) fn wake_event_loop() {
    run_on_main(|| {
        if stopping() {
            drain_ops();
            std::process::exit(0);
        }
    });
}

/// Runs `f` on the main thread in every run-loop mode (also during UIKit's tracking loops).
pub(super) fn run_on_main(f: impl Fn() + Send + 'static) {
    unsafe {
        let block = objc_block!(move || {
            f();
        });
        let main = CFRunLoopGetMain();
        CFRunLoopPerformBlock(main, NSRunLoopCommonModes, &block as *const _ as *const c_void);
        CFRunLoopWakeUp(main);
    }
}

/// Ops are waiting: a run-loop block applies them.
pub(super) fn schedule_drain() {
    run_on_main(drain_ops);
}

/// Applies the ops sessions sent, in order.
fn drain_ops() {
    let ops = Shared::global().take_ops();
    for op in ops {
        let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply(op)));
        if result.is_err() {
            eprintln!("makepad-hal: PANIC contained applying a window-system op; the shell goes on");
        }
        let () = unsafe { msg_send![pool, release] };
    }
}

/// `application:didFinishLaunchingWithOptions:`: the window, its root view controller and the
/// root view exist from here on; sessions waiting in `hal_open` get the screen's scale.
pub(super) fn did_finish_launching() {
    unsafe {
        let a = app();
        let classes = a.classes();
        objc_set_exception_preprocessor(log_objc_exception);
        let screen: ObjcId = msg_send![class!(UIScreen), mainScreen];
        let bounds: NSRect = msg_send![screen, bounds];
        let scale: f64 = msg_send![screen, scale];

        let window: ObjcId = msg_send![class!(UIWindow), alloc];
        let window: ObjcId = msg_send![window, initWithFrame: bounds];
        let controller: ObjcId = msg_send![classes.view_controller, alloc];
        let controller: ObjcId = msg_send![controller, init];
        let root: ObjcId = msg_send![classes.view, alloc];
        let root: ObjcId = msg_send![root, initWithFrame: bounds];
        super::window::init_view(root, true);
        let () = msg_send![controller, setView: root];
        a.pulse.set(msg_send![classes.pulse, new]);
        super::window::add_gestures(root);

        // the view controller owns its view in the window: safe-area insets then follow rotation
        let () = msg_send![window, setRootViewController: controller];
        let () = msg_send![window, makeKeyAndVisible];
        a.window.set(window);
        a.controller.set(controller);
        a.root.set(root);

        #[cfg(target_os = "ios")]
        {
            super::text::init(root);
            super::menu::init(root);
        }
        read_appearance();
        publish_screens();
        Shared::global().publish_ready(scale as f32);
        if super::log_enabled() {
            eprintln!("[uikit] launched: {}x{} points at scale {}", bounds.size.width, bounds.size.height, scale);
        }
    }
}

/// Publishes the screen for `makepad_hal::hal_screens` (one screen, the window's work area is the
/// whole of it: the safe area is the window's business).
pub(super) fn publish_screens() {
    let bounds: NSRect = unsafe {
        let screen: ObjcId = msg_send![class!(UIScreen), mainScreen];
        msg_send![screen, bounds]
    };
    let rect = [0.0, 0.0, bounds.size.width, bounds.size.height];
    *lock(&Shared::global().screens) = vec![crate::ScreenInfo { bounds: rect, work_area: rect, is_primary: true }];
}

extern "C" {
    /// libobjc: the hook called with an exception object before it is thrown.
    #[link_name = "objc_setExceptionPreprocessor"]
    fn objc_set_exception_preprocessor(f: extern "C" fn(ObjcId) -> ObjcId) -> Option<extern "C" fn(ObjcId) -> ObjcId>;
}

extern "C" fn log_objc_exception(exception: ObjcId) -> ObjcId {
    unsafe {
        let name: ObjcId = msg_send![exception, name];
        let reason: ObjcId = msg_send![exception, reason];
        let text = |s: ObjcId| if s == nil { String::new() } else { nsstring_to_string(s) };
        eprintln!("makepad-hal: ObjC exception {}: {}", text(name), text(reason));
    }
    exception
}

// ------------------------------------------------------------------ lifecycle

impl App {
    pub fn classes(&self) -> &Classes {
        self.classes.get().expect("shell classes")
    }

    pub fn add_window(&self, win: &'static Win) {
        self.windows.borrow_mut().push(win);
    }

    pub fn live_windows(&self) -> Vec<&'static Win> {
        self.windows.borrow().iter().copied().filter(|w| !w.retired.get()).collect()
    }

    pub(super) fn window_view(&self, sid: u32, id: Id) -> Option<ObjcId> {
        self.find(sid, id).map(|w| w.view)
    }

    pub(super) fn find(&self, sid: u32, id: Id) -> Option<&'static Win> {
        self.windows.borrow().iter().copied().find(|w| !w.retired.get()
            && ((w.sid.get() == sid && w.id.get() == id) || (w.owner_sid == sid && w.owner_id == id)))
    }

    /// The window a view belongs to.
    #[cfg(target_os = "ios")]
    pub fn win_of_view(&self, view: ObjcId) -> Option<&'static Win> {
        self.windows.borrow().iter().copied().find(|w| !w.retired.get() && w.view == view)
    }

    /// The newest normal window (the one on top): the text field, menus and dialogs act in it.
    pub fn top_window(&self) -> Option<&'static Win> {
        self.windows.borrow().iter().rev().copied().find(|w| !w.retired.get() && !w.popup)
    }

    /// The root view is free again when the window that had it closes.
    pub fn root_taken(&self) -> bool {
        let root = self.root.get();
        self.windows.borrow().iter().any(|w| !w.retired.get() && w.view == root)
    }

    pub fn active(&self) -> bool {
        self.active.get()
    }

    /// Every session with a live window hears of the app's lifecycle.
    fn broadcast(&self, kind: u32) {
        let mut sids: Vec<u32> = self.live_windows().iter().map(|w| w.sid.get()).collect();
        sids.sort();
        sids.dedup();
        for sid in sids {
            Shared::global().push(sid, |q| q.lifecycle(kind));
        }
    }

    /// The window a URL or file the OS handed the app goes to: the newest session's.
    pub fn send_app_open(&self, items: &[(u32, String)]) {
        let Some(sid) = self.top_window().map(|w| w.sid.get()) else { return };
        let list: Vec<(u32, &str)> = items.iter().map(|(k, s)| (*k, s.as_str())).collect();
        let payload = crate::buffers::pack_drag_items(&list);
        Shared::global().push(sid, |q| q.app_open(items.len() as u32, &payload));
    }
}

/// `applicationWillEnterForeground:` and the others: `lifecycle::*`. The GPU may only be used
/// while the app is active: windows that asked for a frame meanwhile get it on activation.
pub(super) fn lifecycle(kind: u32) {
    let a = app();
    match kind {
        lifecycle::RESUME => {
            a.active.set(true);
            for win in a.live_windows() {
                win.focus(true);
                if win.want_frame.get() {
                    request_frame(win);
                }
            }
        }
        lifecycle::PAUSE => {
            a.active.set(false);
            for win in a.live_windows() {
                win.focus(false);
            }
        }
        _ => {}
    }
    a.broadcast(kind);
}

// ------------------------------------------------------------------ ops

fn apply(op: Op) {
    match op {
        Op::Host(f) => f(),
        Op::Lend { host, session, alias } => {
            if let Some(win) = app().live_windows().into_iter().find(|w| w.owner_sid == crate::session::HOST && w.owner_id == host) {
                win.lend(session, alias);
            }
        }
        // a UIKit window is on screen from launch
        Op::Reveal => {}
        Op::CloseSession(sid) => {
            for win in app().live_windows().into_iter().filter(|w| w.sid.get() == sid) {
                win.close_silently();
            }
            #[cfg(target_os = "ios")]
            {
                super::text::session_closed(sid);
                super::menu::session_closed(sid);
            }
        }
        Op::Cmd(sid, cmd, taken) => command(sid, cmd, taken),
    }
}

#[allow(unused_variables)]
fn command(sid: u32, cmd: OsCmd, taken: u64) {
    let a = app();
    let window_of = |id: Id| a.find(sid, id);
    match cmd {
        OsCmd::WindowCreate { id, kind, parent, geom } => {
            let parent = if parent.is_none() { None } else { window_of(parent) };
            let win = Win::create(sid, id, kind, parent, geom);
            win.send_geom();
            win.focus(a.active());
            // the app is in the foreground while windows are made (a cold launch has no
            // `willEnterForeground`), and UIKit may have announced the activation before the
            // session had a window to hear it: the session's first window tells it
            if !win.popup && !a.live_windows().iter().any(|w| w.sid.get() == sid && !std::ptr::eq(*w, win)) {
                let active = a.active();
                Shared::global().push(sid, |q| {
                    q.lifecycle(lifecycle::FOREGROUND);
                    if active {
                        q.lifecycle(lifecycle::RESUME);
                    }
                });
            }
            #[cfg(target_os = "ios")]
            super::text::announce_keyboard(win);
        }
        OsCmd::WindowClose { id } => {
            if let Some(win) = window_of(id) {
                win.close();
            }
        }
        OsCmd::WindowGeom { id, geom } => {
            if let Some(win) = window_of(id) {
                win.set_geom(geom);
                win.send_geom();
            }
        }
        OsCmd::WindowState { id, state } => {
            if let Some(win) = window_of(id) {
                match state {
                    window_state::FULLSCREEN => set_fullscreen(true),
                    window_state::RESTORE | window_state::MAXIMIZE => set_fullscreen(false),
                    window_state::HIDE => win.set_hidden(true),
                    window_state::SHOW => win.set_hidden(false),
                    _ => {}
                }
            }
        }
        OsCmd::FrameRequest { id } => {
            if let Some(win) = window_of(id) {
                request_frame(win);
            }
        }
        OsCmd::DragStart { id, .. } => {
            // there are no file drags out of an iOS app: the app's drag ends at once, with no drop
            if let Some(win) = window_of(id) {
                win.drag_ended();
            }
        }
        OsCmd::MobileUi { op, value } => mobile_ui(op, value),
        #[cfg(target_os = "ios")]
        OsCmd::ImeShow { id, area, config } => {
            if let Some(win) = window_of(id) {
                super::text::show(win, area, config);
            }
        }
        #[cfg(target_os = "ios")]
        OsCmd::ImeHide { id } => {
            if window_of(id).is_some() {
                super::text::hide();
            }
        }
        #[cfg(target_os = "ios")]
        OsCmd::ImeSync { id, selection, composition, text } => {
            if window_of(id).is_some() {
                super::text::sync(&text, selection, composition, taken);
            }
        }
        #[cfg(target_os = "ios")]
        OsCmd::ClipSet { text } => super::text::clipboard_set(&text),
        #[cfg(target_os = "ios")]
        OsCmd::ClipRead { req, .. } => {
            let content = super::text::clipboard_read();
            Shared::global().push(sid, |q| {
                q.payload_size(content.len() as u32);
                q.clipboard(req, true, &content);
            });
        }
        #[cfg(target_os = "ios")]
        OsCmd::ClipboardActions { id, show, area, selection } => {
            if let Some(win) = window_of(id) {
                super::menu::clipboard_actions(win, show, area, selection);
            }
        }
        #[cfg(target_os = "ios")]
        OsCmd::SelectionHandles { id, op, start, end } => {
            if let Some(win) = window_of(id) {
                super::menu::selection_handles(win, op, start, end);
            }
        }
        #[cfg(target_os = "ios")]
        OsCmd::FileDialog { req, kind, title, location, filename, filters } => super::dialog::open(sid, req, kind, &title, &location, &filename, &filters),
        #[cfg(target_os = "tvos")]
        OsCmd::ClipRead { req, .. } => {
            // tvOS has no pasteboard
            Shared::global().push(sid, |q| q.clipboard(req, false, &[]));
        }
        #[cfg(target_os = "tvos")]
        OsCmd::FileDialog { req, .. } => {
            // tvOS has no document picker: the dialog ends at once, with nothing chosen
            Shared::global().push(sid, |q| q.dialog_result(req, 0, &[]));
        }
        #[cfg(target_os = "tvos")]
        OsCmd::ImeShow { .. } | OsCmd::ImeHide { .. } | OsCmd::ImeSync { .. } | OsCmd::ClipSet { .. } | OsCmd::ClipboardActions { .. } | OsCmd::SelectionHandles { .. } => {}
        // desktop window decoration, pointers, menus and the Dock have no UIKit counterpart
        OsCmd::WindowTitle { .. }
        | OsCmd::WindowVisuals { .. }
        | OsCmd::WindowTopmost { .. }
        | OsCmd::WindowButtons { .. }
        | OsCmd::WindowButtonsCenter { .. }
        | OsCmd::ChromelessWhenMax { .. }
        | OsCmd::Cursor { .. }
        | OsCmd::PointerLock { .. }
        | OsCmd::ShowInDock { .. }
        | OsCmd::ActivateApp
        | OsCmd::BackUnhandled
        | OsCmd::AppIcon { .. }
        | OsCmd::MenuSet { .. }
        | OsCmd::SelectionSet { .. }
        | OsCmd::ShareText { .. }
        | OsCmd::DragReply { .. }
        | OsCmd::WindowDpiOverride { .. } => {}
    }
}

// ------------------------------------------------------------------ the system UI

/// The view controller's answers UIKit asks for (status bar, home indicator, orientations,
/// deferred edge gestures) live in its ivars; UIKit is told to ask again.
pub(super) fn mobile_ui(op: u32, value: u32) {
    let vc = app().controller.get();
    if vc == nil {
        return;
    }
    unsafe {
        match op {
            mobile_ui::STATUS_BAR_DARK_ICONS => {
                // UIStatusBarStyleDarkContent = 3 (iOS 13+), UIStatusBarStyleLightContent = 1
                (*vc).set_ivar::<i64>("status_bar_style", if value != 0 { 3 } else { 1 });
                let () = msg_send![vc, setNeedsStatusBarAppearanceUpdate];
            }
            #[cfg(target_os = "ios")]
            mobile_ui::ORIENTATION => set_orientation(vc, value),
            mobile_ui::KEEP_SCREEN_ON => {
                let app: ObjcId = msg_send![class!(UIApplication), sharedApplication];
                let () = msg_send![app, setIdleTimerDisabled: if value != 0 { YES } else { NO }];
            }
            #[cfg(target_os = "ios")]
            mobile_ui::DEFER_SYSTEM_GESTURES => {
                // `mobile_ui::edge` bits are UIRectEdge's
                (*vc).set_ivar::<u64>("deferred_edges", value as u64);
                let () = msg_send![vc, setNeedsUpdateOfScreenEdgesDeferringSystemGestures];
            }
            _ => {}
        }
    }
}

/// Hides (`fullscreen`) or shows the status bar and the home indicator.
fn set_fullscreen(fullscreen: bool) {
    let vc = app().controller.get();
    if vc == nil {
        return;
    }
    unsafe {
        let val = if fullscreen { YES } else { NO };
        (*vc).set_ivar::<BOOL>("status_bar_hidden", val);
        let () = msg_send![vc, setNeedsStatusBarAppearanceUpdate];
        #[cfg(target_os = "ios")]
        {
            (*vc).set_ivar::<BOOL>("home_indicator_hidden", val);
            let () = msg_send![vc, setNeedsUpdateOfHomeIndicatorAutoHidden];
        }
    }
}

/// Locks the interface orientation, or with AUTO releases the lock: the view controller answers
/// `supportedInterfaceOrientations` with the mask, and UIKit is told to ask again; iOS 16+ also
/// asks the window scene to rotate now.
#[cfg(target_os = "ios")]
unsafe fn set_orientation(vc: ObjcId, orientation: u32) {
    // UIInterfaceOrientationMask: portrait 2, landscape (left | right) 24; 0 = no lock
    let mask: u64 = match orientation {
        mobile_ui::orientation::PORTRAIT => 2,
        mobile_ui::orientation::LANDSCAPE => 24,
        _ => 0,
    };
    (*vc).set_ivar::<u64>("orientations", mask);
    let can_update: BOOL = msg_send![vc, respondsToSelector: sel!(setNeedsUpdateOfSupportedInterfaceOrientations)];
    if can_update == NO {
        let () = msg_send![class!(UIViewController), attemptRotationToDeviceOrientation];
        return;
    }
    let () = msg_send![vc, setNeedsUpdateOfSupportedInterfaceOrientations];
    if mask == 0 {
        return;
    }
    let window = app().window.get();
    let scene: ObjcId = if window == nil { nil } else { msg_send![window, windowScene] };
    let prefs_class = makepad_objc_sys::runtime::objc_getClass(b"UIWindowSceneGeometryPreferencesIOS\0".as_ptr() as *const _);
    if scene == nil || prefs_class.is_null() {
        return;
    }
    let prefs: ObjcId = msg_send![prefs_class as ObjcId, alloc];
    let prefs: ObjcId = msg_send![prefs, initWithInterfaceOrientations: mask];
    if prefs != nil {
        let () = msg_send![scene, requestGeometryUpdateWithPreferences: prefs errorHandler: nil];
        let () = msg_send![prefs, release];
    }
}

// ------------------------------------------------------------------ frame pacing

/// The app has work for the window: a frame is announced at the next vsync. The display link is
/// unpaused for it and paused again once the frame was announced, so an idle app costs nothing.
/// While the app is not active (in the background the GPU is off limits) the request waits for
/// the activation.
pub fn request_frame(win: &'static Win) {
    win.want_frame.set(true);
    if !app().active() {
        return;
    }
    unsafe {
        let mut link = win.link.get();
        if link == nil {
            link = msg_send![class!(CADisplayLink), displayLinkWithTarget: app().pulse.get() selector: sel!(receivedDisplayLink:)];
            if link == nil {
                return;
            }
            // pin the rate to the panel's maximum (ProMotion): an unconstrained link may be
            // throttled for a "static" view
            let responds: bool = msg_send![link, respondsToSelector: sel!(setPreferredFrameRateRange:)];
            if responds {
                let screen: ObjcId = msg_send![class!(UIScreen), mainScreen];
                let maximum: isize = msg_send![screen, maximumFramesPerSecond];
                let fps = maximum.max(1) as f32;
                let () = msg_send![link, setPreferredFrameRateRange: CAFrameRateRange { minimum: fps, maximum: fps, preferred: fps }];
            }
            let () = msg_send![link, retain];
            let runloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
            let () = msg_send![link, addToRunLoop: runloop forMode: NSRunLoopCommonModes];
            win.link.set(link);
        }
        let () = msg_send![link, setPaused: NO];
    }
}

pub fn display_link_fired(link: ObjcId) {
    let win = app().windows.borrow().iter().copied().find(|w| w.link.get() == link);
    let Some(win) = win else { return };
    unsafe {
        let () = msg_send![link, setPaused: YES];
    }
    if !win.want_frame.replace(false) || win.retired.get() {
        return;
    }
    let (target, media): (f64, f64) = unsafe { (msg_send![link, targetTimestamp], CACurrentMediaTime()) };
    let next_ns = crate::clock_ns() + ((target - media).clamp(0.0, 0.1) * 1e9) as u64;
    Shared::global().push(win.sid.get(), |q| q.frame_ready(win.id.get(), next_ns));
}
