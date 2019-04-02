// Life is too short for leaky abstractions.
// Gleaned/learned/templated from https://github.com/tomaka/winit/blob/master/src/platform/macos/

use std::collections::HashMap;

use cocoa::base::{id, nil};
use cocoa::{appkit, foundation};
use cocoa::appkit::{NSApplication, NSImage, NSEvent, NSWindow, NSView,NSEventMask,NSRunningApplication};
use cocoa::foundation::{ NSRange, NSPoint, NSDictionary, NSRect, NSSize, NSUInteger,NSInteger};
use cocoa::foundation::{NSAutoreleasePool, NSString};
use objc::runtime::{Class, Object, Protocol, Sel, BOOL, YES, NO};
use objc::declare::ClassDecl;
use std::os::raw::c_void;
use objc::*;
use core_graphics::display::CGDisplay;

use crate::cx::*;

#[derive(Default)]
pub struct CocoaWindow{
    pub window_delegate:Option<id>,
    pub view:Option<id>,
    pub window:Option<id>,
    pub init_resize:bool,
    pub last_size:Vec2,
    pub last_dpi_factor:f32,
    pub fingers_down:Vec<bool>,
    pub cursors:HashMap<MouseCursor, id>,
    pub current_cursor:MouseCursor,
    pub last_mouse_pos:Vec2,
    pub event_callback:Option<*mut FnMut(&mut Vec<Event>)>
}

impl CocoaWindow{
    pub fn cocoa_app_init(){
        unsafe{
            let ns_app = appkit::NSApp();
            if ns_app == nil {
                panic!("App is nil");
            } 
            ns_app.setActivationPolicy_( appkit::NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular);
            ns_app.finishLaunching();
            let current_app = appkit::NSRunningApplication::currentApplication(nil);
            current_app.activateWithOptions_(appkit::NSApplicationActivateIgnoringOtherApps);
        }
    }

    pub fn set_mouse_cursor(&mut self, cursor:MouseCursor){
        if self.current_cursor != cursor{
            self.current_cursor = cursor;
            unsafe{
                let _: () = msg_send![
                    self.window.unwrap(),
                    invalidateCursorRectsForView:self.view.unwrap()
                ];
            }
        }
    }

