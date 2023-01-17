use {
    std::{
        mem,
        os::raw::{c_ulong, c_long, c_void},
        ptr,
    },
    crate::{
        window::WindowId,
        makepad_math::{DVec2,Rect},
        event::*,
        cursor::MouseCursor,
        os::linux::xlib_app::*,
        os::linux::x11_sys,
    },
};


#[derive(Clone)]
pub struct XlibWindow {
    pub window: Option<c_ulong>,
    pub xic: Option<x11_sys::XIC>,
    pub attributes: Option<x11_sys::XSetWindowAttributes>,
    pub visual_info: Option<x11_sys::XVisualInfo>,
    pub child_windows: Vec<XlibChildWindow>,
    
    pub last_nc_mode: Option<c_long>,
    pub window_id: WindowId,
    pub last_window_geom: WindowGeom,
    pub time_start: u64,
    
    pub ime_spot: DVec2,
    pub current_cursor: MouseCursor,
    pub last_mouse_pos: DVec2,
    pub fingers_down: Vec<bool>,
}

#[derive(Clone)]
pub struct XlibChildWindow {
    pub window: c_ulong,
    visible: bool,
    x: i32,
    y: i32,
    w: u32,
    h: u32
}


impl XlibWindow {
    
    pub fn new(window_id: WindowId) -> XlibWindow {
        
        XlibWindow {
            window: None,
            xic: None,
            attributes: None,
            visual_info: None,
            child_windows: Vec::new(),
            window_id,
            last_window_geom: WindowGeom::default(),
            last_nc_mode: None,
            ime_spot: DVec2::default(),
            current_cursor: MouseCursor::Default,
            last_mouse_pos: DVec2::default(),
        }
    }
    
    pub fn init(&mut self, title: &str, size: DVec2, position: Option<DVec2>, visual_info: x11_sys::XVisualInfo) {
        unsafe {
            let display = get_xlib_app_global().display;
            
            // The default screen of the display
            let default_screen = x11_sys::XDefaultScreen(display);
            
            // The root window of the default screen
            let root_window = X11_sys::XRootWindow(display, default_screen);
            
            let mut attributes = mem::zeroed::<X11_sys::XSetWindowAttributes>();
            
            attributes.border_pixel = 0;
            //attributes.override_redirect = 1;
            attributes.colormap =
            X11_sys::XCreateColormap(display, root_window, visual_info.visual, X11_sys::AllocNone as i32);
            attributes.event_mask = (
                X11_sys::ExposureMask
                    | X11_sys::StructureNotifyMask
                    | X11_sys::ButtonMotionMask
                    | X11_sys::PointerMotionMask
                    | X11_sys::ButtonPressMask
                    | X11_sys::ButtonReleaseMask
                    | X11_sys::KeyPressMask
                    | X11_sys::KeyReleaseMask
                    | X11_sys::VisibilityChangeMask
                    | X11_sys::FocusChangeMask
                    | X11_sys::EnterWindowMask
                    | X11_sys::LeaveWindowMask
            ) as c_long;
            
            let dpi_factor = self.get_dpi_factor();
            // Create a window
            let window = X11_sys::XCreateWindow(
                display,
                root_window,
                if position.is_some() {position.unwrap().x}else {150.0} as i32,
                if position.is_some() {position.unwrap().y}else {60.0} as i32,
                (size.x * dpi_factor) as u32,
                (size.y * dpi_factor) as u32,
                0,
                visual_info.depth,
                X11_sys::InputOutput as u32,
                visual_info.visual,
                (X11_sys::CWBorderPixel | X11_sys::CWColormap | X11_sys::CWEventMask) as c_ulong, // | X11_sys::CWOverrideRedirect,
                &mut attributes,
            );
            
            // Tell the window manager that we want to be notified when the window is closed
            X11_sys::XSetWMProtocols(display, window, &mut (*self.xlib_app).atom_wm_delete_window, 1);
            
            if LINUX_CUSTOM_WINDOW_CHROME{
                let hints = MwmHints {
                    flags: MWM_HINTS_DECORATIONS,
                    functions: 0,
                    decorations: 0,
                    input_mode: 0,
                    status: 0,
                };
                
                let atom_motif_wm_hints = (*self.xlib_app).atom_motif_wm_hints;
                
                X11_sys::XChangeProperty(display, window, atom_motif_wm_hints, atom_motif_wm_hints, 32, X11_sys::PropModeReplace as i32, &hints as *const _ as *const u8, 5);
            }
            
            (*self.xlib_app).dnd.enable_for_window(window);
            
            // Map the window to the screen
            X11_sys::XMapWindow(display, window);
            X11_sys::XFlush(display);
            
            let title_bytes = format!("{}\0",title);
            X11_sys::XStoreName(display, window, title_bytes.as_bytes().as_ptr() as *const ::std::os::raw::c_char);
            
            let xic = X11_sys::XCreateIC((*self.xlib_app).xim, CStr::from_bytes_with_nul(X11_sys::XNInputStyle.as_ref()).unwrap().as_ptr(), (X11_sys::XIMPreeditNothing | X11_sys::XIMStatusNothing) as i32, CStr::from_bytes_with_nul(X11_sys::XNClientWindow.as_ref()).unwrap().as_ptr(), window, CStr::from_bytes_with_nul(X11_sys::XNFocusWindow.as_ref()).unwrap().as_ptr(), window, ptr::null_mut() as *mut c_void);
            
            // Create a window
            (*self.xlib_app).window_map.insert(window, self);
            
            self.attributes = Some(attributes);
            self.visual_info = Some(visual_info);
            self.window = Some(window);
            self.xic = Some(xic);
            self.last_window_geom = self.get_window_geom();
            
            (*self.xlib_app).event_recur_block = false;
            let new_geom = self.get_window_geom();
            self.do_callback(&mut vec![
                Event::WindowGeomChange(WindowGeomChangeEvent {
                    window_id: self.window_id,
                    old_geom: new_geom.clone(),
                    new_geom: new_geom
                })
            ]);
            (*self.xlib_app).event_recur_block = true;
        }
    }
    
