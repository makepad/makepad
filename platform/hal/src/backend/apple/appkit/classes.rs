//! The Objective-C classes of the shell: the window and its delegate, the content view (mouse,
//! text input, drag and drop), the menu and application delegates and the target of the display
//! links and fallback timers. Moved from os/apple/macos/macos_delegates.rs; a callback no longer
//! calls into Cx, it writes an event for the session (`Win` does) and AppKit's questions that
//! used to be answered by the app (may the window close, what does a drag do) are answered from
//! what the app said last or by the shell itself.

use super::app::{self, app};
use super::keys;
use super::window::Win;
use crate::types::*;
use makepad_apple_sys::*;
use std::os::raw::c_void;

/// The class pointers, made once on the main thread.
pub struct Classes {
    pub window: *const Class,
    pub panel: *const Class,
    pub titlebar_container: *const Class,
    pub window_delegate: *const Class,
    pub view: *const Class,
    pub menu_target: *const Class,
    pub menu_delegate: *const Class,
    pub app_delegate: *const Class,
    pub pulse: *const Class,
}

impl Classes {
    pub fn new() -> Classes {
        Classes {
            window: define_window_class(),
            panel: define_panel_class(),
            titlebar_container: define_titlebar_container_class(),
            window_delegate: define_window_delegate(),
            view: define_view_class(),
            menu_target: define_menu_target_class(),
            menu_delegate: define_menu_delegate(),
            app_delegate: define_app_delegate(),
            pulse: define_pulse_class(),
        }
    }
}

/// A panic must not unwind across an Objective-C boundary: it aborts the whole app with a
/// message that hides the real one. Catch at the edge, say what happened, keep the loop alive.
fn shielded(name: &str, call: impl FnOnce() + std::panic::UnwindSafe) {
    if std::panic::catch_unwind(call).is_err() {
        eprintln!("makepad-hal: PANIC contained at the {name} callback boundary; the shell goes on");
    }
}

/// The window behind a delegate or a view.
fn win_of(this: &Object) -> &'static Win {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar("macos_window_ptr");
        &*(ptr as *const Win)
    }
}

// ------------------------------------------------------------------ pulses: links and timers