    pub fn init(&mut self, title:&str){
        unsafe{
            for _i in 0..10{
                self.fingers_down.push(false);
            }
            let autoreleasepool = NSAutoreleasePool::new(nil);
            // construct a window with our class
            let window_class = define_cocoa_window_class();
            let window:id = msg_send![window_class, alloc];
            //let window_id:id = msg_send![window_class, alloc];
            
            let window_frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(800., 600.));
            let window_masks = appkit::NSWindowStyleMask::NSClosableWindowMask |
                appkit::NSWindowStyleMask::NSMiniaturizableWindowMask |
                appkit::NSWindowStyleMask::NSResizableWindowMask |
                appkit::NSWindowStyleMask::NSTitledWindowMask;

            window.initWithContentRect_styleMask_backing_defer_(
                window_frame,
                window_masks,
                appkit::NSBackingStoreBuffered,
                NO
            );
            
            let window_delegate_class = define_cocoa_window_delegate();
            let window_delegate:id = msg_send![window_delegate_class, new];

            (*window_delegate).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
    
            msg_send![window, setDelegate:window_delegate];            

            let title = NSString::alloc(nil).init_str(title);
            window.setReleasedWhenClosed_(NO);
            window.setTitle_(title);
            window.setAcceptsMouseMovedEvents_(YES);

            let view_class = define_cocoa_view_class();
            let view:id = msg_send![view_class, alloc];
            msg_send![view, initWithPtr:self as *mut _ as *mut c_void];
            
            window.setContentView_(view);
            window.makeFirstResponder_(view);
            window.makeKeyAndOrderFront_(nil);

            window.center();

            self.window_delegate = Some(window_delegate);
            self.window = Some(window);
            self.view = Some(view);
            self.last_size = self.get_inner_size();
            self.last_dpi_factor = self.get_dpi_factor();
            let _: () = msg_send![autoreleasepool, drain];
        }
    }

    pub fn set_position(&mut self, pos:Vec2){
        let ns_point =  NSPoint::new(pos.x as f64,CGDisplay::main().pixels_high() as f64 - pos.y as f64);
        unsafe {
            NSWindow::setFrameTopLeftPoint_(self.window.unwrap(), ns_point);
        }        
    }

    pub fn get_inner_size(&self)->Vec2{
        if self.view.is_none(){
            return vec2(0.,0.);
        }
        let view_frame = unsafe { NSView::frame(self.view.unwrap()) };
        vec2(view_frame.size.width as f32, view_frame.size.height as f32)
    }

    pub fn get_dpi_factor(&self)->f32{
        if self.window.is_none(){
            return 1.0;
        }
        unsafe{NSWindow::backingScaleFactor(self.window.unwrap()) as f32}
    }

     unsafe fn ns_event_to_event(&mut self, ns_event: cocoa::base::id) -> Option<Event> {
        if ns_event == cocoa::base::nil {
            return None;
        }

        if ns_event.eventType() as u64 == 21 { // some missing event from cocoa-rs crate
            return None;
        }

        //let event_type = ns_event.eventType();
        //let ns_window = ns_event.window();
        
        appkit::NSApp().sendEvent_(ns_event);

        match ns_event.eventType(){
            appkit::NSKeyUp => {},
            appkit::NSKeyDown => {},
            appkit::NSFlagsChanged => {},
            appkit::NSMouseEntered => {},
            appkit::NSMouseExited => {},
            appkit::NSMouseMoved |
            appkit::NSLeftMouseDragged |
            appkit::NSOtherMouseDragged |
            appkit::NSRightMouseDragged => {},
            appkit::NSScrollWheel => {
                return if ns_event.hasPreciseScrollingDeltas() == cocoa::base::YES {
                    Some(Event::FingerScroll(FingerScrollEvent{
                        scroll:vec2(
                            ns_event.scrollingDeltaX() as f32,
                            -ns_event.scrollingDeltaY() as f32
                        ),
                        abs:vec2(self.last_mouse_pos.x, self.last_mouse_pos.y),
                        rel:vec2(0.,0.),
                        handled:false
                    }))
                } else {
                    Some(Event::FingerScroll(FingerScrollEvent{
                        scroll:vec2(
                            ns_event.scrollingDeltaX() as f32 * 32.,
                            -ns_event.scrollingDeltaY() as f32 * 32.
                        ),
                        abs:vec2(self.last_mouse_pos.x, self.last_mouse_pos.y),
                        rel:vec2(0.,0.),
                        handled:false
                    }))
                }
            },
            appkit::NSEventTypePressure => {},
            appkit::NSApplicationDefined => match ns_event.subtype() {
                appkit::NSEventSubtype::NSApplicationActivatedEventType => {
                },
                _=>(),
            },
            _=>(),
        }
        None
    }

    pub fn poll_events<F>(&mut self, first_block:bool, mut event_handler:F)
    where F: FnMut(&mut Vec<Event>),
    {   
        let mut do_first_block = first_block;

        unsafe{
            self.event_callback = Some(&mut event_handler as *const FnMut(&mut Vec<Event>) as *mut FnMut(&mut Vec<Event>));

            if !self.init_resize{
                self.init_resize = true;
                self.send_resize_event();
            }

            loop{
                let pool = foundation::NSAutoreleasePool::new(cocoa::base::nil);

                let ns_event = appkit::NSApp().nextEventMatchingMask_untilDate_inMode_dequeue_(
                    NSEventMask::NSAnyEventMask.bits() | NSEventMask::NSEventMaskPressure.bits(),
                    if do_first_block{
                        do_first_block = false;
                        foundation::NSDate::distantFuture(cocoa::base::nil)
                    }
                    else{
                        foundation::NSDate::distantPast(cocoa::base::nil)
                    },
                    foundation::NSDefaultRunLoopMode,
                    cocoa::base::YES);
                
                if ns_event == nil{
                    break;
                }

                let event = self.ns_event_to_event(ns_event);

                let _: () = msg_send![pool, release];

                if !event.is_none(){
                    event_handler(&mut vec![event.unwrap()])
                }
            }
            self.event_callback = None;
        }
    }

    pub fn do_callback(&mut self, events:&mut Vec<Event>){
        unsafe{
            if self.event_callback.is_none(){
                return
            };
            let callback = self.event_callback.unwrap();
            (*callback)(events);
        }
    }

    pub fn send_resize_event(&mut self){
        let new_dpi_factor = self.get_dpi_factor();
        let new_size = self.get_inner_size(); 
        let old_dpi_factor = self.last_dpi_factor;
        let old_size = self.last_size;
        self.last_dpi_factor = new_dpi_factor;
        self.last_size = new_size;
        self.do_callback(&mut vec![Event::Resized(ResizedEvent{
            old_size:old_size,
            old_dpi_factor:old_dpi_factor,
            new_size:new_size,
            new_dpi_factor:new_dpi_factor
        })]);
    }

    pub fn send_focus_event(&mut self, focus:bool){
        self.do_callback(&mut vec![Event::AppFocus(focus)]);
    }

    pub fn send_finger_down(&mut self, digit:usize){
        self.fingers_down[digit] = true;
        self.do_callback(&mut vec![Event::FingerDown(FingerDownEvent{
            abs:self.last_mouse_pos,
            rel:self.last_mouse_pos,
            digit:digit,
            handled:false,
            is_touch:false
        })]);
    }

    pub fn send_finger_up(&mut self, digit:usize){
        self.fingers_down[digit] = false;
        self.do_callback(&mut vec![Event::FingerUp(FingerUpEvent{
            abs:self.last_mouse_pos,
            rel:self.last_mouse_pos,
            abs_start:vec2(0., 0.),
            rel_start:vec2(0., 0.),
            digit:digit,
            is_over:false,
            is_touch:false
        })]);
    }

    pub fn send_finger_hover_and_move(&mut self, pos:Vec2){
        self.last_mouse_pos = pos;
        let mut events = Vec::new();
        for (digit, down) in self.fingers_down.iter().enumerate(){
            if *down{
                events.push(Event::FingerMove(FingerMoveEvent{
                    abs:pos,
                    rel:pos,
                    digit:digit,
                    abs_start:vec2(0.,0.),
                    rel_start:vec2(0.,0.),
                    is_over:false,
                    is_touch:false
                }));
            }
        };
        events.push(Event::FingerHover(FingerHoverEvent{
            abs:pos,
            rel:pos,
            handled:false,
            hover_state:HoverState::Over,
        }));
        self.do_callback(&mut events);
    }

    pub fn send_close_requested_event(&mut self){
        self.do_callback(&mut vec![Event::CloseRequested])
    }
}


