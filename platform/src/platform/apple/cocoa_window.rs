use {
    std::{
        ptr,
        time::Instant,
        os::raw::{c_void}
    },
    crate::{
        makepad_math::{
            Vec2,
        },
        platform::{
            apple::frameworks::*,
            apple::apple_util::{
                str_to_nsstring,
            },
            cocoa_app::CocoaApp,
        },
        event::{
            WindowGeom,
            NUM_FINGERS,
            Event,
            FingerInputType,
            WindowDragQueryResponse,
            WindowResizeLoopEvent,
            WindowGeomChangeEvent,
            WindowDragQueryEvent,
            KeyModifiers,
            FingerDownEvent,
            FingerUpEvent,
            FingerMoveEvent,
            FingerHoverEvent,
            WindowCloseRequestedEvent,
            WindowClosedEvent,
            TextInputEvent,
            DraggedItem,
            //HoverState
        },
        //turtle::Rect,
    }
};

#[derive(Clone)]
pub struct CocoaWindow {
    pub window_id: usize,
    pub window_delegate: ObjcId,
    //pub layer_delegate: id,
    pub view: ObjcId,
    pub window: ObjcId,
    pub live_resize_timer: ObjcId,
    pub cocoa_app: *mut CocoaApp,
    pub last_window_geom: Option<WindowGeom>,
    pub ime_spot: Vec2,
    pub time_start: Instant,
    pub is_fullscreen: bool,
    pub fingers_down: Vec<bool>,
    pub last_mouse_pos: Vec2,
}

impl CocoaWindow {
    
