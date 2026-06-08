use crate::{
    event::{
        Ease, KeyCode, KeyEvent, KeyModifiers, SelectionHandleKind, SelectionHandlePhase, TouchState,
        VirtualKeyboardEvent,
    },
    makepad_math::*,
    os::{
        apple::apple_sys::*,
        apple::apple_util::key_modifiers_from_flags,
        apple::ios::ios_event::IosEvent,
        apple::ios_app::IosApp,
        apple::ios_app::{with_ios_app, IOS_APP},
    },
};
use std::ffi::c_void;

/// Helper to safely access IosApp without causing re-entrant borrow panics.
/// Returns None if we're already inside a with_ios_app call.
/// This is critical for callbacks from UIKit that can occur during borrows.
///
/// This function is shared by ios_delegates and ios_text_input modules.
pub fn try_with_ios_app<R>(
    f: impl FnOnce(&mut crate::os::apple::ios::ios_app::IosApp) -> R,
) -> Option<R> {
    IOS_APP
        .try_with(|app| {
            match app.try_borrow_mut() {
                Ok(mut app_ref) => {
                    if let Some(app) = app_ref.as_mut() {
                        return Some(f(app));
                    }
                }
                Err(_) => {
                    crate::log!("Warning: try_with_ios_app skipped due to re-entrant borrow");
                }
            }
            None
        })
        .ok()
        .flatten()
}

// Maps a USB HID keyboard usage code (iOS UIKey.keyCode) to a Makepad KeyCode.
fn hid_usage_to_key_code(usage: u64) -> KeyCode {
    match usage {
        0x04 => KeyCode::KeyA,
        0x05 => KeyCode::KeyB,
        0x06 => KeyCode::KeyC,
        0x07 => KeyCode::KeyD,
        0x08 => KeyCode::KeyE,
        0x09 => KeyCode::KeyF,
        0x0A => KeyCode::KeyG,
        0x0B => KeyCode::KeyH,
        0x0C => KeyCode::KeyI,
        0x0D => KeyCode::KeyJ,
        0x0E => KeyCode::KeyK,
        0x0F => KeyCode::KeyL,
        0x10 => KeyCode::KeyM,
        0x11 => KeyCode::KeyN,
        0x12 => KeyCode::KeyO,
        0x13 => KeyCode::KeyP,
        0x14 => KeyCode::KeyQ,
        0x15 => KeyCode::KeyR,
        0x16 => KeyCode::KeyS,
        0x17 => KeyCode::KeyT,
        0x18 => KeyCode::KeyU,
        0x19 => KeyCode::KeyV,
        0x1A => KeyCode::KeyW,
        0x1B => KeyCode::KeyX,
        0x1C => KeyCode::KeyY,
        0x1D => KeyCode::KeyZ,
        0x1E => KeyCode::Key1,
        0x1F => KeyCode::Key2,
        0x20 => KeyCode::Key3,
        0x21 => KeyCode::Key4,
        0x22 => KeyCode::Key5,
        0x23 => KeyCode::Key6,
        0x24 => KeyCode::Key7,
        0x25 => KeyCode::Key8,
        0x26 => KeyCode::Key9,
        0x27 => KeyCode::Key0,
        0x28 => KeyCode::ReturnKey,
        0x29 => KeyCode::Escape,
        0x2A => KeyCode::Backspace,
        0x2B => KeyCode::Tab,
        0x2C => KeyCode::Space,
        0x2D => KeyCode::Minus,
        0x2E => KeyCode::Equals,
        0x2F => KeyCode::LBracket,
        0x30 => KeyCode::RBracket,
        0x31 => KeyCode::Backslash,
        0x33 => KeyCode::Semicolon,
        0x34 => KeyCode::Quote,
        0x35 => KeyCode::Backtick,
        0x36 => KeyCode::Comma,
        0x37 => KeyCode::Period,
        0x38 => KeyCode::Slash,
        0x39 => KeyCode::Capslock,
        0x3A => KeyCode::F1,
        0x3B => KeyCode::F2,
        0x3C => KeyCode::F3,
        0x3D => KeyCode::F4,
        0x3E => KeyCode::F5,
        0x3F => KeyCode::F6,
        0x40 => KeyCode::F7,
        0x41 => KeyCode::F8,
        0x42 => KeyCode::F9,
        0x43 => KeyCode::F10,
        0x44 => KeyCode::F11,
        0x45 => KeyCode::F12,
        0x49 => KeyCode::Insert,
        0x4A => KeyCode::Home,
        0x4B => KeyCode::PageUp,
        0x4C => KeyCode::Delete,
        0x4D => KeyCode::End,
        0x4E => KeyCode::PageDown,
        0x4F => KeyCode::ArrowRight,
        0x50 => KeyCode::ArrowLeft,
        0x51 => KeyCode::ArrowDown,
        0x52 => KeyCode::ArrowUp,
        0x53 => KeyCode::Numlock,
        0x54 => KeyCode::NumpadDivide,
        0x55 => KeyCode::NumpadMultiply,
        0x56 => KeyCode::NumpadSubtract,
        0x57 => KeyCode::NumpadAdd,
        0x58 => KeyCode::NumpadEnter,
        0x59 => KeyCode::Numpad1,
        0x5A => KeyCode::Numpad2,
        0x5B => KeyCode::Numpad3,
        0x5C => KeyCode::Numpad4,
        0x5D => KeyCode::Numpad5,
        0x5E => KeyCode::Numpad6,
        0x5F => KeyCode::Numpad7,
        0x60 => KeyCode::Numpad8,
        0x61 => KeyCode::Numpad9,
        0x62 => KeyCode::Numpad0,
        0x63 => KeyCode::NumpadDecimal,
        0x67 => KeyCode::NumpadEquals,
        _ => KeyCode::Unknown,
    }
}

