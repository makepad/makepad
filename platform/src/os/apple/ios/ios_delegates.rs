use crate::{
    event::{Ease, SelectionHandleKind, SelectionHandlePhase, TouchState, VirtualKeyboardEvent},
    makepad_math::*,
    os::{
        apple::apple_sys::*,
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

pub fn define_makepad_view_controller() -> *const Class {
    let superclass = class!(UIViewController);
    let mut decl = ClassDecl::new("MakepadViewController", superclass).unwrap();

    decl.add_ivar::<BOOL>("_prefersStatusBarHidden");
    decl.add_ivar::<BOOL>("_prefersHomeIndicatorAutoHidden");

    extern "C" fn prefers_status_bar_hidden(this: &Object, _: Sel) -> BOOL {
        unsafe { *this.get_ivar("_prefersStatusBarHidden") }
    }

    extern "C" fn prefers_home_indicator_auto_hidden(this: &Object, _: Sel) -> BOOL {
        unsafe { *this.get_ivar("_prefersHomeIndicatorAutoHidden") }
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

    unsafe {
        decl.add_method(
            sel!(application: didFinishLaunchingWithOptions:),
            did_finish_launching_with_options
                as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> BOOL,
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
    let mut decl =
        ClassDecl::new("SelectionHandlePanRecognizerHandler", class!(NSObject)).unwrap();

    decl.add_ivar::<i64>("handle_kind");

    extern "C" fn handle_selection_handle_pan(
        this: &Object,
        _: Sel,
        gesture_recognizer: ObjcId,
    ) {
        unsafe {
            let state: i64 = msg_send![gesture_recognizer, state];
            let phase = match state {
                1 => Some(SelectionHandlePhase::Begin),  // UIGestureRecognizerStateBegan
                2 => Some(SelectionHandlePhase::Move),   // UIGestureRecognizerStateChanged
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

    // Keyboard notification helpers - used for resizing the canvas when keyboard is shown/hidden
    fn get_height_delta(notif: ObjcId) -> f64 {
        unsafe {
            let info: ObjcId = msg_send![notif, userInfo];
            let obj: ObjcId = msg_send![info, objectForKey: UIKeyboardFrameBeginUserInfoKey];
            let begin: NSRect = msg_send![obj, CGRectValue];
            let obj: ObjcId = msg_send![info, objectForKey: UIKeyboardFrameEndUserInfoKey];
            let end: NSRect = msg_send![obj, CGRectValue];
            begin.origin.y - end.origin.y
        }
    }
    fn get_curve_duration(notif: ObjcId) -> (f64, Ease) {
        unsafe {
            let info: ObjcId = msg_send![notif, userInfo];
            let obj: ObjcId = msg_send![info, objectForKey: UIKeyboardAnimationDurationUserInfoKey];
            let duration: f64 = msg_send![obj, doubleValue];
            let obj: ObjcId = msg_send![info, objectForKey: UIKeyboardAnimationCurveUserInfoKey];
            let curve: i64 = msg_send![obj, intValue];

            let ease = match curve >> 16 {
                // UIViewAnimationOptionCurveEaseInOut - approximated with bezier
                0 => Ease::Bezier {
                    cp0: 0.25,
                    cp1: 0.1,
                    cp2: 0.25,
                    cp3: 0.1,
                },
                1 => Ease::InExp,  //UIViewAnimationOptionCurveEaseIn = 1 << 16,
                2 => Ease::OutExp, //UIViewAnimationOptionCurveEaseOut = 2 << 16,
                _ => Ease::Linear, //UIViewAnimationOptionCurveLinear = 3 << 16,
            };
            (duration, ease)
        }
    }

    // Required stubs for keyboard frame change notifications (registered with notification center)
    extern "C" fn keyboard_did_change_frame(_: &Object, _: Sel, _notif: ObjcId) {}
    extern "C" fn keyboard_will_change_frame(_: &Object, _: Sel, _notif: ObjcId) {}

    extern "C" fn keyboard_will_hide(_: &Object, _: Sel, notif: ObjcId) {
        // Get notification data OUTSIDE the borrow
        let height = get_height_delta(notif);
        let (duration, ease) = get_curve_duration(notif);
        // Now borrow to get time and queue event
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| {
                app.queue_virtual_keyboard_event(VirtualKeyboardEvent::WillHide {
                    time,
                    ease,
                    height: -height,
                    duration,
                })
            });
        }
    }

    extern "C" fn keyboard_did_hide(_: &Object, _: Sel, _notif: ObjcId) {
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| {
                app.queue_virtual_keyboard_event(VirtualKeyboardEvent::DidHide { time })
            });
        }
    }

    extern "C" fn keyboard_will_show(_: &Object, _: Sel, notif: ObjcId) {
        // Get notification data OUTSIDE the borrow
        let height = get_height_delta(notif);
        let (duration, ease) = get_curve_duration(notif);
        // Now borrow to get time and queue event
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| {
                app.queue_virtual_keyboard_event(VirtualKeyboardEvent::WillShow {
                    time,
                    height,
                    ease,
                    duration,
                })
            });
        }
    }

    extern "C" fn keyboard_did_show(_: &Object, _: Sel, notif: ObjcId) {
        // Get notification data OUTSIDE the borrow
        let height = get_height_delta(notif);
        // Now borrow to get time and queue event
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| {
                app.queue_virtual_keyboard_event(VirtualKeyboardEvent::DidShow { time, height })
            });
        }
    }
    extern "C" fn input_mode_did_change(_: &Object, _: Sel, _notif: ObjcId) {
        // When keyboard language changes, reload input views so iOS re-queries
        // autocorrectionType (which dynamically checks CJK vs non-CJK)
        try_with_ios_app(|app| {
            if let Some(text_input_view) = app.text_input_view {
                unsafe {
                    let () = msg_send![text_input_view, reloadInputViews];
                }
            }
        });
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
