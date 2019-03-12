use cocoa::base::{id, nil};
use cocoa::{appkit, foundation};
use cocoa::appkit::{NSApplication, NSEvent, NSWindow, NSView,NSEventMask,NSRunningApplication};
use cocoa::foundation::{ NSRange, NSPoint, NSRect, NSSize, NSUInteger,NSInteger};
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
    pub window:Option<id>
}

impl CocoaWindow{
    pub fn CocoaAppInit(){
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

    pub fn init(&mut self, title:&str){
        unsafe{
            let autoreleasepool = NSAutoreleasePool::new(nil);
            // construct a window with our class
            let window_class = define_cocoa_window_class();
            let window:id = msg_send![window_class, alloc];
            //let window_id:id = msg_send![window_class, alloc];
            
            let window_frame =  NSRect::new(NSPoint::new(0., 0.), NSSize::new(800., 600.));
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

            let title = NSString::alloc(nil).init_str("HELLO WORLD");
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

            let _: () = msg_send![autoreleasepool, drain];
        }
        
    }

    pub fn get_inner_size(&self)->Vec2{
        let view_frame = unsafe { NSView::frame(self.view.unwrap()) };
        vec2(view_frame.size.width as f32, view_frame.size.height as f32)
    }

    unsafe fn ns_event_to_event(&mut self, ns_event: cocoa::base::id) -> Option<Event> {
        if ns_event == cocoa::base::nil {
            return None;
        }

        // FIXME: Despite not being documented anywhere, an `NSEvent` is produced when a user opens
        // Spotlight while the NSApplication is in focus. This `NSEvent` produces a `NSEventType`
        // with value `21`. This causes a SEGFAULT as soon as we try to match on the `NSEventType`
        // enum as there is no variant associated with the value. Thus, we return early if this
        // sneaky event occurs. If someone does find some documentation on this, please fix this by
        // adding an appropriate variant to the `NSEventType` enum in the cocoa-rs crate.
        if ns_event.eventType() as u64 == 21 {
            return None;
        }
        let event_type = ns_event.eventType();
        let ns_window = ns_event.window();
        //let window_id = super::window::get_window_id(ns_window);
        appkit::NSApp().sendEvent_(ns_event);
        
        //event_handler(vec![Event::None]);

        match ns_event.eventType(){
            appkit::NSKeyUp  => {
                            },
            // similar to above, but for `<Cmd-.>`, the keyDown is suppressed instead of the
            // KeyUp, and the above trick does not appear to work.
            appkit::NSKeyDown => {
                /*
                let flags = unsafe {
                    NSEvent::modifierFlags(event)
                };
                ModifiersState {
                    shift: flags.contains(NSEventModifierFlags::NSShiftKeyMask),
                    ctrl: flags.contains(NSEventModifierFlags::NSControlKeyMask),
                    alt: flags.contains(NSEventModifierFlags::NSAlternateKeyMask),
                    logo: flags.contains(NSEventModifierFlags::NSCommandKeyMask),
                }*/
                //let keycode = NSEvent::keyCode(ns_event);
                //if modifiers.logo && keycode == 47 {
                //    modifier_event(ns_event, NSEventModifierFlags::NSCommandKeyMask, false)
                //       .map(into_event)
            },
            appkit::NSFlagsChanged => {
                
            },

            appkit::NSMouseEntered => {
                /*
                let window_point = ns_event.locationInWindow();
                let view_point = if ns_window == cocoa::base::nil {
                    let ns_size = foundation::NSSize::new(0.0, 0.0);
                    let ns_rect = foundation::NSRect::new(window_point, ns_size);
                    let window_rect = window.window.convertRectFromScreen_(ns_rect);
                    window.view.convertPoint_fromView_(window_rect.origin, cocoa::base::nil)
                } else {
                    window.view.convertPoint_fromView_(window_point, cocoa::base::nil)
                };

                let view_rect = NSView::frame(*window.view);
                let x = view_point.x as f64;
                let y = (view_rect.size.height - view_point.y) as f64;
                // event with x/y
                */
            },
            appkit::NSMouseExited => {

            },
            appkit::NSMouseMoved |
            appkit::NSLeftMouseDragged |
            appkit::NSOtherMouseDragged |
            appkit::NSRightMouseDragged => {
                //do studff
            },
            appkit::NSScrollWheel => {
                /*
                let delta = if ns_event.hasPreciseScrollingDeltas() == cocoa::base::YES {
                    PixelDelta((
                        ns_event.scrollingDeltaX() as f64,
                        ns_event.scrollingDeltaY() as f64,
                    ).into())
                } else {
                    // TODO: This is probably wrong
                    LineDelta(
                        ns_event.scrollingDeltaX() as f32,
                        ns_event.scrollingDeltaY() as f32,
                    )
                };
                
                let phase = match ns_event.phase() {
                    NSEventPhase::NSEventPhaseMayBegin | NSEventPhase::NSEventPhaseBegan => TouchPhase::Started,
                    NSEventPhase::NSEventPhaseEnded => TouchPhase::Ended,
                    _ => TouchPhase::Moved,
                };*/
            },

            appkit::NSEventTypePressure => {
                let pressure = ns_event.pressure();
                let stage = ns_event.stage();
            },

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
        }
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
        let _cocoa_window = get_cocoa_window(this);
        NO
    }

    extern fn window_will_close(this: &Object, _: Sel, _: id) {
        let _cocoa_window = get_cocoa_window(this);
    }

    extern fn window_did_resize(this: &Object, _: Sel, _: id) {
        let _cocoa_window = get_cocoa_window(this);
        //WindowDelegate::emit_resize_event(state);
        // WindowDelegate::emit_move_event(state);
    }

    // This won't be triggered if the move was part of a resize.
    extern fn window_did_move(this: &Object, _: Sel, _: id) {
        let _cocoa_window = get_cocoa_window(this);
        //WindowDelegate::emit_move_event(state);
    }

    extern fn window_did_change_screen(this: &Object, _: Sel, _: id) {
        let _cocoa_window = get_cocoa_window(this);
        //let dpi_factor = NSWindow::backingScaleFactor(*state.window) as f64;
        //if state.previous_dpi_factor != dpi_factor {
        //    state.previous_dpi_factor = dpi_factor;
        //    WindowDelegate::emit_event(state, WindowEvent::HiDpiFactorChanged(dpi_factor));
        //    WindowDelegate::emit_resize_event(state);
        //}
    }

    // This will always be called before `window_did_change_screen`.
    extern fn window_did_change_backing_properties(this: &Object, _:Sel, _:id) {
        let _cocoa_window = get_cocoa_window(this);
        /*
        let dpi_factor = NSWindow::backingScaleFactor(*state.window) as f64;
        if state.previous_dpi_factor != dpi_factor {
            state.previous_dpi_factor = dpi_factor;
            WindowDelegate::emit_event(state, WindowEvent::HiDpiFactorChanged(dpi_factor));
            WindowDelegate::emit_resize_event(state);
        }
        }*/
    }

    extern fn window_did_become_key(this: &Object, _: Sel, _: id) {
        let _cocoa_window = get_cocoa_window(this);
        //WindowDelegate::emit_event(state, WindowEvent::Focused(true));
    }

    extern fn window_did_resign_key(this: &Object, _: Sel, _: id) {
        let _cocoa_window = get_cocoa_window(this);
        //WindowDelegate::emit_event(state, WindowEvent::Focused(false));
    }

    // Invoked when the dragged image enters destination bounds or frame
    extern fn dragging_entered(this: &Object, _: Sel, sender: id) -> BOOL {/*
        use cocoa::appkit::NSPasteboard;
        use cocoa::foundation::NSFastEnumeration;
        use std::path::PathBuf;

        let pb: id = unsafe { msg_send![sender, draggingPasteboard] };
        let filenames = unsafe { NSPasteboard::propertyListForType(pb, appkit::NSFilenamesPboardType) };

        for file in unsafe { filenames.iter() } {
            use cocoa::foundation::NSString;
            use std::ffi::CStr;
            let f = NSString::UTF8String(file);
            let path = CStr::from_ptr(f).to_string_lossy().into_owned();
            let cocoa_window = get_cocoa_window(this);
            WindowDelegate::emit_event(state, WindowEvent::HoveredFile(PathBuf::from(path)));
        };*/
        YES
    }

    // Invoked when the image is released
    extern fn prepare_for_drag_operation(_: &Object, _: Sel, _: id) -> BOOL {
        YES
    }

    // Invoked after the released image has been removed from the screen
    extern fn perform_drag_operation(this: &Object, _: Sel, sender: id) -> BOOL {/*
        use cocoa::appkit::NSPasteboard;
        use cocoa::foundation::NSFastEnumeration;
        use std::path::PathBuf;

        let pb: id = unsafe { msg_send![sender, draggingPasteboard] };
        let filenames = unsafe { NSPasteboard::propertyListForType(pb, appkit::NSFilenamesPboardType) };

        for file in unsafe { filenames.iter() } {
            use cocoa::foundation::NSString;
            use std::ffi::CStr;

            unsafe {
                let f = NSString::UTF8String(file);
                let path = CStr::from_ptr(f).to_string_lossy().into_owned();

                let state: *mut c_void = *this.get_ivar("winitState");
                let state = &mut *(state as *mut DelegateState);
                WindowDelegate::emit_event(state, WindowEvent::DroppedFile(PathBuf::from(path)));
            }
        };*/
        YES
    }

    // Invoked when the dragging operation is complete
    extern fn conclude_drag_operation(_: &Object, _: Sel, _: id) {}

    // Invoked when the dragging operation is cancelled
    extern fn dragging_exited(this: &Object, _: Sel, _: id) {
        let _cocoa_window = get_cocoa_window(this);
        //WindowDelegate::emit_event(state, WindowEvent::HoveredFileCancelled);
    }

    // Invoked when entered fullscreen
    extern fn window_did_enter_fullscreen(this: &Object, _: Sel, _: id){
        let _cocoa_window = get_cocoa_window(this);
        //state.win_attribs.borrow_mut().fullscreen = Some(get_current_monitor(*state.window));
        //state.handle_with_fullscreen = false;
    }

    // Invoked when before enter fullscreen
    extern fn window_will_enter_fullscreen(this: &Object, _: Sel, _: id) {
        let _cocoa_window = get_cocoa_window(this);
    }

    // Invoked when exited fullscreen
    extern fn window_did_exit_fullscreen(this: &Object, _: Sel, _: id){
        let _cocoa_window = get_cocoa_window(this);
    }

    extern fn window_did_fail_to_enter_fullscreen(this: &Object, _: Sel, _: id) {
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
            //let state: *mut c_void = *this.get_ivar("winitState");
            //Box::from_raw(state as *mut ViewState);
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

    extern fn draw_rect(this: &Object, _sel: Sel, rect: NSRect) {
        let cocoa_window = get_cocoa_window(this);
        unsafe {
            //TODO, do a redraw?
            let superclass = superclass(this);
            let () = msg_send![super(this, superclass), drawRect:rect];
        }
    }

    extern fn reset_cursor_rects(this: &Object, _sel: Sel) {
        let cocoa_window = get_cocoa_window(this);
        unsafe {
            let bounds: NSRect = msg_send![this, bounds];
            //let cursor = state.cursor.lock().unwrap().load();
            //let _: () = msg_send![this,
             //   addCursorRect:bounds
             //   cursor:cursor
            //];
        }
    }
/*
    extern fn has_marked_text(this: &Object, _sel: Sel) -> BOOL {
        //println!("hasMarkedText");
        unsafe {
            let marked_text: id = *this.get_ivar("markedText");
            (marked_text.length() > 0) as i8
        }
    }

    extern fn marked_range(this: &Object, _sel: Sel) -> NSRange {
        //println!("markedRange");
        unsafe {
            let marked_text: id = *this.get_ivar("markedText");
            let length = marked_text.length();
            if length > 0 {
                NSRange::new(0, length - 1)
            } else {
                util::EMPTY_RANGE
            }
        }
    }*/

    extern fn selected_range(_this: &Object, _sel: Sel) -> NSRange {
        //println!("selectedRange");
        NSRange {
            location: NSInteger::max_value() as NSUInteger,
            length: 0,
        }
    }

    extern fn set_marked_text(
        this: &mut Object,
        _sel: Sel,
        string: id,
        _selected_range: NSRange,
        _replacement_range: NSRange,
    ) {
        /*
        //println!("setMarkedText");
        unsafe {
            let marked_text_ref: &mut id = this.get_mut_ivar("markedText");
            let _: () = msg_send![(*marked_text_ref), release];
            let marked_text = NSMutableAttributedString::alloc(nil);
            let has_attr = msg_send![string, isKindOfClass:class!(NSAttributedString)];
            if has_attr {
                marked_text.initWithAttributedString(string);
            } else {
                marked_text.initWithString(string);
            };
            *marked_text_ref = marked_text;
        }
        */
    }

    extern fn unmark_text(this: &Object, _sel: Sel) {
        /*
        //println!("unmarkText");
        unsafe {
            let marked_text: id = *this.get_ivar("markedText");
            let mutable_string = marked_text.mutableString();
            let _: () = msg_send![mutable_string, setString:""];
            let input_context: id = msg_send![this, inputContext];
            let _: () = msg_send![input_context, discardMarkedText];
        }
        */
    }

    extern fn valid_attributes_for_marked_text(_this: &Object, _sel: Sel) -> id {
        //println!("validAttributesForMarkedText");
        unsafe { msg_send![class!(NSArray), array] }
    }

    extern fn attributed_substring_for_proposed_range(
        _this: &Object,
        _sel: Sel,
        _range: NSRange,
        _actual_range: *mut c_void, // *mut NSRange
    ) -> id {
        //println!("attributedSubstringForProposedRange");
        nil
    }

    extern fn character_index_for_point(_this: &Object, _sel: Sel, _point: NSPoint) -> NSUInteger {
        //println!("characterIndexForPoint");
        0
    }

    extern fn first_rect_for_character_range(this: &Object, _sel: Sel, _range: NSRange, _actual_range: *mut c_void) -> NSRect {
        let cocoa_window = get_cocoa_window(this);
        unsafe {
            /*
            let (x, y) = state.ime_spot.unwrap_or_else(|| {
                let content_rect = NSWindow::contentRectForFrameRect_(
                    state.window,
                    NSWindow::frame(state.window),
                );
                let x = content_rect.origin.x;
                let y = util::bottom_left_to_top_left(content_rect);
                (x, y)
            });
            let content_rect = NSWindow::contentRectForFrameRect_(
                cx.resources.cocoa_window,
                NSWindow::frame(cx.resources.cocoa_window),
            );
            let x = content_rect.origin.x;
            let y = bottom_left_to_top_left(content_rect);*/
            NSRect::new(
                NSPoint::new(0.0, 0.0),// as _, y as _),
                NSSize::new(0.0, 0.0),
            )
        }
    }

    extern fn insert_text(this: &Object, _sel: Sel, string: id, _replacement_range: NSRange) {
        //println!("insertText");
        let cocoa_window = get_cocoa_window(this);
        unsafe {
            /*
            let has_attr = msg_send![string, isKindOfClass:class!(NSAttributedString)];
            let characters = if has_attr {
                // This is a *mut NSAttributedString
                msg_send![string, string]
            } else {
                // This is already a *mut NSString
                string
            };

            let slice = slice::from_raw_parts(
                characters.UTF8String() as *const c_uchar,
                characters.len(),
            );
            let string = str::from_utf8_unchecked(slice);
            */
            /*
            state.is_key_down = true;
            let mut events = VecDeque::with_capacity(characters.len());
            for character in string.chars() {
                events.push_back(Event::WindowEvent {
                    window_id: WindowId(get_window_id(state.window)),
                    event: WindowEvent::ReceivedCharacter(character),
                });
            }*/
        }
    }

    extern fn do_command_by_selector(this: &Object, _sel: Sel, command: Sel) {
        //println!("doCommandBySelector");
        // Basically, we're sent this message whenever a keyboard event that doesn't generate a "human readable" character
        // happens, i.e. newlines, tabs, and Ctrl+C.
        let _cocoa_window = get_cocoa_window(this);
        /*
        unsafe {
            if command == sel!(insertNewline:) {
                // The `else` condition would emit the same character, but I'm keeping this here both...
                // 1) as a reminder for how `doCommandBySelector` works
                // 2) to make our use of carriage return explicit
                //events.push_back(Event::WindowEvent {
                //    window_id: WindowId(get_window_id(state.window)),
                //    event: WindowEvent::ReceivedCharacter('\r'),
                //});
            } else {
                let raw_characters = state.raw_characters.take();
                if let Some(raw_characters) = raw_characters {
                    for character in raw_characters.chars() {
                        //events.push_back(Event::WindowEvent {
                        //    window_id: WindowId(get_window_id(state.window)),
                        //    event: WindowEvent::ReceivedCharacter(character),
                        //});
                    }
                }
            };
        //0}*/
    }
/*
    fn get_characters(event: id, ignore_modifiers: bool) -> String {
        unsafe {
            let characters: id = if ignore_modifiers {
                msg_send![event, charactersIgnoringModifiers]
            } else {
                msg_send![event, characters]
            };

            assert_ne!(characters, nil);
            let slice = slice::from_raw_parts(
                characters.UTF8String() as *const c_uchar,
                characters.len(),
            );

            let string = str::from_utf8_unchecked(slice);

            string.to_owned()
        }
    }
*/
    // Retrieves a layout-independent keycode given an event.
    /*
    fn retrieve_keycode(event: id) -> Option<events::VirtualKeyCode> {
        #[inline]
        fn get_code(ev: id, raw: bool) -> Option<events::VirtualKeyCode> {
            let characters = get_characters(ev, raw);
            characters.chars().next().map_or(None, |c| char_to_keycode(c))
        }

        // Cmd switches Roman letters for Dvorak-QWERTY layout, so we try modified characters first.
        // If we don't get a match, then we fall back to unmodified characters.
        let code = get_code(event, false)
            .or_else(|| {
                get_code(event, true)
            });

        // We've checked all layout related keys, so fall through to scancode.
        // Reaching this code means that the key is layout-independent (e.g. Backspace, Return).
        //
        // We're additionally checking here for F21-F24 keys, since their keycode
        // can vary, but we know that they are encoded
        // in characters property.
        code.or_else(|| {
            let scancode = get_scancode(event);
            scancode_to_keycode(scancode)
                .or_else(|| {
                    check_function_keys(&get_characters(event, true))
                })
        })
    }*/

    extern fn key_down(this: &Object, _sel: Sel, event: id) {
        let _cocoa_window = get_cocoa_window(this);
        //println!("keyDown");
        unsafe {
            //let state_ptr: *mut c_void = *this.get_ivar("winitState");
            //let state = &mut *(state_ptr as *mut ViewState);
            //let window_id = WindowId(get_window_id(state.window));
            //let characters = get_characters(event, false);

           // state.raw_characters = Some(characters.clone());

            //let scancode = get_scancode(event) as u32;
            //let virtual_keycode = retrieve_keycode(event);
           // let is_repeat = msg_send![event, isARepeat];

            /*
            let window_event = Event::WindowEvent {
                window_id,
                event: WindowEvent::KeyboardInput {
                    device_id: DEVICE_ID,
                    input: KeyboardInput {
                        state: ElementState::Pressed,
                        scancode,
                        virtual_keycode,
                        modifiers: event_mods(event),
                    },
                },
            };
            if is_repeat && state.is_key_down{
                for character in characters.chars() {
                    let window_event = Event::WindowEvent {
                        window_id,
                        event: WindowEvent::ReceivedCharacter(character),
                    };
                }
            } else {
                // Some keys (and only *some*, with no known reason) don't trigger `insertText`, while others do...
                // So, we don't give repeats the opportunity to trigger that, since otherwise our hack will cause some
                // keys to generate twice as many characters.
                let array: id = msg_send![class!(NSArray), arrayWithObject:event];
                let (): _ = msg_send![this, interpretKeyEvents:array];
            }*/
        }
    }

    extern fn key_up(this: &Object, _sel: Sel, event: id) {
        let _cocoa_window = get_cocoa_window(this);
        //println!("keyUp");
        unsafe {
            //let state_ptr: *mut c_void = *this.get_ivar("winitState");
            //let state = &mut *(state_ptr as *mut ViewState);
            /*
            state.is_key_down = false;

            let scancode = get_scancode(event) as u32;
            let virtual_keycode = retrieve_keycode(event);

            let window_event = Event::WindowEvent {
                window_id: WindowId(get_window_id(state.window)),
                event: WindowEvent::KeyboardInput {
                    device_id: DEVICE_ID,
                    input: KeyboardInput {
                        state: ElementState::Released,
                        scancode,
                        virtual_keycode,
                        modifiers: event_mods(event),
                    },
                },
            };
            */
        }
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

    fn mouse_click(this: &Object, event: id, button: usize, button_state: usize) {
        let _cocoa_window = get_cocoa_window(this);
        unsafe {
            /*
            let window_event = Event::WindowEvent {
                window_id: WindowId(get_window_id(state.window)),
                event: WindowEvent::MouseInput {
                    device_id: DEVICE_ID,
                    state: button_state,
                    button,
                },
            };*/
        }
    }

    extern fn mouse_down(this: &Object, _sel: Sel, event: id) {
        //mouse_click(this, event, MouseButton::Left, ElementState::Pressed);
    }

    extern fn mouse_up(this: &Object, _sel: Sel, event: id) {
        //mouse_click(this, event, MouseButton::Left, ElementState::Released);
    }

    extern fn right_mouse_down(this: &Object, _sel: Sel, event: id) {
        //mouse_click(this, event, MouseButton::Right, ElementState::Pressed);
    }

    extern fn right_mouse_up(this: &Object, _sel: Sel, event: id) {
        //mouse_click(this, event, MouseButton::Right, ElementState::Released);
    }

    extern fn other_mouse_down(this: &Object, _sel: Sel, event: id) {
        //mouse_click(this, event, MouseButton::Middle, ElementState::Pressed);
    }

    extern fn other_mouse_up(this: &Object, _sel: Sel, event: id) {
        //mouse_click(this, event, MouseButton::Middle, ElementState::Released);
    }

    fn mouse_motion(this: &Object, event: id) {
        let _cocoa_window = get_cocoa_window(this);
        unsafe {
            // We have to do this to have access to the `NSView` trait...
            let view: id = this as *const _ as *mut _;

            //let window_point = event.locationInWindow();
            //let view_point = view.convertPoint_fromView_(window_point, nil);
            //let view_rect = NSView::frame(view);
            /*
            if view_point.x.is_sign_negative()
            || view_point.y.is_sign_negative()
            || view_point.x > view_rect.size.width
            || view_point.y > view_rect.size.height {
                // Point is outside of the client area (view)
                return;
            }*/
            //let x = view_point.x as f64;
            //let y = view_rect.size.height as f64 - view_point.y as f64;
            // use x,y
        }
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
