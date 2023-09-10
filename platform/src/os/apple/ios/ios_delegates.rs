use {
    crate::{
        makepad_math::*,
        makepad_objc_sys::runtime::{ObjcId},
        event::TouchState,
        os::{
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
        let ca = get_ios_app_global();
        ca.did_finish_launching_with_options();
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
    
    fn on_touch(this: &Object, event: ObjcId, state:TouchState) {
       unsafe {
            let enumerator: ObjcId = msg_send![event, allTouches];
            let size: u64 = msg_send![enumerator, count];
            let enumerator: ObjcId = msg_send![enumerator, objectEnumerator];

            for touch_id in 0..size {
                let ios_touch: ObjcId = msg_send![enumerator, nextObject];
                let uid_obj: ObjcId = msg_send![ios_touch, estimationUpdateIndex];
                let uid:u64 = if uid_obj != nil{
                    msg_send![uid_obj, intValue]
                }
                else{ 
                    touch_id as u64
                };
                let p: NSPoint = msg_send![ios_touch, locationInView: this];
                get_ios_app_global().update_touch(uid, dvec2(p.x,p.y), state);
            }
        }
    }
    
    extern "C" fn touches_began(this: &Object, _: Sel, _: ObjcId, event: ObjcId) {
        on_touch(this, event, TouchState::Start);
        get_ios_app_global().send_touch_update();
    }

    extern "C" fn touches_moved(this: &Object, _: Sel, _: ObjcId, event: ObjcId) {
        on_touch(this, event, TouchState::Move);
        get_ios_app_global().send_touch_update();
    }
  
    extern "C" fn touches_ended(this: &Object, _: Sel, _: ObjcId, event: ObjcId) {
        on_touch(this, event, TouchState::Stop);
        get_ios_app_global().send_touch_update();
    }

    extern "C" fn touches_canceled(this: &Object, _: Sel, _: ObjcId, event: ObjcId) {
        on_touch(this, event, TouchState::Stop);
        get_ios_app_global().send_touch_update();
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

pub fn define_mtk_view_dlg() -> *const Class {
    let mut decl = ClassDecl::new("MakepadViewDlg", class!(NSObject)).unwrap();

    extern "C" fn draw_in_rect(_this: &Object, _: Sel, _: ObjcId) {
        get_ios_app_global().draw_in_rect();
    }
    
    extern "C" fn draw_size_will_change(_this: &Object, _: Sel, _: ObjcId, _:ObjcId) {
        get_ios_app_global().draw_in_rect();
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
        get_ios_app_global().send_timer_received(nstimer);
    }
    
    extern fn received_live_resize(_this: &Object, _: Sel, _nstimer: ObjcId) {
        get_ios_app_global().send_paint_event();
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

