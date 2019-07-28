use crate::cx::*;
use libc;
use std::collections::HashMap;
use std::ffi::CString;
use std::ffi::CStr;
use std::mem;
use std::os::raw::{c_int, c_ulong};
use std::ptr;
use time::precise_time_ns;
use x11_dl::xlib;
use x11_dl::xlib::{Display, XVisualInfo, Xlib};

static mut GLOBAL_XLIB_APP: *mut XlibApp = 0 as *mut _;

pub struct XlibApp {
    pub xlib: Xlib,
    pub display: *mut Display,
    
    pub display_fd: c_int,
    pub window_map: HashMap<c_ulong, *mut XlibWindow>,
    
    pub time_start: u64,
    pub event_callback: Option<*mut FnMut(&mut XlibApp, &mut Vec<Event>) -> bool>,
    pub event_recur_block: bool,
    pub event_loop_running: bool,
    pub timers: Vec<XlibTimer>,
    pub free_timers: Vec<usize>,
    
    pub loop_block: bool,
    pub current_cursor: MouseCursor,
}

#[derive(Clone)]
pub struct XlibWindow {
    pub window: Option<c_ulong>,
    
    pub window_id: usize,
    pub xlib_app: *mut XlibApp,
    pub last_window_geom: WindowGeom,
    
    pub time_start: u64,
    
    pub last_key_mod: KeyModifiers,
    pub ime_spot: Vec2,
    pub current_cursor: MouseCursor,
    pub last_mouse_pos: Vec2,
    pub fingers_down: Vec<bool>,
}

#[derive(Clone)]
pub enum XlibTimer {
    Free,
    Timer {timer_id: u64, interval: f64, repeats: bool}
}

impl XlibApp {
    pub fn new() -> XlibApp {
        unsafe {
            let xlib = Xlib::open().unwrap();
            let display = (xlib.XOpenDisplay)(ptr::null());
            let display_fd = (xlib.XConnectionNumber)(display);
            XlibApp {
                xlib,
                display,
                display_fd,
                window_map: HashMap::new(),
                time_start: precise_time_ns(),
                event_callback: None,
                event_recur_block: false,
                event_loop_running: true,
                loop_block: false,
                timers: Vec::new(),
                free_timers: Vec::new(),
                current_cursor: MouseCursor::Default
            }
        }
    }
    
