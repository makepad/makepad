use {
    std::{
        ffi::CStr,
        os::raw::{c_void}
    },
    crate::{
        makepad_math::{
            Vec2,
        },
        platform::{
            apple::frameworks::*,
            cocoa_app::{
                CocoaApp,
                get_cocoa_app,
                get_cocoa_app_global
            },
            cocoa_window::{
                get_cocoa_window
            },
            apple_util::{
                str_to_nsstring,
                nsstring_to_string,
                get_event_key_modifier,
                superclass,
                load_mouse_cursor
            },
        },
        menu::{
            CommandId
        },
        event::{
            Signal,
            Event,
            DragState,
            FingerDragEvent,
            FingerDropEvent,
            DraggedItem,
            DragAction
        },
    }
};

pub struct KeyValueObserver {
    callback: Box<Box<dyn Fn() >>,
    observer: RcObjcId
}

impl Drop for KeyValueObserver {
    fn drop(&mut self) {
        unsafe {
            (*self.observer.as_id()).set_ivar("key_value_observer_callback", 0 as *mut c_void);
        }
    }
}

impl KeyValueObserver {
    pub fn new(target: ObjcId, name: &str, callback: Box<dyn Fn()>) -> Self {
        unsafe {
            let double_box = Box::new(callback);
            let cocoa_app = get_cocoa_app_global();
            let observer = RcObjcId::from_owned(msg_send![cocoa_app.key_value_observing_delegate_class, alloc]);
            
            (*observer.as_id()).set_ivar("key_value_observer_callback", &*double_box as *const _ as *const c_void);
            
            let () = msg_send![
                target,
                addObserver: observer.as_id()
                forKeyPath: str_to_nsstring(name)
                options: 15u64 // if its not 1+2+4+8 it does nothing
                context: nil
            ];
            Self {
                callback: double_box,
                observer
            }
        }
        
    }
}

