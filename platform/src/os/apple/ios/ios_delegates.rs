use {
    crate::{
        makepad_math::*,
        event::{TouchState, VirtualKeyboardEvent},
        animator::Ease,
        os::{
            apple::ios_app::IosApp,
            apple::apple_util::{nsstring_to_string, str_to_nsstring},
            apple::apple_sys::*,
            apple::ios_app::{with_ios_app, get_ios_class_global, IOS_APP},
        },
    },
    makepad_objc_sys::runtime::Protocol,
};

/// Count UTF-16 code units in a Rust string.
/// This is needed because iOS NSString uses UTF-16 internally, and UITextInput
/// positions are measured in UTF-16 code units, not characters or bytes.
/// For ASCII: 1 char = 1 UTF-16 code unit
/// For most Unicode (including CJK): 1 char = 1 UTF-16 code unit
/// For emoji and other astral plane characters: 1 char = 2 UTF-16 code units (surrogate pair)
fn utf16_len(s: &str) -> i64 {
    s.encode_utf16().count() as i64
}

/// Convert a UTF-16 code unit index to a character index.
/// This is needed because iOS sends UTF-16 indices but Makepad uses character indices.
fn utf16_index_to_char_index(s: &str, utf16_index: usize) -> usize {
    let mut char_index = 0;
    let mut utf16_pos = 0;
    for c in s.chars() {
        let c_len = c.len_utf16();
        if utf16_pos + c_len > utf16_index {
            break;
        }
        utf16_pos += c_len;
        char_index += 1;
    }
    char_index
}

