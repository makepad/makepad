use {
    crate::{
        //makepad_math::*,
        //event::{TouchState, VirtualKeyboardEvent},
        //animator::Ease,
        os::{
            apple::tvos_app::TvosApp,
            //apple::apple_util::nsstring_to_string,
            apple::apple_sys::*,
            apple::tvos_app::get_tvos_app_global,
        },
    }
};


pub fn define_tvos_app_delegate() -> *const Class {
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("NSAppDelegate", superclass).unwrap();
    
    extern "C" fn did_finish_launching_with_options(
        _: &Object,
        _: Sel,
        _: ObjcId,
        _: ObjcId,
    ) -> BOOL {
        get_tvos_app_global().did_finish_launching_with_options();
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
    
    unsafe {
        decl.add_method(sel!(isOpaque), yes as extern "C" fn(&Object, Sel) -> BOOL);
       
    }
    
    return decl.register();
}

pub fn define_mtk_view_delegate() -> *const Class {
    let mut decl = ClassDecl::new("MakepadViewDlg", class!(NSObject)).unwrap();
    
    extern "C" fn draw_in_rect(_this: &Object, _: Sel, _: ObjcId) {
        TvosApp::draw_in_rect();
    }
    
    extern "C" fn draw_size_will_change(_this: &Object, _: Sel, _: ObjcId, _: ObjcId) {
        crate::log!("Draw size will change");
        TvosApp::draw_size_will_change();
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
        TvosApp::send_timer_received(nstimer);
    }
    
    extern fn received_live_resize(_this: &Object, _: Sel, _nstimer: ObjcId) {
        TvosApp::send_paint_event();
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
