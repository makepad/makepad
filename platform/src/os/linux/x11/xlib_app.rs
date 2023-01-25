use {
    std::{
        collections::{HashMap, VecDeque},
        time::Instant,
        mem,
        rc::Rc,
        cell::{Cell, RefCell},
        os::raw::{c_char, c_int, c_uint, c_ulong, c_void, c_uchar, c_long},
        ptr,
    },
    self::super::{
        x11_sys,
        xlib_event::XlibEvent,
        xlib_window::*,
        super::libc_sys,
    },
    crate::{
        makepad_math::DVec2,
        event::*,
        cursor::MouseCursor,
        os::cx_desktop::EventFlow,
    },
};

static mut XLIB_APP: *mut XlibApp = 0 as *mut _;

pub fn get_xlib_app_global() -> &'static mut XlibApp {
    unsafe {
        &mut *(XLIB_APP)
    }
}

pub fn init_xlib_app_global(event_callback: Box<dyn FnMut(&mut XlibApp, XlibEvent) -> EventFlow>) {
    unsafe {
        XLIB_APP = Box::into_raw(Box::new(XlibApp::new(event_callback)));
    }
}

pub struct XlibApp {
    pub display: *mut x11_sys::Display,
    event_loop_running: bool,
    pub xim: x11_sys::XIM,
    pub clipboard: String,
    pub display_fd: c_int,
    pub signal_fds: [c_int; 2],
    pub window_map: HashMap<c_ulong, *mut XlibWindow>,
    pub time_start: Instant,
    pub last_scroll_time: f64,
    pub last_click_time: f64,
    pub last_click_pos: (i32, i32),
    pub event_callback: Option<Box<dyn FnMut(&mut XlibApp, XlibEvent) -> EventFlow >>,
    pub timers: VecDeque<XlibTimer>,
    pub free_timers: Vec<usize>,
    pub event_flow: EventFlow,
    pub current_cursor: MouseCursor,
    pub atoms: XlibAtoms,
    pub dnd: Dnd,
}

#[derive(Clone, Copy)]
pub struct XlibTimer {
    id: u64,
    timeout: f64,
    repeats: bool,
    delta_timeout: f64,
}