// Helper to safely access IosApp without causing re-entrant borrow panics.
// Returns None if we're already inside a with_ios_app call.
// This is critical for callbacks from UIKit that can occur during borrows.
fn try_with_ios_app<R>(f: impl FnOnce(&mut crate::os::apple::ios::ios_app::IosApp) -> R) -> Option<R> {
    IOS_APP.try_with(|app| {
        if let Ok(mut app_ref) = app.try_borrow_mut() {
            if let Some(app) = app_ref.as_mut() {
                return Some(f(app));
            }
        }
        None
    }).ok().flatten()
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
                }
                else {
                    touch_id as u64
                };
                let p: NSPoint = msg_send![ios_touch, locationInView: this];

                // Get touch radius and force from UITouch
                // majorRadius is in points, representing the radius of the touch area
                let major_radius: f64 = msg_send![ios_touch, majorRadius];
                let force: f64 = msg_send![ios_touch, force];

                with_ios_app(|app| app.update_touch_with_details(
                    uid,
                    dvec2(p.x, p.y),
                    state,
                    dvec2(major_radius, major_radius),
                    force
                ));
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
    
    extern "C" fn draw_size_will_change(_this: &Object, _: Sel, _: ObjcId, _: ObjcId) {
        crate::log!("Draw size will change");
        IosApp::draw_size_will_change();
    }
    unsafe {
        decl.add_method(
            sel!(drawInMTKView:),
            draw_in_rect as extern "C" fn(&Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(mtkView: drawableSizeWillChange:),
            draw_size_will_change as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
    }
    
    decl.add_ivar::<*mut c_void>("display_ptr");
    return decl.register();
}

/// Defines a class that acts as the target "receiver" for the long press gesture recognizer's
/// "gesture recognized" action.
pub fn define_gesture_recognizer_handler() -> *const Class {
    let mut decl = ClassDecl::new("LongPressGestureRecognizerHandler", class!(NSObject)).unwrap();

    extern "C" fn handle_long_press_gesture(_this: &Object, _: Sel, gesture_recognizer: ObjcId, _: ObjcId) {
        unsafe {
            let state: i64 = msg_send![gesture_recognizer, state];
            // One might expect that we want to trigger on the "Recognized" or "Ended" state,
            // but that state is not triggered until the user lifts their finger.
            // We want to trigger on the "Began" state, which occurs only once the user has long-pressed
            // for a long-enough time interval to trigger the gesture (without having to lift their finger).
            if state == 1 { // UIGestureRecognizerStateBegan
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
        decl.add_method(sel!(receivedTimer:), received_timer as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(receivedLiveResize:), received_live_resize as extern "C" fn(&Object, Sel, ObjcId));
    }
    
    return decl.register();
}

pub fn define_textfield_delegate() -> *const Class {
    let mut decl = ClassDecl::new("NSTextFieldDlg", class!(NSObject)).unwrap();
    
    // those 3 callbacks are for resizing the canvas when keyboard is opened
    // which is not currenlty supported by miniquad
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
                0 => Ease::Bezier { // this is not the right curve.
                    cp0: 0.25,
                    cp1: 0.1,
                    cp2: 0.25,
                    cp3: 0.1
                }, //::UIViewAnimationOptionCurveEaseInOut = 0 << 16,
                1 => Ease::InExp, //UIViewAnimationOptionCurveEaseIn = 1 << 16,
                2 => Ease::OutExp, //UIViewAnimationOptionCurveEaseOut = 2 << 16,
                _ => Ease::Linear //UIViewAnimationOptionCurveLinear = 3 << 16,
            };
            (duration, ease)
        }
    }
    
    // Stubs for keyboard frame change notifications - could be used for finer-grained
    // keyboard tracking in the future (e.g., during interactive keyboard dismissal)
    extern "C" fn keyboard_did_change_frame(_: &Object, _: Sel, _notif: ObjcId) {}
    extern "C" fn keyboard_will_change_frame(_: &Object, _: Sel, _notif: ObjcId) {}

    extern "C" fn keyboard_will_hide(_: &Object, _: Sel, notif: ObjcId) {
        // Get notification data OUTSIDE the borrow
        let height = get_height_delta(notif);
        let (duration, ease) = get_curve_duration(notif);
        // Now borrow to get time and queue event
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| app.queue_virtual_keyboard_event(VirtualKeyboardEvent::WillHide {
                time,
                ease,
                height: -height,
                duration
            }));
        }
    }

    extern "C" fn keyboard_did_hide(_: &Object, _: Sel, _notif: ObjcId) {
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| app.queue_virtual_keyboard_event(VirtualKeyboardEvent::DidHide {
                time,
            }));
        }
    }

    extern "C" fn keyboard_will_show(_: &Object, _: Sel, notif: ObjcId) {
        // Get notification data OUTSIDE the borrow
        let height = get_height_delta(notif);
        let (duration, ease) = get_curve_duration(notif);
        // Now borrow to get time and queue event
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| app.queue_virtual_keyboard_event(VirtualKeyboardEvent::WillShow {
                time,
                height,
                ease,
                duration
            }));
        }
    }

    extern "C" fn keyboard_did_show(_: &Object, _: Sel, notif: ObjcId) {
        // Get notification data OUTSIDE the borrow
        let height = get_height_delta(notif);
        // Now borrow to get time and queue event
        if let Some(time) = try_with_ios_app(|app| app.time_now()) {
            try_with_ios_app(|app| app.queue_virtual_keyboard_event(VirtualKeyboardEvent::DidShow {
                time,
                height,
            }));
        }
    }
    extern "C" fn should_change_characters_in_range(
        _this: &Object,
        _: Sel,
        _textfield: ObjcId,
        range: NSRange,
        string: ObjcId,
    ) -> BOOL {
        unsafe {
            let len: u64 = msg_send![string, length];
            if len > 0 {
                let string = nsstring_to_string(string);
                IosApp::send_text_input(string, range.length != 0);
            } else {
                IosApp::send_backspace();
            }
        }
        NO
    }
    
    unsafe {
        decl.add_method(sel!(keyboardDidChangeFrame:), keyboard_did_change_frame as extern "C" fn(&Object, Sel, ObjcId),);
        decl.add_method(sel!(keyboardWillChangeFrame:), keyboard_will_change_frame as extern "C" fn(&Object, Sel, ObjcId),);
        decl.add_method(sel!(keyboardWillShow:), keyboard_will_show as extern "C" fn(&Object, Sel, ObjcId),);
        decl.add_method(sel!(keyboardDidShow:), keyboard_did_show as extern "C" fn(&Object, Sel, ObjcId),);
        decl.add_method(sel!(keyboardWillHide:), keyboard_will_hide as extern "C" fn(&Object, Sel, ObjcId),);
        decl.add_method(sel!(keyboardDidHide:), keyboard_did_hide as extern "C" fn(&Object, Sel, ObjcId),);
        decl.add_method(
            sel!(textField: shouldChangeCharactersInRange: replacementString:),
            should_change_characters_in_range
            as extern "C" fn(&Object, Sel, ObjcId, NSRange, ObjcId) -> BOOL,
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
                    size: NSSize { width: 1.0, height: 1.0 },
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

// =============================================================================
// UITextInput Protocol Implementation for IME Support
// =============================================================================

/// Defines a custom UITextPosition subclass.
/// UITextInput protocol requires custom position/range classes (token-based, not integer-based).
pub fn define_makepad_text_position() -> *const Class {
    let mut decl = ClassDecl::new("MakepadTextPosition", class!(UITextPosition)).unwrap();

    decl.add_ivar::<i64>("_offset");

    extern "C" fn get_offset(this: &Object, _: Sel) -> i64 {
        unsafe { *this.get_ivar::<i64>("_offset") }
    }

    extern "C" fn set_offset(this: &mut Object, _: Sel, offset: i64) {
        unsafe { this.set_ivar::<i64>("_offset", offset); }
    }

    extern "C" fn position_with_offset(cls: &Class, _: Sel, offset: i64) -> ObjcId {
        unsafe {
            let obj: ObjcId = msg_send![cls, alloc];
            let obj: ObjcId = msg_send![obj, init];
            (*obj).set_ivar::<i64>("_offset", offset);
            let obj: ObjcId = msg_send![obj, autorelease];
            obj
        }
    }

    unsafe {
        decl.add_method(sel!(offset), get_offset as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(sel!(setOffset:), set_offset as extern "C" fn(&mut Object, Sel, i64));
        decl.add_class_method(
            sel!(positionWithOffset:),
            position_with_offset as extern "C" fn(&Class, Sel, i64) -> ObjcId,
        );
    }

    return decl.register();
}

/// Defines a custom UITextRange subclass.
/// Stores start and end offsets directly, creates positions on demand.
pub fn define_makepad_text_range() -> *const Class {
    let mut decl = ClassDecl::new("MakepadTextRange", class!(UITextRange)).unwrap();

    // Store offsets directly instead of position objects to avoid memory management issues
    decl.add_ivar::<i64>("_startOffset");
    decl.add_ivar::<i64>("_endOffset");

    extern "C" fn get_start(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let offset: i64 = *this.get_ivar::<i64>("_startOffset");
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: offset]
        }
    }

    extern "C" fn get_end(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let offset: i64 = *this.get_ivar::<i64>("_endOffset");
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: offset]
        }
    }

    extern "C" fn is_empty(this: &Object, _: Sel) -> BOOL {
        unsafe {
            let start: i64 = *this.get_ivar::<i64>("_startOffset");
            let end: i64 = *this.get_ivar::<i64>("_endOffset");
            if start == end { YES } else { NO }
        }
    }

    // Simplified class method using the class parameter directly
    extern "C" fn range_with_start_end(cls: &Class, _: Sel, start: i64, end: i64) -> ObjcId {
        unsafe {
            let obj: ObjcId = msg_send![cls, alloc];
            let obj: ObjcId = msg_send![obj, init];
            (*obj).set_ivar::<i64>("_startOffset", start);
            (*obj).set_ivar::<i64>("_endOffset", end);
            let obj: ObjcId = msg_send![obj, autorelease];
            obj
        }
    }

    extern "C" fn range_with_positions(cls: &Class, _: Sel, start: ObjcId, end: ObjcId) -> ObjcId {
        unsafe {
            let obj: ObjcId = msg_send![cls, alloc];
            let obj: ObjcId = msg_send![obj, init];
            let start_offset: i64 = if start != nil { msg_send![start, offset] } else { 0 };
            let end_offset: i64 = if end != nil { msg_send![end, offset] } else { 0 };
            (*obj).set_ivar::<i64>("_startOffset", start_offset);
            (*obj).set_ivar::<i64>("_endOffset", end_offset);
            let obj: ObjcId = msg_send![obj, autorelease];
            obj
        }
    }

    unsafe {
        decl.add_method(sel!(start), get_start as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(sel!(end), get_end as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(sel!(isEmpty), is_empty as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_class_method(
            sel!(rangeWithStart:end:),
            range_with_start_end as extern "C" fn(&Class, Sel, i64, i64) -> ObjcId,
        );
        decl.add_class_method(
            sel!(rangeWithStartPosition:endPosition:),
            range_with_positions as extern "C" fn(&Class, Sel, ObjcId, ObjcId) -> ObjcId,
        );
    }

    return decl.register();
}

/// Defines the main text input view conforming to UITextInput protocol.
/// This replaces the hidden UITextField and provides full IME support.
pub fn define_text_input_view() -> *const Class {
    let mut decl = ClassDecl::new("MakepadTextInputView", class!(UIView)).unwrap();

    // Instance variables for text input state
    decl.add_ivar::<ObjcId>("markedText");           // NSMutableAttributedString
    decl.add_ivar::<ObjcId>("textBuffer");           // NSMutableString - tracks text for iOS context
    decl.add_ivar::<i64>("cursorPosition");          // Current cursor position
    decl.add_ivar::<i64>("selectionStart");          // Selection start
    decl.add_ivar::<i64>("selectionEnd");            // Selection end
    decl.add_ivar::<ObjcId>("_inputDelegate");       // id<UITextInputDelegate>
    decl.add_ivar::<ObjcId>("_tokenizer");           // UITextInputStringTokenizer
    // IME position stored in ivars to avoid re-entrant borrow issues
    decl.add_ivar::<f64>("ime_pos_x");
    decl.add_ivar::<f64>("ime_pos_y");

    // ==========================================================================
    // UIResponder methods
    // ==========================================================================

    extern "C" fn can_become_first_responder(_: &Object, _: Sel) -> BOOL {
        YES
    }

    // ==========================================================================
    // UIKeyInput protocol methods
    // ==========================================================================

    extern "C" fn has_text(_this: &Object, _: Sel) -> BOOL {
        // Always return YES so iOS keeps sending deleteBackward events
        // even when our buffer is empty (user might be deleting initial text
        // that we don't track)
        YES
    }

    // Helper to get or create the text buffer
    unsafe fn get_text_buffer(this: &Object) -> ObjcId {
        let buffer: ObjcId = *this.get_ivar("textBuffer");
        if buffer != nil {
            return buffer;
        }
        // Create new buffer
        let new_buffer: ObjcId = msg_send![class!(NSMutableString), alloc];
        let new_buffer: ObjcId = msg_send![new_buffer, init];
        (*(this as *const _ as *mut Object)).set_ivar("textBuffer", new_buffer);
        new_buffer
    }

    extern "C" fn insert_text(this: &Object, _: Sel, text: ObjcId) {
        unsafe {
            let string = nsstring_to_string(text);

            // Handle Enter/Return key specially
            if string == "\n" {
                let buffer = get_text_buffer(this);
                let () = msg_send![buffer, appendString: text];
                IosApp::send_return_key();
                return;
            }

            // Get inputDelegate for notifications
            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");

            // Notify that text and selection will change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
            }

            // Clear marked text BEFORE sending to Makepad
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text != nil {
                let len: u64 = msg_send![marked_text, length];
                if len > 0 {
                    let mutable_string: ObjcId = msg_send![marked_text, mutableString];
                    let empty = str_to_nsstring("");
                    let () = msg_send![mutable_string, setString: empty];
                }
            }

            // Send the text input event to Makepad
            IosApp::send_text_input(string.clone(), false);

            // Update text buffer - insert at cursor position, not append
            let buffer = get_text_buffer(this);
            let cursor: i64 = *this.get_ivar("cursorPosition");
            let buffer_len: u64 = msg_send![buffer, length];
            let insert_pos = (cursor.max(0) as u64).min(buffer_len);
            let () = msg_send![buffer, insertString: text atIndex: insert_pos];

            // Update cursor position using UTF-16 code units (matches iOS NSString.length)
            let new_cursor = cursor + utf16_len(&string);
            (*(this as *const _ as *mut Object)).set_ivar("cursorPosition", new_cursor);

            // Notify that text and selection did change
            if input_delegate != nil {
                let () = msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
        }
    }

    extern "C" fn delete_backward(this: &Object, _: Sel) {
        unsafe {
            // Update text buffer - remove character BEFORE cursor position
            let buffer = get_text_buffer(this);
            let cursor: i64 = *this.get_ivar("cursorPosition");

            if cursor > 0 {
                let delete_pos = (cursor - 1) as u64;
                let buffer_len: u64 = msg_send![buffer, length];
                if delete_pos < buffer_len {
                    let range = NSRange { location: delete_pos, length: 1 };
                    let () = msg_send![buffer, deleteCharactersInRange: range];
                }
                // Update cursor position
                (*(this as *const _ as *mut Object)).set_ivar("cursorPosition", cursor - 1);
            }
        }
        // Always send backspace - Makepad handles actual text deletion and bounds
        IosApp::send_backspace();
    }

    // ==========================================================================
    // UITextInput protocol - Marked text (composition) methods
    // ==========================================================================

    extern "C" fn has_marked_text(this: &Object, _: Sel) -> BOOL {
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text == nil {
                return NO;
            }
            let len: u64 = msg_send![marked_text, length];
            if len > 0 { YES } else { NO }
        }
    }

    extern "C" fn marked_text_range(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text == nil {
                return nil;
            }
            let len: u64 = msg_send![marked_text, length];
            if len > 0 {
                let cursor: i64 = *this.get_ivar("cursorPosition");
                let range_class = get_ios_class_global().text_range;
                msg_send![range_class, rangeWithStart: cursor end: cursor + (len as i64)]
            } else {
                nil
            }
        }
    }

    extern "C" fn set_marked_text(this: &mut Object, _: Sel, marked_text_input: ObjcId, _selected_range: NSRange) {
        unsafe {
            // Notify inputDelegate that text will change
            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
            }

            let marked_text_ref: &mut ObjcId = this.get_mut_ivar("markedText");

            if *marked_text_ref != nil {
                let () = msg_send![*marked_text_ref, release];
            }

            // Create new marked text storage
            let new_marked: ObjcId = msg_send![class!(NSMutableAttributedString), alloc];

            if marked_text_input != nil {
                let has_attr: BOOL = msg_send![marked_text_input, isKindOfClass: class!(NSAttributedString)];
                if has_attr == YES {
                    let () = msg_send![new_marked, initWithAttributedString: marked_text_input];
                } else {
                    let () = msg_send![new_marked, initWithString: marked_text_input];
                }
            } else {
                let () = msg_send![new_marked, init];
            }

            *marked_text_ref = new_marked;

            // Send marked text to Makepad for inline display
            let text_string: ObjcId = msg_send![new_marked, string];
            let marked_string = nsstring_to_string(text_string);

            // Always send with replace_last=true - empty string clears composition preview
            IosApp::send_text_input(marked_string, true);

            // Notify inputDelegate that text did change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
        }
    }

    extern "C" fn unmark_text(this: &Object, _: Sel) {
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text == nil {
                return;
            }

            let len: u64 = msg_send![marked_text, length];
            if len == 0 {
                return;
            }

            // Get the marked text string
            let text_string: ObjcId = msg_send![marked_text, string];
            let string = nsstring_to_string(text_string);

            // Get inputDelegate for notifications
            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");

            // Notify that text will change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
            }

            // Commit the marked text to Makepad
            IosApp::send_text_input(string.clone(), false);

            // Update text buffer - insert at cursor position, not append
            let buffer = get_text_buffer(this);
            let cursor: i64 = *this.get_ivar("cursorPosition");
            let buffer_len: u64 = msg_send![buffer, length];
            let insert_pos = (cursor.max(0) as u64).min(buffer_len);
            let () = msg_send![buffer, insertString: text_string atIndex: insert_pos];

            // Update cursor position using UTF-16 code units (matches iOS NSString.length)
            let new_cursor = cursor + utf16_len(&string);
            (*(this as *const _ as *mut Object)).set_ivar("cursorPosition", new_cursor);

            // Clear the marked text
            let mutable_string: ObjcId = msg_send![marked_text, mutableString];
            let empty = str_to_nsstring("");
            let () = msg_send![mutable_string, setString: empty];

            // Notify that text did change
            if input_delegate != nil {
                let () = msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
        }
    }

    extern "C" fn marked_text_style(_: &Object, _: Sel) -> ObjcId {
        nil
    }

    extern "C" fn set_marked_text_style(_: &mut Object, _: Sel, _style: ObjcId) {
        // Not implemented - we don't use custom styling
    }

    // ==========================================================================
    // UITextInput protocol - Selection methods
    // ==========================================================================

    extern "C" fn selected_text_range(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let cursor: i64 = *this.get_ivar("cursorPosition");
            let range_class = get_ios_class_global().text_range;
            msg_send![range_class, rangeWithStart: cursor end: cursor]
        }
    }

    extern "C" fn set_selected_text_range(this: &mut Object, _: Sel, range: ObjcId) {
        if range == nil {
            return;
        }
        unsafe {
            let start: ObjcId = msg_send![range, start];
            let end: ObjcId = msg_send![range, end];
            if start != nil && end != nil {
                let start_offset: i64 = msg_send![start, offset];
                let end_offset: i64 = msg_send![end, offset];
                this.set_ivar("selectionStart", start_offset);
                this.set_ivar("selectionEnd", end_offset);
                this.set_ivar("cursorPosition", end_offset);
            }
        }
    }

    // ==========================================================================
    // UITextInput protocol - Text storage methods
    // ==========================================================================

    extern "C" fn text_in_range(this: &Object, _: Sel, range: ObjcId) -> ObjcId {
        if range == nil {
            return str_to_nsstring("");
        }
        unsafe {
            let start: ObjcId = msg_send![range, start];
            let end: ObjcId = msg_send![range, end];

            if start == nil || end == nil {
                return str_to_nsstring("");
            }

            let start_offset: i64 = msg_send![start, offset];
            let end_offset: i64 = msg_send![end, offset];

            if start_offset >= end_offset {
                return str_to_nsstring("");
            }

            let buffer = get_text_buffer(this);
            let buffer_len: u64 = msg_send![buffer, length];
            let cursor: i64 = *this.get_ivar("cursorPosition");

            // Check if this is querying the marked text range
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text != nil {
                let marked_len: u64 = msg_send![marked_text, length];
                if marked_len > 0 {
                    // Marked text is at cursor position
                    let marked_start = cursor;
                    let marked_end = cursor + marked_len as i64;

                    // If query overlaps with marked text range, return marked text
                    if start_offset >= marked_start && end_offset <= marked_end {
                        let text_string: ObjcId = msg_send![marked_text, string];
                        return text_string;
                    }
                }
            }

            // Otherwise return from buffer
            let start_idx = (start_offset.max(0) as u64).min(buffer_len);
            let end_idx = (end_offset.max(0) as u64).min(buffer_len);

            if start_idx >= end_idx {
                return str_to_nsstring("");
            }

            let range = NSRange { location: start_idx, length: end_idx - start_idx };
            msg_send![buffer, substringWithRange: range]
        }
    }

    extern "C" fn replace_range_with_text(this: &Object, _: Sel, range: ObjcId, text: ObjcId) {
        unsafe {
            let new_string = nsstring_to_string(text);

            // Get inputDelegate for notifications
            let input_delegate: ObjcId = *this.get_ivar("_inputDelegate");

            // Notify that text will change
            if input_delegate != nil {
                let () = msg_send![input_delegate, textWillChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, selectionWillChange: this as *const _ as ObjcId];
            }

            // Clear marked text first
            let marked_text: ObjcId = *this.get_ivar("markedText");
            if marked_text != nil {
                let len: u64 = msg_send![marked_text, length];
                if len > 0 {
                    let mutable_string: ObjcId = msg_send![marked_text, mutableString];
                    let empty = str_to_nsstring("");
                    let () = msg_send![mutable_string, setString: empty];
                }
            }

            // Get range bounds by extracting offsets from our custom position objects
            let (range_start, range_end) = if range != nil {
                let start_pos: ObjcId = msg_send![range, start];
                let end_pos: ObjcId = msg_send![range, end];

                let start: i64 = if start_pos != nil {
                    let responds: BOOL = msg_send![start_pos, respondsToSelector: sel!(offset)];
                    if responds == YES { msg_send![start_pos, offset] } else { 0 }
                } else { 0 };

                let end: i64 = if end_pos != nil {
                    let responds: BOOL = msg_send![end_pos, respondsToSelector: sel!(offset)];
                    if responds == YES { msg_send![end_pos, offset] } else { start }
                } else { start };

                (start.max(0) as usize, end.max(0) as usize)
            } else {
                let cursor: i64 = *this.get_ivar("cursorPosition");
                let pos = cursor.max(0) as usize;
                (pos, pos)
            };

            // Update iOS text buffer directly
            let buffer = get_text_buffer(this);
            let buffer_len: u64 = msg_send![buffer, length];
            let buf_start = (range_start as u64).min(buffer_len);
            let buf_end = (range_end as u64).min(buffer_len);

            // Get buffer contents BEFORE modification for UTF-16 to char index conversion
            let buffer_string = nsstring_to_string(buffer);

            // Delete the range from buffer
            if buf_start < buf_end {
                let delete_range = NSRange { location: buf_start, length: buf_end - buf_start };
                let () = msg_send![buffer, deleteCharactersInRange: delete_range];
            }

            // Insert new text at buf_start
            if !new_string.is_empty() {
                let new_buf_len: u64 = msg_send![buffer, length];
                let insert_pos = buf_start.min(new_buf_len);
                let () = msg_send![buffer, insertString: text atIndex: insert_pos];
            }

            // Convert UTF-16 indices to character indices for Makepad
            // iOS uses UTF-16 code units, but Makepad expects character indices
            let char_start = utf16_index_to_char_index(&buffer_string, range_start);
            let char_end = utf16_index_to_char_index(&buffer_string, range_end);

            // Send the range replacement event to Makepad
            IosApp::send_text_range_replace(char_start, char_end, new_string.clone());

            // Update cursor position to end of inserted text (using UTF-16 code units)
            let new_cursor = range_start as i64 + utf16_len(&new_string);
            (*(this as *const _ as *mut Object)).set_ivar("cursorPosition", new_cursor);

            // Notify that text did change
            if input_delegate != nil {
                let () = msg_send![input_delegate, selectionDidChange: this as *const _ as ObjcId];
                let () = msg_send![input_delegate, textDidChange: this as *const _ as ObjcId];
            }
        }
    }

    // ==========================================================================
    // UITextInput protocol - Position/Range methods
    // ==========================================================================

    extern "C" fn beginning_of_document(_: &Object, _: Sel) -> ObjcId {
        unsafe {
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: 0i64]
        }
    }

    extern "C" fn end_of_document(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            // Return the actual buffer length. The iOS text buffer is kept in sync
            // with text input operations, so this should match reality.
            let buffer = get_text_buffer(this);
            let buffer_len: i64 = msg_send![buffer, length];
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: buffer_len]
        }
    }

    extern "C" fn position_from_position_offset(_: &Object, _: Sel, position: ObjcId, offset: i64) -> ObjcId {
        if position == nil {
            return nil;
        }
        unsafe {
            let pos: i64 = msg_send![position, offset];
            let new_pos = pos + offset;
            if new_pos < 0 {
                return nil;
            }
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: new_pos]
        }
    }

    extern "C" fn position_from_position_in_direction_offset(
        _: &Object,
        _: Sel,
        position: ObjcId,
        direction: i64,
        offset: i64,
    ) -> ObjcId {
        if position == nil {
            return nil;
        }
        unsafe {
            let pos: i64 = msg_send![position, offset];
            // UITextLayoutDirection: 0=right, 1=left, 2=up, 3=down
            let actual_offset = if direction == 1 { -offset } else { offset };
            let new_pos = pos + actual_offset;
            if new_pos < 0 {
                return nil;
            }
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: new_pos]
        }
    }

    extern "C" fn text_range_from_position_to_position(
        _: &Object,
        _: Sel,
        from_position: ObjcId,
        to_position: ObjcId,
    ) -> ObjcId {
        if from_position == nil || to_position == nil {
            return nil;
        }
        unsafe {
            let range_class = get_ios_class_global().text_range;
            msg_send![range_class, rangeWithStartPosition: from_position endPosition: to_position]
        }
    }

    extern "C" fn compare_position_to_position(
        _: &Object,
        _: Sel,
        position: ObjcId,
        other: ObjcId,
    ) -> i64 {
        if position == nil || other == nil {
            return 0; // NSOrderedSame
        }
        unsafe {
            let pos1: i64 = msg_send![position, offset];
            let pos2: i64 = msg_send![other, offset];
            if pos1 < pos2 {
                -1 // NSOrderedAscending
            } else if pos1 > pos2 {
                1 // NSOrderedDescending
            } else {
                0 // NSOrderedSame
            }
        }
    }

    extern "C" fn offset_from_position_to_position(
        _: &Object,
        _: Sel,
        from: ObjcId,
        to: ObjcId,
    ) -> i64 {
        if from == nil || to == nil {
            return 0;
        }
        unsafe {
            let from_pos: i64 = msg_send![from, offset];
            let to_pos: i64 = msg_send![to, offset];
            to_pos - from_pos
        }
    }

    extern "C" fn position_within_range_farthest_in_direction(
        _: &Object,
        _: Sel,
        range: ObjcId,
        direction: i64,
    ) -> ObjcId {
        if range == nil {
            return nil;
        }
        unsafe {
            // UITextLayoutDirection: 0=right, 1=left, 2=up, 3=down
            // For left/up, return start; for right/down, return end
            if direction == 1 || direction == 2 {
                msg_send![range, start]
            } else {
                msg_send![range, end]
            }
        }
    }

    extern "C" fn character_range_by_extending_position_in_direction(
        _: &Object,
        _: Sel,
        position: ObjcId,
        _direction: i64,
    ) -> ObjcId {
        if position == nil {
            return nil;
        }
        // Return a zero-width range at the position
        unsafe {
            let range_class = get_ios_class_global().text_range;
            msg_send![range_class, rangeWithStartPosition: position endPosition: position]
        }
    }

    // ==========================================================================
    // UITextInput protocol - Geometry methods
    // ==========================================================================

    extern "C" fn first_rect_for_range(this: &Object, _: Sel, _range: ObjcId) -> NSRect {
        // Return a rect at the IME position for candidate window placement
        // Read directly from ivars to avoid any re-entrant borrow issues
        unsafe {
            let x: f64 = *this.get_ivar("ime_pos_x");
            let y: f64 = *this.get_ivar("ime_pos_y");

            let view = this as *const _ as ObjcId;
            let view_frame: NSRect = msg_send![view, frame];

            NSRect {
                origin: NSPoint { x, y: view_frame.size.height - y },
                size: NSSize { width: 1.0, height: 20.0 },
            }
        }
    }

    extern "C" fn caret_rect_for_position(this: &Object, sel: Sel, _position: ObjcId) -> NSRect {
        first_rect_for_range(this, sel, nil)
    }

    extern "C" fn selection_rects_for_range(_: &Object, _: Sel, _range: ObjcId) -> ObjcId {
        // Return empty array
        unsafe { msg_send![class!(NSArray), array] }
    }

    extern "C" fn closest_position_to_point(_: &Object, _: Sel, _point: NSPoint) -> ObjcId {
        // Return position 0 - we don't do hit testing
        unsafe {
            let pos_class = get_ios_class_global().text_position;
            msg_send![pos_class, positionWithOffset: 0i64]
        }
    }

    extern "C" fn closest_position_to_point_within_range(
        _: &Object,
        _: Sel,
        _point: NSPoint,
        range: ObjcId,
    ) -> ObjcId {
        if range == nil {
            return nil;
        }
        // Return start of range
        unsafe { msg_send![range, start] }
    }

    extern "C" fn character_range_at_point(_: &Object, _: Sel, _point: NSPoint) -> ObjcId {
        nil
    }

    // ==========================================================================
    // UITextInput protocol - Writing direction (required methods)
    // ==========================================================================

    extern "C" fn base_writing_direction_for_position(
        _: &Object,
        _: Sel,
        _position: ObjcId,
        _direction: i64,
    ) -> i64 {
        0 // NSWritingDirectionNatural
    }

    extern "C" fn set_base_writing_direction(
        _: &Object,
        _: Sel,
        _direction: i64,
        _range: ObjcId,
    ) {
        // No-op - we don't support changing writing direction
    }

    // ==========================================================================
    // UITextInput protocol - Input delegate and tokenizer
    // ==========================================================================

    extern "C" fn input_delegate(this: &Object, _: Sel) -> ObjcId {
        unsafe { *this.get_ivar("_inputDelegate") }
    }

    extern "C" fn set_input_delegate(this: &mut Object, _: Sel, delegate: ObjcId) {
        unsafe { this.set_ivar("_inputDelegate", delegate); }
    }

    extern "C" fn tokenizer(this: &Object, _: Sel) -> ObjcId {
        unsafe {
            let tok: ObjcId = *this.get_ivar("_tokenizer");
            if tok != nil {
                return tok;
            }
            // Create tokenizer on first access
            let view = this as *const _ as ObjcId;
            let new_tok: ObjcId = msg_send![class!(UITextInputStringTokenizer), alloc];
            let new_tok: ObjcId = msg_send![new_tok, initWithTextInput: view];
            (*(this as *const _ as *mut Object)).set_ivar("_tokenizer", new_tok);
            new_tok
        }
    }

    // ==========================================================================
    // UITextInputTraits protocol
    // ==========================================================================

    extern "C" fn keyboard_type(_: &Object, _: Sel) -> i64 {
        0 // UIKeyboardTypeDefault
    }

    extern "C" fn autocorrection_type(_this: &Object, _: Sel) -> i64 {
        unsafe {
            // Try to get current input mode to check keyboard language
            let input_mode: ObjcId = msg_send![class!(UITextInputMode), currentInputMode];
            if input_mode != nil {
                let primary_lang: ObjcId = msg_send![input_mode, primaryLanguage];
                if primary_lang != nil {
                    let lang = nsstring_to_string(primary_lang);
                    // Disable autocorrect for CJK languages (composition-based IME)
                    if lang.starts_with("zh")      // Chinese
                        || lang.starts_with("ja")  // Japanese
                        || lang.starts_with("ko")  // Korean
                    {
                        return 1; // UITextAutocorrectionTypeNo
                    }
                }
            }
            0 // UITextAutocorrectionTypeDefault
        }
    }

    extern "C" fn autocapitalization_type(_: &Object, _: Sel) -> i64 {
        2 // UITextAutocapitalizationTypeSentences
    }

    extern "C" fn spell_checking_type(_: &Object, _: Sel) -> i64 {
        0 // UITextSpellCheckingTypeDefault
    }

    extern "C" fn smart_quotes_type(_: &Object, _: Sel) -> i64 {
        1 // UITextSmartQuotesTypeNo - disable for code/text editing
    }

    extern "C" fn smart_dashes_type(_: &Object, _: Sel) -> i64 {
        1 // UITextSmartDashesTypeNo
    }

    extern "C" fn smart_insert_delete_type(_: &Object, _: Sel) -> i64 {
        1 // UITextSmartInsertDeleteTypeNo
    }

    extern "C" fn keyboard_appearance(_: &Object, _: Sel) -> i64 {
        0 // UIKeyboardAppearanceDefault
    }

    extern "C" fn return_key_type(_: &Object, _: Sel) -> i64 {
        0 // UIReturnKeyDefault
    }

    extern "C" fn enables_return_key_automatically(_: &Object, _: Sel) -> BOOL {
        NO
    }

    extern "C" fn is_secure_text_entry(_: &Object, _: Sel) -> BOOL {
        NO
    }

    extern "C" fn text_content_type(_: &Object, _: Sel) -> ObjcId {
        nil // No specific content type
    }

    // ==========================================================================
    // Register all methods
    // ==========================================================================

    unsafe {
        // UIResponder
        decl.add_method(
            sel!(canBecomeFirstResponder),
            can_become_first_responder as extern "C" fn(&Object, Sel) -> BOOL,
        );

        // UIKeyInput
        decl.add_method(sel!(hasText), has_text as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(insertText:), insert_text as extern "C" fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(deleteBackward), delete_backward as extern "C" fn(&Object, Sel));

        // UITextInput - Marked text
        decl.add_method(sel!(hasMarkedText), has_marked_text as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(markedTextRange), marked_text_range as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(
            sel!(setMarkedText:selectedRange:),
            set_marked_text as extern "C" fn(&mut Object, Sel, ObjcId, NSRange),
        );
        decl.add_method(sel!(unmarkText), unmark_text as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(markedTextStyle), marked_text_style as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(
            sel!(setMarkedTextStyle:),
            set_marked_text_style as extern "C" fn(&mut Object, Sel, ObjcId),
        );

        // UITextInput - Selection
        decl.add_method(sel!(selectedTextRange), selected_text_range as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(
            sel!(setSelectedTextRange:),
            set_selected_text_range as extern "C" fn(&mut Object, Sel, ObjcId),
        );

        // UITextInput - Text storage
        decl.add_method(sel!(textInRange:), text_in_range as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId);
        decl.add_method(
            sel!(replaceRange:withText:),
            replace_range_with_text as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );

        // UITextInput - Position/Range
        decl.add_method(sel!(beginningOfDocument), beginning_of_document as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(sel!(endOfDocument), end_of_document as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(
            sel!(positionFromPosition:offset:),
            position_from_position_offset as extern "C" fn(&Object, Sel, ObjcId, i64) -> ObjcId,
        );
        decl.add_method(
            sel!(positionFromPosition:inDirection:offset:),
            position_from_position_in_direction_offset as extern "C" fn(&Object, Sel, ObjcId, i64, i64) -> ObjcId,
        );
        decl.add_method(
            sel!(textRangeFromPosition:toPosition:),
            text_range_from_position_to_position as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> ObjcId,
        );
        decl.add_method(
            sel!(comparePosition:toPosition:),
            compare_position_to_position as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> i64,
        );
        decl.add_method(
            sel!(offsetFromPosition:toPosition:),
            offset_from_position_to_position as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> i64,
        );
        decl.add_method(
            sel!(positionWithinRange:farthestInDirection:),
            position_within_range_farthest_in_direction as extern "C" fn(&Object, Sel, ObjcId, i64) -> ObjcId,
        );
        decl.add_method(
            sel!(characterRangeByExtendingPosition:inDirection:),
            character_range_by_extending_position_in_direction as extern "C" fn(&Object, Sel, ObjcId, i64) -> ObjcId,
        );

        // UITextInput - Geometry
        decl.add_method(
            sel!(firstRectForRange:),
            first_rect_for_range as extern "C" fn(&Object, Sel, ObjcId) -> NSRect,
        );
        decl.add_method(
            sel!(caretRectForPosition:),
            caret_rect_for_position as extern "C" fn(&Object, Sel, ObjcId) -> NSRect,
        );
        decl.add_method(
            sel!(selectionRectsForRange:),
            selection_rects_for_range as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId,
        );
        decl.add_method(
            sel!(closestPositionToPoint:),
            closest_position_to_point as extern "C" fn(&Object, Sel, NSPoint) -> ObjcId,
        );
        decl.add_method(
            sel!(closestPositionToPoint:withinRange:),
            closest_position_to_point_within_range as extern "C" fn(&Object, Sel, NSPoint, ObjcId) -> ObjcId,
        );
        decl.add_method(
            sel!(characterRangeAtPoint:),
            character_range_at_point as extern "C" fn(&Object, Sel, NSPoint) -> ObjcId,
        );

        // UITextInput - Writing direction
        decl.add_method(
            sel!(baseWritingDirectionForPosition:inDirection:),
            base_writing_direction_for_position as extern "C" fn(&Object, Sel, ObjcId, i64) -> i64,
        );
        decl.add_method(
            sel!(setBaseWritingDirection:forRange:),
            set_base_writing_direction as extern "C" fn(&Object, Sel, i64, ObjcId),
        );

        // UITextInput - Delegate and tokenizer
        decl.add_method(sel!(inputDelegate), input_delegate as extern "C" fn(&Object, Sel) -> ObjcId);
        decl.add_method(
            sel!(setInputDelegate:),
            set_input_delegate as extern "C" fn(&mut Object, Sel, ObjcId),
        );
        decl.add_method(sel!(tokenizer), tokenizer as extern "C" fn(&Object, Sel) -> ObjcId);

        // UITextInputTraits
        decl.add_method(sel!(keyboardType), keyboard_type as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(sel!(autocorrectionType), autocorrection_type as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(sel!(autocapitalizationType), autocapitalization_type as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(sel!(spellCheckingType), spell_checking_type as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(sel!(smartQuotesType), smart_quotes_type as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(sel!(smartDashesType), smart_dashes_type as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(sel!(smartInsertDeleteType), smart_insert_delete_type as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(sel!(keyboardAppearance), keyboard_appearance as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(sel!(returnKeyType), return_key_type as extern "C" fn(&Object, Sel) -> i64);
        decl.add_method(
            sel!(enablesReturnKeyAutomatically),
            enables_return_key_automatically as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(sel!(isSecureTextEntry), is_secure_text_entry as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(textContentType), text_content_type as extern "C" fn(&Object, Sel) -> ObjcId);
    }

    // Protocol conformance
    if let Some(protocol) = Protocol::get("UIKeyInput") {
        decl.add_protocol(protocol);
    }
    if let Some(protocol) = Protocol::get("UITextInput") {
        decl.add_protocol(protocol);
    }

    return decl.register();
}