/// The target of the display links and the fallback timers.
fn define_pulse_class() -> *const Class {
    extern "C" fn received_display_link(_this: &Object, _: Sel, link: ObjcId) {
        shielded("display-link", std::panic::AssertUnwindSafe(|| app::display_link_fired(link)));
    }
    extern "C" fn received_fallback(_this: &Object, _: Sel, timer: ObjcId) {
        shielded("frame-fallback", std::panic::AssertUnwindSafe(|| app::fallback_fired(timer)));
    }
    extern "C" fn appearance_changed(_this: &Object, _: Sel, _note: ObjcId) {
        shielded("appearance", std::panic::AssertUnwindSafe(app::read_appearance));
    }
    extern "C" fn screens_changed(_this: &Object, _: Sel, _note: ObjcId) {
        shielded("screens", std::panic::AssertUnwindSafe(app::publish_screens));
    }
    let mut decl = ClassDecl::new("HalPulse", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(screensChanged:), screens_changed as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(appearanceChanged:), appearance_changed as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(receivedDisplayLink:), received_display_link as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(receivedFallback:), received_fallback as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.register()
}

// ------------------------------------------------------------------ application, menu

fn define_app_delegate() -> *const Class {
    extern "C" fn application_should_terminate(_: &Object, _: Sel, _: ObjcId) -> isize {
        const CANCEL: isize = 0;
        const NOW: isize = 1;
        app::note_exit_requested();
        // the sessions decide: each is asked to quit, the app answers with a `Quit` command
        if app().ask_sessions_to_quit() {
            CANCEL
        } else {
            app::windows_gone_before_exit();
            NOW
        }
    }

    // Finder, `open`, the Dock and URL schemes: files arrive as file URLs
    extern "C" fn application_open_urls(_: &Object, _: Sel, _: ObjcId, urls: ObjcId) {
        let items: Vec<(u32, String)> = unsafe {
            let count: usize = msg_send![urls, count];
            (0..count).filter_map(|i| open_item(msg_send![urls, objectAtIndex: i])).collect()
        };
        if !items.is_empty() {
            app().send_app_open(&items);
        }
    }

    /// A file URL as its path, anything else as the URL string.
    unsafe fn open_item(url: ObjcId) -> Option<(u32, String)> {
        if url == nil {
            return None;
        }
        let is_file: BOOL = msg_send![url, isFileURL];
        if is_file != NO {
            let path: ObjcId = msg_send![url, path];
            if path != nil {
                return Some((input::drag_item::FILE, nsstring_to_string(path)));
            }
        }
        let string: ObjcId = msg_send![url, absoluteString];
        (string != nil).then(|| (input::drag_item::STRING, nsstring_to_string(string)))
    }

    let mut decl = ClassDecl::new("HalAppDelegate", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(applicationShouldTerminate:), application_should_terminate as extern "C" fn(&Object, Sel, ObjcId) -> isize);
        decl.add_method(sel!(application:openURLs:), application_open_urls as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
    }
    decl.register()
}

fn define_menu_target_class() -> *const Class {
    extern "C" fn menu_action(this: &Object, _sel: Sel, _item: ObjcId) {
        shielded("menu", std::panic::AssertUnwindSafe(|| {
            let command: usize = unsafe { *this.get_ivar("command_u64") };
            app().menu_command(command as u64);
        }));
    }
    let mut decl = ClassDecl::new("HalMenuTarget", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(menuAction:), menu_action as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.add_ivar::<usize>("command_u64");
    decl.register()
}

fn define_menu_delegate() -> *const Class {
    extern "C" fn menu_will_open(_this: &Object, _sel: Sel, _item: ObjcId) {}
    let mut decl = ClassDecl::new("HalMenuDelegate", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(menuWillOpen:), menu_will_open as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.add_protocol(&Protocol::get("NSMenuDelegate").unwrap());
    decl.register()
}

// ------------------------------------------------------------------ window

/// The transparent titlebar hosts AppKit's buttons; every other hit belongs to the app's
/// caption. This also lets a zoomed titlebar container cover controls without stealing them.
fn define_titlebar_container_class() -> *const Class {
    let Some(container_class) = Class::get("NSTitlebarContainerView") else { return std::ptr::null() };
    extern "C" fn hit_test(this: &Object, _: Sel, point: NSPoint) -> ObjcId {
        unsafe {
            let superclass: ObjcId = msg_send![this, superclass];
            let hit: ObjcId = msg_send![super(this, &*(superclass as *const Class)), hitTest: point];
            let mut view = hit;
            while view != nil {
                let is_button: bool = msg_send![view, isKindOfClass: class!(NSButton)];
                if is_button {
                    return hit;
                }
                view = msg_send![view, superview];
            }
            nil
        }
    }
    let mut decl = ClassDecl::new("HalTitlebarContainerView", container_class).unwrap();
    unsafe {
        decl.add_method(sel!(hitTest:), hit_test as extern "C" fn(&Object, Sel, NSPoint) -> ObjcId);
    }
    decl.register()
}

fn define_window_class() -> *const Class {
    extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }
    let mut decl = ClassDecl::new("HalWindow", class!(NSWindow)).unwrap();
    unsafe {
        decl.add_method(sel!(canBecomeMainWindow), yes as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(canBecomeKeyWindow), yes as extern "C" fn(&Object, Sel) -> BOOL);
    }
    decl.register()
}

fn define_panel_class() -> *const Class {
    extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }
    let mut decl = ClassDecl::new("HalPanel", class!(NSPanel)).unwrap();
    unsafe {
        decl.add_method(sel!(canBecomeMainWindow), yes as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(canBecomeKeyWindow), yes as extern "C" fn(&Object, Sel) -> BOOL);
    }
    decl.register()
}

fn define_window_delegate() -> *const Class {
    extern "C" fn window_should_close(this: &Object, _: Sel, _: ObjcId) -> BOOL {
        let mut close = NO;
        shielded("windowShouldClose", std::panic::AssertUnwindSafe(|| {
            if win_of(this).close_requested() {
                close = YES;
            }
        }));
        close
    }
    extern "C" fn window_will_close(this: &Object, _: Sel, _: ObjcId) {
        shielded("windowWillClose", std::panic::AssertUnwindSafe(|| win_of(this).will_close()));
    }
    extern "C" fn window_will_start_live_resize(this: &Object, _: Sel, _: ObjcId) {
        shielded("liveResize", std::panic::AssertUnwindSafe(|| app::resize_loop(win_of(this), true)));
    }
    extern "C" fn window_did_end_live_resize(this: &Object, _: Sel, _: ObjcId) {
        shielded("liveResize", std::panic::AssertUnwindSafe(|| app::resize_loop(win_of(this), false)));
    }
    // geometry: a move, a screen or scale change, the end of a fullscreen transition
    extern "C" fn geom_changed(this: &Object, _: Sel, _: ObjcId) {
        shielded("geom", std::panic::AssertUnwindSafe(|| win_of(this).send_geom()));
    }
    extern "C" fn window_did_change_backing_properties(this: &Object, _: Sel, _: ObjcId) {
        shielded("backing", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            unsafe {
                let scale = win.dpi();
                let () = msg_send![win.layer, setContentsScale: scale];
            }
            win.send_geom();
        }));
    }
    extern "C" fn window_did_become_key(this: &Object, _: Sel, _: ObjcId) {
        shielded("becomeKey", std::panic::AssertUnwindSafe(|| {
            // macOS re-associates the cursor whenever the app deactivates: a logically-locked
            // pointer re-applies its physical lock on every focus gain
            app().focus_gained();
            let win = win_of(this);
            win.send_geom();
            win.got_focus();
        }));
    }
    extern "C" fn window_did_resign_key(this: &Object, _: Sel, _: ObjcId) {
        shielded("resignKey", std::panic::AssertUnwindSafe(|| {
            // suspend (never unset) the lock while unfocused
            let win = win_of(this);
            app().focus_lost(win);
            win.send_geom();
            win.lost_focus();
        }));
    }
    extern "C" fn window_miniaturize_changed(this: &Object, _: Sel, _: ObjcId) {
        shielded("miniaturize", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            win.send_geom();
            app::display_links_changed();
        }));
    }
    // fullscreen: `is_fullscreen` flips in the `will` callbacks so the widget layer can show or
    // hide its caption bar at the start of the animation
    extern "C" fn window_will_enter_fullscreen(this: &Object, _: Sel, _: ObjcId) {
        shielded("fullscreen", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            win.fullscreen.set(true);
            win.send_geom();
        }));
    }
    extern "C" fn window_will_exit_fullscreen(this: &Object, _: Sel, _: ObjcId) {
        shielded("fullscreen", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            win.fullscreen.set(false);
            win.exiting_fullscreen.set(true);
            // AppKit keeps its own button layout during the exit animation.
            if win.window_buttons_zoom() > 1.0 {
                win.set_titlebar_visible(false);
            }
            win.send_geom();
        }));
    }
    extern "C" fn window_did_exit_fullscreen(this: &Object, _: Sel, _: ObjcId) {
        shielded("fullscreen", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            win.exiting_fullscreen.set(false);
            win.position_window_buttons();
            win.set_titlebar_visible(true);
            win.send_geom();
        }));
    }
    extern "C" fn window_did_fail_to_enter_fullscreen(this: &Object, _: Sel, _: ObjcId) {
        shielded("fullscreen", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            win.fullscreen.set(false);
            win.send_geom();
        }));
    }
    extern "C" fn window_did_fail_to_exit_fullscreen(this: &Object, _: Sel, _: ObjcId) {
        shielded("fullscreen", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            win.fullscreen.set(true);
            win.exiting_fullscreen.set(false);
            win.set_titlebar_visible(true);
            win.send_geom();
        }));
    }

    let mut decl = ClassDecl::new("HalWindowDelegate", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(windowShouldClose:), window_should_close as extern "C" fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(windowWillClose:), window_will_close as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowWillStartLiveResize:), window_will_start_live_resize as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidEndLiveResize:), window_did_end_live_resize as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidResize:), geom_changed as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidMove:), geom_changed as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidChangeScreen:), geom_changed as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidChangeBackingProperties:), window_did_change_backing_properties as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidBecomeKey:), window_did_become_key as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidResignKey:), window_did_resign_key as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidMiniaturize:), window_miniaturize_changed as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidDeminiaturize:), window_miniaturize_changed as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidEnterFullScreen:), geom_changed as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowWillEnterFullScreen:), window_will_enter_fullscreen as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowWillExitFullScreen:), window_will_exit_fullscreen as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidExitFullScreen:), window_did_exit_fullscreen as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidFailToEnterFullScreen:), window_did_fail_to_enter_fullscreen as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidFailToExitFullScreen:), window_did_fail_to_exit_fullscreen as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.add_ivar::<*mut c_void>("macos_window_ptr");
    decl.register()
}

