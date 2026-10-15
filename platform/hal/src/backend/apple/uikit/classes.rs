//! The Objective-C classes of the shell: the application delegate, the root view controller,
//! the window view (a `UIView` backed by a `CAMetalLayer`: touches, hardware keys, the edit
//! menu's actions) and the target of the display links and gesture recognizers. Moved from
//! os/apple/ios/ios_delegates.rs and os/apple/tvos/tvos_delegates.rs; a callback no longer calls
//! into Cx, it writes an event for the session.

use super::app::{self, app};
use super::window::Win;
use crate::types::*;
use makepad_apple_sys::*;
use std::os::raw::c_void;

/// The class pointers, made once on the main thread.
pub struct Classes {
    pub app_delegate: *const Class,
    pub view_controller: *const Class,
    pub view: *const Class,
    pub pulse: *const Class,
}

impl Classes {
    pub fn new() -> Classes {
        Classes {
            app_delegate: define_app_delegate(),
            view_controller: define_view_controller(),
            view: define_view(),
            pulse: define_pulse(),
        }
    }
}

/// A panic must not unwind across an Objective-C boundary: it aborts the app with a message that
/// hides the real one. Catch at the edge, say what happened, keep the loop alive.
/// (Main-thread state is only ever read again after a contained panic, never assumed whole.)
pub(super) fn shielded(name: &str, call: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).is_err() {
        eprintln!("makepad-hal: PANIC contained at the {name} callback boundary; the shell goes on");
    }
}

/// The window behind a view (`None` for the root view before a session took it).
pub(super) fn win_of(this: &Object) -> Option<&'static Win> {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar("hal_win");
        (!ptr.is_null()).then(|| &*(ptr as *const Win)).filter(|w| !w.retired.get())
    }
}

// ------------------------------------------------------------------ application

fn define_app_delegate() -> *const Class {
    extern "C" fn did_finish_launching(_: &Object, _: Sel, _: ObjcId, _: ObjcId) -> BOOL {
        shielded("launch", app::did_finish_launching);
        YES
    }
    // A URL scheme, a universal link or a file handed to the app; UIKit calls this after the
    // launch for a cold launch too.
    extern "C" fn open_url(_: &Object, _: Sel, _: ObjcId, url: ObjcId, _: ObjcId) -> BOOL {
        let item = unsafe { url_item(url) };
        match item {
            Some(item) => {
                shielded("open-url", || app().send_app_open(&[item]));
                YES
            }
            None => NO,
        }
    }
    extern "C" fn will_enter_foreground(_: &Object, _: Sel, _: ObjcId) {
        shielded("foreground", || app::lifecycle(lifecycle::FOREGROUND));
    }
    extern "C" fn did_enter_background(_: &Object, _: Sel, _: ObjcId) {
        shielded("background", || app::lifecycle(lifecycle::BACKGROUND));
    }
    extern "C" fn will_resign_active(_: &Object, _: Sel, _: ObjcId) {
        shielded("resign-active", || app::lifecycle(lifecycle::PAUSE));
    }
    extern "C" fn did_become_active(_: &Object, _: Sel, _: ObjcId) {
        shielded("become-active", || app::lifecycle(lifecycle::RESUME));
    }
    extern "C" fn will_terminate(_: &Object, _: Sel, _: ObjcId) {
        shielded("terminate", || app::lifecycle(lifecycle::DESTROYED));
    }
    let mut decl = ClassDecl::new("HalAppDelegate", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(application:didFinishLaunchingWithOptions:), did_finish_launching as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> BOOL);
        decl.add_method(sel!(application:openURL:options:), open_url as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId) -> BOOL);
        decl.add_method(sel!(applicationWillEnterForeground:), will_enter_foreground as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(applicationDidEnterBackground:), did_enter_background as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(applicationWillResignActive:), will_resign_active as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(applicationDidBecomeActive:), did_become_active as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(applicationWillTerminate:), will_terminate as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.register()
}