impl XlibApp {
    pub fn new(event_callback: Box<dyn FnMut(&mut XlibApp, XlibEvent) -> EventFlow>) -> XlibApp {
        unsafe {
            let display = x11_sys::XOpenDisplay(ptr::null());
            let display_fd = x11_sys::XConnectionNumber(display);
            let xim = x11_sys::XOpenIM(display, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            let mut signal_fds = [0, 0];
            libc_sys::pipe(signal_fds.as_mut_ptr());
            x11_sys::XrmInitialize();
            XlibApp {
                event_loop_running: true,
                event_callback: Some(event_callback),
                atoms: XlibAtoms::new(display),
                xim,
                display,
                display_fd,
                signal_fds,
                clipboard: String::new(),
                last_scroll_time: 0.0,
                last_click_time: 0.0,
                last_click_pos: (0, 0),
                window_map: HashMap::new(),
                time_start: Instant::now(),
                event_flow: EventFlow::Poll,
                timers: VecDeque::new(),
                free_timers: Vec::new(),
                current_cursor: MouseCursor::Default,
                dnd: Dnd::new(display),
            }
        }
    }
    
    pub unsafe fn event_loop_poll(&mut self) {
        // Update the current time, and compute the amount of time that elapsed since we
        // last recorded the current time.
        while self.display != ptr::null_mut() && x11_sys::XPending(self.display) != 0 {
            let mut event = mem::MaybeUninit::uninit();
            x11_sys::XNextEvent(self.display, event.as_mut_ptr());
            let mut event = event.assume_init();
            match event.type_ as u32 {
                x11_sys::SelectionNotify => {
                    let selection = event.xselection;
                    if selection.property == self.dnd.atoms.selection {
                        self.dnd.handle_selection_event(&selection);
                    } else {
                        // first get the size of the thing
                        let mut actual_type = mem::MaybeUninit::uninit();
                        let mut actual_format = mem::MaybeUninit::uninit();
                        let mut n_items = mem::MaybeUninit::uninit();
                        let mut bytes_to_read = mem::MaybeUninit::uninit();
                        let mut ret = mem::MaybeUninit::uninit();
                        x11_sys::XGetWindowProperty(
                            self.display,
                            selection.requestor,
                            selection.property,
                            0,
                            0,
                            0,
                            x11_sys::AnyPropertyType as c_ulong,
                            actual_type.as_mut_ptr(),
                            actual_format.as_mut_ptr(),
                            n_items.as_mut_ptr(),
                            bytes_to_read.as_mut_ptr(),
                            ret.as_mut_ptr()
                        );
                        //let actual_type = actual_type.assume_init();
                        //let actual_format = actual_format.assume_init();
                        //let n_items = n_items.assume_init();
                        let bytes_to_read = bytes_to_read.assume_init();
                        //let mut ret = ret.assume_init();
                        let mut bytes_after = mem::MaybeUninit::uninit();
                        x11_sys::XGetWindowProperty(
                            self.display,
                            selection.requestor,
                            selection.property,
                            0,
                            bytes_to_read as c_long,
                            0,
                            x11_sys::AnyPropertyType as c_ulong,
                            actual_type.as_mut_ptr(),
                            actual_format.as_mut_ptr(),
                            n_items.as_mut_ptr(),
                            bytes_after.as_mut_ptr(),
                            ret.as_mut_ptr()
                        );
                        let ret = ret.assume_init();
                        //let bytes_after = bytes_after.assume_init();
                        if ret != ptr::null_mut() && bytes_to_read > 0 {
                            let utf8_slice = std::slice::from_raw_parts::<u8>(ret as *const _ as *const u8, bytes_to_read as usize);
                            if let Ok(utf8_string) = String::from_utf8(utf8_slice.to_vec()) {
                                self.do_callback(XlibEvent::TextInput(TextInputEvent {
                                    input: utf8_string,
                                    was_paste: true,
                                    replace_last: false
                                }));
                            }
                            x11_sys::XFree(ret as *mut _ as *mut c_void);
                        }
                    }
                },
                x11_sys::SelectionRequest => {
                    let request = event.xselectionrequest;
                    let mut response = x11_sys::XSelectionEvent {
                        type_: x11_sys::SelectionNotify as i32,
                        serial: 0,
                        send_event: 0,
                        display: self.display,
                        requestor: request.requestor,
                        selection: request.selection,
                        target: request.target,
                        time: request.time,
                        property: request.property,
                    };
                    if request.target == self.atoms.targets {
                        let mut targets = [self.atoms.utf8_string];
                        x11_sys::XChangeProperty(
                            self.display,
                            request.requestor,
                            request.property,
                            4,
                            32,
                            x11_sys::PropModeReplace as i32,
                            targets.as_mut() as *mut _ as *mut c_uchar,
                            targets.len() as i32
                        );
                    }
                    else if request.target == self.atoms.utf8_string {
                        x11_sys::XChangeProperty(
                            self.display,
                            request.requestor,
                            request.property,
                            self.atoms.utf8_string,
                            8,
                            x11_sys::PropModeReplace as i32,
                            self.clipboard.as_ptr() as *const _ as *const c_uchar,
                            self.clipboard.len() as i32
                        );
                    }
                    else {
                        response.property = 0;
                    }
                    x11_sys::XSendEvent(self.display, request.requestor, 1, 0, &mut response as *mut _ as *mut x11_sys::XEvent);
                },
                x11_sys::DestroyNotify => { // our window got destroyed
                    let destroy_window = event.xdestroywindow;
                    if let Some(window_ptr) = self.window_map.get(&destroy_window.window) {
                        let window = &mut (**window_ptr);
                        window.do_callback(XlibEvent::WindowClosed(WindowClosedEvent {
                            window_id: window.window_id,
                        }));
                    }
                },
                x11_sys::ConfigureNotify => {
                    let cfg = event.xconfigure;
                    if let Some(window_ptr) = self.window_map.get(&cfg.window) {
                        let window = &mut (**window_ptr);
                        if cfg.window == window.window.unwrap() {
                            window.send_change_event();
                        }
                    }
                },
                x11_sys::EnterNotify => {},
                x11_sys::LeaveNotify => {
                    let crossing = event.xcrossing;
                    if crossing.detail == 4 {
                        if let Some(_window_ptr) = self.window_map.get(&crossing.window) {
                            //TODO figure this out
                            /*
                            let window = &mut (**window_ptr);
                            window.do_callback(Event::FingerHover(FingerHoverEvent {
                                digit: 0,
                                window_id: window.window_id,
                                any_down: false,
                                abs: window.last_mouse_pos,
                                rel: window.last_mouse_pos,
                                rect: Rect::default(),
                                handled: false,
                                hover_state: HoverState::Out,
                                modifiers: KeyModifiers::default(),
                                time: window.time_now()
                            }));
                            */
                        }
                    }
                },
                x11_sys::MotionNotify => { // mousemove
                    let motion = event.xmotion;
                    if let Some(window_ptr) = self.window_map.get(&motion.window) {
                        let window = &mut (**window_ptr);
                        let x = motion.x;
                        let y = motion.y;
                        if window.window.is_none() {
                            return; // shutdown
                        }
                        if motion.window != window.window.unwrap() {
                            // find the right child
                            /*
                            for child in &window.child_windows {
                                if child.window == motion.window {
                                    x += child.x;
                                    y += child.y;
                                    break
                                }
                            }*/
                        }
                        
                        let pos = DVec2 {x: x as f64 / window.last_window_geom.dpi_factor, y: y as f64 / window.last_window_geom.dpi_factor};
                        
                        // query window for chrome
                        let response = Rc::new(Cell::new(WindowDragQueryResponse::NoAnswer));
                        window.do_callback(XlibEvent::WindowDragQuery(WindowDragQueryEvent {
                            window_id: window.window_id,
                            abs: window.last_mouse_pos,
                            response: response.clone()
                        }));
                        // otherwise lets check if we are hover the window edge to resize the window
                        //println!("{} {}", window.last_window_geom.inner_size.x, pos.x);
                        window.send_mouse_move(pos, KeyModifiers::default());
                        let window_size = window.last_window_geom.inner_size;
                        if pos.x >= 0.0 && pos.x < 10.0 && pos.y >= 0.0 && pos.y < 10.0 {
                            window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_TOPLEFT);
                            self.set_mouse_cursor(MouseCursor::NwResize);
                        }
                        else if pos.x >= 0.0 && pos.x < 10.0 && pos.y >= window_size.y - 10.0 {
                            window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_BOTTOMLEFT);
                            self.set_mouse_cursor(MouseCursor::SwResize);
                        }
                        else if pos.x >= 0.0 && pos.x < 5.0 {
                            window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_LEFT);
                            self.set_mouse_cursor(MouseCursor::WResize);
                        }
                        else if pos.x >= window_size.x - 10.0 && pos.y >= 0.0 && pos.y < 10.0 {
                            window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_TOPRIGHT);
                            self.set_mouse_cursor(MouseCursor::NeResize);
                        }
                        else if pos.x >= window_size.x - 10.0 && pos.y >= window_size.y - 10.0 {
                            window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_BOTTOMRIGHT);
                            self.set_mouse_cursor(MouseCursor::SeResize);
                        }
                        else if pos.x >= window_size.x - 5.0 {
                            window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_RIGHT);
                            self.set_mouse_cursor(MouseCursor::EResize);
                        }
                        else if pos.y <= 5.0 {
                            window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_TOP);
                            self.set_mouse_cursor(MouseCursor::NResize);
                        }
                        else if pos.y > window_size.y - 5.0 {
                            window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_BOTTOM);
                            self.set_mouse_cursor(MouseCursor::SResize);
                        }
                        else {
                            match response.get() {
                                WindowDragQueryResponse::Caption => {
                                    window.last_nc_mode = Some(_NET_WM_MOVERESIZE_MOVE);
                                },
                                _ => {
                                    window.last_nc_mode = None;
                                }
                            }
                        }
                    }
                },
                x11_sys::ButtonPress => { // mouse down
                    let button = event.xbutton;
                    let time_now = self.time_now();
                    if let Some(window_ptr) = self.window_map.get(&button.window) {
                        let window = &mut (**window_ptr);
                        x11_sys::XSetInputFocus(
                            self.display,
                            window.window.unwrap(),
                            x11_sys::None as i32,
                            x11_sys::CurrentTime as c_ulong
                        );
                        
                        if button.button >= 4 && button.button <= 7 {
                            let last_scroll_time = self.last_scroll_time;
                            self.last_scroll_time = time_now;
                            // completely arbitrary scroll acceleration curve.
                            let speed = 1200.0 * (0.2 - 2. * (self.last_scroll_time - last_scroll_time)).max(0.01);
                            
                            self.do_callback(XlibEvent::Scroll(ScrollEvent {
                                window_id: window.window_id,
                                scroll: DVec2 {
                                    x: if button.button == 6 {-speed} else if button.button == 7 {speed} else {0.},
                                    y: if button.button == 4 {-speed} else if button.button == 5 {speed} else {0.}
                                },
                                abs: window.last_mouse_pos,
                                modifiers: self.xkeystate_to_modifiers(button.state),
                                is_mouse: true,
                                handled_x: Cell::new(false),
                                handled_y: Cell::new(false),
                                time: self.last_scroll_time
                            }))
                            
                        }
                        else {
                            // do all the 'nonclient' area messaging to the window manager
                            if let Some(last_nc_mode) = window.last_nc_mode {
                                if (time_now - self.last_click_time) < 0.35
                                    && (button.x_root - self.last_click_pos.0).abs() < 5
                                    && (button.y_root - self.last_click_pos.1).abs() < 5
                                    && last_nc_mode == _NET_WM_MOVERESIZE_MOVE {
                                    if window.get_is_maximized() {
                                        window.restore();
                                    }
                                    else {
                                        window.maximize();
                                    }
                                }
                                else {
                                    
                                    let default_screen = x11_sys::XDefaultScreen(self.display);
                                    let root_window = x11_sys::XRootWindow(self.display, default_screen);
                                    x11_sys::XUngrabPointer(self.display, 0);
                                    x11_sys::XFlush(self.display);
                                    let mut xclient = x11_sys::XClientMessageEvent {
                                        type_: x11_sys::ClientMessage as i32,
                                        serial: 0,
                                        send_event: 0,
                                        display: self.display,
                                        window: window.window.unwrap(),
                                        message_type: self.atoms.net_wm_moveresize,
                                        format: 32,
                                        data: {
                                            let mut msg = mem::zeroed::<x11_sys::XClientMessageEvent__bindgen_ty_1>();
                                            msg.l[0] = button.x_root as c_long;
                                            msg.l[1] = button.y_root as c_long;
                                            msg.l[2] = last_nc_mode;
                                            msg
                                        }
                                    };
                                    x11_sys::XSendEvent(
                                        self.display,
                                        root_window,
                                        0,
                                        (x11_sys::SubstructureRedirectMask | x11_sys::SubstructureNotifyMask) as c_long,
                                        &mut xclient as *mut _ as *mut x11_sys::XEvent
                                    );
                                }
                            }
                            else {
                                window.send_mouse_down(button.button as usize, self.xkeystate_to_modifiers(button.state))
                            }
                        }
                    }
                    self.last_click_time = time_now;
                    self.last_click_pos = (button.x_root, button.y_root);
                },
                x11_sys::ButtonRelease => { // mouse up
                    let button = event.xbutton;
                    if let Some(window_ptr) = self.window_map.get(&button.window) {
                        let window = &mut (**window_ptr);
                        window.send_mouse_up(button.button as usize, self.xkeystate_to_modifiers(button.state))
                    }
                },
                x11_sys::KeyPress => {
                    if let Some(window_ptr) = self.window_map.get(&event.xkey.window) {
                        let window = &mut (**window_ptr);
                        let block_text = if event.xkey.keycode != 0 {
                            let key_code = self.xkeyevent_to_keycode(&mut event.xkey);
                            let modifiers = self.xkeystate_to_modifiers(event.xkey.state);
                            
                            if modifiers.control || modifiers.logo {
                                match key_code {
                                    KeyCode::KeyV => { // paste
                                        // request the pasteable text from the other side
                                        x11_sys::XConvertSelection(
                                            self.display,
                                            self.atoms.clipboard,
                                            self.atoms.utf8_string,
                                            self.atoms.clipboard,
                                            window.window.unwrap(),
                                            event.xkey.time
                                        );
                                        /*
                                        self.do_callback(&mut vec![
                                            Event::TextInput(TextInputEvent {
                                                input: String::new(),
                                                was_paste: true,
                                                replace_last: false
                                            })
                                        ]);
                                        */
                                    }
                                    KeyCode::KeyX | KeyCode::KeyC => {
                                        let response = Rc::new(RefCell::new(None));
                                        self.do_callback(XlibEvent::TextCopy(TextCopyEvent {
                                            response: response.clone()
                                        }));
                                        let response = response.borrow();
                                        if let Some(response) = response.as_ref() {
                                            // store the text on the clipboard
                                            self.clipboard = response.clone();
                                            // lets set the owner
                                            println!("Set selection owner");
                                            x11_sys::XSetSelectionOwner(
                                                self.display,
                                                self.atoms.clipboard,
                                                window.window.unwrap(),
                                                event.xkey.time
                                            );
                                            x11_sys::XFlush(self.display);
                                        }
                                    }
                                    _ => ()
                                }
                            }
                            
                            let block_text = modifiers.control || modifiers.logo || modifiers.alt;
                            self.do_callback(XlibEvent::KeyDown(KeyEvent {
                                key_code: key_code,
                                is_repeat: false,
                                modifiers: modifiers,
                                time: self.time_now()
                            }));
                            block_text
                        }else {false};
                        
                        if !block_text {
                            // decode the character
                            let mut buffer = [0u8; 32];
                            let mut keysym = mem::MaybeUninit::uninit();
                            let mut status = mem::MaybeUninit::uninit();
                            let count = x11_sys::Xutf8LookupString(
                                window.xic.unwrap(),
                                &mut event.xkey,
                                buffer.as_mut_ptr() as *mut c_char,
                                buffer.len() as c_int,
                                keysym.as_mut_ptr(),
                                status.as_mut_ptr(),
                            );
                            //let keysym = keysym.assume_init();
                            let status = status.assume_init();
                            if status != x11_sys::XBufferOverflow {
                                let utf8 = std::str::from_utf8(&buffer[..count as usize]).unwrap_or("").to_string();
                                let char_code = utf8.chars().next().unwrap_or('\0');
                                if char_code >= ' ' && char_code != 127 as char {
                                    self.do_callback(XlibEvent::TextInput(TextInputEvent {
                                        input: utf8,
                                        was_paste: false,
                                        replace_last: false
                                    }));
                                }
                            }
                        }
                    }
                },
                x11_sys::KeyRelease => {
                    self.do_callback(XlibEvent::KeyUp(KeyEvent {
                        key_code: self.xkeyevent_to_keycode(&mut event.xkey),
                        is_repeat: false,
                        modifiers: self.xkeystate_to_modifiers(event.xkey.state),
                        time: self.time_now()
                    }));
                },
                x11_sys::ClientMessage => {
                    let event = event.xclient;
                    if event.message_type == self.atoms.wm_protocols {
                        if let Some(window_ptr) = self.window_map.get(&event.window) {
                            let window = &mut (**window_ptr);
                            window.close_window();
                        }
                    }
                    if event.message_type == self.dnd.atoms.enter {
                        self.dnd.handle_enter_event(&event);
                    } else if event.message_type == self.dnd.atoms.drop {
                        self.dnd.handle_drop_event(&event);
                    } else if event.message_type == self.dnd.atoms.leave {
                        self.dnd.handle_leave_event(&event);
                    } else if event.message_type == self.dnd.atoms.position {
                        self.dnd.handle_position_event(&event);
                    }
                },
                x11_sys::Expose => {
                    /* 
                    (glx.glXMakeCurrent)(display, window, context);
                    gl::ClearColor(1.0, 0.0, 0.0, 1.0);
                    gl::Clear(gl::COLOR_BUFFER_BIT);
                    (glx.glXSwapBuffers)(display, window);
                    */
                },
                _ => {}
            }
        }
        self.do_callback(XlibEvent::Paint);
    }
    
    pub fn event_loop(&mut self) {
        unsafe {
            
            self.do_callback(XlibEvent::Paint);
            
            let mut select_time = self.time_now();
            while self.event_loop_running {
                match self.event_flow {
                    EventFlow::Exit => {
                        break;
                    }
                    EventFlow::Wait => {
                        let mut fds = mem::MaybeUninit::uninit();
                        libc_sys::FD_ZERO(fds.as_mut_ptr());
                        libc_sys::FD_SET(self.display_fd, fds.as_mut_ptr());
                        libc_sys::FD_SET(self.signal_fds[0], fds.as_mut_ptr());
                        // If there are any timers, we set the timeout for select to the `delta_timeout`
                        // of the first timer that should be fired. Otherwise, we set the timeout to
                        // None, so that select will block indefinitely.
                        let timeout = if let Some(timer) = self.timers.front() {
                           
                            Some(libc_sys::timeval {
                                // `tv_sec` is in seconds, so take the integer part of `delta_timeout`
                                tv_sec: timer.delta_timeout.trunc() as libc_sys::time_t,
                                // `tv_usec` is in microseconds, so take the fractional part of
                                // `delta_timeout` 1000000.0.
                                tv_usec: (timer.delta_timeout.fract() * 1000000.0) as libc_sys::time_t,
                            })
                        }
                        else {
                            
                            None
                        };
                        let _nfds = libc_sys::select(
                            self.display_fd.max(self.signal_fds[0]) + 1,
                            fds.as_mut_ptr(),
                            ptr::null_mut(), 
                            ptr::null_mut(),
                            if let Some(mut timeout) = timeout {&mut timeout} else {ptr::null_mut()}
                        );
                        self.event_flow = EventFlow::Poll;
                    }
                    EventFlow::Poll => { 
                        let last_select_time = select_time;
                        select_time = self.time_now();
                        let mut select_time_used = select_time - last_select_time;
                        //println!("{}", self.timers.len());
                        while let Some(timer) = self.timers.front_mut() {
                            // If the amount of time that elapsed is less than `delta_timeout` for the
                            // next timer, then no more timers need to be fired.
                            //  println!("TIMER COMPARE {} {}", select_time_used, timer.delta_timeout);
                            if select_time_used < timer.delta_timeout {
                                timer.delta_timeout -= select_time_used;
                                break;
                            }
                            
                            let timer = *self.timers.front().unwrap();
                            select_time_used -= timer.delta_timeout;
                            
                            // Stop the timer to remove it from the list.
                            self.stop_timer(timer.id);
                            // If the timer is repeating, simply start it again.
                            if timer.repeats {
                                self.start_timer(timer.id, timer.timeout, timer.repeats);
                            }
                            
                            // Fire the timer, and allow the callback to cancel the repeat
                            self.do_callback(
                                XlibEvent::Timer(TimerEvent {timer_id: timer.id})
                            );
                        }
                        
                        self.event_loop_poll();
                    }
                }
            }
        }
    }
    
    pub fn do_callback(&mut self, event: XlibEvent) {
        if let Some(mut callback) = self.event_callback.take() {
            self.event_flow = callback(self, event);
            if let EventFlow::Exit = self.event_flow {
                self.terminate_event_loop();
            }
            self.event_callback = Some(callback);
        }
    }
    
    pub fn terminate_event_loop(&mut self) {
        self.event_loop_running = false;
        unsafe {x11_sys::XCloseIM(self.xim)};
        unsafe {x11_sys::XCloseDisplay(self.display)};
        self.display = ptr::null_mut();
    }
    
    pub fn start_timer(&mut self, id: u64, timeout: f64, repeats: bool) {
        //println!("STARTING TIMER {:?} {:?} {:?}", id, timeout, repeats);
        
        // Timers are stored in an ordered list. Each timer stores the amount of time between
        // when its predecessor in the list should fire and when the timer itself should fire
        // in `delta_timeout`.
        
        // Since we are starting a new timer, our first step is to find where in the list this
        // new timer should be inserted. `delta_timeout` is initially set to `timeout`. As we move
        // through the list, we subtract the `delta_timeout` of the timers preceding the new timer
        // in the list. Once this subtraction would cause an overflow, we have found the correct
        // position in the list. The timer should fire after the one preceding it in the list, and
        // before the one succeeding it in the list. Moreover `delta_timeout` is now set to the
        // correct value.
        let mut delta_timeout = timeout;
        let index = self.timers.iter().position( | timer | {
            if delta_timeout < timer.delta_timeout {
                return true;
            }
            delta_timeout -= timer.delta_timeout;
            false
        }).unwrap_or(self.timers.len());
        
        // Insert the timer in the list.
        //
        // We also store the original `timeout` with each timer. This is necessary if the timer is
        // repeatable and we want to restart it later on.
        self.timers.insert(
            index,
            XlibTimer {
                id,
                timeout,
                repeats,
                delta_timeout,
            },
        );
        
        // The timer succeeding the newly inserted timer now has a new timer preceding it, so we
        // need to adjust its `delta_timeout`.
        //
        // Note that by construction, `timer.delta_timeout < delta_timeout`. Otherwise, the newly
        // inserted timer would have been inserted *after* the timer succeeding it, not before it.
        if index < self.timers.len() - 1 {
            let timer = &mut self.timers[index + 1];
            // This computation should never underflow (see above)
            timer.delta_timeout -= delta_timeout;
        }
    }
    
    pub fn stop_timer(&mut self, id: u64) {
        //println!("STOPPING TIMER {:?}", id);
        
        // Since we are stopping an existing timer, our first step is to find where in the list this
        // timer should be removed.
        let index = if let Some(index) = self.timers.iter().position( | timer | timer.id == id) {
            index
        } else {
            return;
        };
        
        // Remove the timer from the list.
        let delta_timeout = self.timers.remove(index).unwrap().delta_timeout;
        
        // The timer succeeding the removed timer now has a different timer preceding it, so we need
        // to adjust its `delta timeout`.
        if index < self.timers.len() {
            self.timers[index].delta_timeout += delta_timeout;
        }
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_micros() as f64 / 1_000_000.0
    }
    
    pub fn load_first_cursor(&self, names: &[&[u8]]) -> Option<c_ulong> {
        unsafe {
            for name in names {
                let cursor = x11_sys::XcursorLibraryLoadCursor(
                    self.display,
                    name.as_ptr() as *const c_char,
                );
                if cursor != 0 {
                    return Some(cursor)
                }
            }
        }
        return None
    }
    
    pub fn set_mouse_cursor(&mut self, cursor: MouseCursor) {
        if self.current_cursor != cursor {
            self.current_cursor = cursor.clone();
            let x11_cursor = match cursor {
                MouseCursor::Hidden => {
                    return;
                },
                MouseCursor::EResize => self.load_first_cursor(&[b"right_side\0"]),
                MouseCursor::NResize => self.load_first_cursor(&[b"top_side\0"]),
                MouseCursor::NeResize => self.load_first_cursor(&[b"top_right_corner\0"]),
                MouseCursor::NwResize => self.load_first_cursor(&[b"top_left_corner\0"]),
                MouseCursor::SResize => self.load_first_cursor(&[b"bottom_side\0"]),
                MouseCursor::SeResize => self.load_first_cursor(&[b"bottom_right_corner\0"]),
                MouseCursor::SwResize => self.load_first_cursor(&[b"bottom_left_corner\0"]),
                MouseCursor::WResize => self.load_first_cursor(&[b"left_side\0"]),
                
                MouseCursor::Default => self.load_first_cursor(&[b"left_ptr\0"]),
                MouseCursor::Crosshair => self.load_first_cursor(&[b"crosshair"]),
                MouseCursor::Hand => self.load_first_cursor(&[b"left_ptr\0", b"hand1\0"]),
                MouseCursor::Arrow => self.load_first_cursor(&[b"left_ptr\0\0"]),
                MouseCursor::Move => self.load_first_cursor(&[b"move\0"]),
                MouseCursor::NotAllowed => self.load_first_cursor(&[b"crossed_circle\0"]),
                MouseCursor::Text => self.load_first_cursor(&[b"text\0", b"xterm\0"]),
                MouseCursor::Wait => self.load_first_cursor(&[b"watch\0"]),
                MouseCursor::Help => self.load_first_cursor(&[b"question_arrow\0"]),
                MouseCursor::NsResize => self.load_first_cursor(&[b"v_double_arrow\0"]),
                MouseCursor::NeswResize => self.load_first_cursor(&[b"fd_double_arrow\0", b"size_fdiag\0"]),
                MouseCursor::EwResize => self.load_first_cursor(&[b"h_double_arrow\0"]),
                MouseCursor::NwseResize => self.load_first_cursor(&[b"bd_double_arrow\0", b"size_bdiag\0"]),
                MouseCursor::ColResize => self.load_first_cursor(&[b"split_h\0", b"h_double_arrow\0"]),
                MouseCursor::RowResize => self.load_first_cursor(&[b"split_v\0", b"v_double_arrow\0"]),
            };
            if let Some(x11_cursor) = x11_cursor {
                unsafe {
                    for (k, v) in &self.window_map {
                        if !(**v).window.is_none() {
                            x11_sys::XDefineCursor(self.display, *k, x11_cursor);
                        }
                    }
                    x11_sys::XFreeCursor(self.display, x11_cursor);
                }
            }
        }
    }
    
    fn xkeystate_to_modifiers(&self, state: c_uint) -> KeyModifiers {
        KeyModifiers {
            alt: state & x11_sys::Mod1Mask != 0,
            shift: state & x11_sys::ShiftMask != 0,
            control: state & x11_sys::ControlMask != 0,
            logo: state & x11_sys::Mod4Mask != 0,
        }
    }
    
    fn xkeyevent_to_keycode(&self, key_event: &mut x11_sys::XKeyEvent) -> KeyCode {
        let mut keysym = 0;
        unsafe {
            x11_sys::XLookupString(
                key_event,
                ptr::null_mut(),
                0,
                &mut keysym,
                ptr::null_mut(),
            );
        }
        match keysym as u32 {
            x11_sys::XK_a => KeyCode::KeyA,
            x11_sys::XK_A => KeyCode::KeyA,
            x11_sys::XK_b => KeyCode::KeyB,
            x11_sys::XK_B => KeyCode::KeyB,
            x11_sys::XK_c => KeyCode::KeyC,
            x11_sys::XK_C => KeyCode::KeyC,
            x11_sys::XK_d => KeyCode::KeyD,
            x11_sys::XK_D => KeyCode::KeyD,
            x11_sys::XK_e => KeyCode::KeyE,
            x11_sys::XK_E => KeyCode::KeyE,
            x11_sys::XK_f => KeyCode::KeyF,
            x11_sys::XK_F => KeyCode::KeyF,
            x11_sys::XK_g => KeyCode::KeyG,
            x11_sys::XK_G => KeyCode::KeyG,
            x11_sys::XK_h => KeyCode::KeyH,
            x11_sys::XK_H => KeyCode::KeyH,
            x11_sys::XK_i => KeyCode::KeyI,
            x11_sys::XK_I => KeyCode::KeyI,
            x11_sys::XK_j => KeyCode::KeyJ,
            x11_sys::XK_J => KeyCode::KeyJ,
            x11_sys::XK_k => KeyCode::KeyK,
            x11_sys::XK_K => KeyCode::KeyK,
            x11_sys::XK_l => KeyCode::KeyL,
            x11_sys::XK_L => KeyCode::KeyL,
            x11_sys::XK_m => KeyCode::KeyM,
            x11_sys::XK_M => KeyCode::KeyM,
            x11_sys::XK_n => KeyCode::KeyN,
            x11_sys::XK_N => KeyCode::KeyN,
            x11_sys::XK_o => KeyCode::KeyO,
            x11_sys::XK_O => KeyCode::KeyO,
            x11_sys::XK_p => KeyCode::KeyP,
            x11_sys::XK_P => KeyCode::KeyP,
            x11_sys::XK_q => KeyCode::KeyQ,
            x11_sys::XK_Q => KeyCode::KeyQ,
            x11_sys::XK_r => KeyCode::KeyR,
            x11_sys::XK_R => KeyCode::KeyR,
            x11_sys::XK_s => KeyCode::KeyS,
            x11_sys::XK_S => KeyCode::KeyS,
            x11_sys::XK_t => KeyCode::KeyT,
            x11_sys::XK_T => KeyCode::KeyT,
            x11_sys::XK_u => KeyCode::KeyU,
            x11_sys::XK_U => KeyCode::KeyU,
            x11_sys::XK_v => KeyCode::KeyV,
            x11_sys::XK_V => KeyCode::KeyV,
            x11_sys::XK_w => KeyCode::KeyW,
            x11_sys::XK_W => KeyCode::KeyW,
            x11_sys::XK_x => KeyCode::KeyX,
            x11_sys::XK_X => KeyCode::KeyX,
            x11_sys::XK_y => KeyCode::KeyY,
            x11_sys::XK_Y => KeyCode::KeyY,
            x11_sys::XK_z => KeyCode::KeyZ,
            x11_sys::XK_Z => KeyCode::KeyZ,
            
            x11_sys::XK_0 => KeyCode::Key0,
            x11_sys::XK_1 => KeyCode::Key1,
            x11_sys::XK_2 => KeyCode::Key2,
            x11_sys::XK_3 => KeyCode::Key3,
            x11_sys::XK_4 => KeyCode::Key4,
            x11_sys::XK_5 => KeyCode::Key5,
            x11_sys::XK_6 => KeyCode::Key6,
            x11_sys::XK_7 => KeyCode::Key7,
            x11_sys::XK_8 => KeyCode::Key8,
            x11_sys::XK_9 => KeyCode::Key9,
            
            x11_sys::XK_Alt_L => KeyCode::Alt,
            x11_sys::XK_Alt_R => KeyCode::Alt,
            x11_sys::XK_Meta_L => KeyCode::Logo,
            x11_sys::XK_Meta_R => KeyCode::Logo,
            x11_sys::XK_Shift_L => KeyCode::Shift,
            x11_sys::XK_Shift_R => KeyCode::Shift,
            x11_sys::XK_Control_L => KeyCode::Control,
            x11_sys::XK_Control_R => KeyCode::Control,
            
            x11_sys::XK_equal => KeyCode::Equals,
            x11_sys::XK_minus => KeyCode::Minus,
            x11_sys::XK_bracketright => KeyCode::RBracket,
            x11_sys::XK_bracketleft => KeyCode::LBracket,
            x11_sys::XK_Return => KeyCode::ReturnKey,
            x11_sys::XK_grave => KeyCode::Backtick,
            x11_sys::XK_semicolon => KeyCode::Semicolon,
            x11_sys::XK_backslash => KeyCode::Backslash,
            x11_sys::XK_comma => KeyCode::Comma,
            x11_sys::XK_slash => KeyCode::Slash,
            x11_sys::XK_period => KeyCode::Period,
            x11_sys::XK_Tab => KeyCode::Tab,
            x11_sys::XK_ISO_Left_Tab => KeyCode::Tab,
            x11_sys::XK_space => KeyCode::Space,
            x11_sys::XK_BackSpace => KeyCode::Backspace,
            x11_sys::XK_Escape => KeyCode::Escape,
            x11_sys::XK_Caps_Lock => KeyCode::Capslock,
            x11_sys::XK_KP_Decimal => KeyCode::NumpadDecimal,
            x11_sys::XK_KP_Multiply => KeyCode::NumpadMultiply,
            x11_sys::XK_KP_Add => KeyCode::NumpadAdd,
            x11_sys::XK_Num_Lock => KeyCode::Numlock,
            x11_sys::XK_KP_Divide => KeyCode::NumpadDivide,
            x11_sys::XK_KP_Enter => KeyCode::NumpadEnter,
            x11_sys::XK_KP_Subtract => KeyCode::NumpadSubtract,
            //keysim::XK_9 => KeyCode::NumpadEquals,
            x11_sys::XK_KP_0 => KeyCode::Numpad0,
            x11_sys::XK_KP_1 => KeyCode::Numpad1,
            x11_sys::XK_KP_2 => KeyCode::Numpad2,
            x11_sys::XK_KP_3 => KeyCode::Numpad3,
            x11_sys::XK_KP_4 => KeyCode::Numpad4,
            x11_sys::XK_KP_5 => KeyCode::Numpad5,
            x11_sys::XK_KP_6 => KeyCode::Numpad6,
            x11_sys::XK_KP_7 => KeyCode::Numpad7,
            x11_sys::XK_KP_8 => KeyCode::Numpad8,
            x11_sys::XK_KP_9 => KeyCode::Numpad9,
            
            x11_sys::XK_F1 => KeyCode::F1,
            x11_sys::XK_F2 => KeyCode::F2,
            x11_sys::XK_F3 => KeyCode::F3,
            x11_sys::XK_F4 => KeyCode::F4,
            x11_sys::XK_F5 => KeyCode::F5,
            x11_sys::XK_F6 => KeyCode::F6,
            x11_sys::XK_F7 => KeyCode::F7,
            x11_sys::XK_F8 => KeyCode::F8,
            x11_sys::XK_F9 => KeyCode::F9,
            x11_sys::XK_F10 => KeyCode::F10,
            x11_sys::XK_F11 => KeyCode::F11,
            x11_sys::XK_F12 => KeyCode::F12,
            
            x11_sys::XK_Print => KeyCode::PrintScreen,
            x11_sys::XK_Home => KeyCode::Home,
            x11_sys::XK_Page_Up => KeyCode::PageUp,
            x11_sys::XK_Delete => KeyCode::Delete,
            x11_sys::XK_End => KeyCode::End,
            x11_sys::XK_Page_Down => KeyCode::PageDown,
            x11_sys::XK_Left => KeyCode::ArrowLeft,
            x11_sys::XK_Right => KeyCode::ArrowRight,
            x11_sys::XK_Down => KeyCode::ArrowDown,
            x11_sys::XK_Up => KeyCode::ArrowUp,
            _ => KeyCode::Unknown,
        }
    }
}

