use {
    crate::{
        makepad_math::*,
        event::{TouchState, VirtualKeyboardEvent},
        animator::Ease,
        os::{
            apple::ios_app::IosApp,
            apple::apple_util::nsstring_to_string,
            apple::apple_sys::*,
            apple::ios_app::with_ios_app,
        },
    }
};


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
    extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
        YES
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
    let mut decl = ClassDecl::new("NSTexfieldDlg", class!(NSObject)).unwrap();
    
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
    
    extern "C" fn keyboard_did_change_frame(_: &Object, _: Sel, _notif: ObjcId) {
    }
    
    extern "C" fn keyboard_will_change_frame(_: &Object, _: Sel, _notif: ObjcId) {
    }
    
    extern "C" fn keyboard_will_hide(_: &Object, _: Sel, notif: ObjcId) {
        let height = get_height_delta(notif);
        let (duration, ease) = get_curve_duration(notif);
        let time = with_ios_app(|app| app.time_now());
        with_ios_app(|app| app.queue_virtual_keyboard_event(VirtualKeyboardEvent::WillHide {
            time,
            ease,
            height: -height,
            duration
        }));
    }
    
    extern "C" fn keyboard_did_hide(_: &Object, _: Sel, _notif: ObjcId) {
        let time = with_ios_app(|app| app.time_now());
        IosApp::send_virtual_keyboard_event(VirtualKeyboardEvent::DidHide {
            time,
        });
    }
    extern "C" fn keyboard_will_show(_: &Object, _: Sel, notif: ObjcId) {
        let height = get_height_delta(notif);
        let (duration, ease) = get_curve_duration(notif);
        let time = with_ios_app(|app| app.time_now());
        IosApp::send_virtual_keyboard_event(VirtualKeyboardEvent::WillShow {
            time,
            height,
            ease,
            duration
        });
    }
    extern "C" fn keyboard_did_show(_: &Object, _: Sel, notif: ObjcId) {
        let height = get_height_delta(notif);
        let time = with_ios_app(|app| app.time_now());
        IosApp::send_virtual_keyboard_event(VirtualKeyboardEvent::DidShow {
            time,
            height: height
        });
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
        /*decl.add_method(
            sel!(drawInMTKView:),
            draw_in_rect as extern "C" fn(&Object, Sel, ObjcId),
        );*/
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