fn is_hardware_navigation_key(key_code: KeyCode) -> bool {
    matches!(
        key_code,
        KeyCode::ArrowLeft
            | KeyCode::ArrowRight
            | KeyCode::ArrowUp
            | KeyCode::ArrowDown
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
    )
}

// Nav keys owned by the auto-repeating UIKeyCommand path (key_commands in
// ios_text_input.rs) when an editor is focused; skip them here so they don't
// also move via the one-shot pressesBegan path.
fn is_keycommand_nav_key(key_code: KeyCode) -> bool {
    match key_code {
        KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ArrowUp | KeyCode::ArrowDown => true,
        // Home/End/PageUp/PageDown only when their iOS 13.4+ commands resolved;
        // otherwise they keep navigating via the one-shot pressesBegan path.
        KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown => {
            super::ios_text_input::doc_nav_keycommands_available()
        }
        _ => false,
    }
}

fn text_input_is_first_responder() -> bool {
    try_with_ios_app(|app| {
        app.text_input_view
            .map(|view| unsafe {
                let is_fr: BOOL = msg_send![view, isFirstResponder];
                is_fr == YES
            })
            .unwrap_or(false)
    })
    .unwrap_or(false)
}

unsafe fn makepad_key_press(press: ObjcId) -> Option<(KeyCode, crate::event::KeyModifiers)> {
    let key: ObjcId = msg_send![press, key];
    if key == nil {
        return None;
    }
    let flags: u64 = msg_send![key, modifierFlags];
    let modifiers = key_modifiers_from_flags(flags);
    let usage: u64 = msg_send![key, keyCode];
    let key_code = hid_usage_to_key_code(usage);
    let is_enter = key_code == KeyCode::ReturnKey || key_code == KeyCode::NumpadEnter;
    if (!is_enter && !modifiers.logo && !is_hardware_navigation_key(key_code)) || key_code.is_unknown() {
        None
    } else {
        Some((key_code, modifiers))
    }
}

// Dispatches hardware-keyboard presses we handle (Return/NumpadEnter,
// navigation keys, and Command shortcuts) as Makepad events. The first pass
// verifies the whole UIPresses set is consumable; otherwise callers should
// forward the event to UIKit instead of partially dispatching it.
pub fn dispatch_makepad_key_code(key_code: KeyCode, modifiers: KeyModifiers, is_down: bool) -> bool {
    if key_code.is_unknown() {
        return false;
    }

    // Cmd+C/X/V route to the shared copy/cut/paste path instead of a
    // generic key event; KeyUp is swallowed.
    if modifiers.logo {
        match key_code {
            KeyCode::KeyC => {
                if is_down {
                    IosApp::send_clipboard_action("copy");
                }
                return true;
            }
            KeyCode::KeyX => {
                if is_down {
                    IosApp::send_clipboard_action("cut");
                }
                return true;
            }
            KeyCode::KeyV => {
                if is_down {
                    IosApp::send_clipboard_paste();
                }
                return true;
            }
            _ => {}
        }
    }

    let time = try_with_ios_app(|app| app.time_now()).unwrap_or(0.0);
    let key_event = KeyEvent {
        key_code,
        is_repeat: false,
        modifiers,
        time,
    };
    if is_down {
        IosApp::do_callback(IosEvent::KeyDown(key_event));
    } else {
        IosApp::do_callback(IosEvent::KeyUp(key_event));
    }
    true
}

pub unsafe fn dispatch_hardware_key_presses(
    presses: ObjcId,
    is_down: bool,
    dispatch_navigation: bool,
) -> bool {
    let enumerator: ObjcId = msg_send![presses, objectEnumerator];
    let mut saw_press = false;
    loop {
        let press: ObjcId = msg_send![enumerator, nextObject];
        if press == nil {
            break;
        }
        saw_press = true;
        if makepad_key_press(press).is_none() {
            return false;
        }
    }
    if !saw_press {
        return false;
    }

    let enumerator: ObjcId = msg_send![presses, objectEnumerator];
    let mut handled_press = false;
    loop {
        let press: ObjcId = msg_send![enumerator, nextObject];
        if press == nil {
            break;
        }
        let Some((key_code, modifiers)) = makepad_key_press(press) else {
            continue;
        };
        if is_keycommand_nav_key(key_code) && text_input_is_first_responder() {
            continue;
        }
        if !dispatch_navigation && is_hardware_navigation_key(key_code) {
            continue;
        }
        dispatch_makepad_key_code(key_code, modifiers, is_down);
        handled_press = true;
    }
    handled_press
}