fn get_cocoa_window(this:&Object)->&mut CocoaWindow{
    unsafe{
        let ptr: *mut c_void = *this.get_ivar("cocoa_window_ptr");
        &mut *(ptr as *mut CocoaWindow)
    }
}

pub fn define_cocoa_window_delegate()->*const Class{
    use std::os::raw::c_void;
    
    extern fn window_should_close(this: &Object, _: Sel, _: id) -> BOOL {
        let cw = get_cocoa_window(this);
        cw.send_close_requested_event();
        NO
    }

    extern fn window_will_close(this: &Object, _: Sel, _: id) {
        let _cw = get_cocoa_window(this);
    }

    extern fn window_did_resize(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_resize_event();
    }

    // This won't be triggered if the move was part of a resize.
    extern fn window_did_move(this: &Object, _: Sel, _: id) {
        let _cw = get_cocoa_window(this);
    }

    extern fn window_did_change_screen(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_resize_event();
    }

    // This will always be called before `window_did_change_screen`.
    extern fn window_did_change_backing_properties(this: &Object, _:Sel, _:id) {
        let cw = get_cocoa_window(this);
        cw.send_resize_event();
    }

    extern fn window_did_become_key(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_focus_event(true);
    }

    extern fn window_did_resign_key(this: &Object, _: Sel, _: id) {
        let cw = get_cocoa_window(this);
        cw.send_focus_event(false);
    }

    // Invoked when the dragged image enters destination bounds or frame
    extern fn dragging_entered(_this: &Object, _: Sel, _sender: id) -> BOOL {
        YES
    }

    // Invoked when the image is released
    extern fn prepare_for_drag_operation(_: &Object, _: Sel, _: id) -> BOOL {
        YES
    }

    // Invoked after the released image has been removed from the screen
    extern fn perform_drag_operation(_this: &Object, _: Sel, _sender: id) -> BOOL {
        YES
    }

    // Invoked when the dragging operation is complete
    extern fn conclude_drag_operation(_: &Object, _: Sel, _: id) {}

    // Invoked when the dragging operation is cancelled
    extern fn dragging_exited(this: &Object, _: Sel, _: id) {
        let _cw = get_cocoa_window(this);
        //WindowDelegate::emit_event(state, WindowEvent::HoveredFileCancelled);
    }

    // Invoked when entered fullscreen
    extern fn window_did_enter_fullscreen(this: &Object, _: Sel, _: id){
        let cw = get_cocoa_window(this);
        cw.send_resize_event();
    }

    // Invoked when before enter fullscreen
    extern fn window_will_enter_fullscreen(this: &Object, _: Sel, _: id) {
        let _cw = get_cocoa_window(this);
    }

    // Invoked when exited fullscreen
    extern fn window_did_exit_fullscreen(this: &Object, _: Sel, _: id){
        let cw = get_cocoa_window(this);
        cw.send_resize_event();
    }

    extern fn window_did_fail_to_enter_fullscreen(_this: &Object, _: Sel, _: id) {
    }

    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("RenderWindowDelegate", superclass).unwrap();

    // Add callback methods
    unsafe{
        decl.add_method(sel!(windowShouldClose:), window_should_close as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(windowWillClose:), window_will_close as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidResize:), window_did_resize as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidMove:), window_did_move as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidChangeScreen:), window_did_change_screen as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidChangeBackingProperties:), window_did_change_backing_properties as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidBecomeKey:), window_did_become_key as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidResignKey:), window_did_resign_key as extern fn(&Object, Sel, id));

        // callbacks for drag and drop events
        decl.add_method(sel!(draggingEntered:), dragging_entered as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(prepareForDragOperation:), prepare_for_drag_operation as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(performDragOperation:), perform_drag_operation as extern fn(&Object, Sel, id) -> BOOL);
        decl.add_method(sel!(concludeDragOperation:), conclude_drag_operation as extern fn(&Object, Sel, id));
        decl.add_method(sel!(draggingExited:), dragging_exited as extern fn(&Object, Sel, id));

        // callbacks for fullscreen events
        decl.add_method(sel!(windowDidEnterFullScreen:), window_did_enter_fullscreen as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowWillEnterFullScreen:), window_will_enter_fullscreen as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidExitFullScreen:), window_did_exit_fullscreen as extern fn(&Object, Sel, id));
        decl.add_method(sel!(windowDidFailToEnterFullScreen:), window_did_fail_to_enter_fullscreen as extern fn(&Object, Sel, id));
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("cocoa_window_ptr");

    return decl.register();
}