    pub fn hide_child_windows(&mut self) {
        unsafe {
            let display = (*self.xlib_app).display;
            for child in &mut self.child_windows {
                if child.visible {
                    X11_sys::XUnmapWindow(display, child.window);
                    child.visible = false
                }
            }
        }
    }
    
    pub fn alloc_child_window(&mut self, x: i32, y: i32, w: u32, h: u32) -> Option<c_ulong> {
        unsafe {
            let display = (*self.xlib_app).display;
            
            // ok lets find a childwindow that matches x/y/w/h and show it if need be
            for child in &mut self.child_windows {
                if child.x == x && child.y == y && child.w == w && child.h == h {
                    if!child.visible {
                        X11_sys::XMapWindow(display, child.window);
                        child.visible = true;
                    }
                    X11_sys::XRaiseWindow(display, child.window);
                    return Some(child.window);
                }
            }
            
            for child in &mut self.child_windows {
                if !child.visible {
                    child.x = x;
                    child.y = y;
                    child.w = w;
                    child.h = h;
                    X11_sys::XMoveResizeWindow(display, child.window, x, y, w, h);
                    X11_sys::XMapWindow(display, child.window);
                    X11_sys::XRaiseWindow(display, child.window);
                    child.visible = true;
                    return Some(child.window);
                }
            }
            
            let new_child = X11_sys::XCreateWindow(
                display,
                self.window.unwrap(),
                x,
                y,
                w,
                h,
                0,
                self.visual_info.unwrap().depth,
                X11_sys::InputOutput as u32,
                self.visual_info.unwrap().visual,
                (X11_sys::CWBorderPixel | X11_sys::CWColormap | X11_sys::CWEventMask | X11_sys::CWOverrideRedirect) as c_ulong,
                self.attributes.as_mut().unwrap(),
            );
            
            // Map the window to the screen
            //X11_sys::XMapWindow(display, window_dirty);
            (*self.xlib_app).window_map.insert(new_child, self);
            X11_sys::XMapWindow(display, new_child);
            X11_sys::XFlush(display);
            
            self.child_windows.push(XlibChildWindow {
                window: new_child,
                x: x,
                y: y,
                w: w,
                h: h,
                visible: true
            });
            
            return Some(new_child)
            
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
            for i in 0..self.child_windows.len() {
                (*self.xlib_app).window_map.insert(self.child_windows[i].window, self);
            }
        }
    }
    