// ------------------------------------------------------------------ view

/// Clears AppKit's marked text without telling the app: callers have already committed the
/// composition or moved focus. Our state goes first so a reentrant `unmarkText` is a no-op.
pub fn clear_marked_text(this: &Object) {
    unsafe {
        let marked_text: ObjcId = *this.get_ivar("markedText");
        let mutable_string = marked_text.mutable_string();
        let empty = str_to_nsstring("");
        let _: () = msg_send![mutable_string, setString: empty];
        let input_context: ObjcId = msg_send![this, inputContext];
        let _: () = msg_send![input_context, discardMarkedText];
    }
}

fn marked_length(this: &Object) -> u64 {
    unsafe {
        let marked_text: ObjcId = *this.get_ivar("markedText");
        marked_text.length()
    }
}

fn define_view_class() -> *const Class {
    extern "C" fn dealloc(this: &Object, _sel: Sel) {
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            let _: () = msg_send![marked_text, release];
        }
    }

    extern "C" fn init_with_ptr(this: &Object, _sel: Sel, cx: *mut c_void) -> ObjcId {
        unsafe {
            let this: ObjcId = msg_send![this, init];
            if this != nil {
                (*this).set_ivar("macos_window_ptr", cx);
                let marked_text = <ObjcId as NSMutableAttributedString>::init(NSMutableAttributedString::alloc(nil));
                (*this).set_ivar("markedText", marked_text);
            }
            let types = [NSPasteboardTypeFileURL];
            let types_nsarray: ObjcId = msg_send![class!(NSArray), arrayWithObjects: types.as_ptr() count: types.len()];
            let _: () = msg_send![this, registerForDraggedTypes: types_nsarray];
            this
        }
    }

    // ---- pointer

    /// The event's pointer in view points, y down from the top.
    fn event_pos(this: &Object, event: ObjcId) -> [f64; 2] {
        let window_point: NSPoint = unsafe { msg_send![event, locationInWindow] };
        win_of(this).view_point(window_point)
    }

    fn button_down(this: &Object, event: ObjcId, button: u32) {
        let win = win_of(this);
        // a locked pointer reports its pinned position, not where the frozen cursor is
        if !app().pointer_locked() {
            win.set_pointer(event_pos(this, event));
        }
        win.mouse_down(event, button, keys::event_mods(event));
    }

    fn button_up(this: &Object, event: ObjcId, button: u32) {
        let win = win_of(this);
        if !app().pointer_locked() {
            win.set_pointer(event_pos(this, event));
        }
        win.mouse_up(button, keys::event_mods(event));
    }

    extern "C" fn mouse_down(this: &Object, _sel: Sel, event: ObjcId) {
        shielded("mouseDown", std::panic::AssertUnwindSafe(|| button_down(this, event, 1)));
    }
    extern "C" fn mouse_up(this: &Object, _sel: Sel, event: ObjcId) {
        shielded("mouseUp", std::panic::AssertUnwindSafe(|| button_up(this, event, 1)));
    }
    extern "C" fn right_mouse_down(this: &Object, _sel: Sel, event: ObjcId) {
        shielded("rightMouseDown", std::panic::AssertUnwindSafe(|| button_down(this, event, 2)));
    }
    extern "C" fn right_mouse_up(this: &Object, _sel: Sel, event: ObjcId) {
        shielded("rightMouseUp", std::panic::AssertUnwindSafe(|| button_up(this, event, 2)));
    }
    extern "C" fn other_mouse_down(this: &Object, _sel: Sel, event: ObjcId) {
        shielded("otherMouseDown", std::panic::AssertUnwindSafe(|| button_down(this, event, keys::button_bit(keys::event_button_number(event)))));
    }
    extern "C" fn other_mouse_up(this: &Object, _sel: Sel, event: ObjcId) {
        shielded("otherMouseUp", std::panic::AssertUnwindSafe(|| button_up(this, event, keys::button_bit(keys::event_button_number(event)))));
    }

    fn mouse_motion(this: &Object, event: ObjcId) {
        let pos = event_pos(this, event);
        win_of(this).mouse_moved(event, pos, keys::event_mods(event));
    }
    extern "C" fn mouse_moved(this: &Object, _sel: Sel, event: ObjcId) {
        shielded("mouseMoved", std::panic::AssertUnwindSafe(|| {
            // no button is held during `mouseMoved:` (a held button moves as `mouseDragged:`):
            // a button the window still counts down lost its up
            win_of(this).release_lost_buttons(keys::event_mods(event));
            mouse_motion(this, event);
        }));
    }
    extern "C" fn mouse_dragged(this: &Object, _sel: Sel, event: ObjcId) {
        shielded("mouseDragged", std::panic::AssertUnwindSafe(|| mouse_motion(this, event)));
    }
    extern "C" fn mouse_entered_exited(_this: &Object, _sel: Sel, _event: ObjcId) {}

    extern "C" fn touches_began_with_event(this: &Object, _sel: Sel, event: ObjcId) {
        // A raw finger contact on the trackpad: the only reliable signal for stopping kinetic
        // scrolling on a tap (the tap's click can be suppressed by the system's tap rejection).
        shielded("touchesBegan", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            // NSTouchPhaseTouching = Began | Moved | Stationary = 0b111
            let touching: ObjcId = unsafe { msg_send![event, touchesMatchingPhase: 7u64 inView: nil] };
            let count: u64 = unsafe { msg_send![touching, count] };
            win.touches_began(count == 1);
            win.scroll([0.0, 0.0], keys::event_mods(event), false, input::scroll::TOUCHED);
        }));
    }
    extern "C" fn touches_ended_with_event(this: &Object, _sel: Sel, event: ObjcId) {
        shielded("touchesEnded", std::panic::AssertUnwindSafe(|| win_of(this).touches_ended(keys::event_mods(event))));
    }
    extern "C" fn touches_cancelled_with_event(this: &Object, _sel: Sel, _event: ObjcId) {
        shielded("touchesCancelled", std::panic::AssertUnwindSafe(|| win_of(this).touches_began(false)));
    }

    extern "C" fn draw_rect(this: &Object, _sel: Sel, rect: NSRect) {
        unsafe {
            let superclass: ObjcId = msg_send![this, superclass];
            let () = msg_send![super (this, &*(superclass as *const Class)), drawRect: rect];
        }
    }

    extern "C" fn reset_cursor_rects(this: &Object, _sel: Sel) {
        // AppKit fires this from the display cycle, often re-entrantly: never panic across
        // this frame, never hand AppKit a cursor object we do not own (the cache retains)
        let call = std::panic::AssertUnwindSafe(|| unsafe {
            let (cursor, id) = app().current_cursor_object();
            if id.is_null() {
                return;
            }
            let bounds: NSRect = msg_send![this, bounds];
            if cursor == cursor::HIDDEN {
                // +[NSCursor setHiddenUntilMouseMoves:] is a CLASS method
                let _: () = msg_send![class!(NSCursor), setHiddenUntilMouseMoves: true];
            }
            let _: () = msg_send![this, addCursorRect: bounds cursor: id];
        });
        if std::panic::catch_unwind(call).is_err() {
            eprintln!("makepad-hal: PANIC contained at the resetCursorRects callback boundary; cursor rects skipped this round");
        }
    }

    // ---- NSTextInputClient

    extern "C" fn marked_range(this: &Object, _sel: Sel) -> NSRange {
        let length = marked_length(this);
        if length > 0 {
            NSRange { location: 0, length: length - 1 }
        } else {
            NSRange { location: i64::MAX as u64, length: 0 }
        }
    }
    extern "C" fn selected_range(_this: &Object, _sel: Sel) -> NSRange {
        NSRange { location: 0, length: 1 }
    }
    extern "C" fn has_marked_text(this: &Object, _sel: Sel) -> BOOL {
        (marked_length(this) > 0) as BOOL
    }

    extern "C" fn set_marked_text(this: &mut Object, _sel: Sel, string: ObjcId, _selected_range: NSRange, _replacement_range: NSRange) {
        shielded("setMarkedText", std::panic::AssertUnwindSafe(|| unsafe {
            let first = marked_length(this) == 0;
            app().ime_composition_changed(this as *const Object as usize);
            let marked_text_ref: &mut ObjcId = this.get_mut_ivar("markedText");
            let _: () = msg_send![(*marked_text_ref), release];
            let marked_text = NSMutableAttributedString::alloc(nil);
            let has_attr = msg_send![string, isKindOfClass: class!(NSAttributedString)];
            if has_attr {
                marked_text.init_with_attributed_string(string);
            } else {
                marked_text.init_with_string(string);
            };
            *marked_text_ref = marked_text;
            // echo the composition into the focused text field as an inline preview
            let characters: ObjcId = if has_attr { msg_send![string, string] } else { string };
            let composition = nsstring_to_string(characters);
            win_of(this).composition(&composition, first);
        }));
    }

    extern "C" fn unmark_text(this: &Object, _sel: Sel) {
        shielded("unmarkText", std::panic::AssertUnwindSafe(|| {
            // native cleanup clears the ivar before discardMarkedText, which may call us again:
            // do not edit the newly focused widget on that path
            if marked_length(this) == 0 {
                return;
            }
            app().ime_composition_changed(this as *const Object as usize);
            clear_marked_text(this);
            win_of(this).composition_cancelled();
        }));
    }

    extern "C" fn valid_attributes_for_marked_text(_this: &Object, _sel: Sel) -> ObjcId {
        app().marked_text_attributes()
    }
    extern "C" fn attributed_substring_for_proposed_range(_this: &Object, _sel: Sel, _range: NSRange, _actual_range: *mut c_void) -> ObjcId {
        nil
    }
    extern "C" fn character_index_for_point(_this: &Object, _sel: Sel, _point: NSPoint) -> u64 {
        0
    }

    extern "C" fn first_rect_for_character_range(this: &Object, _sel: Sel, _range: NSRange, _actual_range: *mut c_void) -> NSRect {
        let win = win_of(this);
        let view: ObjcId = this as *const _ as *mut _;
        unsafe {
            let view_rect: NSRect = msg_send![view, frame];
            // The render view is not flipped (y up from the bottom) while `ime_rect` is y down
            // from the top. The caret line box is padded vertically so the IME parks its
            // candidate window with a gap whichever side it lands on.
            let [x, y, w, h] = win.ime_rect.get();
            let clearance = h * 0.6;
            let local = NSRect {
                origin: NSPoint { x, y: view_rect.size.height - (y + h) - clearance },
                size: NSSize { width: w, height: h + 2.0 * clearance },
            };
            let in_window: NSRect = msg_send![view, convertRect: local toView: nil];
            msg_send![win.window, convertRectToScreen: in_window]
        }
    }

    extern "C" fn insert_text(this: &Object, _sel: Sel, string: ObjcId, replacement_range: NSRange) {
        shielded("insertText", std::panic::AssertUnwindSafe(|| unsafe {
            if marked_length(this) > 0 {
                app().ime_composition_changed(this as *const Object as usize);
            }
            let has_attr = msg_send![string, isKindOfClass: class!(NSAttributedString)];
            let characters = if has_attr { msg_send![string, string] } else { string };
            let string = nsstring_to_string(characters);
            // finish native composition before the app can move focus or start another one
            let composing = marked_length(this) > 0;
            clear_marked_text(this);
            win_of(this).text_commit(&string, replacement_range.length != 0 || composing);
            let input_context: ObjcId = msg_send![this, inputContext];
            let () = msg_send![input_context, invalidateCharacterCoordinates];
            let () = msg_send![this, setNeedsDisplay: YES];
        }));
    }

    extern "C" fn do_command_by_selector(this: &Object, _sel: Sel, command: Sel) {
        shielded("doCommand", std::panic::AssertUnwindSafe(|| {
            let cancel = command == sel!(cancelOperation:);
            app().ime_command(this as *const Object as usize, cancel);
            if cancel {
                unmark_text(this, _sel);
            }
        }));
    }

    extern "C" fn key_down(this: &Object, sel: Sel, event: ObjcId) {
        // Only forward to the input context while a text field has the focus, or typing outside
        // a text input still triggers the system IME.
        if win_of(this).ime_active.get() {
            shielded("keyDown", std::panic::AssertUnwindSafe(|| unsafe {
                let view = this as *const Object as usize;
                let marked = has_marked_text(this, sel) != NO;
                app().ime_begin_input_context(view, marked);
                let input_context: ObjcId = msg_send![this, inputContext];
                let handled: BOOL = msg_send![input_context, handleEvent: event];
                let marked = has_marked_text(this, sel) != NO;
                app().ime_end_input_context(view, handled != NO, marked);
            }));
        }
    }
    extern "C" fn key_up(_this: &Object, _sel: Sel, _event: ObjcId) {}

    extern "C" fn yes_function(_this: &Object, _se: Sel, _event: ObjcId) -> BOOL {
        YES
    }

    extern "C" fn needs_panel_to_become_key(this: &Object, _sel: Sel) -> BOOL {
        if win_of(this).needs_panel_to_become_key() {
            YES
        } else {
            NO
        }
    }

    // the layer's delegate: the layer redisplays when the view's bounds change
    extern "C" fn display_layer(this: &Object, _: Sel, _calayer: ObjcId) {
        shielded("displayLayer", std::panic::AssertUnwindSafe(|| win_of(this).send_geom()));
    }

    extern "C" fn view_did_change_effective_appearance(this: &Object, _: Sel) {
        shielded("appearanceLayout", std::panic::AssertUnwindSafe(|| {
            unsafe {
                let superclass: ObjcId = msg_send![this, superclass];
                let () = msg_send![super(this, &*(superclass as *const Class)), viewDidChangeEffectiveAppearance];
            }
            win_of(this).send_geom();
        }));
    }

    // ---- drag and drop

    extern "C" fn dragging_session_ended_at_point_operation(this: &Object, _: Sel, _session: ObjcId, _point: NSPoint, _operation: NSDragOperation) {
        shielded("dragEnded", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            let pos = win.view_point(NSPoint { x: 0.0, y: 0.0 });
            win.drag_drop(pos, false, &[]);
        }));
    }

    /// External file drags always copy: a destination must never negotiate a move and remove
    /// the managed file.
    extern "C" fn dragging_session_source_operation_mask(_this: &Object, _: Sel, _session: ObjcId, _context: i64) -> NSDragOperation {
        NSDragOperation::Copy
    }

    fn dragging(this: &Object, sender: ObjcId, phase: u32) -> NSDragOperation {
        let win = win_of(this);
        let (items, pos) = drag_items_from_pasteboard(this, sender);
        win.drag_over(phase, pos, &items);
        // the app answers asynchronously (`DragReply`): AppKit gets what it said the last time
        match win.drag_answer.get() {
            input::drag_response::COPY => NSDragOperation::Copy,
            input::drag_response::LINK => NSDragOperation::Link,
            input::drag_response::MOVE => NSDragOperation::Move,
            _ => NSDragOperation::None,
        }
    }

    extern "C" fn dragging_entered(this: &Object, _: Sel, sender: ObjcId) -> NSDragOperation {
        let mut op = NSDragOperation::None;
        shielded("draggingEntered", std::panic::AssertUnwindSafe(|| {
            win_of(this).drag_answer.set(input::drag_response::NONE);
            op = dragging(this, sender, 0);
        }));
        op
    }
    extern "C" fn dragging_updated(this: &Object, _: Sel, sender: ObjcId) -> NSDragOperation {
        let mut op = NSDragOperation::None;
        shielded("draggingUpdated", std::panic::AssertUnwindSafe(|| op = dragging(this, sender, 1)));
        op
    }
    extern "C" fn dragging_exited(this: &Object, _: Sel, _sender: ObjcId) {
        shielded("draggingExited", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            win.drag_answer.set(input::drag_response::NONE);
            win.drag_drop([-100000.0, -100000.0], false, &[]);
        }));
    }
    extern "C" fn dragging_ended(_this: &Object, _: Sel, _sender: ObjcId) {}

    /// Decode %XX escapes into their bytes; the result is the UTF-8 path the filesystem knows.
    fn percent_decode_path(encoded: &str) -> String {
        let bytes = encoded.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = |b: u8| match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                };
                if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push(hi * 16 + lo);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8(out).unwrap_or_else(|_| encoded.to_string())
    }

    /// The dragged file URLs as items, and the pointer in view points.
    fn drag_items_from_pasteboard(this: &Object, sender: ObjcId) -> (Vec<(u32, String)>, [f64; 2]) {
        let window_point: NSPoint = unsafe { msg_send![sender, draggingLocation] };
        let pos = win_of(this).view_point(window_point);
        let pasteboard: ObjcId = unsafe { msg_send![sender, draggingPasteboard] };
        let class: ObjcId = unsafe { msg_send![class!(NSURL), class] };
        let classes: ObjcId = unsafe { msg_send![class!(NSArray), arrayWithObject: class] };
        let object: ObjcId = unsafe { msg_send![class!(NSNumber), numberWithBool: true] };
        let options: ObjcId = unsafe { msg_send![class!(NSDictionary), dictionaryWithObject: object forKey: NSPasteboardURLReadingFileURLsOnlyKey] };
        let urls: ObjcId = unsafe { msg_send![pasteboard, readObjectsForClasses: classes options: options] };
        let count: usize = if urls == nil { 0 } else { unsafe { msg_send![urls, count] } };
        let mut items = Vec::with_capacity(count);
        for index in 0..count {
            let url: ObjcId = unsafe { msg_send![urls, objectAtIndex: index] };
            let url: ObjcId = unsafe { msg_send![url, filePathURL] };
            if url == nil {
                continue;
            }
            let string: ObjcId = unsafe { msg_send![url, absoluteString] };
            if string == nil {
                continue;
            }
            let string = nsstring_to_string(string);
            // rip off file:// and #id
            if let Some(string) = string.strip_prefix("file://") {
                let path = string.split("#makepad_internal_id=").next().unwrap_or("");
                // `absoluteString` is percent-ENCODED: a drop of "My File.png" arrives as
                // My%20File.png, and a FilePath item carries a filesystem path
                let path = if path == "makepad_internal_empty" { String::new() } else { percent_decode_path(path) };
                items.push((input::drag_item::FILE, path));
            }
        }
        (items, pos)
    }

    extern "C" fn perform_drag_operation(this: &Object, _: Sel, sender: ObjcId) -> BOOL {
        let mut handled = NO;
        shielded("performDrag", std::panic::AssertUnwindSafe(|| {
            let win = win_of(this);
            let (items, pos) = drag_items_from_pasteboard(this, sender);
            win.drag_drop(pos, true, &items);
            // the drop is accepted when the app took the drag over the window
            if win.drag_answer.get() != input::drag_response::NONE {
                handled = YES;
            }
        }));
        handled
    }

    /// AppKit starts a window drag in the titlebar strip on the window server, before the app
    /// sees the mouse-down, wherever it believes the content is see-through. This override
    /// claims the whole view, so every mouse-down reaches the view and the app's drag query
    /// alone decides caption drags.
    extern "C" fn opaque_rect_for_window_move_when_in_titlebar(this: &Object, _: Sel) -> NSRect {
        unsafe { msg_send![this, bounds] }
    }

    extern "C" fn mouse_down_can_move_window(_: &Object, _: Sel) -> BOOL {
        NO
    }

    extern "C" fn accepts_first_mouse(this: &Object, _: Sel, event: ObjcId) -> BOOL {
        // The caption answer crosses the asynchronous pipe. Accept the press here, then
        // mouseDown asks the app; only its Caption reply can start a native window drag.
        if event != nil && !win_of(this).retired.get() { YES } else { NO }
    }

    let mut decl = ClassDecl::new("HalView", class!(NSView)).unwrap();
    unsafe {
        decl.add_method(sel!(_opaqueRectForWindowMoveWhenInTitlebar), opaque_rect_for_window_move_when_in_titlebar as extern "C" fn(&Object, Sel) -> NSRect);
        decl.add_method(sel!(mouseDownCanMoveWindow), mouse_down_can_move_window as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(acceptsFirstMouse:), accepts_first_mouse as extern "C" fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(dealloc), dealloc as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(initWithPtr:), init_with_ptr as extern "C" fn(&Object, Sel, *mut c_void) -> ObjcId);
        decl.add_method(sel!(drawRect:), draw_rect as extern "C" fn(&Object, Sel, NSRect));
        decl.add_method(sel!(resetCursorRects), reset_cursor_rects as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(hasMarkedText), has_marked_text as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(markedRange), marked_range as extern "C" fn(&Object, Sel) -> NSRange);
        decl.add_method(sel!(selectedRange), selected_range as extern "C" fn(&Object, Sel) -> NSRange);
        decl.add_method(sel!(setMarkedText: selectedRange: replacementRange:), set_marked_text as extern "C" fn(&mut Object, Sel, ObjcId, NSRange, NSRange));
        decl.add_method(sel!(unmarkText), unmark_text as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(validAttributesForMarkedText), valid_attributes_for_marked_text as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(sel!(attributedSubstringForProposedRange: actualRange:), attributed_substring_for_proposed_range as extern "C" fn(&Object, Sel, NSRange, *mut c_void) -> ObjcId);
        decl.add_method(sel!(insertText: replacementRange:), insert_text as extern "C" fn(&Object, Sel, ObjcId, NSRange));
        decl.add_method(sel!(characterIndexForPoint:), character_index_for_point as extern "C" fn(&Object, Sel, NSPoint) -> u64);
        decl.add_method(sel!(firstRectForCharacterRange: actualRange:), first_rect_for_character_range as extern "C" fn(&Object, Sel, NSRange, *mut c_void) -> NSRect);
        decl.add_method(sel!(doCommandBySelector:), do_command_by_selector as extern "C" fn(&Object, Sel, Sel));
        decl.add_method(sel!(keyDown:), key_down as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(keyUp:), key_up as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(touchesBeganWithEvent:), touches_began_with_event as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(touchesEndedWithEvent:), touches_ended_with_event as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(touchesCancelledWithEvent:), touches_cancelled_with_event as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(mouseDown:), mouse_down as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(mouseUp:), mouse_up as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(rightMouseDown:), right_mouse_down as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(rightMouseUp:), right_mouse_up as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(otherMouseDown:), other_mouse_down as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(otherMouseUp:), other_mouse_up as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(mouseMoved:), mouse_moved as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(mouseDragged:), mouse_dragged as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(rightMouseDragged:), mouse_dragged as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(otherMouseDragged:), mouse_dragged as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(mouseEntered:), mouse_entered_exited as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(mouseExited:), mouse_entered_exited as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(wantsKeyDownForEvent:), yes_function as extern "C" fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(needsPanelToBecomeKey), needs_panel_to_become_key as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(acceptsFirstResponder:), yes_function as extern "C" fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(becomeFirstResponder:), yes_function as extern "C" fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(resignFirstResponder:), yes_function as extern "C" fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(displayLayer:), display_layer as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(viewDidChangeEffectiveAppearance), view_did_change_effective_appearance as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(draggingSession: sourceOperationMaskForDraggingContext:), dragging_session_source_operation_mask as extern "C" fn(&Object, Sel, ObjcId, i64) -> NSDragOperation);
        decl.add_method(sel!(draggingSession: endedAtPoint: operation:), dragging_session_ended_at_point_operation as extern "C" fn(&Object, Sel, ObjcId, NSPoint, NSDragOperation));
        decl.add_method(sel!(draggingEntered:), dragging_entered as extern "C" fn(&Object, Sel, ObjcId) -> NSDragOperation);
        decl.add_method(sel!(draggingExited:), dragging_exited as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(draggingUpdated:), dragging_updated as extern "C" fn(&Object, Sel, ObjcId) -> NSDragOperation);
        decl.add_method(sel!(performDragOperation:), perform_drag_operation as extern "C" fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(draggingEnded:), dragging_ended as extern "C" fn(&Object, Sel, ObjcId));
    }
    decl.add_ivar::<*mut c_void>("macos_window_ptr");
    decl.add_ivar::<ObjcId>("markedText");
    decl.add_protocol(&Protocol::get("NSTextInputClient").unwrap());
    decl.add_protocol(&Protocol::get("CALayerDelegate").unwrap());
    decl.add_protocol(&Protocol::get("NSDraggingSource").unwrap());
    decl.register()
}