pub fn define_cocoa_window_class()->*const Class{
    extern fn yes(_: &Object, _: Sel) -> BOOL {
        YES
    }
 
    let window_superclass = class!(NSWindow);
    let mut decl = ClassDecl::new("RenderWindow", window_superclass).unwrap();
    unsafe{
        decl.add_method(sel!(canBecomeMainWindow), yes as extern fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(canBecomeKeyWindow), yes as extern fn(&Object, Sel) -> BOOL);
    }
    return decl.register();
}

pub fn define_cocoa_view_class()->*const Class{

    extern fn dealloc(this: &Object, _sel: Sel) {
        unsafe {
            let marked_text: id = *this.get_ivar("markedText");
            let _: () = msg_send![marked_text, release];
        }
    }

    extern fn init_with_ptr(this: &Object, _sel: Sel, cx: *mut c_void) -> id {
        unsafe {
            let this: id = msg_send![this, init];
            if this != nil {
                (*this).set_ivar("cocoa_window_ptr", cx);
                //let marked_text = <id as NSMutableAttributedString>::init(
                //    NSMutableAttributedString::alloc(nil),
                //);
                //(*this).set_ivar("markedText", marked_text);
            }
            this
        }
    }

    extern fn mouse_down(this: &Object, _sel: Sel, _event: id) {
        let cw = get_cocoa_window(this);
        cw.send_finger_down(0);
    }

    extern fn mouse_up(this: &Object, _sel: Sel, _event: id) {
        let cw = get_cocoa_window(this);
        cw.send_finger_up(0);
    }

    extern fn right_mouse_down(this: &Object, _sel: Sel, _event: id) {
        let cw = get_cocoa_window(this);
        cw.send_finger_down(1);
    }

    extern fn right_mouse_up(this: &Object, _sel: Sel, _event: id) {
        let cw = get_cocoa_window(this);
        cw.send_finger_up(1);
    }

    extern fn other_mouse_down(this: &Object, _sel: Sel, _event: id) {
        let cw = get_cocoa_window(this);
        cw.send_finger_down(2);
    }

    extern fn other_mouse_up(this: &Object, _sel: Sel, _event: id) {
        let cw = get_cocoa_window(this);
        cw.send_finger_up(2);
    }

    fn mouse_pos_from_event(this: &Object, event: id)->Vec2{
        // We have to do this to have access to the `NSView` trait...
        unsafe{
            let view: id = this as *const _ as *mut _;
            let window_point = event.locationInWindow();
            let view_point = view.convertPoint_fromView_(window_point, nil);
            let view_rect = NSView::frame(view);
            vec2(view_point.x as f32, view_rect.size.height as f32 - view_point.y as f32)
        }
    }

    fn mouse_motion(this: &Object, event: id) {
        let cw = get_cocoa_window(this);
        let pos = mouse_pos_from_event(this, event);
        cw.send_finger_hover_and_move(pos);
    }

    extern fn mouse_moved(this: &Object, _sel: Sel, event: id) {
        mouse_motion(this, event);
    }

    extern fn mouse_dragged(this: &Object, _sel: Sel, event: id) {
        mouse_motion(this, event);
    }

    extern fn right_mouse_dragged(this: &Object, _sel: Sel, event: id) {
        mouse_motion(this, event);
    }

    extern fn other_mouse_dragged(this: &Object, _sel: Sel, event: id) {
        mouse_motion(this, event);
    }

    extern fn draw_rect(this: &Object, _sel: Sel, rect: NSRect) {
        let _cw = get_cocoa_window(this);
        unsafe {
            let superclass = superclass(this);
            let () = msg_send![super(this, superclass), drawRect:rect];
        }
    }

    extern fn reset_cursor_rects(this: &Object, _sel: Sel) {
        let cw = get_cocoa_window(this);
        let current_cursor = cw.current_cursor.clone();
        let cursor_id =  *cw.cursors.entry(current_cursor.clone()).or_insert_with(||{
           load_mouse_cursor(current_cursor.clone())
        });
        unsafe {
            let bounds: NSRect = msg_send![this, bounds];
            let _: () = msg_send![this,
              addCursorRect:bounds
              cursor:cursor_id
            ];
        }
    }

    extern fn selected_range(_this: &Object, _sel: Sel) -> NSRange {
        //println!("selectedRange");
        NSRange {
            location: NSInteger::max_value() as NSUInteger,
            length: 0,
        }
    }

    extern fn set_marked_text(
        _this: &mut Object,
        _sel: Sel,
        _string: id,
        _selected_range: NSRange,
        _replacement_range: NSRange,
    ) {
    }

    extern fn unmark_text(_this: &Object, _sel: Sel) {
    }

    extern fn valid_attributes_for_marked_text(_this: &Object, _sel: Sel) -> id {
        unsafe { msg_send![class!(NSArray), array] }
    }

    extern fn attributed_substring_for_proposed_range(
        _this: &Object,
        _sel: Sel,
        _range: NSRange,
        _actual_range: *mut c_void, // *mut NSRange
    ) -> id {
        nil
    }

    extern fn character_index_for_point(_this: &Object, _sel: Sel, _point: NSPoint) -> NSUInteger {
        0
    }

    extern fn first_rect_for_character_range(this: &Object, _sel: Sel, _range: NSRange, _actual_range: *mut c_void) -> NSRect {
        let _cw = get_cocoa_window(this);
            NSRect::new(
                NSPoint::new(0.0, 0.0),// as _, y as _),
                NSSize::new(0.0, 0.0),
            )
    }

    extern fn insert_text(this: &Object, _sel: Sel, _string: id, _replacement_range: NSRange) {
        let _cw = get_cocoa_window(this);
    }

    extern fn do_command_by_selector(this: &Object, _sel: Sel, _command: Sel) {
        let _cw = get_cocoa_window(this);
    }

    extern fn key_down(this: &Object, _sel: Sel, _event: id) {
        let _cw = get_cocoa_window(this);
    }

    extern fn key_up(this: &Object, _sel: Sel, _event: id) {
        let _cw = get_cocoa_window(this);
    }

    extern fn insert_tab(this: &Object, _sel: Sel, _sender: id) {
        unsafe {
            let window: id = msg_send![this, window];
            let first_responder: id = msg_send![window, firstResponder];
            let this_ptr = this as *const _ as *mut _;
            if first_responder == this_ptr {
                let (): _ = msg_send![window, selectNextKeyView:this];
            }
        }
    }

    extern fn insert_back_tab(this: &Object, _sel: Sel, _sender: id) {
        unsafe {
            let window: id = msg_send![this, window];
            let first_responder: id = msg_send![window, firstResponder];
            let this_ptr = this as *const _ as *mut _;
            if first_responder == this_ptr {
                let (): _ = msg_send![window, selectPreviousKeyView:this];
            }
        }
    }

    extern fn wants_key_down_for_event(_this: &Object, _se: Sel, _event: id) -> BOOL {
        YES
    }

    let superclass = class!(NSView);
    let mut decl = ClassDecl::new("RenderViewClass", superclass).unwrap();
    unsafe{
        decl.add_method(sel!(dealloc), dealloc as extern fn(&Object, Sel));
        decl.add_method(sel!(initWithPtr:), init_with_ptr as extern fn(&Object, Sel, *mut c_void) -> id);
        decl.add_method(sel!(drawRect:), draw_rect as extern fn(&Object, Sel, NSRect));
        decl.add_method(sel!(resetCursorRects), reset_cursor_rects as extern fn(&Object, Sel));
        //decl.add_method(sel!(hasMarkedText), has_marked_text as extern fn(&Object, Sel) -> BOOL);
        //decl.add_method(sel!(markedRange), marked_range as extern fn(&Object, Sel) -> NSRange);
        //decl.add_method(sel!(selectedRange), selected_range as extern fn(&Object, Sel) -> NSRange);
        //decl.add_method(sel!(setMarkedText:selectedRange:replacementRange:),set_marked_text as extern fn(&mut Object, Sel, id, NSRange, NSRange));
        decl.add_method(sel!(unmarkText), unmark_text as extern fn(&Object, Sel));
        decl.add_method(sel!(validAttributesForMarkedText),valid_attributes_for_marked_text as extern fn(&Object, Sel) -> id);
        //decl.add_method(
        //   sel!(attributedSubstringForProposedRange:actualRange:),
        //    attributed_substring_for_proposed_range
        //       as extern fn(&Object, Sel, NSRange, *mut c_void) -> id,
        //);
        //decl.add_method(
        //    sel!(insertText:replacementRange:),
        //    insert_text as extern fn(&Object, Sel, id, NSRange),
        //);
        decl.add_method(
            sel!(characterIndexForPoint:),
            character_index_for_point as extern fn(&Object, Sel, NSPoint) -> NSUInteger,
        );
        //decl.add_method(
        //    sel!(firstRectForCharacterRange:actualRange:),
        //    first_rect_for_character_range
        //        as extern fn(&Object, Sel, NSRange, *mut c_void) -> NSRect,
        //);
        decl.add_method(sel!(doCommandBySelector:),do_command_by_selector as extern fn(&Object, Sel, Sel));
        decl.add_method(sel!(keyDown:), key_down as extern fn(&Object, Sel, id));
        decl.add_method(sel!(keyUp:), key_up as extern fn(&Object, Sel, id));
        //decl.add_method(sel!(insertTab:), insert_tab as extern fn(&Object, Sel, id));
        //decl.add_method(sel!(insertBackTab:), insert_back_tab as extern fn(&Object, Sel, id));
        decl.add_method(sel!(mouseDown:), mouse_down as extern fn(&Object, Sel, id));
        decl.add_method(sel!(mouseUp:), mouse_up as extern fn(&Object, Sel, id));
        decl.add_method(sel!(rightMouseDown:), right_mouse_down as extern fn(&Object, Sel, id));
        decl.add_method(sel!(rightMouseUp:), right_mouse_up as extern fn(&Object, Sel, id));
        decl.add_method(sel!(otherMouseDown:), other_mouse_down as extern fn(&Object, Sel, id));
        decl.add_method(sel!(otherMouseUp:), other_mouse_up as extern fn(&Object, Sel, id));
        decl.add_method(sel!(mouseMoved:), mouse_moved as extern fn(&Object, Sel, id));
        decl.add_method(sel!(mouseDragged:), mouse_dragged as extern fn(&Object, Sel, id));
        decl.add_method(sel!(rightMouseDragged:), right_mouse_dragged as extern fn(&Object, Sel, id));
        decl.add_method(sel!(otherMouseDragged:), other_mouse_dragged as extern fn(&Object, Sel, id));
        decl.add_method(sel!(_wantsKeyDownForEvent:), wants_key_down_for_event as extern fn(&Object, Sel, id) -> BOOL);
    }
    decl.add_ivar::<*mut c_void>("cocoa_window_ptr");
    decl.add_ivar::<id>("markedText");
    let protocol = Protocol::get("NSTextInputClient").unwrap();
    decl.add_protocol(&protocol);

    return decl.register();
}