    pub fn on_mouse_move(&self) {
    }
    
    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {
    }
    
    fn restore_or_maximize(&self, add_remove: c_long) {
        unsafe {
            let xlib_app = &(*self.xlib_app);
            let default_screen = X11_sys::XDefaultScreen(xlib_app.display);
            let root_window = X11_sys::XRootWindow(xlib_app.display, default_screen);
            let mut xclient = X11_sys::XClientMessageEvent {
                type_: X11_sys::ClientMessage as i32,
                serial: 0,
                send_event: 0,
                display: xlib_app.display,
                window: self.window.unwrap(),
                message_type: xlib_app.atom_net_wm_state,
                format: 32,
                data: {
                    let mut msg = mem::zeroed::<X11_sys::XClientMessageEvent__bindgen_ty_1>();
                    msg.l[0] = add_remove;
                    msg.l[1] = xlib_app.atom_new_wm_state_maximized_horz as c_long;
                    msg.l[2] = xlib_app.atom_new_wm_state_maximized_vert as c_long;
                    msg
                }
            };
            X11_sys::XSendEvent(xlib_app.display, root_window, 0, (X11_sys::SubstructureNotifyMask | X11_sys::SubstructureRedirectMask) as c_long, &mut xclient as *mut _ as *mut X11_sys::XEvent);
        }
    }
    
    pub fn restore(&self) {
        self.restore_or_maximize(_NET_WM_STATE_REMOVE);
    }
    
    pub fn maximize(&self) {
        self.restore_or_maximize(_NET_WM_STATE_ADD);
    }
    
    pub fn close_window(&mut self) {
        unsafe {
            let xlib_app = &(*self.xlib_app);
            X11_sys::XDestroyWindow(xlib_app.display, self.window.unwrap());
            self.window = None;
            // lets remove us from the mapping
            
        }
    }
    
    pub fn minimize(&self) {
        unsafe {
            let xlib_app = &(*self.xlib_app);
            let default_screen = X11_sys::XDefaultScreen(xlib_app.display);
            X11_sys::XIconifyWindow(xlib_app.display, self.window.unwrap(), default_screen);
            X11_sys::XFlush(xlib_app.display);
        }
    }
    
    pub fn set_topmost(&self, _topmost: bool) {
    }
    
    pub fn get_is_topmost(&self) -> bool {
        false
    }
    
    pub fn get_window_geom(&self) -> WindowGeom {
        WindowGeom {
            xr_is_presenting: false,
            xr_can_present: false,
            can_fullscreen: false,
            is_topmost: self.get_is_topmost(),
            is_fullscreen: self.get_is_maximized(),
            inner_size: self.get_inner_size(),
            outer_size: self.get_outer_size(),
            dpi_factor: self.get_dpi_factor(),
            position: self.get_position()
        }
    }
    