pub fn define_key_value_observing_delegate() -> *const Class {
    
    extern fn observe_value_for_key_path(
        this: &Object,
        _: Sel,
        _key_path: ObjcId,
        _of_object: ObjcId,
        _change: ObjcId,
        _data: *mut std::ffi::c_void
    ) {
        unsafe {
            let ptr: *const c_void = *this.get_ivar("key_value_observer_callback");
            if ptr == 0 as *const c_void { // owner gone
                return
            }
            (*(ptr as *const Box<dyn Fn()>))();
        }
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("KeyValueObservingDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(
            sel!(observeValueForKeyPath: ofObject: change: context:),
            observe_value_for_key_path as extern fn(&Object, Sel, ObjcId, ObjcId, ObjcId, *mut std::ffi::c_void)
        );
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("key_value_observer_callback");
    
    return decl.register();
}



pub fn define_cocoa_timer_delegate() -> *const Class {
    
    extern fn received_timer(this: &Object, _: Sel, nstimer: ObjcId) {
        let ca = get_cocoa_app(this);
        ca.send_timer_received(nstimer);
    }
    
    extern fn received_live_resize(this: &Object, _: Sel, _nstimer: ObjcId) {
        let ca = get_cocoa_app(this);
        ca.send_paint_event();
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("TimerDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(receivedTimer:), received_timer as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(receivedLiveResize:), received_live_resize as extern fn(&Object, Sel, ObjcId));
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    
    return decl.register();
}

pub fn define_app_delegate() -> *const Class {
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("NSAppDelegate", superclass).unwrap();
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    return decl.register();
}

pub fn define_menu_target_class() -> *const Class {
    
    extern fn menu_action(this: &Object, _sel: Sel, _item: ObjcId) {
        //println!("markedRange");
        let ca = get_cocoa_app(this);
        unsafe {
            let command_u64: u64 = *this.get_ivar("command_usize");
            /*let cmd = if let Ok(status_map) = ca.status_map.lock() {
                *status_map.usize_to_command.get(&command_usize).expect("")
            }
            else {
                panic!("Cannot lock cmd_map")
            };*/
            ca.send_command_event(CommandId(command_u64));
        }
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MenuTarget", superclass).unwrap();
    unsafe {
        decl.add_method(sel!(menuAction:), menu_action as extern fn(&Object, Sel, ObjcId));
    }
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    decl.add_ivar::<usize>("command_usize");
    return decl.register();
}

pub fn define_menu_delegate() -> *const Class {
    // NSMenuDelegate protocol
    extern fn menu_will_open(this: &Object, _sel: Sel, _item: ObjcId) {
        //println!("markedRange");
        let _ca = get_cocoa_app(this);
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MenuDelegate", superclass).unwrap();
    unsafe {
        decl.add_method(sel!(menuWillOpen:), menu_will_open as extern fn(&Object, Sel, ObjcId));
    }
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    decl.add_protocol(&Protocol::get("NSMenuDelegate").unwrap());
    return decl.register();
}

struct CocoaPostInit {
    cocoa_app_ptr: *mut CocoaApp,
    signal_id: u64,
}

pub fn define_cocoa_post_delegate() -> *const Class {
    
    extern fn received_post(this: &Object, _: Sel, _nstimer: ObjcId) {
        let ca = get_cocoa_app(this);
        unsafe {
            let signal_id: usize = *this.get_ivar("signal_id");
            let status: u64 = *this.get_ivar("status");
            /*let status = if let Ok(status_map) = ca.status_map.lock() {
                *status_map.usize_to_status.get(&status).expect("status invalid")
            }
            else {
                panic!("cannot lock cmd_map")
            };*/
            ca.send_signal_event(Signal {signal_id: signal_id}, status);
        }
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("PostDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(receivedPost:), received_post as extern fn(&Object, Sel, ObjcId));
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    decl.add_ivar::<usize>("signal_id");
    decl.add_ivar::<usize>("status");
    
    return decl.register();
}

pub fn define_cocoa_window_delegate() -> *const Class {
    
    extern fn window_should_close(this: &Object, _: Sel, _: ObjcId) -> BOOL {
        let cw = get_cocoa_window(this);
        if cw.send_window_close_requested_event() {
            YES
        }
        else {
            NO
        }
    }
    
    extern fn window_will_close(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.send_window_closed_event();
    }
    
    extern fn window_did_resize(this: &Object, _: Sel, _: ObjcId) {
        let _cw = get_cocoa_window(this);
        //cw.send_change_event();
    }
    
    extern fn window_will_start_live_resize(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.start_live_resize();
    }
    
    extern fn window_did_end_live_resize(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.end_live_resize();
    }
    
    // This won't be triggered if the move was part of a resize.
    extern fn window_did_move(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.send_change_event();
    }
    
    extern fn window_did_change_screen(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.send_change_event();
    }
    
    // This will always be called before `window_did_change_screen`.
    extern fn window_did_change_backing_properties(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.send_change_event();
    }
    
    extern fn window_did_become_key(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.send_focus_event();
    }
    
    extern fn window_did_resign_key(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.send_focus_lost_event();
    }
    
    // Invoked when the dragged image enters destination bounds or frame
    extern fn dragging_entered(_this: &Object, _: Sel, _sender: ObjcId) -> BOOL {
        YES
    }
    
    // Invoked when the image is released
    extern fn prepare_for_drag_operation(_: &Object, _: Sel, _: ObjcId) -> BOOL {
        YES
    }
    
    // Invoked after the released image has been removed from the screen
    extern fn perform_drag_operation(_this: &Object, _: Sel, _sender: ObjcId) -> BOOL {
        YES
    }
    
    // Invoked when the dragging operation is complete
    extern fn conclude_drag_operation(_: &Object, _: Sel, _: ObjcId) {}
    
    // Invoked when the dragging operation is cancelled
    extern fn dragging_exited(this: &Object, _: Sel, _: ObjcId) {
        let _cw = get_cocoa_window(this);
        //WindowDelegate::emit_event(state, WindowEvent::HoveredFileCancelled);
    }
    
    // Invoked when entered fullscreen
    extern fn window_did_enter_fullscreen(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.is_fullscreen = true;
        cw.send_change_event();
    }
    
    // Invoked when before enter fullscreen
    extern fn window_will_enter_fullscreen(this: &Object, _: Sel, _: ObjcId) {
        let _cw = get_cocoa_window(this);
    }
    
    // Invoked when exited fullscreen
    extern fn window_did_exit_fullscreen(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.is_fullscreen = false;
        cw.send_change_event();
    }
    
    extern fn window_did_fail_to_enter_fullscreen(_this: &Object, _: Sel, _: ObjcId) {
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("RenderWindowDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(windowShouldClose:), window_should_close as extern fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(windowWillClose:), window_will_close as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidResize:), window_did_resize as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowWillStartLiveResize:), window_will_start_live_resize as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidEndLiveResize:), window_did_end_live_resize as extern fn(&Object, Sel, ObjcId));
        
        decl.add_method(sel!(windowDidMove:), window_did_move as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowChangedScreen:), window_did_change_screen as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowChangedBackingProperties:), window_did_change_backing_properties as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidBecomeKey:), window_did_become_key as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidResignKey:), window_did_resign_key as extern fn(&Object, Sel, ObjcId));
        
        // callbacks for drag and drop events
        decl.add_method(sel!(draggingEntered:), dragging_entered as extern fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(prepareForDragOperation:), prepare_for_drag_operation as extern fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(performDragOperation:), perform_drag_operation as extern fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(concludeDragOperation:), conclude_drag_operation as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(draggingExited:), dragging_exited as extern fn(&Object, Sel, ObjcId));
        
        // callbacks for fullscreen events
        decl.add_method(sel!(windowDidEnterFullScreen:), window_did_enter_fullscreen as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowWillEnterFullScreen:), window_will_enter_fullscreen as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidExitFullScreen:), window_did_exit_fullscreen as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(windowDidFailToEnterFullScreen:), window_did_fail_to_enter_fullscreen as extern fn(&Object, Sel, ObjcId));
        // custom timer fn
        //decl.add_method(sel!(windowReceivedTimer:), window_received_timer as extern fn(&Object, Sel, id));
        
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("cocoa_window_ptr");
    
    return decl.register();
}

pub fn define_cocoa_window_class() -> *const Class {
    extern fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }
    
    extern fn is_movable_by_window_background(_: &Object, _: Sel) -> BOOL {
        YES
    }
    
    let window_superclass = class!(NSWindow);
    let mut decl = ClassDecl::new("RenderWindow", window_superclass).unwrap();
    unsafe {
        decl.add_method(sel!(canBecomeMainWindow), yes as extern fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(canBecomeKeyWindow), yes as extern fn(&Object, Sel) -> BOOL);
    }
    return decl.register();
}

pub fn define_cocoa_view_class() -> *const Class {
    
    extern fn dealloc(this: &Object, _sel: Sel) {
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            let _: () = msg_send![marked_text, release];
        }
    }
    
    extern fn init_with_ptr(this: &Object, _sel: Sel, cx: *mut c_void) -> ObjcId {
        unsafe {
            let this: ObjcId = msg_send![this, init];
            if this != nil {
                (*this).set_ivar("cocoa_window_ptr", cx);
                let marked_text = <ObjcId as NSMutableAttributedString>::init(
                    NSMutableAttributedString::alloc(nil),
                );
                (*this).set_ivar("markedText", marked_text);
            }
            let types = [NSPasteboardTypeFileURL];
            let types_nsarray: ObjcId = msg_send![
                class!(NSArray),
                arrayWithObjects: types.as_ptr()
                count: types.len()
            ];
            let _: () = msg_send![this, registerForDraggedTypes: types_nsarray];
            
            this
        }
    }
    
    extern fn mouse_down(this: &Object, _sel: Sel, event: ObjcId) {
        
        let cw = get_cocoa_window(this);
        unsafe {
            if cw.mouse_down_can_drag_window() {
                let () = msg_send![cw.window, performWindowDragWithEvent: event];
                return
            }
        }
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_down(0, modifiers);
    }
    
    extern fn mouse_up(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_up(0, modifiers);
    }
    
    extern fn right_mouse_down(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_down(1, modifiers);
    }
    
    extern fn right_mouse_up(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_up(1, modifiers);
    }
    
    extern fn other_mouse_down(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_down(2, modifiers);
    }
    
    extern fn other_mouse_up(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_up(2, modifiers);
    }
    
    fn mouse_pos_from_event(view: &Object, event: ObjcId) -> Vec2 {
        let window_point: NSPoint = unsafe {msg_send![event, locationInWindow]};
        let view_point = window_point_to_view_point(view, window_point);
        ns_point_to_vec2(view_point)
    }
    
    fn window_point_to_view_point(view: &Object, window_point: NSPoint) -> NSPoint {
        let view_point: NSPoint = unsafe {msg_send![view, convertPoint: window_point fromView: nil]};
        let view_frame: NSRect = unsafe {msg_send![view, frame]};
        NSPoint {
            x: view_point.x,
            y: view_frame.size.height - view_point.y
        }
    }
    
    fn ns_point_to_vec2(point: NSPoint) -> Vec2 {
        Vec2 {
            x: point.x as f32,
            y: point.y as f32,
        }
    }
    
    fn mouse_motion(this: &Object, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let pos = mouse_pos_from_event(this, event);
        let modifiers = get_event_key_modifier(event);
        cw.send_finger_hover_and_move(event, pos, modifiers);
    }
    
    extern fn mouse_moved(this: &Object, _sel: Sel, event: ObjcId) {
        mouse_motion(this, event);
    }
    
    extern fn mouse_dragged(this: &Object, _sel: Sel, event: ObjcId) {
        mouse_motion(this, event);
    }
    
    extern fn right_mouse_dragged(this: &Object, _sel: Sel, event: ObjcId) {
        mouse_motion(this, event);
    }
    
    extern fn other_mouse_dragged(this: &Object, _sel: Sel, event: ObjcId) {
        mouse_motion(this, event);
    }
    
    extern fn draw_rect(this: &Object, _sel: Sel, rect: NSRect) {
        let _cw = get_cocoa_window(this);
        unsafe {
            let superclass = superclass(this);
            let () = msg_send![super (this, superclass), drawRect: rect];
        }
    }
    
    extern fn reset_cursor_rects(this: &Object, _sel: Sel) {
        let cw = get_cocoa_window(this);
        unsafe {
            let cocoa_app = &mut (*cw.cocoa_app);
            let current_cursor = cocoa_app.current_cursor.clone();
            let cursor_id = *cocoa_app.cursors.entry(current_cursor.clone()).or_insert_with( || {
                load_mouse_cursor(current_cursor.clone())
            });
            let bounds: NSRect = msg_send![this, bounds];
            let _: () = msg_send![
                this,
                addCursorRect: bounds
                cursor: cursor_id
            ];
        }
    }
    
    // NSTextInput protocol
    extern fn marked_range(this: &Object, _sel: Sel) -> NSRange {
        //println!("markedRange");
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            let length = marked_text.length();
            if length >0 {
                NSRange {
                    location: 0,
                    length: length - 1
                }
            } else {
                NSRange {
                    location: i64::max_value() as u64,
                    length: 0,
                }
            }
        }
    }
    
    extern fn selected_range(_this: &Object, _sel: Sel) -> NSRange {
        NSRange {
            location: 0,
            length: 1,
        }
    }
    
    extern fn has_marked_text(this: &Object, _sel: Sel) -> BOOL {
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            (marked_text.length() >0) as BOOL
        }
    }
    
    extern fn set_marked_text(this: &mut Object, _sel: Sel, string: ObjcId, _selected_range: NSRange, _replacement_range: NSRange) {
        unsafe {
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
        }
    }
    
    extern fn unmark_text(this: &Object, _sel: Sel) {
        let cw = get_cocoa_window(this);
        unsafe {
            let cocoa_app = &(*cw.cocoa_app);
            let marked_text: ObjcId = *this.get_ivar("markedText");
            let mutable_string = marked_text.mutable_string();
            let _: () = msg_send![mutable_string, setString: cocoa_app.const_empty_string.as_id()];
            let input_context: ObjcId = msg_send![this, inputContext];
            let _: () = msg_send![input_context, discardMarkedText];
        }
    }
    
    extern fn valid_attributes_for_marked_text(this: &Object, _sel: Sel) -> ObjcId {
        let cw = get_cocoa_window(this);
        unsafe {
            let cocoa_app = &(*cw.cocoa_app);
            cocoa_app.const_attributes_for_marked_text
        }
    }
    
    extern fn attributed_substring_for_proposed_range(_this: &Object, _sel: Sel, _range: NSRange, _actual_range: *mut c_void) -> ObjcId {
        nil
    }
    
    extern fn character_index_for_point(_this: &Object, _sel: Sel, _point: NSPoint) -> u64 {
        // println!("character_index_for_point");
        0
    }
    
    extern fn first_rect_for_character_range(this: &Object, _sel: Sel, _range: NSRange, _actual_range: *mut c_void) -> NSRect {
        let cw = get_cocoa_window(this);
        
        let view: ObjcId = this as *const _ as *mut _;
        //let window_point = event.locationInWindow();
        //et view_point = view.convertPoint_fromView_(window_point, nil);
        let view_rect: NSRect = unsafe {msg_send![view, frame]};
        let window_rect: NSRect = unsafe {msg_send![cw.window, frame]};
        
        let origin = cw.get_ime_origin();
        let bar = (window_rect.size.height - view_rect.size.height) as f32 - 5.;
        NSRect {
            origin: NSPoint {x: (origin.x + cw.ime_spot.x) as f64, y: (origin.y + (view_rect.size.height as f32 - cw.ime_spot.y - bar)) as f64},
            // as _, y as _),
            size: NSSize {width: 0.0, height: 0.0},
        }
    }
    
    extern fn insert_text(this: &Object, _sel: Sel, string: ObjcId, replacement_range: NSRange) {
        let cw = get_cocoa_window(this);
        unsafe {
            let has_attr = msg_send![string, isKindOfClass: class!(NSAttributedString)];
            let characters = if has_attr {
                msg_send![string, string]
            } else {
                string
            };
            let string = nsstring_to_string(characters);
            cw.send_text_input(string, replacement_range.length != 0);
            let input_context: ObjcId = msg_send![this, inputContext];
            let () = msg_send![input_context, invalidateCharacterCoordinates];
            let () = msg_send![cw.view, setNeedsDisplay: YES];
            unmark_text(this, _sel);
        }
    }
    
    extern fn do_command_by_selector(this: &Object, _sel: Sel, _command: Sel) {
        let _cw = get_cocoa_window(this);
    }
    
    extern fn key_down(this: &Object, _sel: Sel, event: ObjcId) {
        let _cw = get_cocoa_window(this);
        unsafe {
            let input_context: ObjcId = msg_send![this, inputContext];
            let () = msg_send![input_context, handleEvent: event];
        }
    }
    
    extern fn key_up(_this: &Object, _sel: Sel, _event: ObjcId) {
    }
    
    extern fn insert_tab(this: &Object, _sel: Sel, _sender: ObjcId) {
        unsafe {
            let window: ObjcId = msg_send![this, window];
            let first_responder: ObjcId = msg_send![window, firstResponder];
            let this_ptr = this as *const _ as *mut _;
            if first_responder == this_ptr {
                let (): _ = msg_send![window, selectNextKeyView: this];
            }
        }
    }
    
    extern fn insert_back_tab(this: &Object, _sel: Sel, _sender: ObjcId) {
        unsafe {
            let window: ObjcId = msg_send![this, window];
            let first_responder: ObjcId = msg_send![window, firstResponder];
            let this_ptr = this as *const _ as *mut _;
            if first_responder == this_ptr {
                let (): _ = msg_send![window, selectPreviousKeyView: this];
            }
        }
    }
    
    extern fn yes_function(_this: &Object, _se: Sel, _event: ObjcId) -> BOOL {
        YES
    }
    
    
    extern fn display_layer(this: &Object, _: Sel, _calayer: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.send_change_event();
    }
    
    
    extern fn dragging_session_ended_at_point_operation(this: &Object, _: Sel, _session: ObjcId, _point: NSPoint, _operation: NSDragOperation) {
        let window = get_cocoa_window(this);
        window.fingers_down[0] = false;
        let mut events = vec![Event::DragEnd];
        window.do_callback(&mut events);
    }
    
    extern fn dragging_entered(this: &Object, _: Sel, sender: ObjcId) -> NSDragOperation {
        let window = get_cocoa_window(this);
        window.start_live_resize();
        dragging(this, sender)
    }
    
    extern fn dragging_updated(this: &Object, _: Sel, sender: ObjcId) -> NSDragOperation {
        dragging(this, sender)
    }
    
    extern fn dragging_exited(this: &Object, _: Sel, sender: ObjcId) {
        dragging(this, sender);
    }
    
    fn dragging(this: &Object, sender: ObjcId) -> NSDragOperation {
        let window = get_cocoa_window(this);
        let pos = ns_point_to_vec2(window_point_to_view_point(this, unsafe {
            msg_send![sender, draggingLocation]
        }));
        let mut events = vec![Event::FingerDrag(FingerDragEvent {
            handled: false,
            abs: pos,
            //rel: pos,
            //rect: Rect::default(),
            state: DragState::Over,
            action: DragAction::None,
        })];
        window.do_callback(&mut events);
        match &events[0] {
            Event::FingerDrag(event) => {
                match event.action {
                    DragAction::None => NSDragOperation::None,
                    DragAction::Copy => NSDragOperation::Copy,
                    DragAction::Link => NSDragOperation::Link,
                    DragAction::Move => NSDragOperation::Move,
                }
            },
            _ => panic!()
        }
    }
    
    extern fn dragging_ended(this: &Object, _: Sel, _sender: ObjcId) {
        let window = get_cocoa_window(this);
        window.end_live_resize();
    }
    
    extern fn perform_drag_operation(this: &Object, _: Sel, sender: ObjcId) {
        let window = get_cocoa_window(this);
        let pos = ns_point_to_vec2(window_point_to_view_point(this, unsafe {
            msg_send![sender, draggingLocation]
        }));
        let pasteboard: ObjcId = unsafe {msg_send![sender, draggingPasteboard]};
        let class: ObjcId = unsafe {msg_send![class!(NSURL), class]};
        let classes: ObjcId = unsafe {
            msg_send![class!(NSArray), arrayWithObject: class]
        };
        let object: ObjcId = unsafe {
            msg_send![class!(NSNumber), numberWithBool: true]
        };
        let options: ObjcId = unsafe {
            msg_send![
                class!(NSDictionary),
                dictionaryWithObject: object
                forKey: NSPasteboardURLReadingFileURLsOnlyKey
            ]
        };
        let urls: ObjcId = unsafe {
            msg_send![pasteboard, readObjectsForClasses: classes options: options]
        };
        let count: usize = unsafe {msg_send![urls, count]};
        let mut file_urls = Vec::with_capacity(count);
        for index in 0..count {
            let url: ObjcId = unsafe {msg_send![urls, objectAtIndex: index]};
            let url: ObjcId = unsafe {msg_send![url, filePathURL]};
            let string: ObjcId = unsafe {msg_send![url, absoluteString]};
            let string = unsafe {CStr::from_ptr(msg_send![string, UTF8String])};
            file_urls.push(string.to_str().unwrap().to_string());
        }
        let mut events = vec![Event::FingerDrop(FingerDropEvent {
            handled: false,
            abs: pos,
            dragged_item: DraggedItem {
                file_urls,
            }
        })];
        window.do_callback(&mut events);
    }
    
    /*
    extern fn draw(this: &Object, _: Sel, _calayer: id, _cgcontext: id) {
        println!("draw");
        //let cw = get_cocoa_window(this);
        //cw.send_change_event();
    }

    extern fn layer_will_draw(this: &Object, _: Sel, _calayer: id) {
        println!("layer_will_draw");
        //let cw = get_cocoa_window(this);
        //cw.send_change_event();
    }*/
    
    
    let superclass = class!(NSView);
    let mut decl = ClassDecl::new("RenderViewClass", superclass).unwrap();
    unsafe {
        decl.add_method(sel!(dealloc), dealloc as extern fn(&Object, Sel));
        decl.add_method(sel!(initWithPtr:), init_with_ptr as extern fn(&Object, Sel, *mut c_void) -> ObjcId);
        decl.add_method(sel!(drawRect:), draw_rect as extern fn(&Object, Sel, NSRect));
        decl.add_method(sel!(resetCursorRects), reset_cursor_rects as extern fn(&Object, Sel));
        decl.add_method(sel!(hasMarkedText), has_marked_text as extern fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(markedRange), marked_range as extern fn(&Object, Sel) -> NSRange);
        decl.add_method(sel!(selectedRange), selected_range as extern fn(&Object, Sel) -> NSRange);
        decl.add_method(sel!(setMarkedText: selectedRange: replacementRange:), set_marked_text as extern fn(&mut Object, Sel, ObjcId, NSRange, NSRange));
        decl.add_method(sel!(unmarkText), unmark_text as extern fn(&Object, Sel));
        decl.add_method(sel!(validAttributesForMarkedText), valid_attributes_for_marked_text as extern fn(&Object, Sel) -> ObjcId);
        decl.add_method(
            sel!(attributedSubstringForProposedRange: actualRange:),
            attributed_substring_for_proposed_range
            as extern fn(&Object, Sel, NSRange, *mut c_void) -> ObjcId,
        );
        decl.add_method(
            sel!(insertText: replacementRange:),
            insert_text as extern fn(&Object, Sel, ObjcId, NSRange),
        );
        decl.add_method(
            sel!(characterIndexForPoint:),
            character_index_for_point as extern fn(&Object, Sel, NSPoint) -> u64,
        );
        decl.add_method(
            sel!(firstRectForCharacterRange: actualRange:),
            first_rect_for_character_range
            as extern fn(&Object, Sel, NSRange, *mut c_void) -> NSRect,
        );
        decl.add_method(sel!(doCommandBySelector:), do_command_by_selector as extern fn(&Object, Sel, Sel));
        decl.add_method(sel!(keyDown:), key_down as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(keyUp:), key_up as extern fn(&Object, Sel, ObjcId));
        //decl.add_method(sel!(insertTab:), insert_tab as extern fn(&Object, Sel, id));
        //decl.add_method(sel!(insertBackTab:), insert_back_tab as extern fn(&Object, Sel, id));
        decl.add_method(sel!(mouseDown:), mouse_down as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(mouseUp:), mouse_up as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(rightMouseDown:), right_mouse_down as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(rightMouseUp:), right_mouse_up as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(otherMouseDown:), other_mouse_down as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(otherMouseUp:), other_mouse_up as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(mouseMoved:), mouse_moved as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(mouseDragged:), mouse_dragged as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(rightMouseDragged:), right_mouse_dragged as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(otherMouseDragged:), other_mouse_dragged as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(wantsKeyDownForEvent:), yes_function as extern fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(acceptsFirstResponder:), yes_function as extern fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(becomeFirstResponder:), yes_function as extern fn(&Object, Sel, ObjcId) -> BOOL);
        decl.add_method(sel!(resignFirstResponder:), yes_function as extern fn(&Object, Sel, ObjcId) -> BOOL);
        
        decl.add_method(sel!(displayLayer:), display_layer as extern fn(&Object, Sel, ObjcId));
        
        decl.add_method(sel!(draggingSession: endedAtPoint: operation:), dragging_session_ended_at_point_operation as extern fn(&Object, Sel, ObjcId, NSPoint, NSDragOperation));
        
        decl.add_method(sel!(draggingEntered:), dragging_entered as extern fn(&Object, Sel, ObjcId) -> NSDragOperation);
        decl.add_method(sel!(draggingExited:), dragging_exited as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(draggingUpdated:), dragging_updated as extern fn(&Object, Sel, ObjcId) -> NSDragOperation);
        decl.add_method(sel!(performDragOperation:), perform_drag_operation as extern fn(&Object, Sel, ObjcId));
        decl.add_method(sel!(draggingEnded:), dragging_ended as extern fn(&Object, Sel, ObjcId));
    }
    decl.add_ivar::<*mut c_void>("cocoa_window_ptr");
    decl.add_ivar::<ObjcId>("markedText");
    decl.add_protocol(&Protocol::get("NSTextInputClient").unwrap());
    decl.add_protocol(&Protocol::get("CALayerDelegate").unwrap());
    return decl.register();
}