pub fn define_makepad_view_controller() -> *const Class {
    let superclass = class!(UIViewController);
    let mut decl = ClassDecl::new("MakepadViewController", superclass).unwrap();

    decl.add_ivar::<BOOL>("_prefersStatusBarHidden");
    decl.add_ivar::<BOOL>("_prefersHomeIndicatorAutoHidden");
    // UIStatusBarStyle raw value: 0 = Default, 1 = LightContent, 3 = DarkContent.
    decl.add_ivar::<i64>("_preferredStatusBarStyle");

    extern "C" fn prefers_status_bar_hidden(this: &Object, _: Sel) -> BOOL {
        unsafe { *this.get_ivar("_prefersStatusBarHidden") }
    }

    extern "C" fn prefers_home_indicator_auto_hidden(this: &Object, _: Sel) -> BOOL {
        unsafe { *this.get_ivar("_prefersHomeIndicatorAutoHidden") }
    }

    extern "C" fn preferred_status_bar_style(this: &Object, _: Sel) -> i64 {
        unsafe { *this.get_ivar("_preferredStatusBarStyle") }
    }

    // Called by iOS when the safe area insets change (e.g., device rotation).
    // At this point the view's safeAreaInsets reflect the new orientation,
    // unlike draw_size_will_change which may fire before the update.
    extern "C" fn view_safe_area_insets_did_change(_this: &Object, _: Sel) {
        let should_call = IOS_APP
            .try_with(|app| match app.try_borrow_mut() {
                Ok(app_ref) => app_ref.is_some(),
                Err(_) => false,
            })
            .unwrap_or(false);
        if should_call {
            IosApp::check_window_geom();
        }
    }

    unsafe {
        decl.add_method(
            sel!(prefersStatusBarHidden),
            prefers_status_bar_hidden as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(prefersHomeIndicatorAutoHidden),
            prefers_home_indicator_auto_hidden as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(preferredStatusBarStyle),
            preferred_status_bar_style as extern "C" fn(&Object, Sel) -> i64,
        );
        decl.add_method(
            sel!(viewSafeAreaInsetsDidChange),
            view_safe_area_insets_did_change as extern "C" fn(&Object, Sel),
        );
    }

    decl.register()
}

pub fn define_ios_app_delegate() -> *const Class {
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("NSAppDelegate", superclass).unwrap();

    extern "C" fn did_finish_launching_with_options(
        _: &Object,
        _: Sel,
        _: ObjcId,
        _: ObjcId,
    ) -> BOOL {
        with_ios_app(|app| app.did_finish_launching_with_options());
        YES
    }

    extern "C" fn application_will_enter_foreground(_: &Object, _: Sel, _: ObjcId) {
        IosApp::do_callback(crate::os::apple::ios::ios_event::IosEvent::Foreground);
    }

    extern "C" fn application_did_enter_background(_: &Object, _: Sel, _: ObjcId) {
        IosApp::do_callback(crate::os::apple::ios::ios_event::IosEvent::Background);
    }

    extern "C" fn application_will_resign_active(_: &Object, _: Sel, _: ObjcId) {
        IosApp::do_callback(crate::os::apple::ios::ios_event::IosEvent::Pause);
    }

    extern "C" fn application_did_become_active(_: &Object, _: Sel, _: ObjcId) {
        IosApp::do_callback(crate::os::apple::ios::ios_event::IosEvent::Resume);
    }

    extern "C" fn application_will_terminate(_: &Object, _: Sel, _: ObjcId) {
        IosApp::do_callback(crate::os::apple::ios::ios_event::IosEvent::Shutdown);
    }

    unsafe {
        decl.add_method(
            sel!(application: didFinishLaunchingWithOptions:),
            did_finish_launching_with_options
                as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> BOOL,
        );
        decl.add_method(
            sel!(applicationWillEnterForeground:),
            application_will_enter_foreground as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(applicationDidEnterBackground:),
            application_did_enter_background as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(applicationWillResignActive:),
            application_will_resign_active as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(applicationDidBecomeActive:),
            application_did_become_active as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(applicationWillTerminate:),
            application_will_terminate as extern "C" fn(&Object, Sel, ObjcId),
        );
    }

    return decl.register();
}

