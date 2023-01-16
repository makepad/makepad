use {
    std::{
        sync::Mutex,
        collections::{HashMap, BTreeSet, VecDeque},
        ffi::CString,
        ffi::CStr,
        fs::File,
        io::Write,
        os::unix::io::FromRawFd,
        mem,
        os::raw::{c_char, c_uchar, c_int, c_uint, c_ulong, c_long, c_void},
        ptr,
    },
    crate::{
        event::{TimerEvent},
        cursor::MouseCursor,
        os::cx_desktop::EventFlow, 
        os::linux::x11_sys,
        os::linux::xlib_event::XlibEvent,
        os::linux::xlib_window::XlibWindow,
    },
};

static mut XLIB_APP: *mut XlibApp = 0 as *mut _;


pub fn get_xlib_app_global() -> &'static mut XlibApp {
    unsafe {
        &mut *(XLIB_APP)
    }
}

pub fn init_win32_app_global(event_callback: Box<dyn FnMut(&mut XlibApp, Vec<XlibEvent>) -> EventFlow>) {
    unsafe {
        XLIB_APP = Box::into_raw(Box::new(XlibApp::new(event_callback)));
    }
}


pub struct XlibApp {
    pub display: *mut x11_sys::Display,
    pub xim: x11_sys::XIM,
    pub clipboard: String,
    pub display_fd: c_int,
    pub signal_fds: [c_int; 2],
    pub window_map: HashMap<c_ulong, *mut XlibWindow>,
    pub time_start: u64,
    pub last_scroll_time: f64,
    pub last_click_time: f64,
    pub last_click_pos: (i32, i32),
    pub event_callback: Option<Box<dyn FnMut(&mut XlibApp, Vec<XlibEvent>) -> EventFlow >>,
    pub event_recur_block: bool,
    pub event_loop_running: bool,
    pub timers: VecDeque<XlibTimer>,
    pub free_timers: Vec<usize>,
    pub loop_block: bool,
    pub current_cursor: MouseCursor,
    
