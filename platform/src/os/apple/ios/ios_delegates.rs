use {
    crate::{
        makepad_objc_sys::runtime::{ObjcId},
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
    /*fn on_touch(_this: &Object, _event: ObjcId, mut _callback: impl FnMut(u64, f32, f32)) {
       /* unsafe {
            let enumerator: ObjcId = msg_send![event, allTouches];
            let size: u64 = msg_send![enumerator, count];
            let enumerator: ObjcId = msg_send![enumerator, objectEnumerator];

            for touch_id in 0..size {
                let ios_touch: ObjcId = msg_send![enumerator, nextObject];
                let mut ios_pos: NSPoint = msg_send![ios_touch, locationInView: this];

                ios_pos.x *= 2.;
                ios_pos.y *= 2.;

                callback(touch_id, ios_pos.x as _, ios_pos.y as _);
            }
        }*/
    }*/
    extern "C" fn touches_began(_this: &Object, _: Sel, _: ObjcId, _event: ObjcId) {
        /*let payload = get_window_payload(this);

        if let Some(ref mut event_handler) = payload.event_handler {
            on_touch(this, event, |id, x, y| {
                event_handler.touch_event(TouchPhase::Started, id, x as _, y as _);
            });
        }*/
    }

    extern "C" fn touches_moved(_this: &Object, _: Sel, _: ObjcId, _event: ObjcId) {
        /*let payload = get_window_payload(this);

        if let Some(ref mut event_handler) = payload.event_handler {
            on_touch(this, event, |id, x, y| {
                event_handler.touch_event(TouchPhase::Moved, id, x as _, y as _);
            });
        }*/
    }
  
    extern "C" fn touches_ended(_this: &Object, _: Sel, _: ObjcId, _event: ObjcId) {
        /*let payload = get_window_payload(this);

        if let Some(ref mut event_handler) = payload.event_handler {
            on_touch(this, event, |id, x, y| {
                event_handler.touch_event(TouchPhase::Ended, id, x as _, y as _);
            });
        }*/
    }

    extern "C" fn touches_canceled(_: &Object, _: Sel, _: ObjcId, _: ObjcId) {}

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
        let ca = get_ios_app_global();
        ca.draw_in_rect();
        
        /*
        let payload = get_window_payload(this);
        if payload.event_handler.is_none() {
            let f = payload.f.take().unwrap();

            if payload.gfx_api == AppleGfxApi::OpenGl {
                crate::native::gl::load_gl_funcs(|proc| {
                    let name = std::ffi::CString::new(proc).unwrap();

                    unsafe { get_proc_address(name.as_ptr() as _) }
                });
            }

            payload.event_handler = Some(f());
        }

        let main_screen: ObjcId = unsafe { msg_send![class!(UIScreen), mainScreen] };
        let screen_rect: NSRect = unsafe { msg_send![main_screen, bounds] };
        let high_dpi = native_display().lock().unwrap().high_dpi;

        let (screen_width, screen_height) = if high_dpi {
            (
                screen_rect.size.width as i32 * 2,
                screen_rect.size.height as i32 * 2,
            )
        } else {
            (
                screen_rect.size.width as i32,
                screen_rect.size.height as i32,
            )
        };

        if native_display().lock().unwrap().screen_width != screen_width
            || native_display().lock().unwrap().screen_height != screen_height
        {
            {
                let mut d = native_display().lock().unwrap();
                d.screen_width = screen_width;
                d.screen_height = screen_height;
            }
            if let Some(ref mut event_handler) = payload.event_handler {
                event_handler.resize_event(screen_width as _, screen_height as _);
            }
        }

        if let Some(ref mut event_handler) = payload.event_handler {
            event_handler.update();
            event_handler.draw();
        }
        */
    }
    unsafe {
        decl.add_method(
            sel!(drawInMTKView:),
            draw_in_rect as extern "C" fn(&Object, Sel, ObjcId),
        );
    }

    decl.add_ivar::<*mut c_void>("display_ptr");
    return decl.register();
}

pub fn define_ios_timer_delegate() -> *const Class {
    
    extern fn received_timer(_this: &Object, _: Sel, nstimer: ObjcId) {
        let ca = get_ios_app_global();
        ca.send_timer_received(nstimer);
    }
    
    extern fn received_live_resize(_this: &Object, _: Sel, _nstimer: ObjcId) {
        let ca = get_ios_app_global();
        ca.send_paint_event();
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