pub struct XlibAtoms {
    pub clipboard: x11_sys::Atom,
    pub net_wm_moveresize: x11_sys::Atom,
    pub wm_delete_window: x11_sys::Atom,
    pub wm_protocols: x11_sys::Atom,
    pub motif_wm_hints: x11_sys::Atom,
    pub net_wm_state: x11_sys::Atom,
    pub new_wm_state_maximized_horz: x11_sys::Atom,
    pub new_wm_state_maximized_vert: x11_sys::Atom,
    pub targets: x11_sys::Atom,
    pub utf8_string: x11_sys::Atom,
    pub text: x11_sys::Atom,
    pub multiple: x11_sys::Atom,
    pub text_plain: x11_sys::Atom,
    pub atom: x11_sys::Atom,
}

impl XlibAtoms {
    fn new(display: *mut x11_sys::Display) -> Self {
        unsafe {Self {
            clipboard: x11_sys::XInternAtom(display, "CLIPBOARD\n".as_ptr() as *const _, 0),
            net_wm_moveresize: x11_sys::XInternAtom(display, "_NET_WM_MOVERESIZE\0".as_ptr() as *const _, 0),
            wm_delete_window: x11_sys::XInternAtom(display, "WM_DELETE_WINDOW\0".as_ptr() as *const _, 0),
            wm_protocols: x11_sys::XInternAtom(display, "WM_PROTOCOLS\0".as_ptr() as *const _, 0),
            motif_wm_hints: x11_sys::XInternAtom(display, "_MOTIF_WM_HINTS\0".as_ptr() as *const _, 0),
            net_wm_state: x11_sys::XInternAtom(display, "_NET_WM_STATE\0".as_ptr() as *const _, 0),
            new_wm_state_maximized_horz: x11_sys::XInternAtom(display, "_NET_WM_STATE_MAXIMIZED_HORZ\0".as_ptr() as *const _, 0),
            new_wm_state_maximized_vert: x11_sys::XInternAtom(display, "_NET_WM_STATE_MAXIMIZED_VERT\0".as_ptr() as *const _, 0),
            targets: x11_sys::XInternAtom(display, "TARGETS\0".as_ptr() as *const _, 0),
            utf8_string: x11_sys::XInternAtom(display, "UTF8_STRING\0".as_ptr() as *const _, 1),
            atom: x11_sys::XInternAtom(display, "ATOM\0".as_ptr() as *const _, 0),
            text: x11_sys::XInternAtom(display, "TEXT\0".as_ptr() as *const _, 0),
            text_plain: x11_sys::XInternAtom(display, "text/plain\0".as_ptr() as *const _, 0),
            multiple: x11_sys::XInternAtom(display, "MULTIPLE\0".as_ptr() as *const _, 0),
        }}
    }
}

