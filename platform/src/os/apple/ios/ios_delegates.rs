use {
    crate::{
        makepad_math::*,
        event::{TouchState, VirtualKeyboardEvent},
        animator::Ease,
        os::{
            apple::ios_app::IosApp,
            apple::apple_util::nsstring_to_string,
            apple::apple_sys::*,
            apple::ios_app::get_ios_app_global,
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
        get_ios_app_global().did_finish_launching_with_options();
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
    extern fn yes(_: &Object, _: Sel) -> BOOL {
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
                get_ios_app_global().update_touch(uid, dvec2(p.x, p.y), state);
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

pub fn define_ios_timer_delegate() -> *const Class {
    
    extern fn received_timer(_this: &Object, _: Sel, nstimer: ObjcId) {
        IosApp::send_timer_received(nstimer);
    }
    
    extern fn received_live_resize(_this: &Object, _: Sel, _nstimer: ObjcId) {
        IosApp::send_paint_event();
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("TimerDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(receivedTimer:), received_timer as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(receivedLiveResize:), received_live_resize as extern fn(&Object, Sel, ObjcId));
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
        let time = get_ios_app_global().time_now();
        get_ios_app_global().queue_virtual_keyboard_event(VirtualKeyboardEvent::WillHide {
            time,
            ease,
            height: -height,
            duration
        });
    }
    
    extern "C" fn keyboard_did_hide(_: &Object, _: Sel, _notif: ObjcId) {
        let time = get_ios_app_global().time_now();
        IosApp::send_virtual_keyboard_event(VirtualKeyboardEvent::DidHide {
            time,
        });
    }
    extern "C" fn keyboard_will_show(_: &Object, _: Sel, notif: ObjcId) {
        let height = get_height_delta(notif);
        let (duration, ease) = get_curve_duration(notif);
        let time = get_ios_app_global().time_now();
        IosApp::send_virtual_keyboard_event(VirtualKeyboardEvent::WillShow {
            time,
            height,
            ease,
            duration
        });
    }
    extern "C" fn keyboard_did_show(_: &Object, _: Sel, notif: ObjcId) {
        let height = get_height_delta(notif);
        let time = get_ios_app_global().time_now();
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