pub fn define_mtk_view() -> *const Class {
    let mut decl = ClassDecl::new("MakepadView", class!(MTKView)).unwrap();

    // Add instance variables for clipboard menu state
    decl.add_ivar::<BOOL>("has_selection");
    decl.add_ivar::<f64>("menu_rect_x");
    decl.add_ivar::<f64>("menu_rect_y");
    decl.add_ivar::<f64>("menu_rect_width");
    decl.add_ivar::<f64>("menu_rect_height");
    decl.add_ivar::<*mut c_void>("edit_menu_interaction");

    extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }

    // Required for UIEditMenuInteraction to work - view must be able to become first responder
    extern "C" fn can_become_first_responder(_: &Object, _: Sel) -> BOOL {
        YES
    }

    // Return nil to prevent keyboard from showing when MakepadView becomes first responder
    // (The hidden UITextField handles keyboard input separately)
    extern "C" fn input_view(_: &Object, _: Sel) -> ObjcId {
        nil
    }

    // Filter which clipboard actions are available based on selection state
    extern "C" fn can_perform_action(this: &Object, _: Sel, action: Sel, _sender: ObjcId) -> BOOL {
        unsafe {
            let has_selection: BOOL = *this.get_ivar("has_selection");

            // Copy and Cut require a selection
            if action == sel!(copy:) || action == sel!(cut:) {
                return has_selection;
            }

            // Paste requires clipboard to have text content
            if action == sel!(paste:) {
                let pasteboard: ObjcId = msg_send![class!(UIPasteboard), generalPasteboard];
                let has_strings: BOOL = msg_send![pasteboard, hasStrings];
                return has_strings;
            }

            // Select All is always available
            if action == sel!(selectAll:) {
                return YES;
            }

            NO
        }
    }

    // Action handlers for clipboard operations
    extern "C" fn copy_action(_this: &Object, _: Sel, _sender: ObjcId) {
        IosApp::send_clipboard_action("copy");
    }

    extern "C" fn cut_action(_this: &Object, _: Sel, _sender: ObjcId) {
        IosApp::send_clipboard_action("cut");
    }

    extern "C" fn paste_action(_this: &Object, _: Sel, _sender: ObjcId) {
        IosApp::send_clipboard_paste();
    }

    extern "C" fn select_all_action(_this: &Object, _: Sel, _sender: ObjcId) {
        IosApp::send_clipboard_action("select_all");
    }

    fn on_touch(this: &Object, event: ObjcId, state: TouchState) {
        unsafe {
            let enumerator: ObjcId = msg_send![event, allTouches];
            let size: u64 = msg_send![enumerator, count];
            let enumerator: ObjcId = msg_send![enumerator, objectEnumerator];

            for touch_id in 0..size {
                let ios_touch: ObjcId = msg_send![enumerator, nextObject];
                let uid_obj: ObjcId = msg_send![ios_touch, estimationUpdateIndex];
                let uid: u64 = if uid_obj != nil {
                    msg_send![uid_obj, intValue]
                } else {
                    touch_id as u64
                };
                let p: NSPoint = msg_send![ios_touch, locationInView: this];

                // Get touch radius and force from UITouch
                // majorRadius is in points, representing the radius of the touch area
                let major_radius: f64 = msg_send![ios_touch, majorRadius];
                let force: f64 = msg_send![ios_touch, force];

                with_ios_app(|app| {
                    app.update_touch_with_details(
                        uid,
                        dvec2(p.x, p.y),
                        state,
                        dvec2(major_radius, major_radius),
                        force,
                    )
                });
            }
        }
    }

    extern "C" fn touches_began(this: &Object, _: Sel, _: ObjcId, event: ObjcId) {
        on_touch(this, event, TouchState::Start);
        IosApp::send_touch_update();
    }

    extern "C" fn touches_moved(this: &Object, _: Sel, _: ObjcId, event: ObjcId) {
        on_touch(this, event, TouchState::Move);
        IosApp::send_touch_update();
    }

    extern "C" fn touches_ended(this: &Object, _: Sel, _: ObjcId, event: ObjcId) {
        on_touch(this, event, TouchState::Stop);
        IosApp::send_touch_update();
    }

    extern "C" fn touches_canceled(this: &Object, _: Sel, _: ObjcId, event: ObjcId) {
        on_touch(this, event, TouchState::Stop);
        IosApp::send_touch_update();
    }

    // Hardware-keyboard shortcuts, handled app-wide like desktop (MakepadView is
    // in the responder chain whether or not a text field is focused).
    extern "C" fn presses_began(this: &Object, _: Sel, presses: ObjcId, event: ObjcId) {
        unsafe {
            if !dispatch_hardware_key_presses(presses, true, true) {
                let () = msg_send![super(this, class!(MTKView)), pressesBegan: presses withEvent: event];
            }
        }
    }

    extern "C" fn presses_changed(this: &Object, _: Sel, presses: ObjcId, event: ObjcId) {
        unsafe {
            if !dispatch_hardware_key_presses(presses, true, true) {
                let () = msg_send![super(this, class!(MTKView)), pressesChanged: presses withEvent: event];
            }
        }
    }

    extern "C" fn presses_ended(this: &Object, _: Sel, presses: ObjcId, event: ObjcId) {
        unsafe {
            if !dispatch_hardware_key_presses(presses, false, true) {
                let () = msg_send![super(this, class!(MTKView)), pressesEnded: presses withEvent: event];
            }
        }
    }

    extern "C" fn presses_cancelled(this: &Object, _: Sel, presses: ObjcId, event: ObjcId) {
        unsafe {
            if !dispatch_hardware_key_presses(presses, false, true) {
                let () = msg_send![super(this, class!(MTKView)), pressesCancelled: presses withEvent: event];
            }
        }
    }

    unsafe {
        decl.add_method(sel!(isOpaque), yes as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(
            sel!(touchesBegan: withEvent:),
            touches_began as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(touchesMoved: withEvent:),
            touches_moved as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(touchesEnded: withEvent:),
            touches_ended as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(touchesCanceled: withEvent:),
            touches_canceled as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );

        // First responder support for clipboard menu
        decl.add_method(
            sel!(canBecomeFirstResponder),
            can_become_first_responder as extern "C" fn(&Object, Sel) -> BOOL,
        );
        // Return nil to prevent keyboard from appearing when view becomes first responder
        decl.add_method(
            sel!(inputView),
            input_view as extern "C" fn(&Object, Sel) -> ObjcId,
        );
        decl.add_method(
            sel!(canPerformAction:withSender:),
            can_perform_action as extern "C" fn(&Object, Sel, Sel, ObjcId) -> BOOL,
        );

        // Clipboard action handlers
        decl.add_method(
            sel!(copy:),
            copy_action as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(cut:),
            cut_action as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(paste:),
            paste_action as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(selectAll:),
            select_all_action as extern "C" fn(&Object, Sel, ObjcId),
        );

        // Hardware keyboard shortcuts (e.g. Cmd+Enter, Cmd +/-)
        decl.add_method(
            sel!(pressesBegan: withEvent:),
            presses_began as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(pressesChanged: withEvent:),
            presses_changed as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(pressesEnded: withEvent:),
            presses_ended as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(pressesCancelled: withEvent:),
            presses_cancelled as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
    }

    return decl.register();
}

pub fn define_mtk_view_delegate() -> *const Class {
    let mut decl = ClassDecl::new("MakepadViewDlg", class!(NSObject)).unwrap();

    extern "C" fn draw_in_rect(_this: &Object, _: Sel, _: ObjcId) {
        IosApp::draw_in_rect();
    }

    extern "C" fn draw_size_will_change(_this: &Object, _: Sel, view: ObjcId, size: NSSize) {
        IosApp::draw_size_will_change(view, size);
    }
    unsafe {
        decl.add_method(
            sel!(drawInMTKView:),
            draw_in_rect as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(mtkView: drawableSizeWillChange:),
            draw_size_will_change as extern "C" fn(&Object, Sel, ObjcId, NSSize),
        );
    }

    decl.add_ivar::<*mut c_void>("display_ptr");
    return decl.register();
}

/// Defines a class that acts as the target "receiver" for the long press gesture recognizer's
/// "gesture recognized" action.
pub fn define_gesture_recognizer_handler() -> *const Class {
    let mut decl = ClassDecl::new("LongPressGestureRecognizerHandler", class!(NSObject)).unwrap();

    extern "C" fn handle_long_press_gesture(
        _this: &Object,
        _: Sel,
        gesture_recognizer: ObjcId,
        _: ObjcId,
    ) {
        unsafe {
            let state: i64 = msg_send![gesture_recognizer, state];
            // One might expect that we want to trigger on the "Recognized" or "Ended" state,
            // but that state is not triggered until the user lifts their finger.
            // We want to trigger on the "Began" state, which occurs only once the user has long-pressed
            // for a long-enough time interval to trigger the gesture (without having to lift their finger).
            if state == 1 {
                // UIGestureRecognizerStateBegan
                let view: ObjcId = msg_send![gesture_recognizer, view];
                let location_in_view: NSPoint = msg_send![gesture_recognizer, locationInView: view];
                // There's no way to get the touch event's UID from within a default gesture recognizer
                // (we'd have to fully implement our own). Since UID isn't used for long presses,
                // this isn't worth the effort.
                let uid = 0;
                IosApp::send_long_press(location_in_view, uid);
            }
            // Note: in `did_finish_launching_with_options()`, we set gesture recognizer's `cancelTouchesInView` property
            // to `NO`, which means that the gesture recognizer will still allow Makepad's MTKView
            // to continue receiving touch events even after the long-press gesture has been recognized.
            // Thus, we don't need to handle the UIGestureRecognizerStateChanged or UIGestureRecognizerStateEnded
            // states here, as they'll be handled by the `on_touch` function above, as normal.
        }
    }

    unsafe {
        decl.add_method(
            sel!(handleLongPressGesture: gestureRecognizer:),
            handle_long_press_gesture as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
    }

    return decl.register();
}

pub fn define_selection_handle_gesture_handler() -> *const Class {
    let mut decl = ClassDecl::new("SelectionHandlePanRecognizerHandler", class!(NSObject)).unwrap();

    decl.add_ivar::<i64>("handle_kind");

    extern "C" fn handle_selection_handle_pan(this: &Object, _: Sel, gesture_recognizer: ObjcId) {
        unsafe {
            let state: i64 = msg_send![gesture_recognizer, state];
            let phase = match state {
                1 => Some(SelectionHandlePhase::Begin), // UIGestureRecognizerStateBegan
                2 => Some(SelectionHandlePhase::Move),  // UIGestureRecognizerStateChanged
                3 | 4 | 5 => Some(SelectionHandlePhase::End), // ended/cancelled/failed
                _ => None,
            };
            let Some(phase) = phase else {
                return;
            };

            let handle_kind_raw: i64 = *this.get_ivar("handle_kind");
            let handle = if handle_kind_raw == 0 {
                SelectionHandleKind::Start
            } else {
                SelectionHandleKind::End
            };

            let handle_view: ObjcId = msg_send![gesture_recognizer, view];
            if handle_view == nil {
                return;
            }
            let host_view: ObjcId = msg_send![handle_view, superview];
            if host_view == nil {
                return;
            }
            let location: NSPoint = msg_send![gesture_recognizer, locationInView: host_view];

            // Keep the dragged handle visually under the finger; Rust will send authoritative
            // positions back through update_selection_handles.
            let () = msg_send![handle_view, setCenter: location];

            IosApp::send_selection_handle_drag(handle, phase, dvec2(location.x, location.y));
        }
    }

    unsafe {
        decl.add_method(
            sel!(handleSelectionHandlePan:),
            handle_selection_handle_pan as extern "C" fn(&Object, Sel, ObjcId),
        );
    }

    decl.register()
}

pub fn define_ios_timer_delegate() -> *const Class {
    extern "C" fn received_timer(_this: &Object, _: Sel, nstimer: ObjcId) {
        IosApp::send_timer_received(nstimer);
    }

    extern "C" fn received_live_resize(_this: &Object, _: Sel, _nstimer: ObjcId) {
        IosApp::send_paint_event();
    }

    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("TimerDelegate", superclass).unwrap();

    // Add callback methods
    unsafe {
        decl.add_method(
            sel!(receivedTimer:),
            received_timer as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(receivedLiveResize:),
            received_live_resize as extern "C" fn(&Object, Sel, ObjcId),
        );
    }

    return decl.register();
}

pub fn define_textfield_delegate() -> *const Class {
    let mut decl = ClassDecl::new("NSTextFieldDlg", class!(NSObject)).unwrap();

    #[derive(Clone, Copy)]
    struct KeyboardGeometry {
        is_visible: bool,
        bottom_overlap: f64,
    }

    fn positive_min(a: f64, b: f64) -> Option<f64> {
        match (a > 0.0, b > 0.0) {
            (true, true) => Some(a.min(b)),
            (true, false) => Some(a),
            (false, true) => Some(b),
            (false, false) => None,
        }
    }

    /// Returns the keyboard visibility and docked bottom overlap in the
    /// MTKView's coordinate space, in native UIKit points. `ios.rs` converts
    /// queued keyboard events into Makepad layout points before dispatch.
    ///
    /// UIKit reports the keyboard frame in screen coordinates. Converting
    /// through the screen coordinate space keeps the value correct for iPad
    /// Split View, Slide Over, Stage Manager, rotation, and multi-window
    /// placements where the app window is not the full screen.
    fn get_keyboard_geometry_in_view(notif: ObjcId, view: ObjcId) -> KeyboardGeometry {
        let hidden = KeyboardGeometry {
            is_visible: false,
            bottom_overlap: 0.0,
        };
        if view.is_null() {
            return hidden;
        }
        unsafe {
            let info: ObjcId = msg_send![notif, userInfo];
            let obj: ObjcId = msg_send![info, objectForKey: UIKeyboardFrameEndUserInfoKey];
            if obj.is_null() {
                return hidden;
            }
            let end_in_screen: NSRect = msg_send![obj, CGRectValue];

            // Prefer the screen's coordinate space (correct for multi-window
            // iPad). Fall back to window-coords-to-view-coords if anything in
            // the chain is null. That's only reachable on a not-yet-attached
            // view during early init, when we wouldn't want to shift anyway.
            let window: ObjcId = msg_send![view, window];
            let coord_space: ObjcId = if window.is_null() {
                std::ptr::null_mut()
            } else {
                let screen: ObjcId = msg_send![window, screen];
                if screen.is_null() {
                    std::ptr::null_mut()
                } else {
                    msg_send![screen, coordinateSpace]
                }
            };
            let end_in_view: NSRect = if coord_space.is_null() {
                msg_send![view, convertRect: end_in_screen fromView: nil as ObjcId]
            } else {
                msg_send![view, convertRect: end_in_screen fromCoordinateSpace: coord_space]
            };

            let view_bounds: NSRect = msg_send![view, bounds];
            let view_x2 = view_bounds.origin.x + view_bounds.size.width;
            let view_y2 = view_bounds.origin.y + view_bounds.size.height;
            let view_left = view_bounds.origin.x.min(view_x2);
            let view_top = view_bounds.origin.y.min(view_y2);
            let view_right = view_bounds.origin.x.max(view_x2);
            let view_bottom = view_bounds.origin.y.max(view_y2);
            let kbd_x2 = end_in_view.origin.x + end_in_view.size.width;
            let kbd_y2 = end_in_view.origin.y + end_in_view.size.height;
            let kbd_left = end_in_view.origin.x.min(kbd_x2);
            let kbd_top = end_in_view.origin.y.min(kbd_y2);
            let kbd_right = end_in_view.origin.x.max(kbd_x2);
            let kbd_bottom = end_in_view.origin.y.max(kbd_y2);

            let visible_width = kbd_right.min(view_right) - kbd_left.max(view_left);
            let visible_height = kbd_bottom.min(view_bottom) - kbd_top.max(view_top);
            let is_visible = visible_width > 1.0 && visible_height > 1.0;
            if !is_visible {
                return hidden;
            }

            // A floating or undocked iPad keyboard is visible, but it does not
            // create a bottom obstruction. Report it as visible with zero
            // overlap so focus remains intact while KeyboardView clears any
            // previous docked-keyboard shift.
            let is_docked = kbd_bottom + 1.0 >= view_bottom;
            let bottom_overlap = if is_docked {
                let frame_height = positive_min(
                    end_in_view.size.height.abs(),
                    end_in_screen.size.height.abs(),
                )
                .unwrap_or(visible_height);
                visible_height
                    .max(0.0)
                    .min(frame_height)
                    .min((view_bottom - view_top).max(0.0))
            } else {
                0.0
            };
            KeyboardGeometry {
                is_visible,
                bottom_overlap,
            }
        }
    }

    fn get_curve_duration(notif: ObjcId) -> (f64, Ease) {
        unsafe {
            let info: ObjcId = msg_send![notif, userInfo];
            let obj: ObjcId = msg_send![info, objectForKey: UIKeyboardAnimationDurationUserInfoKey];
            let duration: f64 = msg_send![obj, doubleValue];
            let obj: ObjcId = msg_send![info, objectForKey: UIKeyboardAnimationCurveUserInfoKey];
            let curve: i64 = msg_send![obj, intValue];
            let curve = if curve > 3 { curve >> 16 } else { curve };

            let ease = match curve {
                // UIKit's standard timing curves as cubic-bezier(x1, y1, x2, y2).
                0 => Ease::Bezier {
                    cp0: 0.42,
                    cp1: 0.0,
                    cp2: 0.58,
                    cp3: 1.0,
                },
                1 => Ease::Bezier {
                    cp0: 0.42,
                    cp1: 0.0,
                    cp2: 1.0,
                    cp3: 1.0,
                },
                2 => Ease::Bezier {
                    cp0: 0.0,
                    cp1: 0.0,
                    cp2: 0.58,
                    cp3: 1.0,
                },
                _ => Ease::Linear,
            };
            (duration, ease)
        }
    }

    /// Pull the MTKView pointer out of the global app state without holding a
    /// borrow across the UIKit `msg_send`s that need it. Returns null if the
    /// app isn't initialized yet or the view hasn't been created.
    fn get_mtk_view() -> ObjcId {
        try_with_ios_app(|app| app.mtk_view)
            .flatten()
            .unwrap_or(std::ptr::null_mut())
    }

    /// Queue a `Will{Show,Hide}` event keyed off the absolute post-animation
    /// height. UIKit fires `keyboardWillChangeFrame` for every keyboard frame
    /// transition: initial show, mid-flight changes (language switch,
    /// predictive bar toggle, custom inputAccessoryView height changes,
    /// rotation), and dismissal. This is the single source of truth.
    extern "C" fn keyboard_will_change_frame(_: &Object, _: Sel, notif: ObjcId) {
        let view = get_mtk_view();
        let geometry = get_keyboard_geometry_in_view(notif, view);
        let (duration, ease) = get_curve_duration(notif);
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| {
                let event = if geometry.is_visible {
                    VirtualKeyboardEvent::WillShow {
                        time,
                        height: geometry.bottom_overlap,
                        ease,
                        duration,
                    }
                } else {
                    VirtualKeyboardEvent::WillHide {
                        time,
                        height: 0.0,
                        ease,
                        duration,
                    }
                };
                app.queue_virtual_keyboard_event(event)
            });
        }
    }

    /// Settled state corresponding to `keyboard_will_change_frame`. A visible
    /// floating keyboard is still DidShow, but with zero bottom overlap.
    extern "C" fn keyboard_did_change_frame(_: &Object, _: Sel, notif: ObjcId) {
        let view = get_mtk_view();
        let geometry = get_keyboard_geometry_in_view(notif, view);
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| {
                let event = if geometry.is_visible {
                    VirtualKeyboardEvent::DidShow {
                        time,
                        height: geometry.bottom_overlap,
                    }
                } else {
                    VirtualKeyboardEvent::DidHide { time }
                };
                app.queue_virtual_keyboard_event(event)
            });
        }
    }

    // The Show/Hide notifications fire *in addition* to ChangeFrame for the
    // appearing/disappearing transitions, with the same parameters. We keep
    // the observers registered (so UIKit's notification book-keeping stays
    // intact) but treat ChangeFrame as authoritative. These are no-ops.
    // IosApp stores one pending virtual-keyboard event, so even if both
    // pathways queued, the later write wins. Routing through one path keeps
    // height-change behavior identical to show/hide behavior.
    extern "C" fn keyboard_will_hide(_: &Object, _: Sel, _notif: ObjcId) {}
    extern "C" fn keyboard_did_hide(_: &Object, _: Sel, _notif: ObjcId) {}
    extern "C" fn keyboard_will_show(_: &Object, _: Sel, _notif: ObjcId) {}
    extern "C" fn keyboard_did_show(_: &Object, _: Sel, _notif: ObjcId) {}
    extern "C" fn input_mode_did_change(_: &Object, _: Sel, _notif: ObjcId) {
        // When keyboard language changes, reload input views so iOS re-queries
        // autocorrectionType (which dynamically checks CJK vs non-CJK).
        // Extract the view pointer first; reloadInputViews can trigger
        // synchronous UIKit callbacks that re-enter IOS_APP.
        let view = try_with_ios_app(|app| app.text_input_view).flatten();
        if let Some(text_input_view) = view {
            unsafe {
                let () = msg_send![text_input_view, reloadInputViews];
            }
        }
    }

    extern "C" fn physical_keyboard_changed(_: &Object, _: Sel, _notif: ObjcId) {
        let event = try_with_ios_app(|app| app.sync_physical_keyboard_state()).flatten();
        if let Some(event) = event {
            IosApp::do_callback(crate::os::apple::ios::ios_event::IosEvent::PhysicalKeyboard(
                event,
            ));
        }
    }

    unsafe {
        decl.add_method(
            sel!(keyboardDidChangeFrame:),
            keyboard_did_change_frame as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(keyboardWillChangeFrame:),
            keyboard_will_change_frame as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(keyboardWillShow:),
            keyboard_will_show as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(keyboardDidShow:),
            keyboard_did_show as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(keyboardWillHide:),
            keyboard_will_hide as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(keyboardDidHide:),
            keyboard_did_hide as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(inputModeDidChange:),
            input_mode_did_change as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(physicalKeyboardChanged:),
            physical_keyboard_changed as extern "C" fn(&Object, Sel, ObjcId),
        );
    }
    decl.add_ivar::<*mut c_void>("display_ptr");
    return decl.register();
}

