use {
    crate::{
        area::Area,
        event::{
            finger::MouseButton, DragItem, KeyModifiers, MouseDownEvent, MouseMoveEvent,
            MouseUpEvent, ScrollEvent, TextInputEvent, WindowCloseRequestedEvent,
            WindowClosedEvent, WindowDragQueryEvent, WindowDragQueryResponse, WindowGeom,
            WindowGeomChangeEvent,
        },
        makepad_math::Vec2d,
        os::{
            apple::apple_sys::*,
            apple::apple_util::str_to_nsstring,
            macos::{
                macos_app::{get_macos_class_global, with_macos_app, MacosApp},
                macos_event::MacosEvent,
            },
        },
        window::WindowId,
    },
    std::{cell::Cell, os::raw::c_void, rc::Rc},
};

#[derive(Clone)]
pub struct MacosWindow {
    pub(crate) window_id: WindowId,
    pub(crate) view: ObjcId,
    pub(crate) window: ObjcId,
    pub(crate) ime_spot: Vec2d,
    pub(crate) is_fullscreen: bool,
    pub(crate) last_mouse_pos: Vec2d,
    window_delegate: ObjcId,
    live_resize_timer: ObjcId,
    last_window_geom: Option<WindowGeom>,
}

impl MacosWindow {
    pub fn new(window_id: WindowId) -> MacosWindow {
        unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];

            let window: ObjcId = msg_send![get_macos_class_global().window, alloc];
            let window_delegate: ObjcId = msg_send![get_macos_class_global().window_delegate, new];
            let view: ObjcId = msg_send![get_macos_class_global().view, alloc];