    pub fn new(cocoa_app: &mut CocoaApp, window_id: usize) -> CocoaWindow {
        unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
            
            let window: ObjcId = msg_send![cocoa_app.window_class, alloc];
            let window_delegate: ObjcId = msg_send![cocoa_app.window_delegate_class, new];
            let view: ObjcId = msg_send![cocoa_app.view_class, alloc];
            
            let () = msg_send![pool, drain];
            cocoa_app.cocoa_windows.push((window, view));
            CocoaWindow {
                is_fullscreen: false,
                time_start: cocoa_app.time_start,
                live_resize_timer: nil,
                cocoa_app: cocoa_app,
                window_delegate: window_delegate,
                //layer_delegate:layer_delegate,
                window: window,
                window_id: window_id,
                view: view,
                last_window_geom: None,
                ime_spot: Vec2::default(),
                fingers_down: Vec::new(),
                last_mouse_pos: Vec2::default(),
            }
        }
    }
    
    // complete window initialization with pointers to self
    pub fn init(&mut self, title: &str, size: Vec2, position: Option<Vec2>) {
        unsafe {
            //(*self.cocoa_app).init_app_after_first_window();
            self.fingers_down.resize(NUM_FINGERS, false);
            
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
            
            // set the backpointeers
            (*self.window_delegate).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
            //(*self.layer_delegate).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
            let () = msg_send![self.view, initWithPtr: self as *mut _ as *mut c_void];
            
            let left_top = if let Some(position) = position {
                NSPoint {x: position.x as f64, y: position.y as f64}
            }
            else {
                NSPoint {x: 0., y: 0.}
            };
            let ns_size = NSSize {width: size.x as f64, height: size.y as f64};
            let window_frame = NSRect {origin: left_top, size: ns_size};
            let window_masks = NSWindowStyleMask::NSClosableWindowMask as u64
                | NSWindowStyleMask::NSMiniaturizableWindowMask as u64
                | NSWindowStyleMask::NSResizableWindowMask as u64
                | NSWindowStyleMask::NSTitledWindowMask as u64
                | NSWindowStyleMask::NSFullSizeContentViewWindowMask as u64;
            
            let () = msg_send![
                self.window,
                initWithContentRect: window_frame
                styleMask: window_masks as u64
                backing: NSBackingStoreType::NSBackingStoreBuffered as u64
                defer: NO
            ];
            
            let () = msg_send![self.window, setDelegate: self.window_delegate];
            
            let title = str_to_nsstring(title);
            let () = msg_send![self.window, setReleasedWhenClosed: NO];
            let () = msg_send![self.window, setTitle: title];
            let () = msg_send![self.window, setTitleVisibility: NSWindowTitleVisibility::NSWindowTitleHidden];
            let () = msg_send![self.window, setTitlebarAppearsTransparent: YES];
            
            //let subviews:id = msg_send![self.window, getSubviews];
            //println!("{}", subviews as u64);
            let () = msg_send![self.window, setAcceptsMouseMovedEvents: YES];
            
            let () = msg_send![self.view, setLayerContentsRedrawPolicy: 2]; //duringViewResize
            
            let () = msg_send![self.window, setContentView: self.view];
            let () = msg_send![self.window, makeFirstResponder: self.view];
            let () = msg_send![self.window, makeKeyAndOrderFront: nil];
            
            
            let rect = NSRect {
                origin: NSPoint {x: 0., y: 0.},
                size: ns_size
            };
            let track: ObjcId = msg_send![class!(NSTrackingArea), alloc];
            let track: ObjcId = msg_send![
                track,
                initWithRect: rect
                options: NSTrackignActiveAlways
                    | NSTrackingInVisibleRect
                    | NSTrackingMouseEnteredAndExited
                    | NSTrackingMouseMoved
                    | NSTrackingCursorUpdate
                owner: self.view
                userInfo: nil
            ];
            let () = msg_send![self.view, addTrackingArea: track];
            
            if position.is_none() {
                let () = msg_send![self.window, center];
            }
            let input_context: ObjcId = msg_send![self.view, inputContext];
            let () = msg_send![input_context, invalidateCharacterCoordinates];
            
            let () = msg_send![pool, drain];
        }
    }
    
    pub fn update_ptrs(&mut self) {
        unsafe {
            //(*self.layer_delegate).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
            (*self.window_delegate).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
            (*self.view).set_ivar("cocoa_window_ptr", self as *mut _ as *mut c_void);
        }
    }
    
    pub fn set_ime_spot(&mut self, spot: Vec2) {
        self.ime_spot = spot;
    }
    
    pub fn start_live_resize(&mut self) {
        unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
            let cocoa_app = &(*self.cocoa_app);
            self.live_resize_timer = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: 0.01666666
                target: cocoa_app.timer_delegate_instance
                selector: sel!(receivedLiveResize:)
                userInfo: nil
                repeats: YES
            ];
            let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
            let () = msg_send![nsrunloop, addTimer: self.live_resize_timer forMode: NSRunLoopCommonModes];
            
            let () = msg_send![pool, release];
        }
        
        let mut events = vec![
            Event::WindowResizeLoop(WindowResizeLoopEvent {
                window_id: self.window_id,
                was_started: true
            })
        ];
        self.do_callback(&mut events);
    }
    
    pub fn end_live_resize(&mut self) {
        unsafe {
            let () = msg_send![self.live_resize_timer, invalidate];
            self.live_resize_timer = nil;
        }
        let mut events = vec![
            Event::WindowResizeLoop(WindowResizeLoopEvent {
                window_id: self.window_id,
                was_started: false
            })
        ];
        self.do_callback(&mut events);
    }
    
    pub fn close_window(&mut self) {
        unsafe {
            (*self.cocoa_app).event_recur_block = false;
            let () = msg_send![self.window, close];
        }
    }
    
    pub fn restore(&mut self) {
        unsafe {
            let () = msg_send![self.window, toggleFullScreen: nil];
        }
    }
    
    pub fn maximize(&mut self) {
        unsafe {
            let () = msg_send![self.window, toggleFullScreen: nil];
        }
    }
    
    pub fn minimize(&mut self) {
        unsafe {
            let () = msg_send![self.window, miniaturize: nil];
        }
    }
    
    pub fn set_topmost(&mut self, _topmost: bool) {
    }
    
    
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_micros() as f64 / 1_000_000.0
    }
    
    pub fn get_window_geom(&self) -> WindowGeom {
        WindowGeom {
            xr_is_presenting: false,
            xr_can_present: false,
            is_topmost: false,
            is_fullscreen: self.is_fullscreen,
            can_fullscreen: false,
            inner_size: self.get_inner_size(),
            outer_size: self.get_outer_size(),
            dpi_factor: self.get_dpi_factor(),
            position: self.get_position()
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Event>) {
        unsafe {
            (*self.cocoa_app).do_callback(events);
        }
    }
    
    pub fn set_position(&mut self, pos: Vec2) {
        let mut window_frame: NSRect = unsafe {msg_send![self.window, frame]};
        window_frame.origin.x = pos.x as f64;
        window_frame.origin.y = pos.y as f64;
        //not very nice: CGDisplay::main().pixels_high() as f64
        unsafe {let () = msg_send![self.window, setFrame: window_frame display: YES];};
    }
    
    pub fn get_position(&self) -> Vec2 {
        let window_frame: NSRect = unsafe {msg_send![self.window, frame]};
        Vec2 {x: window_frame.origin.x as f32, y: window_frame.origin.y as f32}
    }
    
    pub fn get_ime_origin(&self) -> Vec2 {
        let rect = NSRect {
            origin: NSPoint {x: 0.0, y: 0.0},
            //view_frame.size.height),
            size: NSSize {width: 0.0, height: 0.0},
        };
        let out: NSRect = unsafe {msg_send![self.window, convertRectToScreen: rect]};
        Vec2 {x: out.origin.x as f32, y: out.origin.y as f32}
    }
    
    pub fn get_inner_size(&self) -> Vec2 {
        let view_frame: NSRect = unsafe {msg_send![self.view, frame]};
        Vec2 {x: view_frame.size.width as f32, y: view_frame.size.height as f32}
    }
    
    pub fn get_outer_size(&self) -> Vec2 {
        let window_frame: NSRect = unsafe {msg_send![self.window, frame]};
        Vec2 {x: window_frame.size.width as f32, y: window_frame.size.height as f32}
    }
    
    pub fn set_outer_size(&self, size: Vec2) {
        let mut window_frame: NSRect = unsafe {msg_send![self.window, frame]};
        window_frame.size.width = size.x as f64;
        window_frame.size.height = size.y as f64;
        unsafe {let () = msg_send![self.window, setFrame: window_frame display: YES];};
    }
    
    pub fn get_dpi_factor(&self) -> f32 {
        let scale: f64 = unsafe {msg_send![self.window, backingScaleFactor]};
        scale as f32
    }
    
    pub fn send_change_event(&mut self) {
        //return;
        let new_geom = self.get_window_geom();
        let old_geom = if let Some(old_geom) = &self.last_window_geom {
            old_geom.clone()
        }
        else {
            new_geom.clone()
        };
        self.last_window_geom = Some(new_geom.clone());
        self.do_callback(&mut vec![
            Event::WindowGeomChange(WindowGeomChangeEvent {
                window_id: self.window_id,
                old_geom: old_geom,
                new_geom: new_geom
            }),
            Event::Paint
        ]);
        // we should schedule a timer for +16ms another Paint
        
    }
    
    pub fn send_focus_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocus]);
    }
    
    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocusLost]);
    }
    
    pub fn mouse_down_can_drag_window(&mut self) -> bool {
        let mut events = vec![
            Event::WindowDragQuery(WindowDragQueryEvent {
                window_id: self.window_id,
                abs: self.last_mouse_pos,
                response: WindowDragQueryResponse::NoAnswer
            })
        ];
        self.do_callback(&mut events);
        match &events[0] {
            Event::WindowDragQuery(wd) => match &wd.response {
                WindowDragQueryResponse::Client => (),
                WindowDragQueryResponse::Caption | WindowDragQueryResponse::SysMenu => {
                    // we start a window drag
                    return true
                },
                _ => ()
            },
            _ => ()
        }
        return false
    }
    
    pub fn send_finger_down(&mut self, digit: usize, modifiers: KeyModifiers) {
        let () = unsafe{msg_send![self.window, makeFirstResponder: self.view]};
        self.fingers_down[digit] = true;
        self.do_callback(&mut vec![Event::FingerDown(FingerDownEvent {
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            digit: digit,
            handled: false,
            input_type: FingerInputType::Mouse,
            modifiers: modifiers,
            tap_count: 0,
            time: self.time_now()
        })]);
    }
    
    pub fn send_finger_up(&mut self, digit: usize, modifiers: KeyModifiers) {
        self.fingers_down[digit] = false;
        self.do_callback(&mut vec![Event::FingerUp(FingerUpEvent {
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            digit: digit,
            input_type: FingerInputType::Mouse,
            modifiers: modifiers,
            time: self.time_now()
        })]);
    }
    
    pub fn send_finger_hover_and_move(&mut self, event: ObjcId, pos: Vec2, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        let mut events = Vec::new();
        
        unsafe {(*self.cocoa_app).startup_focus_hack();}
        
        for (digit, down) in self.fingers_down.iter().enumerate() {
            if *down {
                events.push(Event::FingerMove(FingerMoveEvent {
                    window_id: self.window_id,
                    abs: pos,
                    digit: digit,
                    input_type: FingerInputType::Mouse,
                    modifiers: modifiers.clone(),
                    time: self.time_now()
                }));
            }
        };
        events.push(Event::FingerHover(FingerHoverEvent {
            digit: 0,
            window_id: self.window_id,
            abs: pos,
            //rel: pos,
            //any_down: false,
            //rect: Rect::default(),
            handled: false,
            //hover_state: HoverState::Over,
            modifiers: modifiers,
            time: self.time_now()
        }));
        
        unsafe {(*self.cocoa_app).ns_event = event};
        self.do_callback(&mut events);
        unsafe {(*self.cocoa_app).ns_event = ptr::null_mut()};
    }
    
    pub fn send_window_close_requested_event(&mut self) -> bool {
        let mut events = vec![Event::WindowCloseRequested(WindowCloseRequestedEvent {
            window_id: self.window_id,
            accept_close: true
        })];
        self.do_callback(&mut events);
        if let Event::WindowCloseRequested(cre) = &events[0] {
            return cre.accept_close
        }
        true
    }
    
    pub fn send_window_closed_event(&mut self) {
        self.do_callback(&mut vec![Event::WindowClosed(WindowClosedEvent {
            window_id: self.window_id
        })])
    }
    
    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(&mut vec![Event::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last
        })])
    }
    
    pub fn start_dragging(&mut self, ns_event: ObjcId, dragged_item: DraggedItem) {
        let dragging_items = dragged_item.file_urls.iter().map( | file_url | {
            let pasteboard_item: ObjcId = unsafe {msg_send![class!(NSPasteboardItem), new]};
            let _: () = unsafe {
                msg_send![
                    pasteboard_item,
                    setString: str_to_nsstring(file_url)
                    forType: NSPasteboardTypeFileURL
                ]
            };
            let dragging_item: ObjcId = unsafe {msg_send![class!(NSDraggingItem), alloc]};
            let _: () = unsafe {msg_send![dragging_item, initWithPasteboardWriter: pasteboard_item]};
            let bounds: NSRect = unsafe {msg_send![self.view, bounds]};
            let _: () = unsafe {
                msg_send![dragging_item, setDraggingFrame: bounds contents: self.view]
            };
            dragging_item
        }).collect::<Vec<_ >> ();
        let dragging_items: ObjcId = unsafe {
            msg_send![
                class!(NSArray),
                arrayWithObjects: dragging_items.as_ptr()
                count: dragging_items.len()
            ]
        };
        
        unsafe {
            msg_send![
                self.view,
                beginDraggingSessionWithItems: dragging_items
                event: ns_event
                source: self.view
            ]
        }
        
        /*
         self.delegate?.cellClick(self ,index:self.index)
        //
        let pasteboardItem = NSPasteboardItem()
        pasteboardItem.setString(zText!.stringValue, forType:.string)
        let draggingItem = NSDraggingItem(pasteboardWriter: pasteboardItem)
        draggingItem.setDraggingFrame(self.bounds, contents:self)
        beginDraggingSession(with: [draggingItem], event: event, source: self.zIcon.image)
        */
        
        // TODO
    }
}

pub fn get_cocoa_window(this: &Object) -> &mut CocoaWindow {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar("cocoa_window_ptr");
        &mut *(ptr as *mut CocoaWindow)
    }
}