/// Defines a delegate class for UIEditMenuInteraction
/// This delegate provides the target rect for menu positioning.
pub fn define_edit_menu_interaction_delegate() -> *const Class {
    let mut decl = ClassDecl::new("MakepadEditMenuDelegate", class!(NSObject)).unwrap();

    // Store a reference to the MTKView for accessing menu rect
    decl.add_ivar::<*mut c_void>("mtk_view");

    // editMenuInteraction:targetRectForConfiguration:
    // Returns the rect where the menu should point to (selection rect)
    extern "C" fn target_rect_for_configuration(
        this: &Object,
        _: Sel,
        _interaction: ObjcId,
        _configuration: ObjcId,
    ) -> NSRect {
        unsafe {
            let mtk_view: *mut c_void = *this.get_ivar("mtk_view");
            if mtk_view.is_null() {
                return NSRect {
                    origin: NSPoint { x: 0.0, y: 0.0 },
                    size: NSSize {
                        width: 1.0,
                        height: 1.0,
                    },
                };
            }
            let view = mtk_view as ObjcId;
            let x: f64 = *(*view).get_ivar("menu_rect_x");
            let y: f64 = *(*view).get_ivar("menu_rect_y");
            let width: f64 = *(*view).get_ivar("menu_rect_width");
            let height: f64 = *(*view).get_ivar("menu_rect_height");
            NSRect {
                origin: NSPoint { x, y },
                size: NSSize { width, height },
            }
        }
    }

    unsafe {
        // UIEditMenuInteractionDelegate method: editMenuInteraction:targetRectForConfiguration:
        decl.add_method(
            sel!(editMenuInteraction:targetRectForConfiguration:),
            target_rect_for_configuration as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> NSRect,
        );
    }

    return decl.register();
}