    pub fn init(&mut self) {
        unsafe {
            unsafe {
                (self.xlib.XrmInitialize)();
            }
            GLOBAL_XLIB_APP = self;
        }
    }
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut XlibApp, &mut Vec<Event>) -> bool,
    {
        unsafe {
            self.event_callback = Some(
                &mut event_handler as *const FnMut(&mut XlibApp, &mut Vec<Event>) -> bool
                as *mut FnMut(&mut XlibApp, &mut Vec<Event>) -> bool
            );
            
            self.do_callback(&mut vec![
                Event::Paint,
            ]);
            
            while self.event_loop_running {
                if self.loop_block {
                    let mut fds = mem::uninitialized();
                    libc::FD_ZERO(&mut fds);
                    libc::FD_SET(self.display_fd, &mut fds);
                    let _nfds = libc::select(
                        self.display_fd + 1,
                        &mut fds,
                        ptr::null_mut(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                    );
                }
                while (self.xlib.XPending)(self.display) != 0 {
                    let mut event = mem::uninitialized();
                    (self.xlib.XNextEvent)(self.display, &mut event);
                    match event.type_ {
                        xlib::ConfigureNotify => {
                            let cfg = event.configure;
                            if let Some(window) = self.window_map.get(&cfg.window) {
                                (**window).send_change_event();
                            }
                        },
                        xlib::MotionNotify => { // mousemove 
                            let motion = event.motion;
                            //println!("Motion {} {}", motion.x, motion.y);
                        },
                        xlib::KeyRelease => {
                        },
                        xlib::ClientMessage => {
                            /*
                            if event.client_message.data.get_long(0) as u64 == wm_delete_message {
                                self.event_loop_running = false;
                            }*/
                        },
                        xlib::Expose => {
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
    
    pub fn get_free_timer_slot(&mut self) -> usize {
        if self.free_timers.len()>0 {
            self.free_timers.pop().unwrap()
        }
        else {
            let slot = self.timers.len();
            self.timers.push(XlibTimer::Free);
            slot
        }
    }
    
    pub fn start_timer(&mut self, timer_id: u64, interval: f64, repeats: bool) {
        let slot = self.get_free_timer_slot();
        //let win32_id = unsafe {winuser::SetTimer(NULL as HWND, 0, (interval * 1000.0) as u32, Some(Self::timer_proc))};
        self.timers[slot] = XlibTimer::Timer {
            timer_id: timer_id,
            interval: interval,
            repeats: repeats
        };
    }
    
    pub fn stop_timer(&mut self, which_timer_id: u64) {
        for slot in 0..self.timers.len() {
            if let XlibTimer::Timer {timer_id, ..} = self.timers[slot] {
                if timer_id == which_timer_id {
                    self.timers[slot] = XlibTimer::Free;
                    self.free_timers.push(slot);
                    //unsafe {winuser::KillTimer(NULL as HWND, win32_id);}
                }
            }
        }
    }
    
    pub fn post_signal(_signal_id: usize, _value: usize) {
        //unsafe {
        //let win32_app = &mut (*GLOBAL_WIN32_APP);
        //if win32_app.all_windows.len()>0 {
        //    winuser::PostMessageW(win32_app.all_windows[0], winuser::WM_USER, signal_id as usize, value as isize);
        // }
        //}
    }
    
    pub fn terminate_event_loop(&mut self) {
        // maybe need to do more here
        self.event_loop_running = false;
        
        unsafe {(self.xlib.XCloseDisplay)(self.display)};
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = precise_time_ns();
        (time_now - self.time_start) as f64 / 1_000_000_000.0
    }
    
    pub fn set_mouse_cursor(&mut self, cursor: MouseCursor) {
        if self.current_cursor != cursor {
            /*
            let win32_cursor = match cursor {
                MouseCursor::Hidden => {
                    ptr::null()
                },
                MouseCursor::Default => winuser::IDC_ARROW,
                MouseCursor::Crosshair => winuser::IDC_CROSS,
                MouseCursor::Hand => winuser::IDC_HAND,
                MouseCursor::Arrow => winuser::IDC_ARROW,
                MouseCursor::Move => winuser::IDC_SIZEALL,
                MouseCursor::Text => winuser::IDC_IBEAM,
                MouseCursor::Wait => winuser::IDC_ARROW,
                MouseCursor::Help => winuser::IDC_HELP,
                MouseCursor::Progress => winuser::IDC_ARROW,
                MouseCursor::NotAllowed => winuser::IDC_NO,
                MouseCursor::ContextMenu => winuser::IDC_ARROW,
                MouseCursor::Cell => winuser::IDC_ARROW,
                MouseCursor::VerticalText => winuser::IDC_ARROW,
                MouseCursor::Alias => winuser::IDC_ARROW,
                MouseCursor::Copy => winuser::IDC_ARROW,
                MouseCursor::NoDrop => winuser::IDC_ARROW,
                MouseCursor::Grab => winuser::IDC_ARROW,
                MouseCursor::Grabbing => winuser::IDC_ARROW,
                MouseCursor::AllScroll => winuser::IDC_ARROW,
                MouseCursor::ZoomIn => winuser::IDC_ARROW,
                MouseCursor::ZoomOut => winuser::IDC_ARROW,
                MouseCursor::NsResize => winuser::IDC_SIZENS,
                MouseCursor::NeswResize => winuser::IDC_SIZENESW,
                MouseCursor::EwResize => winuser::IDC_SIZEWE,
                MouseCursor::NwseResize => winuser::IDC_SIZENWSE,
                MouseCursor::ColResize => winuser::IDC_SIZEWE,
                MouseCursor::RowResize => winuser::IDC_SIZEWE,
            };
            */
            self.current_cursor = cursor;
            //TODO
        }
    }
}

impl XlibWindow {
    
    pub fn new(xlib_app: &mut XlibApp, window_id: usize) -> XlibWindow {
        let mut fingers_down = Vec::new();
        fingers_down.resize(NUM_FINGERS, false);
        
        XlibWindow {
            window: None,
            
            window_id: window_id,
            xlib_app: xlib_app,
            last_window_geom: WindowGeom::default(),
            time_start: xlib_app.time_start,
            last_key_mod: KeyModifiers::default(),
            ime_spot: Vec2::zero(),
            current_cursor: MouseCursor::Default,
            last_mouse_pos: Vec2::zero(),
            fingers_down: fingers_down,
        }
    }
    
    pub fn init(&mut self, _title: &str, size: Vec2, position: Option<Vec2>, visual_info: *const XVisualInfo) {
        unsafe {
            let xlib = &(*self.xlib_app).xlib;
            let display = (*self.xlib_app).display;
            
            // The default screen of the display
            let default_screen = (xlib.XDefaultScreen)(display);
            
            // The root window of the default screen
            let root_window = (xlib.XRootWindow)(display, default_screen);
            
            let mut attributes = mem::zeroed::<xlib::XSetWindowAttributes>();
            attributes.border_pixel = 0;
            attributes.override_redirect = 1;
            attributes.colormap =
            (xlib.XCreateColormap)(display, root_window, (*visual_info).visual, xlib::AllocNone);
            attributes.event_mask = xlib::ExposureMask
                | xlib::StructureNotifyMask
                | xlib::ButtonMotionMask
                | xlib::PointerMotionMask
                | xlib::ButtonPressMask
                | xlib::ButtonReleaseMask
                | xlib::KeyPressMask
                | xlib::KeyReleaseMask
                | xlib::VisibilityChangeMask
                | xlib::FocusChangeMask;
            
            let dpi_factor = self.get_dpi_factor();
            // Create a window
            let window = (xlib.XCreateWindow)(
                display,
                root_window,
                if position.is_some() {position.unwrap().x}else {10.0} as i32,
                if position.is_some() {position.unwrap().y}else {10.0} as i32,
                (size.x*dpi_factor) as u32,
                (size.y*dpi_factor) as u32, 
                0,
                (*visual_info).depth,
                xlib::InputOutput as u32,
                (*visual_info).visual,
                xlib::CWBorderPixel | xlib::CWColormap | xlib::CWEventMask | xlib::CWOverrideRedirect,
                &mut attributes,
            );
            
            // Tell the window manager that we want to be notified when the window is closed
            let mut wm_delete_message = (xlib.XInternAtom)(
                display,
                CString::new("WM_DELETE_WINDOW").unwrap().as_ptr(),
                xlib::False,
            );
            (xlib.XSetWMProtocols)(display, window, &mut wm_delete_message, 1);
            
            // Map the window to the screen
            (xlib.XMapWindow)(display, window);
            
            (xlib.XFlush)(display);
            (*self.xlib_app).window_map.insert(window, self);
            self.window = Some(window);
        }
    }
    
    pub fn get_key_modifiers() -> KeyModifiers {
        //unsafe {
        KeyModifiers {
            control: false,
            shift: false,
            alt: false,
            logo: false
        }
        //}
    }
    
    pub fn update_ptrs(&mut self) {
        unsafe {
            (*self.xlib_app).window_map.insert(self.window.unwrap(), self);
        }
    }
    
    pub fn on_mouse_move(&self) {
    }
    
    
    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {
    }
    
    pub fn restore(&self) {
    }
    
    pub fn maximize(&self) {
    }
    
    pub fn close_window(&self) {
    }
    
    pub fn minimize(&self) {
    }
    
    pub fn set_topmost(&self, _topmost: bool) {
    }
    
    pub fn get_is_topmost(&self) -> bool {
        false
    }
    
    pub fn get_window_geom(&self) -> WindowGeom {
        WindowGeom {
            is_topmost: self.get_is_topmost(),
            is_fullscreen: self.get_is_maximized(),
            inner_size: self.get_inner_size(),
            outer_size: self.get_outer_size(),
            dpi_factor: self.get_dpi_factor(),
            position: self.get_position()
        }
    }
    
    pub fn get_is_maximized(&self) -> bool {
        false
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = precise_time_ns();
        (time_now - self.time_start) as f64 / 1_000_000_000.0
    }
    
    pub fn set_ime_spot(&mut self, spot: Vec2) {
        self.ime_spot = spot;
    }
    
    pub fn get_position(&self) -> Vec2 {
        unsafe {
            let mut xwa = mem::uninitialized();
            let xlib = &(*self.xlib_app).xlib;
            let display = (*self.xlib_app).display;
            (xlib.XGetWindowAttributes)(display, self.window.unwrap(), &mut xwa);
            return Vec2 {x: xwa.x as f32, y: xwa.y as f32}
            /*
            let mut child = mem::uninitialized();
            let default_screen = (xlib.XDefaultScreen)(display);
            let root_window = (xlib.XRootWindow)(display, default_screen);
            let mut x:c_int = 0;
            let mut y:c_int = 0;
            (xlib.XTranslateCoordinates)(display, self.window.unwrap(), root_window, 0, 0, &mut x, &mut y, &mut child );
            */
        }
    }
    
    pub fn get_inner_size(&self) -> Vec2 {
        let dpi_factor = self.get_dpi_factor();
        unsafe {
            let mut xwa = mem::uninitialized();
            let xlib = &(*self.xlib_app).xlib;
            let display = (*self.xlib_app).display;
            (xlib.XGetWindowAttributes)(display, self.window.unwrap(), &mut xwa);
            return Vec2 {x: xwa.width as f32 / dpi_factor, y: xwa.height as f32 / dpi_factor}
        }
    }
    
    pub fn get_outer_size(&self) -> Vec2 {
        unsafe {
            let mut xwa = mem::uninitialized();
            let xlib = &(*self.xlib_app).xlib;
            let display = (*self.xlib_app).display;
            (xlib.XGetWindowAttributes)(display, self.window.unwrap(), &mut xwa);
            return Vec2 {x: xwa.width as f32, y: xwa.height as f32}
        }
    }
    
    pub fn set_position(&mut self, _pos: Vec2) {
    }
    
    pub fn set_outer_size(&self, _size: Vec2) {
    }
    
    pub fn set_inner_size(&self, _size: Vec2) {
    }
    
    pub fn get_dpi_factor(&self) -> f32 {
        unsafe {
            let xlib = &(*self.xlib_app).xlib;
            let display = (*self.xlib_app).display;
            let resource_string = (xlib.XResourceManagerString)(display);
            let db = (xlib.XrmGetStringDatabase)(resource_string);
            let mut ty = mem::uninitialized();
            let mut value = mem::uninitialized();
            (xlib.XrmGetResource)(
                db,
                CString::new("Xft.dpi").unwrap().as_ptr(),
                CString::new("String").unwrap().as_ptr(),
                &mut ty,
                &mut value
            );
            let dpi:f32 = CStr::from_ptr(value.addr).to_str().unwrap().parse().unwrap();
            return dpi / 96.0;
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Event>) {
        unsafe {
            (*self.xlib_app).do_callback(events);
        }
    }
    
    pub fn send_change_event(&mut self) {
        
        let new_geom = self.get_window_geom(); 
        let old_geom = self.last_window_geom.clone();
        self.last_window_geom = new_geom.clone();
        
        self.do_callback(&mut vec![
            Event::WindowGeomChange(WindowGeomChangeEvent {
                window_id: self.window_id,
                old_geom: old_geom,
                new_geom: new_geom
            }),
            Event::Paint
        ]);
    }
    
    pub fn send_focus_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocus]);
    }
    
    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocusLost]);
    }
    
    pub fn send_finger_down(&mut self, digit: usize, modifiers: KeyModifiers) {
        let mut down_count = 0;
        for is_down in &self.fingers_down {
            if *is_down {
                down_count += 1;
            }
        }
        if down_count == 0 {
            //unsafe {winuser::SetCapture(self.hwnd.unwrap());}
        }
        self.fingers_down[digit] = true;
        self.do_callback(&mut vec![Event::FingerDown(FingerDownEvent {
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            rel: self.last_mouse_pos,
            rect: Rect::zero(),
            digit: digit,
            handled: false,
            is_touch: false,
            modifiers: modifiers,
            tap_count: 0,
            time: self.time_now()
        })]);
    }
    
    pub fn send_finger_up(&mut self, digit: usize, modifiers: KeyModifiers) {
        self.fingers_down[digit] = false;
        let mut down_count = 0;
        for is_down in &self.fingers_down {
            if *is_down {
                down_count += 1;
            }
        }
        if down_count == 0 {
            // unsafe {winuser::ReleaseCapture();}
        }
        self.do_callback(&mut vec![Event::FingerUp(FingerUpEvent {
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            rel: self.last_mouse_pos,
            rect: Rect::zero(),
            abs_start: Vec2::zero(),
            rel_start: Vec2::zero(),
            digit: digit,
            is_over: false,
            is_touch: false,
            modifiers: modifiers,
            time: self.time_now()
        })]);
    }
    
    pub fn send_finger_hover_and_move(&mut self, pos: Vec2, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        let mut events = Vec::new();
        for (digit, down) in self.fingers_down.iter().enumerate() {
            if *down {
                events.push(Event::FingerMove(FingerMoveEvent {
                    window_id: self.window_id,
                    abs: pos,
                    rel: pos,
                    rect: Rect::zero(),
                    digit: digit,
                    abs_start: Vec2::zero(),
                    rel_start: Vec2::zero(),
                    is_over: false,
                    is_touch: false,
                    modifiers: modifiers.clone(),
                    time: self.time_now()
                }));
            }
        };
        events.push(Event::FingerHover(FingerHoverEvent {
            window_id: self.window_id,
            abs: pos,
            rel: pos,
            any_down: false,
            rect: Rect::zero(),
            handled: false,
            hover_state: HoverState::Over,
            modifiers: modifiers,
            time: self.time_now()
        }));
        self.do_callback(&mut events);
    }
    
    pub fn send_close_requested_event(&mut self) -> bool {
        let mut events = vec![Event::WindowCloseRequested(WindowCloseRequestedEvent {window_id: self.window_id, accept_close: true})];
        self.do_callback(&mut events);
        if let Event::WindowCloseRequested(cre) = &events[0] {
            return cre.accept_close
        }
        true
    }
    
    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(&mut vec![Event::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last
        })])
    }
    
}