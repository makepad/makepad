use crate::apple_util::get_event_mouse_button;

use {
    std::{
        sync::Arc,
        sync::Mutex,
        ffi::CStr,
        os::raw::{c_void}
    },
    crate::{
        makepad_live_id::LiveId,
        makepad_math::{
            DVec2,
        },
        os::{
            apple::apple_sys::*,
            macos::{
                macos_app::{
                    MacosApp,
                    get_macos_app_global
                },
                macos_event::{
                    MacosEvent
                },
                macos_window::{
                    get_cocoa_window
                },
            },
            apple_classes::get_apple_class_global,
            apple_util::{
                nsstring_to_string,
                get_event_key_modifier,
                superclass,
                load_mouse_cursor
            },
        },
        cursor::MouseCursor,
        event::{
            DragEvent,
            DropEvent,
            DragItem,
            DragResponse
        },
    }
};





pub fn define_macos_timer_delegate() -> *const Class {
    
    extern fn received_timer(_this: &Object, _: Sel, nstimer: ObjcId) {
        MacosApp::send_timer_received(nstimer);
    }
    
    extern fn received_live_resize(_this: &Object, _: Sel, _nstimer: ObjcId) {
        MacosApp::send_paint_event();
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


pub fn define_app_delegate() -> *const Class {
    
    let superclass = class!(NSObject);
    let decl = ClassDecl::new("NSAppDelegate", superclass).unwrap();

    return decl.register();
}

pub fn define_menu_target_class() -> *const Class {
    
    extern fn menu_action(this: &Object, _sel: Sel, _item: ObjcId) {
        //println!("markedRange");
        unsafe {
            let command_u64: u64 = *this.get_ivar("command_u64");
            /*let cmd = if let Ok(status_map) = ca.status_map.lock() {
                *status_map.usize_to_command.get(&command_usize).expect("")
            }
            else {
                panic!("Cannot lock cmd_map")
            };*/
            MacosApp::send_command_event(LiveId(command_u64));
        }
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MenuTarget", superclass).unwrap();
    unsafe {
        decl.add_method(sel!(menuAction:), menu_action as extern fn(&Object, Sel, ObjcId));
    }
    decl.add_ivar::<usize>("command_u64");
    return decl.register();
}

pub fn define_menu_delegate() -> *const Class {
    // NSMenuDelegate protocol
    extern fn menu_will_open(_this: &Object, _sel: Sel, _item: ObjcId) {
        //println!("markedRange");
        //let _ca = get_cocoa_app(this);
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MenuDelegate", superclass).unwrap();
    unsafe {
        decl.add_method(sel!(menuWillOpen:), menu_will_open as extern fn(&Object, Sel, ObjcId));
    }
    decl.add_protocol(&Protocol::get("NSMenuDelegate").unwrap());
    return decl.register();
}
/*
struct CocoaPostInit {
    macos_app_ptr: *mut MacosApp,
    signal_id: u64,
}*/
/*
pub fn define_cocoa_post_delegate() -> *const Class {
    
    extern fn received_post(_this: &Object, _: Sel, _nstimer: ObjcId) {
        let ca = get_macos_app_global();
        //unsafe {
            //let signal_id: u64 = *this.get_ivar("signal_id");
            /*let status = if let Ok(status_map) = ca.status_map.lock() {
                *status_map.usize_to_status.get(&status).expect("status invalid")
            }
            else {
                panic!("cannot lock cmd_map")
            };*/
            ca.send_signal_event();
        //}
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("PostDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(receivedPost:), received_post as extern fn(&Object, Sel, ObjcId));
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("macos_app_ptr");
    decl.add_ivar::<usize>("signal_id");
    //decl.add_ivar::<usize>("status");
    
    return decl.register();
}*/

pub fn define_macos_window_delegate() -> *const Class {
    
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
        cw.send_got_focus_event();
    }
    
    extern fn window_did_resign_key(this: &Object, _: Sel, _: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.send_lost_focus_event();
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
    decl.add_ivar::<*mut c_void>("macos_window_ptr");
    
    return decl.register();
}

pub fn define_macos_window_class() -> *const Class {
    extern fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }
    /*
    extern fn is_movable_by_window_background(_: &Object, _: Sel) -> BOOL {
        YES
    }*/
    
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
                (*this).set_ivar("macos_window_ptr", cx);
                let marked_text = <ObjcId as NSMutableAttributedString>::init(
                    NSMutableAttributedString::alloc(nil),
                );
                (*this).set_ivar("markedText", marked_text);
            }
            
            #[cfg(target_os = "macos")]{
                let types = [NSPasteboardTypeFileURL];
                let types_nsarray: ObjcId = msg_send![
                    class!(NSArray),
                    arrayWithObjects: types.as_ptr()
                    count: types.len()
                ];
                let _: () = msg_send![this, registerForDraggedTypes: types_nsarray];
            }
            
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
        cw.send_mouse_down(0, modifiers);
    }
    
    
    extern fn mouse_up(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_mouse_up(0, modifiers);
    }
    
    extern fn right_mouse_down(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_mouse_down(1, modifiers);
    }
    
    extern fn right_mouse_up(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        cw.send_mouse_up(1, modifiers);
    }
    
    extern fn other_mouse_down(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        let button = get_event_mouse_button(event);
        cw.send_mouse_down(button, modifiers);
    }
    
    extern fn other_mouse_up(this: &Object, _sel: Sel, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let modifiers = get_event_key_modifier(event);
        let button = get_event_mouse_button(event);
        cw.send_mouse_up(button, modifiers);
    }
    
    fn mouse_pos_from_event(view: &Object, event: ObjcId) -> DVec2 {
        let window_point: NSPoint = unsafe {msg_send![event, locationInWindow]};
        let view_point = window_point_to_view_point(view, window_point);
        ns_point_to_dvec2(view_point)
    }
    
    fn window_point_to_view_point(view: &Object, window_point: NSPoint) -> NSPoint {
        let view_point: NSPoint = unsafe {msg_send![view, convertPoint: window_point fromView: nil]};
        let view_frame: NSRect = unsafe {msg_send![view, frame]};
        NSPoint {
            x: view_point.x,
            y: view_frame.size.height - view_point.y
        }
    }
    
    fn ns_point_to_dvec2(point: NSPoint) -> DVec2 {
        DVec2 {
            x: point.x,
            y: point.y,
        }
    }
    
    fn mouse_motion(this: &Object, event: ObjcId) {
        let cw = get_cocoa_window(this);
        let pos = mouse_pos_from_event(this, event);
        let modifiers = get_event_key_modifier(event);
        cw.send_mouse_move(event, pos, modifiers);
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
        unsafe {
            let current_cursor = get_macos_app_global().current_cursor.clone();
            let cursor_id = *get_macos_app_global().cursors.entry(current_cursor.clone()).or_insert_with( || {
                load_mouse_cursor(current_cursor.clone())
            });
            let bounds: NSRect = msg_send![this, bounds];
            if let MouseCursor::Hidden = current_cursor{
                let _: () = msg_send![
                    cursor_id,
                    setHiddenUntilMouseMoves: true
                ];
            }
            let _: () = msg_send![
                this,
                addCursorRect: bounds
                cursor: cursor_id
            ];
        }
    }
    
    // NSTextInput protocol
    extern fn marked_range(this: &Object, _sel: Sel) -> NSRange {
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
        unsafe {
            let marked_text: ObjcId = *this.get_ivar("markedText");
            let mutable_string = marked_text.mutable_string();
            let _: () = msg_send![mutable_string, setString: get_apple_class_global().const_empty_string.as_id()];
            let input_context: ObjcId = msg_send![this, inputContext];
            let _: () = msg_send![input_context, discardMarkedText];
        }
    }
    
    extern fn valid_attributes_for_marked_text(_this: &Object, _sel: Sel) -> ObjcId {
        get_apple_class_global().const_attributes_for_marked_text
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
        //let window_rect: NSRect = unsafe {msg_send![cw.window, frame]};
        
        let origin = cw.get_ime_origin();
        //let shift_y = 20.0;
        //let shift_x = 4.0;
        //let bar = 0.0;// (window_rect.size.height - view_rect.size.height) as f32 - 5.;
        NSRect {
            origin: NSPoint {x: (origin.x + cw.ime_spot.x), y: (origin.y + (view_rect.size.height - cw.ime_spot.y))},
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
    /*
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
    }*/
    
    extern fn yes_function(_this: &Object, _se: Sel, _event: ObjcId) -> BOOL {
        YES
    }
    
    
    extern fn display_layer(this: &Object, _: Sel, _calayer: ObjcId) {
        let cw = get_cocoa_window(this);
        cw.send_change_event();
    }
    
    extern fn dragging_session_ended_at_point_operation(this: &Object, _: Sel, _session: ObjcId, _point: NSPoint, _operation: NSDragOperation) {
        let window = get_cocoa_window(this);
        window.do_callback(MacosEvent::DragEnd);
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
        let (items, pos) = get_drag_items_from_pasteboard(this, sender);
        
        /*let pos = ns_point_to_dvec2(window_point_to_view_point(this, unsafe {
            msg_send![sender, draggingLocation]
        }));*/
        
        let response = Arc::new(Mutex::new(DragResponse::None));
        
        let modifiers = unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let ns_event: ObjcId = msg_send![ns_app, currentEvent];
            get_event_key_modifier(ns_event)
        };
        
        window.do_callback(MacosEvent::Drag(DragEvent {
            modifiers,
            handled: Arc::new(Mutex::new(false)),
            abs: pos,
            items,
            response: response.clone()
        }));
        
        let v = response.lock().unwrap();
        match *v{
            DragResponse::None => NSDragOperation::None,
            DragResponse::Copy => NSDragOperation::Copy,
            DragResponse::Link => NSDragOperation::Link,
            DragResponse::Move => NSDragOperation::Move,
        }
    }

    extern fn dragging_ended(this: &Object, _: Sel, _sender: ObjcId) {
        let window = get_cocoa_window(this);
        window.end_live_resize();
    }
    
    fn get_drag_items_from_pasteboard(this: &Object, sender: ObjcId) -> (Arc<Vec<DragItem >>, DVec2) {
        //let window = get_cocoa_window(this);
        let pos = ns_point_to_dvec2(window_point_to_view_point(this, unsafe {
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
        let mut items = Vec::with_capacity(count);
        for index in 0..count {
            let url: ObjcId = unsafe {msg_send![urls, objectAtIndex: index]};
            let url: ObjcId = unsafe {msg_send![url, filePathURL]};
            if url == nil {
                continue;
            }
            let string: ObjcId = unsafe {msg_send![url, absoluteString]};
            if string == nil {
                continue;
            }
            let string = unsafe {CStr::from_ptr(msg_send![string, UTF8String])};
            if let Ok(string) = string.to_str() {
                // lets rip off file:// and #id
                if let Some(string) = string.strip_prefix("file://") {
                    let mut bits = string.split("#makepad_internal_id=");
                    let path = bits.next().unwrap().to_string();
                    let internal_id = if let Some(next) = bits.next() {
                        if let Ok(id) = next.parse::<u64>() {
                            Some(LiveId(id))
                        }
                        else {
                            None
                        }
                    }
                    else {
                        None
                    };
                    items.push(DragItem::FilePath {
                        internal_id,
                        path: if path == "makepad_internal_empty" {"".to_string()}else {path}
                    });
                }
            }
            
        }
        (Arc::new(items), pos)
    }
    
    extern fn perform_drag_operation(this: &Object, _: Sel, sender: ObjcId) {
        //let window = get_cocoa_window(this);
        //window.end_live_resize();
        let modifiers = unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let ns_event: ObjcId = msg_send![ns_app, currentEvent];
            get_event_key_modifier(ns_event)
        };    
        let window = get_cocoa_window(this);
        let (items, pos) = get_drag_items_from_pasteboard(this, sender);
        window.do_callback(MacosEvent::Drop(DropEvent {
            modifiers,
            handled: Arc::new(Mutex::new(false)),
            abs: pos,
            items
        }));
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
        
        #[cfg(target_os = "macos")]{
            decl.add_method(sel!(draggingSession: endedAtPoint: operation:), dragging_session_ended_at_point_operation as extern fn(&Object, Sel, ObjcId, NSPoint, NSDragOperation));
            decl.add_method(sel!(draggingEntered:), dragging_entered as extern fn(&Object, Sel, ObjcId) -> NSDragOperation);
            decl.add_method(sel!(draggingExited:), dragging_exited as extern fn(&Object, Sel, ObjcId));
            decl.add_method(sel!(draggingUpdated:), dragging_updated as extern fn(&Object, Sel, ObjcId) -> NSDragOperation);
            decl.add_method(sel!(performDragOperation:), perform_drag_operation as extern fn(&Object, Sel, ObjcId));
            decl.add_method(sel!(draggingEnded:), dragging_ended as extern fn(&Object, Sel, ObjcId));
        }
    }
    decl.add_ivar::<*mut c_void>("macos_window_ptr");
    decl.add_ivar::<ObjcId>("markedText");
    decl.add_protocol(&Protocol::get("NSTextInputClient").unwrap());
    decl.add_protocol(&Protocol::get("CALayerDelegate").unwrap());
    return decl.register();
}