    pub fn get_is_maximized(&self) -> bool {
        let mut maximized = false;
        unsafe {
            let xlib_app = &(*self.xlib_app);
            let mut prop_type = mem::MaybeUninit::uninit();
            let mut format = mem::MaybeUninit::uninit();
            let mut n_item = mem::MaybeUninit::uninit();
            let mut bytes_after = mem::MaybeUninit::uninit();
            let mut properties = mem::MaybeUninit::uninit();
            let result = X11_sys::XGetWindowProperty(
                xlib_app.display,
                self.window.unwrap(),
                xlib_app.atom_net_wm_state,
                0,
                !0,
                0,
                X11_sys::AnyPropertyType as c_ulong,
                prop_type.as_mut_ptr(),
                format.as_mut_ptr(),
                n_item.as_mut_ptr(),
                bytes_after.as_mut_ptr(),
                properties.as_mut_ptr()
            );
            //let prop_type = prop_type.assume_init();
            //let format = format.assume_init();
            let n_item = n_item.assume_init();
            //let bytes_after = bytes_after.assume_init();
            let properties = properties.assume_init();
            if result == 0 && properties != ptr::null_mut() {
                let items = std::slice::from_raw_parts::<c_ulong>(properties as *mut _, n_item as usize);
                for item in items {
                    if *item == xlib_app.atom_new_wm_state_maximized_horz || *item == xlib_app.atom_new_wm_state_maximized_vert {
                        maximized = true;
                        break;
                    }
                }
                X11_sys::XFree(properties as *mut _);
            }
        }
        maximized
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
            let mut xwa = mem::MaybeUninit::uninit();
            let display = (*self.xlib_app).display;
            X11_sys::XGetWindowAttributes(display, self.window.unwrap(), xwa.as_mut_ptr());
            let xwa = xwa.assume_init();
            return Vec2 {x: xwa.x as f32, y: xwa.y as f32}
            /*
            let mut child = mem::uninitialized();
            let default_screen = X11_sys::XDefaultScreen(display);
            let root_window = X11_sys::XRootWindow(display, default_screen);
            let mut x:c_int = 0;
            let mut y:c_int = 0;
            X11_sys::XTranslateCoordinates(display, self.window.unwrap(), root_window, 0, 0, &mut x, &mut y, &mut child );
            */
        }
    }
    
    pub fn get_inner_size(&self) -> Vec2 {
        let dpi_factor = self.get_dpi_factor();
        unsafe {
            let mut xwa = mem::MaybeUninit::uninit();
            let display = (*self.xlib_app).display;
            X11_sys::XGetWindowAttributes(display, self.window.unwrap(), xwa.as_mut_ptr());
            let xwa = xwa.assume_init();
            return Vec2 {x: xwa.width as f32 / dpi_factor, y: xwa.height as f32 / dpi_factor}
        }
    }
    
    pub fn get_outer_size(&self) -> Vec2 {
        unsafe {
            let mut xwa = mem::MaybeUninit::uninit();
            let display = (*self.xlib_app).display;
            X11_sys::XGetWindowAttributes(display, self.window.unwrap(), xwa.as_mut_ptr());
            let xwa = xwa.assume_init();
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
            //return 2.0;
            let display = (*self.xlib_app).display;
            let resource_string = X11_sys::XResourceManagerString(display);
            if resource_string == std::ptr::null_mut() {
                return 1.0
            }
            let db = X11_sys::XrmGetStringDatabase(resource_string);
            let mut ty = mem::MaybeUninit::uninit();
            let mut value = mem::MaybeUninit::uninit();
            X11_sys::XrmGetResource(
                db,
                CString::new("Xft.dpi").unwrap().as_ptr(),
                CString::new("String").unwrap().as_ptr(),
                ty.as_mut_ptr(),
                value.as_mut_ptr()
            );
            //let ty = ty.assume_init();
            let value = value.assume_init();
            if value.addr == std::ptr::null_mut() {
                return 1.0; // TODO find some other way to figure it out
            }
            else {
                let dpi: f32 = CStr::from_ptr(value.addr).to_str().unwrap().parse().unwrap();
                return dpi / 96.0;
            }
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Event>) {
        unsafe {
            (*self.xlib_app).do_callback(events);
        }
    }
    
    pub fn send_change_event(&mut self) {
        
        let mut new_geom = self.get_window_geom();
        if new_geom.inner_size.x < self.last_window_geom.inner_size.x ||
        new_geom.inner_size.y < self.last_window_geom.inner_size.y {
            new_geom.is_fullscreen = false;
        }
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
            rect: Rect::default(),
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
            rect: Rect::default(),
            abs_start: Vec2::default(),
            rel_start: Vec2::default(),
            digit: digit,
            is_over: false,
            input_type: FingerInputType::Mouse,
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
                    rect: Rect::default(),
                    digit: digit,
                    abs_start: Vec2::default(),
                    rel_start: Vec2::default(),
                    is_over: false,
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
            rel: pos,
            any_down: false,
            rect: Rect::default(),
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