pub unsafe fn superclass<'a>(this: &'a Object) -> &'a Class {
    let superclass: id = msg_send![this, superclass];
    &*(superclass as *const _)
}

pub fn bottom_left_to_top_left(rect: NSRect) -> f64 {
    CGDisplay::main().pixels_high() as f64 - (rect.origin.y + rect.size.height)
}

fn load_mouse_cursor(cursor:MouseCursor)->id{
    match cursor {
        MouseCursor::Arrow | MouseCursor::Default | MouseCursor::Hidden => load_native_cursor("arrowCursor"),
        MouseCursor::Hand => load_native_cursor("pointingHandCursor"),
        MouseCursor::Grabbing | MouseCursor::Grab => load_native_cursor("closedHandCursor"),
        MouseCursor::Text => load_native_cursor("IBeamCursor"),
        MouseCursor::VerticalText => load_native_cursor("IBeamCursorForVerticalLayout"),
        MouseCursor::Copy => load_native_cursor("dragCopyCursor"),
        MouseCursor::Alias => load_native_cursor("dragLinkCursor"),
        MouseCursor::NotAllowed | MouseCursor::NoDrop => load_native_cursor("operationNotAllowedCursor"),
        MouseCursor::ContextMenu => load_native_cursor("contextualMenuCursor"),
        MouseCursor::Crosshair => load_native_cursor("crosshairCursor"),
        MouseCursor::EResize => load_native_cursor("resizeRightCursor"),
        MouseCursor::NResize => load_native_cursor("resizeUpCursor"),
        MouseCursor::WResize => load_native_cursor("resizeLeftCursor"),
        MouseCursor::SResize => load_native_cursor("resizeDownCursor"),
        MouseCursor::EwResize | MouseCursor::ColResize => load_native_cursor("resizeLeftRightCursor"),
        MouseCursor::NsResize | MouseCursor::RowResize => load_native_cursor("resizeUpDownCursor"),

        // Undocumented cursors: https://stackoverflow.com/a/46635398/5435443
        MouseCursor::Help => load_undocumented_cursor("_helpCursor"),
        MouseCursor::ZoomIn => load_undocumented_cursor("_zoomInCursor"),
        MouseCursor::ZoomOut => load_undocumented_cursor("_zoomOutCursor"),
        MouseCursor::NeResize => load_undocumented_cursor("_windowResizeNorthEastCursor"),
        MouseCursor::NwResize => load_undocumented_cursor("_windowResizeNorthWestCursor"),
        MouseCursor::SeResize => load_undocumented_cursor("_windowResizeSouthEastCursor"),
        MouseCursor::SwResize => load_undocumented_cursor("_windowResizeSouthWestCursor"),
        MouseCursor::NeswResize => load_undocumented_cursor("_windowResizeNorthEastSouthWestCursor"),
        MouseCursor::NwseResize => load_undocumented_cursor("_windowResizeNorthWestSouthEastCursor"),

        // While these are available, the former just loads a white arrow,
        // and the latter loads an ugly deflated beachball!
        // MouseCursor::Move => Cursor::Undocumented("_moveCursor"),
        // MouseCursor::Wait => Cursor::Undocumented("_waitCursor"),
        // An even more undocumented cursor...
        // https://bugs.eclipse.org/bugs/show_bug.cgi?id=522349
        // This is the wrong semantics for `Wait`, but it's the same as
        // what's used in Safari and Chrome.
        MouseCursor::Wait | MouseCursor::Progress => load_undocumented_cursor("busyButClickableCursor"),

        // For the rest, we can just snatch the cursors from WebKit...
        // They fit the style of the native cursors, and will seem
        // completely standard to macOS users.
        // https://stackoverflow.com/a/21786835/5435443
        MouseCursor::Move | MouseCursor::AllScroll => load_webkit_cursor("move"),
        MouseCursor::Cell => load_webkit_cursor("cell"),
    }
}