            let () = msg_send![pool, drain];
            with_macos_app(|app| app.cocoa_windows.push((window, view)));
            MacosWindow {
                is_fullscreen: false,
                live_resize_timer: nil,
                window_delegate: window_delegate,
                window: window,
                window_id: window_id,
                view: view,
                last_window_geom: None,
                ime_spot: Vec2d::default(),
                last_mouse_pos: Vec2d::default(),
            }
        }
    }

    // complete window initialization with pointers to self
    pub fn init(&mut self, title: &str, size: Vec2d, position: Option<Vec2d>, is_fullscreen: bool) {
        unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];

            // set the backpointeers
            (*self.window_delegate).set_ivar("macos_window_ptr", self as *mut _ as *mut c_void);
            let () = msg_send![self.view, initWithPtr: self as *mut _ as *mut c_void];

            let left_top = if let Some(position) = position {
                NSPoint {
                    x: position.x as f64,
                    y: position.y as f64,
                }
            } else {
                NSPoint { x: 0., y: 0. }
            };
            let ns_size = NSSize {
                width: size.x as f64,
                height: size.y as f64,
            };
            let window_frame = NSRect {
                origin: left_top,
                size: ns_size,
            };
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
                origin: NSPoint { x: 0., y: 0. },
                size: ns_size,
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
            if is_fullscreen {
                self.maximize();
            }

            // Set application icon from default icon
            Self::set_application_icon();

            let () = msg_send![pool, drain];
        }
    }

    /// Set the application dock icon from the default Makepad icon (RGBA8 bitmap).
    unsafe fn set_application_icon() {
        let icon = crate::app_icon::window_icon();
        let buf = match icon.buffers.first() {
            Some(b) => b,
            None => return,
        };
        let width = buf.width as usize;
        let height = buf.height as usize;

        let bitmap_rep: ObjcId = msg_send![class!(NSBitmapImageRep), alloc];
        let bitmap_rep: ObjcId = msg_send![bitmap_rep,
            initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>()
            pixelsWide: width as i64
            pixelsHigh: height as i64
            bitsPerSample: 8i64
            samplesPerPixel: 4i64
            hasAlpha: YES
            isPlanar: NO
            colorSpaceName: str_to_nsstring("NSDeviceRGBColorSpace")
            bytesPerRow: (width * 4) as i64
            bitsPerPixel: 32i64
        ];
        if bitmap_rep == nil {
            return;
        }

        let bitmap_data: *mut u8 = msg_send![bitmap_rep, bitmapData];
        if !bitmap_data.is_null() {
            std::ptr::copy_nonoverlapping(buf.data.as_ptr(), bitmap_data, width * height * 4);
        }

        let size = NSSize {
            width: width as f64,
            height: height as f64,
        };
        let ns_image: ObjcId = msg_send![class!(NSImage), alloc];
        let ns_image: ObjcId = msg_send![ns_image, initWithSize: size];
        let () = msg_send![ns_image, addRepresentation: bitmap_rep];

        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![ns_app, setApplicationIconImage: ns_image];
    }

    pub fn set_ime_spot(&mut self, spot: Vec2d) {
        self.ime_spot = spot;
    }

    pub fn start_live_resize(&mut self) {
        if self.live_resize_timer != nil {
            return;
        }
        unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
            let timer_delegate_instance = with_macos_app(|app| app.timer_delegate_instance);
            self.live_resize_timer = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: 0.01666666
                target: timer_delegate_instance
                selector: sel!(receivedLiveResize:)
                userInfo: nil
                repeats: YES
            ];
            let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
            let () = msg_send![nsrunloop, addTimer: self.live_resize_timer forMode: NSRunLoopCommonModes];

            let () = msg_send![pool, release];
        }

        self.do_callback(MacosEvent::WindowResizeLoopStart(self.window_id));
    }

    pub fn end_live_resize(&mut self) {
        unsafe {
            if self.live_resize_timer != nil {
                let () = msg_send![self.live_resize_timer, invalidate];
                self.live_resize_timer = nil;
            }
        }
        self.do_callback(MacosEvent::WindowResizeLoopStop(self.window_id));
    }

    pub fn close_window(&mut self) {
        unsafe {
            //get_macos_app_global();
            let () = msg_send![self.window, close];
        }
    }

    pub fn restore(&mut self) {
        unsafe {
            let () = msg_send![self.window, toggleFullScreen: nil];
        }
    }
    pub fn hide(&mut self) {
        unsafe {
            let () = msg_send![self.window, orderOut: nil];
        }
    }
    pub fn deminiaturize(&mut self) {
        unsafe {
            let () = msg_send![self.window, deminiaturize: nil];
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

    pub fn set_window_buttons_visible(&mut self, visible: bool) {
        unsafe {
            let close: ObjcId = msg_send![self.window, standardWindowButton: 0u64];
            let miniaturize: ObjcId = msg_send![self.window, standardWindowButton: 1u64];
            let zoom: ObjcId = msg_send![self.window, standardWindowButton: 2u64];
            let hidden = if visible { NO } else { YES };
            let () = msg_send![close, setHidden: hidden];
            let () = msg_send![miniaturize, setHidden: hidden];
            let () = msg_send![zoom, setHidden: hidden];
        }
    }

    pub fn time_now(&self) -> f64 {
        with_macos_app(|app| app.time_now())
    }

    pub fn get_window_geom(&self) -> WindowGeom {
        WindowGeom {
            xr_is_presenting: false,
            is_topmost: false,
            is_fullscreen: self.is_fullscreen,
            can_fullscreen: false,
            inner_size: self.get_inner_size(),
            outer_size: self.get_outer_size(),
            dpi_factor: self.get_dpi_factor(),
            position: self.get_position(),
        }
    }

    pub fn do_callback(&mut self, event: MacosEvent) {
        MacosApp::do_callback(event);
    }

    pub fn set_position(&mut self, pos: Vec2d) {
        let mut window_frame: NSRect = unsafe { msg_send![self.window, frame] };
        window_frame.origin.x = pos.x as f64;
        window_frame.origin.y = pos.y as f64;
        //not very nice: CGDisplay::main().pixels_high() as f64
        unsafe {
            let () = msg_send![self.window, setFrame: window_frame display: YES];
        };
    }

    pub fn get_position(&self) -> Vec2d {
        let window_frame: NSRect = unsafe { msg_send![self.window, frame] };
        Vec2d {
            x: window_frame.origin.x,
            y: window_frame.origin.y,
        }
    }

    pub fn get_ime_origin(&self) -> Vec2d {
        let shift_x = 5.0; // unknown why
        let shift_y = -10.0;
        let rect = NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            //view_frame.size.height),
            size: NSSize {
                width: 0.0,
                height: 0.0,
            },
        };
        let out: NSRect = unsafe { msg_send![self.window, convertRectToScreen: rect] };
        Vec2d {
            x: out.origin.x + shift_x,
            y: out.origin.y + shift_y,
        }
    }

    pub fn get_inner_size(&self) -> Vec2d {
        let view_frame: NSRect = unsafe { msg_send![self.view, frame] };
        Vec2d {
            x: view_frame.size.width,
            y: view_frame.size.height,
        }
    }

    pub fn get_outer_size(&self) -> Vec2d {
        let window_frame: NSRect = unsafe { msg_send![self.window, frame] };
        Vec2d {
            x: window_frame.size.width,
            y: window_frame.size.height,
        }
    }

    pub fn set_outer_size(&self, size: Vec2d) {
        let mut window_frame: NSRect = unsafe { msg_send![self.window, frame] };
        window_frame.size.width = size.x;
        window_frame.size.height = size.y;
        unsafe {
            let () = msg_send![self.window, setFrame: window_frame display: YES];
        };
    }

    pub fn get_dpi_factor(&self) -> f64 {
        let scale: f64 = unsafe { msg_send![self.window, backingScaleFactor] };
        scale
    }

    pub fn send_change_event(&mut self) {
        //return;
        let new_geom = self.get_window_geom();
        let old_geom = if let Some(old_geom) = &self.last_window_geom {
            old_geom.clone()
        } else {
            new_geom.clone()
        };
        self.last_window_geom = Some(new_geom.clone());
        self.do_callback(MacosEvent::WindowGeomChange(WindowGeomChangeEvent {
            window_id: self.window_id,
            old_geom: old_geom,
            new_geom: new_geom,
        }));
        self.do_callback(MacosEvent::Paint);
        // we should schedule a timer for +16ms another Paint
    }

    pub fn send_got_focus_event(&mut self) {
        self.do_callback(MacosEvent::WindowGotFocus(self.window_id));
    }

    pub fn send_lost_focus_event(&mut self) {
        self.do_callback(MacosEvent::WindowLostFocus(self.window_id));
    }

    pub fn mouse_down_can_drag_window(&mut self) -> bool {
        let response = Rc::new(Cell::new(WindowDragQueryResponse::NoAnswer));
        self.do_callback(MacosEvent::WindowDragQuery(WindowDragQueryEvent {
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            response: response.clone(),
        }));
        match response.get() {
            WindowDragQueryResponse::Caption | WindowDragQueryResponse::SysMenu => true,
            WindowDragQueryResponse::Client | WindowDragQueryResponse::NoAnswer => false,
        }
    }

    pub fn send_mouse_down(&mut self, button: MouseButton, modifiers: KeyModifiers) {
        let () = unsafe { msg_send![self.window, makeFirstResponder: self.view] };
        self.do_callback(MacosEvent::MouseDown(MouseDownEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
    }

    pub fn send_mouse_up(&mut self, button: MouseButton, modifiers: KeyModifiers) {
        self.do_callback(MacosEvent::MouseUp(MouseUpEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now(),
        }));
    }

    pub fn send_mouse_move(&mut self, _event: ObjcId, pos: Vec2d, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;

        with_macos_app(|app| app.startup_focus_hack());

        self.do_callback(MacosEvent::MouseMove(MouseMoveEvent {
            window_id: self.window_id,
            abs: pos,
            modifiers: modifiers,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));

        //get_macos_app_global().ns_event = ptr::null_mut();
    }

    pub fn send_scroll(&mut self, scroll: Vec2d, modifiers: KeyModifiers, is_mouse: bool) {
        self.do_callback(MacosEvent::Scroll(ScrollEvent {
            window_id: self.window_id,
            scroll,
            abs: self.last_mouse_pos,
            modifiers,
            time: self.time_now(),
            is_mouse,
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
        }));
    }

    pub fn send_window_close_requested_event(&mut self) -> bool {
        let accept_close = Rc::new(Cell::new(true));
        self.do_callback(MacosEvent::WindowCloseRequested(
            WindowCloseRequestedEvent {
                window_id: self.window_id,
                accept_close: accept_close.clone(),
            },
        ));
        if !accept_close.get() {
            return false;
        }
        true
    }

    pub fn send_window_closed_event(&mut self) {
        self.do_callback(MacosEvent::WindowClosed(WindowClosedEvent {
            window_id: self.window_id,
        }))
    }

    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(MacosEvent::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last,
            ..Default::default()
        }))
    }

    #[cfg(target_os = "macos")]
    pub fn start_dragging(&mut self, items: Vec<DragItem>) {
        let ns_event: ObjcId = unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            msg_send![ns_app, currentEvent]
        };
        let mut dragged_files = Vec::new();
        for item in items {
            match item {
                DragItem::FilePath { path, internal_id } => {
                    let pasteboard_item: ObjcId =
                        unsafe { msg_send![class!(NSPasteboardItem), new] };
                    let _: () = unsafe {
                        msg_send![
                            pasteboard_item,
                            setString: str_to_nsstring(
                                &if let Some(id) = internal_id{
                                    format!("file://{}#makepad_internal_id={}", if path.len()==0{"makepad_internal_empty"}else {&path}, id.0)
                                }
                                else{
                                    format!("file://{}",if path.len()==0{"makepad_internal_empty"}else {&path})
                                }
                            )
                            forType: NSPasteboardTypeFileURL
                        ]
                    };
                    let dragging_item: ObjcId = unsafe { msg_send![class!(NSDraggingItem), alloc] };
                    let _: () = unsafe {
                        msg_send![dragging_item, initWithPasteboardWriter: pasteboard_item]
                    };
                    let bounds: NSRect = unsafe { msg_send![self.view, bounds] };
                    let _: () = unsafe {
                        msg_send![dragging_item, setDraggingFrame: bounds contents: self.view]
                    };
                    dragged_files.push(dragging_item)
                }
                _ => {
                    crate::error!("Dragging string not implemented on macos yet");
                }
            }
        }

        let dragging_items: ObjcId = unsafe {
            msg_send![
                class!(NSArray),
                arrayWithObjects: dragged_files.as_ptr()
                count: dragged_files.len()
            ]
        };

        unsafe {
            let _: ObjcId = msg_send![
                self.view,
                beginDraggingSessionWithItems: dragging_items
                event: ns_event
                source: self.view
            ];
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

pub fn get_cocoa_window(this: &Object) -> &mut MacosWindow {
    unsafe {
        let ptr: *mut c_void = *this.get_ivar("macos_window_ptr");
        &mut *(ptr as *mut MacosWindow)
    }
}