    pub atom_clipboard: x11_sys::Atom,
    pub atom_net_wm_moveresize: x11_sys::Atom,
    pub atom_wm_delete_window: x11_sys::Atom,
    pub atom_wm_protocols: x11_sys::Atom,
    pub atom_motif_wm_hints: x11_sys::Atom,
    pub atom_net_wm_state: x11_sys::Atom,
    pub atom_new_wm_state_maximized_horz: x11_sys::Atom,
    pub atom_new_wm_state_maximized_vert: x11_sys::Atom,
    pub atom_targets: x11_sys::Atom,
    pub atom_utf8_string: x11_sys::Atom,
    pub atom_text: x11_sys::Atom,
    pub atom_multiple: x11_sys::Atom,
    pub atom_text_plain: x11_sys::Atom,
    pub atom_atom: x11_sys::Atom,
    
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
    pub fn new(event_callback: Box<dyn FnMut(&mut Win32App, Vec<Win32Event>) -> EventFlow>) -> XlibApp {
        unsafe {
            let display = x11_sys::XOpenDisplay(ptr::null());
            let display_fd = x11_sys::XConnectionNumber(display);
            let xim = x11_sys::XOpenIM(display, ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            let mut signal_fds = [0, 0];
            libc::pipe(signal_fds.as_mut_ptr());
            XlibApp {
                atom_clipboard: X11_sys::XInternAtom(display, CString::new("CLIPBOARD").unwrap().as_ptr(), 0),
                atom_net_wm_moveresize: X11_sys::XInternAtom(display, CString::new("_NET_WM_MOVERESIZE").unwrap().as_ptr(), 0),
                atom_wm_delete_window: X11_sys::XInternAtom(display, CString::new("WM_DELETE_WINDOW").unwrap().as_ptr(), 0),
                atom_wm_protocols: X11_sys::XInternAtom(display, CString::new("WM_PROTOCOLS").unwrap().as_ptr(), 0),
                atom_motif_wm_hints: X11_sys::XInternAtom(display, CString::new("_MOTIF_WM_HINTS").unwrap().as_ptr(), 0),
                atom_net_wm_state: X11_sys::XInternAtom(display, CString::new("_NET_WM_STATE").unwrap().as_ptr(), 0),
                atom_new_wm_state_maximized_horz: X11_sys::XInternAtom(display, CString::new("_NET_WM_STATE_MAXIMIZED_HORZ").unwrap().as_ptr(), 0),
                atom_new_wm_state_maximized_vert: X11_sys::XInternAtom(display, CString::new("_NET_WM_STATE_MAXIMIZED_VERT").unwrap().as_ptr(), 0),
                atom_targets: X11_sys::XInternAtom(display, CString::new("TARGETS").unwrap().as_ptr(), 0),
                atom_utf8_string: X11_sys::XInternAtom(display, CString::new("UTF8_STRING").unwrap().as_ptr(), 1),
                atom_atom: X11_sys::XInternAtom(display, CString::new("ATOM").unwrap().as_ptr(), 0),
                atom_text: X11_sys::XInternAtom(display, CString::new("TEXT").unwrap().as_ptr(), 0),
                atom_text_plain: X11_sys::XInternAtom(display, CString::new("text/plain").unwrap().as_ptr(), 0),
                atom_multiple: X11_sys::XInternAtom(display, CString::new("MULTIPLE").unwrap().as_ptr(), 0),
                xim,
                display,
                display_fd,
                signal_fds,
                clipboard: String::new(),
                last_scroll_time: 0.0,
                last_click_time: 0.0,
                last_click_pos: (0, 0),
                window_map: HashMap::new(),
                signals: Mutex::new(Vec::new()),
                time_start: precise_time_ns(),
                event_callback: None,
                event_recur_block: false,
                event_loop_running: true,
                loop_block: false,
                timers: VecDeque::new(),
                free_timers: Vec::new(),
                current_cursor: MouseCursor::Default,
                dnd: Dnd::new(display),
            }
        }
    }
    
    pub fn init(&mut self) {
        unsafe {
            //unsafe {
            X11_sys::XrmInitialize();
            //}
            GLOBAL_XLIB_APP = self;
        }
    }
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut XlibApp, &mut Vec<Event>) -> bool,
    {
        unsafe {
            self.event_callback = Some(
                &mut event_handler as *const dyn FnMut(&mut XlibApp, &mut Vec<Event>) -> bool
                as *mut dyn FnMut(&mut XlibApp, &mut Vec<Event>) -> bool
            );
            
            self.do_callback(&mut vec![
                Event::Paint,
            ]);
            
            // Record the current time.
            let mut select_time = self.time_now();
            
            while self.event_loop_running {
                if self.loop_block {
                    let mut fds = mem::MaybeUninit::uninit();
                    libc::FD_ZERO(fds.as_mut_ptr());
                    libc::FD_SET(self.display_fd, fds.as_mut_ptr());
                    libc::FD_SET(self.signal_fds[0], fds.as_mut_ptr());
                    // If there are any timers, we set the timeout for select to the `delta_timeout`
                    // of the first timer that should be fired. Otherwise, we set the timeout to
                    // None, so that select will block indefinitely.
                    let timeout = if let Some(timer) = self.timers.front() {
                        // println!("Select wait {}",(timer.delta_timeout.fract() * 1000000.0) as i64);
                        Some(timeval {
                            // `tv_sec` is in seconds, so take the integer part of `delta_timeout`
                            tv_sec: timer.delta_timeout.trunc() as libc::time_t,
                            // `tv_usec` is in microseconds, so take the fractional part of
                            // `delta_timeout` 1000000.0.
                            tv_usec: (timer.delta_timeout.fract() * 1000000.0) as libc::time_t,
                        })
                    }
                    else {
                        None
                    };
                    let _nfds = libc::select(
                        self.display_fd.max(self.signal_fds[0]) + 1,
                        fds.as_mut_ptr(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                        if let Some(mut timeout) = timeout {&mut timeout} else {ptr::null_mut()}
                    );
                }
                // Update the current time, and compute the amount of time that elapsed since we
                // last recorded the current time.
                let last_select_time = select_time;
                select_time = self.time_now();
                let mut select_time_used = select_time - last_select_time;
                
                while let Some(timer) = self.timers.front_mut() {
                    // If the amount of time that elapsed is less than `delta_timeout` for the
                    // next timer, then no more timers need to be fired.
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
                    self.do_callback(&mut vec![
                        Event::Timer(TimerEvent {timer_id: timer.id})
                    ]);
                }
                
                while self.display != ptr::null_mut() && X11_sys::XPending(self.display) != 0 {
                    let mut event = mem::MaybeUninit::uninit();
                    X11_sys::XNextEvent(self.display, event.as_mut_ptr());
                    let mut event = event.assume_init();
                    match event.type_ as u32 {
                        X11_sys::SelectionNotify => {
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
                                X11_sys::XGetWindowProperty(
                                    self.display,
                                    selection.requestor,
                                    selection.property,
                                    0,
                                    0,
                                    0,
                                    X11_sys::AnyPropertyType as c_ulong,
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
                                X11_sys::XGetWindowProperty(
                                    self.display,
                                    selection.requestor,
                                    selection.property,
                                    0,
                                    bytes_to_read as c_long,
                                    0,
                                    X11_sys::AnyPropertyType as c_ulong,
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
                                        self.do_callback(&mut vec![
                                            Event::TextInput(TextInputEvent {
                                                input: utf8_string,
                                                was_paste: true,
                                                replace_last: false
                                            })
                                        ]);
                                    }
                                    X11_sys::XFree(ret as *mut _ as *mut c_void);
                                }
                            }
                        },
                        X11_sys::SelectionRequest => {
                            let request = event.xselectionrequest;
                            let mut response = X11_sys::XSelectionEvent {
                                type_: X11_sys::SelectionNotify as i32,
                                serial: 0,
                                send_event: 0,
                                display: self.display,
                                requestor: request.requestor,
                                selection: request.selection,
                                target: request.target,
                                time: request.time,
                                property: request.property,
                            };
                            if request.target == self.atom_targets {
                                let mut targets = [self.atom_utf8_string];
                                X11_sys::XChangeProperty(
                                    self.display,
                                    request.requestor,
                                    request.property,
                                    4,
                                    32,
                                    X11_sys::PropModeReplace as i32,
                                    targets.as_mut() as *mut _ as *mut c_uchar,
                                    targets.len() as i32
                                );
                            }
                            else if request.target == self.atom_utf8_string {
                                X11_sys::XChangeProperty(
                                    self.display,
                                    request.requestor,
                                    request.property,
                                    self.atom_utf8_string,
                                    8,
                                    X11_sys::PropModeReplace as i32,
                                    self.clipboard.as_ptr() as *const _ as *const c_uchar,
                                    self.clipboard.len() as i32
                                );
                            }
                            else {
                                response.property = 0;
                            }
                            X11_sys::XSendEvent(self.display, request.requestor, 1, 0, &mut response as *mut _ as *mut X11_sys::XEvent);
                        },
                        X11_sys::DestroyNotify => { // our window got destroyed
                            
                            let destroy_window = event.xdestroywindow;
                            if let Some(window_ptr) = self.window_map.get(&destroy_window.window) {
                                let window = &mut (**window_ptr);
                                window.do_callback(&mut vec![Event::WindowClosed(WindowClosedEvent {
                                    window_id: window.window_id,
                                })]);
                            }
                        },
                        X11_sys::ConfigureNotify => {
                            let cfg = event.xconfigure;
                            if let Some(window_ptr) = self.window_map.get(&cfg.window) {
                                let window = &mut (**window_ptr);
                                if cfg.window == window.window.unwrap() {
                                    window.send_change_event();
                                }
                            }
                        },
                        X11_sys::EnterNotify => {},
                        X11_sys::LeaveNotify => {
                            let crossing = event.xcrossing;
                            if crossing.detail == 4 {
                                if let Some(window_ptr) = self.window_map.get(&crossing.window) {
                                    let window = &mut (**window_ptr);
                                    window.do_callback(&mut vec![Event::FingerHover(FingerHoverEvent {
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
                                    })]);
                                }
                            }
                        },
                        X11_sys::MotionNotify => { // mousemove
                            let motion = event.xmotion;
                            if let Some(window_ptr) = self.window_map.get(&motion.window) {
                                let window = &mut (**window_ptr);
                                let mut x = motion.x;
                                let mut y = motion.y;
                                if window.window.is_none() {
                                    return; // shutdown
                                }
                                if motion.window != window.window.unwrap() {
                                    // find the right child
                                    for child in &window.child_windows {
                                        if child.window == motion.window {
                                            x += child.x;
                                            y += child.y;
                                            break
                                        }
                                    }
                                }
                                
                                let pos = Vec2 {x: x as f32 / window.last_window_geom.dpi_factor, y: y as f32 / window.last_window_geom.dpi_factor};
                                
                                // query window for chrome
                                let mut drag_query_events = vec![
                                    Event::WindowDragQuery(WindowDragQueryEvent {
                                        window_id: window.window_id,
                                        abs: window.last_mouse_pos,
                                        response: WindowDragQueryResponse::NoAnswer
                                    })
                                ];
                                window.do_callback(&mut drag_query_events);
                                // otherwise lets check if we are hover the window edge to resize the window
                                //println!("{} {}", window.last_window_geom.inner_size.x, pos.x);
                                window.send_finger_hover_and_move(pos, KeyModifiers::default());
                                let window_size = window.last_window_geom.inner_size;
                                if pos.x >= 0.0 && pos.x < 10.0 && pos.y >= 0.0 && pos.y < 10.0 {
                                    window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_TOPLEFT);
                                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NwResize)]);
                                }
                                else if pos.x >= 0.0 && pos.x < 10.0 && pos.y >= window_size.y - 10.0 {
                                    window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_BOTTOMLEFT);
                                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::SwResize)]);
                                }
                                else if pos.x >= 0.0 && pos.x < 5.0 {
                                    window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_LEFT);
                                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::WResize)]);
                                }
                                else if pos.x >= window_size.x - 10.0 && pos.y >= 0.0 && pos.y < 10.0 {
                                    window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_TOPRIGHT);
                                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NeResize)]);
                                }
                                else if pos.x >= window_size.x - 10.0 && pos.y >= window_size.y - 10.0 {
                                    window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_BOTTOMRIGHT);
                                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::SeResize)]);
                                }
                                else if pos.x >= window_size.x - 5.0 {
                                    window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_RIGHT);
                                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::EResize)]);
                                }
                                else if pos.y <= 5.0 {
                                    window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_TOP);
                                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NResize)]);
                                }
                                else if pos.y > window_size.y - 5.0 {
                                    window.last_nc_mode = Some(_NET_WM_MOVERESIZE_SIZE_BOTTOM);
                                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::SResize)]);
                                }
                                else {
                                    match &drag_query_events[0] {
                                        Event::WindowDragQuery(wd) => match &wd.response {
                                            WindowDragQueryResponse::Caption => {
                                                window.last_nc_mode = Some(_NET_WM_MOVERESIZE_MOVE);
                                            },
                                            _ => {
                                                window.last_nc_mode = None;
                                            }
                                        },
                                        _ => ()
                                    }
                                }
                            }
                        },
                        X11_sys::ButtonPress => { // mouse down
                            let button = event.xbutton;
                            let time_now = self.time_now();
                            if let Some(window_ptr) = self.window_map.get(&button.window) {
                                let window = &mut (**window_ptr);
                                X11_sys::XSetInputFocus(self.display, window.window.unwrap(), X11_sys::None as i32, X11_sys::CurrentTime as c_ulong);
                                
                                if button.button >= 4 && button.button <= 7 {
                                    let last_scroll_time = self.last_scroll_time;
                                    self.last_scroll_time = time_now;
                                    // completely arbitrary scroll acceleration curve.
                                    let speed = 1200.0 * (0.2 - 2. * (self.last_scroll_time - last_scroll_time)).max(0.01);
                                    self.do_callback(&mut vec![Event::FingerScroll(FingerScrollEvent {
                                        digit: 0,
                                        window_id: window.window_id,
                                        scroll: Vec2 {
                                            x: if button.button == 6 {-speed as f32} else if button.button == 7 {speed as f32} else {0.},
                                            y: if button.button == 4 {-speed as f32} else if button.button == 5 {speed as f32} else {0.}
                                        },
                                        abs: window.last_mouse_pos,
                                        rel: window.last_mouse_pos,
                                        rect: Rect::default(),
                                        input_type: FingerInputType::Mouse,
                                        modifiers: self.xkeystate_to_modifiers(button.state),
                                        handled_x: false,
                                        handled_y: false,
                                        time: self.last_scroll_time
                                    })])
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
                                            
                                            let default_screen = X11_sys::XDefaultScreen(self.display);
                                            let root_window = X11_sys::XRootWindow(self.display, default_screen);
                                            X11_sys::XUngrabPointer(self.display, 0);
                                            X11_sys::XFlush(self.display);
                                            let mut xclient = X11_sys::XClientMessageEvent {
                                                type_: X11_sys::ClientMessage as i32,
                                                serial: 0,
                                                send_event: 0,
                                                display: self.display,
                                                window: window.window.unwrap(),
                                                message_type: self.atom_net_wm_moveresize,
                                                format: 32,
                                                data: {
                                                    let mut msg = mem::zeroed::<X11_sys::XClientMessageEvent__bindgen_ty_1>();
                                                    msg.l[0] = button.x_root as c_long;
                                                    msg.l[1] = button.y_root as c_long;
                                                    msg.l[2] = last_nc_mode;
                                                    msg
                                                }
                                            };
                                            X11_sys::XSendEvent(
                                                self.display,
                                                root_window,
                                                0,
                                                (X11_sys::SubstructureRedirectMask | X11_sys::SubstructureNotifyMask) as c_long,
                                                &mut xclient as *mut _ as *mut X11_sys::XEvent
                                            );
                                        }
                                    }
                                    else {
                                        window.send_finger_down(button.button as usize, self.xkeystate_to_modifiers(button.state))
                                    }
                                }
                            }
                            self.last_click_time = time_now;
                            self.last_click_pos = (button.x_root, button.y_root);
                        },
                        X11_sys::ButtonRelease => { // mouse up
                            let button = event.xbutton;
                            if let Some(window_ptr) = self.window_map.get(&button.window) {
                                let window = &mut (**window_ptr);
                                window.send_finger_up(button.button as usize, self.xkeystate_to_modifiers(button.state))
                            }
                        },
                        X11_sys::KeyPress => {
                            if let Some(window_ptr) = self.window_map.get(&event.xkey.window) {
                                let window = &mut (**window_ptr);
                                let block_text = if event.xkey.keycode != 0 {
                                    let key_code = self.xkeyevent_to_keycode(&mut event.xkey);
                                    let modifiers = self.xkeystate_to_modifiers(event.xkey.state);
                                    
                                    if modifiers.control || modifiers.logo {
                                        match key_code {
                                            KeyCode::KeyV => { // paste
                                                // request the pasteable text from the other side
                                                X11_sys::XConvertSelection(
                                                    self.display,
                                                    self.atom_clipboard,
                                                    self.atom_utf8_string,
                                                    self.atom_clipboard,
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
                                                let mut events = vec![
                                                    Event::TextCopy(TextCopyEvent {
                                                        response: None
                                                    })
                                                ];
                                                self.do_callback(&mut events);
                                                match &events[0] {
                                                    Event::TextCopy(req) => if let Some(response) = &req.response {
                                                        // store the text on the clipboard
                                                        self.clipboard = response.clone();
                                                        // lets set the owner
                                                        println!("Set selection owner");
                                                        X11_sys::XSetSelectionOwner(
                                                            self.display,
                                                            self.atom_clipboard,
                                                            window.window.unwrap(),
                                                            event.xkey.time
                                                        );
                                                        X11_sys::XFlush(self.display);
                                                    },
                                                    _ => ()
                                                };
                                            }
                                            _ => ()
                                        }
                                    }
                                    
                                    let block_text = modifiers.control || modifiers.logo || modifiers.alt;
                                    self.do_callback(&mut vec![Event::KeyDown(KeyEvent {
                                        key_code: key_code,
                                        is_repeat: false,
                                        modifiers: modifiers,
                                        time: self.time_now()
                                    })]);
                                    block_text
                                }else {false};
                                
                                if !block_text {
                                    // decode the character
                                    let mut buffer = [0u8; 32];
                                    let mut keysym = mem::MaybeUninit::uninit();
                                    let mut status = mem::MaybeUninit::uninit();
                                    let count = X11_sys::Xutf8LookupString(
                                        window.xic.unwrap(),
                                        &mut event.xkey,
                                        buffer.as_mut_ptr() as *mut c_char,
                                        buffer.len() as c_int,
                                        keysym.as_mut_ptr(),
                                        status.as_mut_ptr(),
                                    );
                                    //let keysym = keysym.assume_init();
                                    let status = status.assume_init();
                                    if status != X11_sys::XBufferOverflow {
                                        let utf8 = std::str::from_utf8(&buffer[..count as usize]).unwrap_or("").to_string();
                                        let char_code = utf8.chars().next().unwrap_or('\0');
                                        if char_code >= ' ' && char_code != 127 as char {
                                            self.do_callback(&mut vec![
                                                Event::TextInput(TextInputEvent {
                                                    input: utf8,
                                                    was_paste: false,
                                                    replace_last: false
                                                })
                                            ]);
                                        }
                                    }
                                }
                            }
                        },
                        X11_sys::KeyRelease => {
                            self.do_callback(&mut vec![Event::KeyUp(KeyEvent {
                                key_code: self.xkeyevent_to_keycode(&mut event.xkey),
                                is_repeat: false,
                                modifiers: self.xkeystate_to_modifiers(event.xkey.state),
                                time: self.time_now()
                            })]);
                        },
                        X11_sys::ClientMessage => {
                            let event = event.xclient;
                            if event.message_type == self.atom_wm_protocols {
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
                        X11_sys::Expose => {
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
                // process all signals in the queue
                let mut proc_signals = if let Ok(mut signals) = self.signals.lock() {
                    let sigs = signals.clone();
                    signals.clear();
                    sigs
                }
                else {
                    Vec::new()
                };
                if proc_signals.len() > 0 {
                    self.do_callback(&mut proc_signals);
                }
                
                self.do_callback(&mut vec![
                    Event::Paint,
                ]);
            }
            
            self.event_callback = None;
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Event>) {
        unsafe {
            if self.event_callback.is_none() || self.event_recur_block {
                return
            };
            self.event_recur_block = true;
            let callback = self.event_callback.unwrap();
            self.loop_block = (*callback)(self, events);
            self.event_recur_block = false;
        }
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
    
    pub fn post_signal(signal: Signal, status: StatusId) {
        unsafe {
            if let Ok(mut signals_locked) = (*GLOBAL_XLIB_APP).signals.lock() {
                let mut signals = HashMap::new();
                let mut set = BTreeSet::new();
                set.insert(status);
                signals.insert(signal, set);
                signals_locked.push(Event::Signal(SignalEvent {signals}));
                let mut f = unsafe { File::from_raw_fd((*GLOBAL_XLIB_APP).signal_fds[1]) };
                let _ = write!(&mut f, "\0");
            }
        }
    }
    
    pub fn terminate_event_loop(&mut self) {
        // maybe need to do more here
        self.event_loop_running = false;
        unsafe {X11_sys::XCloseIM(self.xim)};
        unsafe {X11_sys::XCloseDisplay(self.display)};
        self.display = ptr::null_mut();
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = precise_time_ns();
        (time_now - self.time_start) as f64 / 1_000_000_000.0
    }
    
    pub fn load_first_cursor(&self, names: &[&[u8]]) -> Option<c_ulong> {
        unsafe {
            for name in names {
                let cursor = X11_sys::XcursorLibraryLoadCursor(
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
                MouseCursor::Hand => self.load_first_cursor(&[b"hand2\0", b"hand1\0"]),
                MouseCursor::Arrow => self.load_first_cursor(&[b"arrow\0"]),
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
                            X11_sys::XDefineCursor(self.display, *k, x11_cursor);
                        }
                    }
                    X11_sys::XFreeCursor(self.display, x11_cursor);
                }
            }
        }
    }
    
    fn xkeystate_to_modifiers(&self, state: c_uint) -> KeyModifiers {
        KeyModifiers {
            alt: state & X11_sys::Mod1Mask != 0,
            shift: state & X11_sys::ShiftMask != 0,
            control: state & X11_sys::ControlMask != 0,
            logo: state & X11_sys::Mod4Mask != 0,
        }
    }
    
    fn xkeyevent_to_keycode(&self, key_event: &mut X11_sys::XKeyEvent) -> KeyCode {
        let mut keysym = 0;
        unsafe {
            X11_sys::XLookupString(
                key_event,
                ptr::null_mut(),
                0,
                &mut keysym,
                ptr::null_mut(),
            );
        }
        match keysym as u32 {
            X11_sys::XK_a => KeyCode::KeyA,
            X11_sys::XK_A => KeyCode::KeyA,
            X11_sys::XK_b => KeyCode::KeyB,
            X11_sys::XK_B => KeyCode::KeyB,
            X11_sys::XK_c => KeyCode::KeyC,
            X11_sys::XK_C => KeyCode::KeyC,
            X11_sys::XK_d => KeyCode::KeyD,
            X11_sys::XK_D => KeyCode::KeyD,
            X11_sys::XK_e => KeyCode::KeyE,
            X11_sys::XK_E => KeyCode::KeyE,
            X11_sys::XK_f => KeyCode::KeyF,
            X11_sys::XK_F => KeyCode::KeyF,
            X11_sys::XK_g => KeyCode::KeyG,
            X11_sys::XK_G => KeyCode::KeyG,
            X11_sys::XK_h => KeyCode::KeyH,
            X11_sys::XK_H => KeyCode::KeyH,
            X11_sys::XK_i => KeyCode::KeyI,
            X11_sys::XK_I => KeyCode::KeyI,
            X11_sys::XK_j => KeyCode::KeyJ,
            X11_sys::XK_J => KeyCode::KeyJ,
            X11_sys::XK_k => KeyCode::KeyK,
            X11_sys::XK_K => KeyCode::KeyK,
            X11_sys::XK_l => KeyCode::KeyL,
            X11_sys::XK_L => KeyCode::KeyL,
            X11_sys::XK_m => KeyCode::KeyM,
            X11_sys::XK_M => KeyCode::KeyM,
            X11_sys::XK_n => KeyCode::KeyN,
            X11_sys::XK_N => KeyCode::KeyN,
            X11_sys::XK_o => KeyCode::KeyO,
            X11_sys::XK_O => KeyCode::KeyO,
            X11_sys::XK_p => KeyCode::KeyP,
            X11_sys::XK_P => KeyCode::KeyP,
            X11_sys::XK_q => KeyCode::KeyQ,
            X11_sys::XK_Q => KeyCode::KeyQ,
            X11_sys::XK_r => KeyCode::KeyR,
            X11_sys::XK_R => KeyCode::KeyR,
            X11_sys::XK_s => KeyCode::KeyS,
            X11_sys::XK_S => KeyCode::KeyS,
            X11_sys::XK_t => KeyCode::KeyT,
            X11_sys::XK_T => KeyCode::KeyT,
            X11_sys::XK_u => KeyCode::KeyU,
            X11_sys::XK_U => KeyCode::KeyU,
            X11_sys::XK_v => KeyCode::KeyV,
            X11_sys::XK_V => KeyCode::KeyV,
            X11_sys::XK_w => KeyCode::KeyW,
            X11_sys::XK_W => KeyCode::KeyW,
            X11_sys::XK_x => KeyCode::KeyX,
            X11_sys::XK_X => KeyCode::KeyX,
            X11_sys::XK_y => KeyCode::KeyY,
            X11_sys::XK_Y => KeyCode::KeyY,
            X11_sys::XK_z => KeyCode::KeyZ,
            X11_sys::XK_Z => KeyCode::KeyZ,
            
            X11_sys::XK_0 => KeyCode::Key0,
            X11_sys::XK_1 => KeyCode::Key1,
            X11_sys::XK_2 => KeyCode::Key2,
            X11_sys::XK_3 => KeyCode::Key3,
            X11_sys::XK_4 => KeyCode::Key4,
            X11_sys::XK_5 => KeyCode::Key5,
            X11_sys::XK_6 => KeyCode::Key6,
            X11_sys::XK_7 => KeyCode::Key7,
            X11_sys::XK_8 => KeyCode::Key8,
            X11_sys::XK_9 => KeyCode::Key9,
            
            X11_sys::XK_Alt_L => KeyCode::Alt,
            X11_sys::XK_Alt_R => KeyCode::Alt,
            X11_sys::XK_Meta_L => KeyCode::Logo,
            X11_sys::XK_Meta_R => KeyCode::Logo,
            X11_sys::XK_Shift_L => KeyCode::Shift,
            X11_sys::XK_Shift_R => KeyCode::Shift,
            X11_sys::XK_Control_L => KeyCode::Control,
            X11_sys::XK_Control_R => KeyCode::Control,
            
            X11_sys::XK_equal => KeyCode::Equals,
            X11_sys::XK_minus => KeyCode::Minus,
            X11_sys::XK_bracketright => KeyCode::RBracket,
            X11_sys::XK_bracketleft => KeyCode::LBracket,
            X11_sys::XK_Return => KeyCode::Return,
            X11_sys::XK_grave => KeyCode::Backtick,
            X11_sys::XK_semicolon => KeyCode::Semicolon,
            X11_sys::XK_backslash => KeyCode::Backslash,
            X11_sys::XK_comma => KeyCode::Comma,
            X11_sys::XK_slash => KeyCode::Slash,
            X11_sys::XK_period => KeyCode::Period,
            X11_sys::XK_Tab => KeyCode::Tab,
            X11_sys::XK_ISO_Left_Tab => KeyCode::Tab,
            X11_sys::XK_space => KeyCode::Space,
            X11_sys::XK_BackSpace => KeyCode::Backspace,
            X11_sys::XK_Escape => KeyCode::Escape,
            X11_sys::XK_Caps_Lock => KeyCode::Capslock,
            X11_sys::XK_KP_Decimal => KeyCode::NumpadDecimal,
            X11_sys::XK_KP_Multiply => KeyCode::NumpadMultiply,
            X11_sys::XK_KP_Add => KeyCode::NumpadAdd,
            X11_sys::XK_Num_Lock => KeyCode::Numlock,
            X11_sys::XK_KP_Divide => KeyCode::NumpadDivide,
            X11_sys::XK_KP_Enter => KeyCode::NumpadEnter,
            X11_sys::XK_KP_Subtract => KeyCode::NumpadSubtract,
            //keysim::XK_9 => KeyCode::NumpadEquals,
            X11_sys::XK_KP_0 => KeyCode::Numpad0,
            X11_sys::XK_KP_1 => KeyCode::Numpad1,
            X11_sys::XK_KP_2 => KeyCode::Numpad2,
            X11_sys::XK_KP_3 => KeyCode::Numpad3,
            X11_sys::XK_KP_4 => KeyCode::Numpad4,
            X11_sys::XK_KP_5 => KeyCode::Numpad5,
            X11_sys::XK_KP_6 => KeyCode::Numpad6,
            X11_sys::XK_KP_7 => KeyCode::Numpad7,
            X11_sys::XK_KP_8 => KeyCode::Numpad8,
            X11_sys::XK_KP_9 => KeyCode::Numpad9,
            
            X11_sys::XK_F1 => KeyCode::F1,
            X11_sys::XK_F2 => KeyCode::F2,
            X11_sys::XK_F3 => KeyCode::F3,
            X11_sys::XK_F4 => KeyCode::F4,
            X11_sys::XK_F5 => KeyCode::F5,
            X11_sys::XK_F6 => KeyCode::F6,
            X11_sys::XK_F7 => KeyCode::F7,
            X11_sys::XK_F8 => KeyCode::F8,
            X11_sys::XK_F9 => KeyCode::F9,
            X11_sys::XK_F10 => KeyCode::F10,
            X11_sys::XK_F11 => KeyCode::F11,
            X11_sys::XK_F12 => KeyCode::F12,
            
            X11_sys::XK_Print => KeyCode::PrintScreen,
            X11_sys::XK_Home => KeyCode::Home,
            X11_sys::XK_Page_Up => KeyCode::PageUp,
            X11_sys::XK_Delete => KeyCode::Delete,
            X11_sys::XK_End => KeyCode::End,
            X11_sys::XK_Page_Down => KeyCode::PageDown,
            X11_sys::XK_Left => KeyCode::ArrowLeft,
            X11_sys::XK_Right => KeyCode::ArrowRight,
            X11_sys::XK_Down => KeyCode::ArrowDown,
            X11_sys::XK_Up => KeyCode::ArrowUp,
            _ => KeyCode::Unknown,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
#[repr(C)]
struct MwmHints {
    pub flags: c_ulong,
    pub functions: c_ulong,
    pub decorations: c_ulong,
    pub input_mode: c_long,
    pub status: c_ulong,
}

const MWM_HINTS_FUNCTIONS: c_ulong = 1 << 0;
const MWM_HINTS_DECORATIONS: c_ulong = 1 << 1;

const MWM_FUNC_ALL: c_ulong = 1 << 0;
const MWM_FUNC_RESIZE: c_ulong = 1 << 1;
const MWM_FUNC_MOVE: c_ulong = 1 << 2;
const MWM_FUNC_MINIMIZE: c_ulong = 1 << 3;
const MWM_FUNC_MAXIMIZE: c_ulong = 1 << 4;
const MWM_FUNC_CLOSE: c_ulong = 1 << 5;
const _NET_WM_MOVERESIZE_SIZE_TOPLEFT: c_long = 0;
const _NET_WM_MOVERESIZE_SIZE_TOP: c_long = 1;
const _NET_WM_MOVERESIZE_SIZE_TOPRIGHT: c_long = 2;
const _NET_WM_MOVERESIZE_SIZE_RIGHT: c_long = 3;
const _NET_WM_MOVERESIZE_SIZE_BOTTOMRIGHT: c_long = 4;
const _NET_WM_MOVERESIZE_SIZE_BOTTOM: c_long = 5;
const _NET_WM_MOVERESIZE_SIZE_BOTTOMLEFT: c_long = 6;
const _NET_WM_MOVERESIZE_SIZE_LEFT: c_long = 7;
const _NET_WM_MOVERESIZE_MOVE: c_long = 8;/* movement only */
const _NET_WM_MOVERESIZE_SIZE_KEYBOARD: c_long = 9;/* size via keyboard */
const _NET_WM_MOVERESIZE_MOVE_KEYBOARD: c_long = 10;

const _NET_WM_STATE_REMOVE: c_long = 0;/* remove/unset property */
const _NET_WM_STATE_ADD: c_long = 1;/* add/set property */
const _NET_WM_STATE_TOGGLE: c_long = 2;/* toggle property  */

/* move via keyboard */

pub struct Dnd {
    atoms: DndAtoms,
    display: *mut X11_sys::Display,
    type_list: Option<Vec<X11_sys::Atom >>,
    selection: Option<CString>,
}

impl Dnd {
    unsafe fn new(display: *mut X11_sys::Display) -> Dnd {
        Dnd {
            atoms: DndAtoms::new(display),
            display,
            type_list: None,
            selection: None,
        }
    }
    
    /// Enables drag-and-drop for the given window.
    unsafe fn enable_for_window(&mut self, window: X11_sys::Window) {
        // To enable drag-and-drop for a window, we need to set the XDndAware property of the window
        // to the version of XDnd we support.
        
        // I took this value from the Winit source code. Apparently, this is the latest version, and
        // hasn't changed since 2002.
        let version = 5 as c_ulong;
        
        X11_sys::XChangeProperty(
            self.display,
            window,
            self.atoms.aware,
            4, // XA_ATOM
            32,
            X11_sys::PropModeReplace as c_int,
            &version as *const c_ulong as *const c_uchar,
            1
        );
    }
    
    /// Handles a XDndEnter event.
    unsafe fn handle_enter_event(&mut self, event: &X11_sys::XClientMessageEvent) {
        // The XDndEnter event is sent by the source window when a drag begins. That is, the mouse
        // enters the client rectangle of the target window. The target window is supposed to
        // respond to this by requesting the list of types supported by the source.
        
        let source_window = event.data.l[0] as X11_sys::Window;
        let has_more_types = event.data.l[1] & (1 << 0) != 0;
        
        // If the has_more_types flags is set, we have to obtain the list of supported types from
        // the XDndTypeList property. Otherwise, we can obtain the list of supported types from the
        // event itself.
        self.type_list = Some(if has_more_types {
            self.get_type_list_property(source_window)
        } else {
            event.data.l[2..4]
                .iter()
                .map( | &l | l as X11_sys::Atom)
                .filter( | &atom | atom != X11_sys::None as X11_sys::Atom)
                .collect()
        });
    }
    
    /// Handles a XDndDrop event.
    unsafe fn handle_drop_event(&mut self, event: &X11_sys::XClientMessageEvent) {
        // The XDndLeave event is sent by the source window when a drag is confirmed. That is, the
        // mouse button is released while the mouse is inside the client rectangle of the target
        // window. The target window is supposed to respond to this by requesting that the selection
        // representing the thing being dragged is converted to the appropriate data type (in our
        // case, a URI list). The source window, in turn, is supposed to respond this by sending a
        // selection event containing the data to the source window.
        
        let target_window = event.window as X11_sys::Window;
        self.convert_selection(target_window);
        self.type_list = None;
    }
    
    /// Handles a XDndLeave event.
    unsafe fn handle_leave_event(&mut self, _event: &X11_sys::XClientMessageEvent) {
        // The XDndLeave event is sent by the source window when a drag is canceled. That is, the
        // mouse leaves the client rectangle of the target window. The target window is supposed to
        // repsond this this by pretending the drag never happened.
        
        self.type_list = None;
    }
    
    /// Handles a XDndPosition event.
    unsafe fn handle_position_event(&mut self, event: &X11_sys::XClientMessageEvent) {
        // The XDndPosition event is sent by the source window after the XDndEnter event, every time
        // the mouse is moved. The target window is supposed to respond to this by sending a status
        // event to the source window notifying whether it can accept the drag at this position.
        
        let target_window = event.window as X11_sys::Window;
        let source_window = event.data.l[0] as X11_sys::Window;
        
        // For now we accept te drag if and only if the list of types supported by the source
        // includes a uri list.
        //
        // TODO: Extend this test by taking into account the position of the mouse as well.
        let accepted = self.type_list.as_ref().map_or(false, | type_list | type_list.contains(&self.atoms.uri_list));
        
        // Notify the source window whether we can accept the drag at this position.
        self.send_status_event(source_window, target_window, accepted);
        
        // If this is the first time we've accepted the drag, request that the drag-and-drop
        // selection be converted to a URI list. The target window is supposed to respond to this by
        // sending a XSelectionEvent containing the URI list.
        
        // Since this is an asynchronous operation, its possible for another XDndPosition event to
        // come in before the response to the first conversion request has been received. In this
        // case, a second conversion request will be sent, the response to which will be ignored.
        if accepted && self.selection.is_none() {
        }
    }
    
    /// Handles a XSelectionEvent.
    unsafe fn handle_selection_event(&mut self, _event: &X11_sys::XSelectionEvent) {
        // The XSelectionEvent is sent by the source window in response to a request by the source
        // window to convert the selection representing the thing being dragged to the appropriate
        // data type. This request is always sent in response to a XDndDrop event, so this event
        // should only be received after a drop operation has completed.
        
        //let source_window = event.requestor;
        //let selection = CString::new(self.get_selection_property(source_window)).unwrap();
        
        // TODO: Actually use the selection
    }
    
    /// Gets the XDndSelection property from the source window.
    unsafe fn get_selection_property(&mut self, source_window: X11_sys::Window) -> Vec<c_uchar> {
        let mut selection = Vec::new();
        let mut offset = 0;
        let length = 1024;
        let mut actual_type = 0;
        let mut actual_format = 0;
        let mut nitems = 0;
        let mut bytes_after = 0;
        let mut prop = ptr::null_mut();
        loop {
            X11_sys::XGetWindowProperty(
                self.display,
                source_window,
                self.atoms.selection,
                offset,
                length,
                X11_sys::False as c_int,
                self.atoms.uri_list,
                &mut actual_type,
                &mut actual_format,
                &mut nitems,
                &mut bytes_after,
                &mut prop,
            );
            selection.extend_from_slice(slice::from_raw_parts(prop as *mut c_uchar, nitems as usize));
            X11_sys::XFree(prop as *mut c_void);
            if bytes_after == 0 {
                break;
            }
            offset += length;
        };
        selection
    }
    
    /// Gets the XDndTypeList property from the source window.
    unsafe fn get_type_list_property(&mut self, source_window: X11_sys::Window) -> Vec<X11_sys::Atom> {
        let mut type_list = Vec::new();
        let mut offset = 0;
        let length = 1024;
        let mut actual_type = 0;
        let mut actual_format = 0;
        let mut nitems = 0;
        let mut bytes_after = 0;
        let mut prop = ptr::null_mut();
        loop {
            X11_sys::XGetWindowProperty(
                self.display,
                source_window,
                self.atoms.type_list,
                offset,
                length,
                X11_sys::False as c_int,
                4, // XA_ATOM,
                &mut actual_type,
                &mut actual_format,
                &mut nitems,
                &mut bytes_after,
                &mut prop,
            );
            type_list.extend_from_slice(slice::from_raw_parts(prop as *mut X11_sys::Atom, nitems as usize));
            X11_sys::XFree(prop as *mut c_void);
            if bytes_after == 0 {
                break;
            }
            offset += length;
        };
        type_list
    }
    
    /// Sends a XDndStatus event to the target window.
    unsafe fn send_status_event(&mut self, source_window: X11_sys::Window, target_window: X11_sys::Window, accepted: bool) {
        X11_sys::XSendEvent(
            self.display,
            source_window,
            X11_sys::False as c_int,
            X11_sys::NoEventMask as c_long,
            &mut X11_sys::XClientMessageEvent {
                type_: X11_sys::ClientMessage as c_int,
                serial: 0,
                send_event: 0,
                display: self.display,
                window: source_window,
                message_type: self.atoms.status,
                format: 32,
                data: {
                    let mut data = mem::zeroed::<X11_sys::XClientMessageEvent__bindgen_ty_1>();
                    data.l[0] = target_window as c_long;
                    data.l[1] = if accepted {1 << 0} else {0};
                    data.l[2] = 0;
                    data.l[3] = 0;
                    data.l[4] = if accepted {self.atoms.action_private} else {self.atoms.none} as c_long;
                    data
                }
            } as *mut X11_sys::XClientMessageEvent as *mut X11_sys::XEvent
        );
        X11_sys::XFlush(self.display);
    }
    
    // Requests that the selection representing the thing being dragged is converted to the
    // appropriate data type (in our case, a URI list).
    unsafe fn convert_selection(&self, target_window: X11_sys::Window) {
        X11_sys::XConvertSelection(
            self.display,
            self.atoms.selection,
            self.atoms.uri_list,
            self.atoms.selection,
            target_window,
            X11_sys::CurrentTime as X11_sys::Time,
        );
    }
}

struct DndAtoms {
    action_private: X11_sys::Atom,
    aware: X11_sys::Atom,
    drop: X11_sys::Atom,
    enter: X11_sys::Atom,
    leave: X11_sys::Atom,
    none: X11_sys::Atom,
    position: X11_sys::Atom,
    selection: X11_sys::Atom,
    status: X11_sys::Atom,
    type_list: X11_sys::Atom,
    uri_list: X11_sys::Atom,
}

impl DndAtoms {
    unsafe fn new(display: *mut X11_sys::Display) -> DndAtoms {
        DndAtoms {
            action_private: X11_sys::XInternAtom(display, CString::new("XdndActionPrivate").unwrap().as_ptr(), 0),
            aware: X11_sys::XInternAtom(display, CString::new("XdndAware").unwrap().as_ptr(), 0),
            drop: X11_sys::XInternAtom(display, CString::new("XdndDrop").unwrap().as_ptr(), 0),
            enter: X11_sys::XInternAtom(display, CString::new("XdndEnter").unwrap().as_ptr(), 0),
            leave: X11_sys::XInternAtom(display, CString::new("XdndLeave").unwrap().as_ptr(), 0),
            none: X11_sys::XInternAtom(display, CString::new("None").unwrap().as_ptr(), 0),
            position: X11_sys::XInternAtom(display, CString::new("XdndPosition").unwrap().as_ptr(), 0),
            selection: X11_sys::XInternAtom(display, CString::new("XdndSelection").unwrap().as_ptr(), 0),
            status: X11_sys::XInternAtom(display, CString::new("XdndStatus").unwrap().as_ptr(), 0),
            type_list: X11_sys::XInternAtom(display, CString::new("XdndTypeList").unwrap().as_ptr(), 0),
            uri_list: X11_sys::XInternAtom(display, CString::new("text/uri-list").unwrap().as_ptr(), 0),
        }
    }
}