fn load_native_cursor(cursor_name:&str)->id{
    let sel = Sel::register(cursor_name);
    let id:id = unsafe{msg_send![class!(NSCursor), performSelector:sel]};
    id
}

fn load_undocumented_cursor(cursor_name:&str)->id{
    unsafe{
        let class = class!(NSCursor);
        let sel = Sel::register(cursor_name);
        let sel = msg_send![class, respondsToSelector:sel];
        let id:id = msg_send![class, performSelector:sel];
        id
    }
}

fn load_webkit_cursor(cursor_name_str: &str) -> id {
    unsafe{
        static CURSOR_ROOT: &'static str = "/System/Library/Frameworks/ApplicationServices.framework/Versions/A/Frameworks/HIServices.framework/Versions/A/Resources/cursors";
        let cursor_root = NSString::alloc(nil).init_str(CURSOR_ROOT);
        let cursor_name = NSString::alloc(nil).init_str(cursor_name_str);
        let cursor_pdf = NSString::alloc(nil).init_str("cursor.pdf");
        let cursor_plist = NSString::alloc(nil).init_str("info.plist");
        let key_x = NSString::alloc(nil).init_str("hotx");
        let key_y = NSString::alloc(nil).init_str("hoty");

        let cursor_path: id = msg_send![cursor_root,
            stringByAppendingPathComponent:cursor_name
        ];
        let pdf_path: id = msg_send![cursor_path,
            stringByAppendingPathComponent:cursor_pdf
        ];
        let info_path: id = msg_send![cursor_path,
            stringByAppendingPathComponent:cursor_plist
        ];

        let image = NSImage::alloc(nil).initByReferencingFile_(pdf_path);
        let info = NSDictionary::dictionaryWithContentsOfFile_(nil, info_path);

        let x = info.valueForKey_(key_x);
        let y = info.valueForKey_(key_y);
        let point = NSPoint::new(
            msg_send![x, doubleValue],
            msg_send![y, doubleValue],
        );
        let cursor: id = msg_send![class!(NSCursor), alloc];
        msg_send![cursor, initWithImage:image hotSpot:point]
    }
}