/// A file URL is a file item (its path), anything else a string item (the URL).
unsafe fn url_item(url: ObjcId) -> Option<(u32, String)> {
    if url == nil {
        return None;
    }
    let is_file: BOOL = msg_send![url, isFileURL];
    if is_file != NO {
        let path: ObjcId = msg_send![url, path];
        (path != nil).then(|| (input::drag_item::FILE, nsstring_to_string(path)))
    } else {
        let text: ObjcId = msg_send![url, absoluteString];
        (text != nil).then(|| (input::drag_item::STRING, nsstring_to_string(text)))
    }
}

// ------------------------------------------------------------------ the root view controller

fn define_view_controller() -> *const Class {
    extern "C" fn prefers_status_bar_hidden(this: &Object, _: Sel) -> BOOL {
        unsafe { *this.get_ivar("status_bar_hidden") }
    }
    extern "C" fn preferred_status_bar_style(this: &Object, _: Sel) -> i64 {
        unsafe { *this.get_ivar("status_bar_style") }
    }
    #[cfg(target_os = "ios")]
    extern "C" fn prefers_home_indicator_auto_hidden(this: &Object, _: Sel) -> BOOL {
        unsafe { *this.get_ivar("home_indicator_hidden") }
    }
    #[cfg(target_os = "ios")]
    extern "C" fn deferred_edges(this: &Object, _: Sel) -> u64 {
        unsafe { *this.get_ivar("deferred_edges") }
    }
    #[cfg(target_os = "ios")]
    extern "C" fn supported_orientations(this: &Object, _: Sel) -> u64 {
        let mask: u64 = unsafe { *this.get_ivar("orientations") };
        if mask != 0 {
            return mask;
        }
        // UIKit's default: every orientation on iPad (idiom 1), all but upside-down elsewhere
        let idiom: i64 = unsafe {
            let device: ObjcId = msg_send![class!(UIDevice), currentDevice];
            msg_send![device, userInterfaceIdiom]
        };
        if idiom == 1 {
            30
        } else {
            26
        }
    }
    // The safe area changed (rotation, the status bar): every window reports its geometry.
    extern "C" fn safe_area_changed(this: &Object, _: Sel) {
        unsafe {
            let () = msg_send![super(this, class!(UIViewController)), viewSafeAreaInsetsDidChange];
        }
        shielded("safe-area", || {
            for win in app().live_windows() {
                win.send_geom();
            }
        });
    }
    extern "C" fn trait_collection_changed(this: &Object, _: Sel, previous: ObjcId) {
        unsafe {
            let () = msg_send![super(this, class!(UIViewController)), traitCollectionDidChange: previous];
        }
        shielded("appearance", app::read_appearance);
    }
    let mut decl = ClassDecl::new("HalViewController", class!(UIViewController)).unwrap();
    decl.add_ivar::<BOOL>("status_bar_hidden");
    // UIStatusBarStyle: 0 default (system-managed light/dark), 1 light content, 3 dark content
    decl.add_ivar::<i64>("status_bar_style");
    #[cfg(target_os = "ios")]
    {
        decl.add_ivar::<BOOL>("home_indicator_hidden");
        // UIRectEdge bits whose system gestures wait for the app; 0 = none
        decl.add_ivar::<u64>("deferred_edges");
        // UIInterfaceOrientationMask the app locked to; 0 = UIKit's default
        decl.add_ivar::<u64>("orientations");
    }
    unsafe {
        decl.add_method(sel!(prefersStatusBarHidden), prefers_status_bar_hidden as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(preferredStatusBarStyle), preferred_status_bar_style as extern "C" fn(&Object, Sel) -> i64);
        #[cfg(target_os = "ios")]
        {
            decl.add_method(sel!(prefersHomeIndicatorAutoHidden), prefers_home_indicator_auto_hidden as extern "C" fn(&Object, Sel) -> BOOL);
            decl.add_method(sel!(preferredScreenEdgesDeferringSystemGestures), deferred_edges as extern "C" fn(&Object, Sel) -> u64);
            decl.add_method(sel!(supportedInterfaceOrientations), supported_orientations as extern "C" fn(&Object, Sel) -> u64);
        }
        decl.add_method(sel!(viewSafeAreaInsetsDidChange), safe_area_changed as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(traitCollectionDidChange:), trait_collection_changed as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.register()
}

// ------------------------------------------------------------------ the window view

fn define_view() -> *const Class {
    extern "C" fn layer_class(_: &Class, _: Sel) -> *const Class {
        class!(CAMetalLayer)
    }
    extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }
    // the size or the safe area changed: the window reports its geometry
    extern "C" fn layout_subviews(this: &Object, _: Sel) {
        unsafe {
            let () = msg_send![super(this, class!(UIView)), layoutSubviews];
        }
        if let Some(win) = win_of(this) {
            shielded("layout", || win.send_geom());
        }
    }
    #[cfg(target_os = "ios")]
    extern "C" fn touches_began(this: &Object, _: Sel, touches: ObjcId, _: ObjcId) {
        if let Some(win) = win_of(this) {
            shielded("touch", || win.touches(touches, input::touch::START));
        }
    }
    #[cfg(target_os = "ios")]
    extern "C" fn touches_moved(this: &Object, _: Sel, touches: ObjcId, _: ObjcId) {
        if let Some(win) = win_of(this) {
            shielded("touch", || win.touches(touches, input::touch::MOVE));
        }
    }
    #[cfg(target_os = "ios")]
    extern "C" fn touches_ended(this: &Object, _: Sel, touches: ObjcId, _: ObjcId) {
        if let Some(win) = win_of(this) {
            shielded("touch", || win.touches(touches, input::touch::STOP));
        }
    }
    #[cfg(target_os = "ios")]
    extern "C" fn touches_cancelled(this: &Object, _: Sel, touches: ObjcId, _: ObjcId) {
        if let Some(win) = win_of(this) {
            shielded("touch", || win.touches_cancelled(touches));
        }
    }
    let mut decl = ClassDecl::new("HalView", class!(UIView)).unwrap();
    decl.add_ivar::<*mut c_void>("hal_win");
    unsafe {
        decl.add_class_method(sel!(layerClass), layer_class as extern "C" fn(&Class, Sel) -> *const Class);
        decl.add_method(sel!(layoutSubviews), layout_subviews as extern "C" fn(&Object, Sel));
        // tvOS: the remote's touch surface is no pointer on the screen (game input reads it)
        #[cfg(target_os = "ios")]
        {
            decl.add_method(sel!(touchesBegan:withEvent:), touches_began as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
            decl.add_method(sel!(touchesMoved:withEvent:), touches_moved as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
            decl.add_method(sel!(touchesEnded:withEvent:), touches_ended as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
            decl.add_method(sel!(touchesCancelled:withEvent:), touches_cancelled as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
        }
        // the edit menu and hardware keys reach the view as a responder
        decl.add_method(sel!(canBecomeFirstResponder), yes as extern "C" fn(&Object, Sel) -> BOOL);
    }
    #[cfg(target_os = "ios")]
    {
        super::keys::add_press_methods(&mut decl);
        super::menu::add_view_methods(&mut decl);
    }
    decl.register()
}

// ------------------------------------------------------------------ pulses: links and gestures

fn define_pulse() -> *const Class {
    extern "C" fn received_display_link(_: &Object, _: Sel, link: ObjcId) {
        shielded("display-link", || app::display_link_fired(link));
    }
    // A long press begins once the finger rested long enough (UIGestureRecognizerStateBegan),
    // without waiting for the lift; the touches keep reaching the view (cancelsTouchesInView
    // is off), so the later states need nothing here.
    #[cfg(target_os = "ios")]
    extern "C" fn long_press(_: &Object, _: Sel, recognizer: ObjcId) {
        shielded("long-press", std::panic::AssertUnwindSafe(|| unsafe {
            let state: i64 = msg_send![recognizer, state];
            if state != 1 {
                return;
            }
            let view: ObjcId = msg_send![recognizer, view];
            let Some(win) = app().win_of_view(view) else { return };
            let at: NSPoint = msg_send![recognizer, locationInView: view];
            win.long_press(at.x, at.y);
        }));
    }
    let mut decl = ClassDecl::new("HalPulse", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(receivedDisplayLink:), received_display_link as extern "C" fn(&Object, Sel, ObjcId));
        #[cfg(target_os = "ios")]
        decl.add_method(sel!(longPress:), long_press as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.register()
}
