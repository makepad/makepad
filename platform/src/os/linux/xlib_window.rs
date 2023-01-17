use {
    std::{
        mem,
        cell::Cell,
        rc::Rc,
        os::raw::{c_ulong, c_long, c_void},
        ptr,
        ffi::{CStr,CString}, 
    },
    crate::{
        area::Area,
        window::WindowId,
        makepad_math::{DVec2},
        event::*,
        cursor::MouseCursor,
        os::linux::xlib_event::*,
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
    //pub child_windows: Vec<XlibChildWindow>,
    
    pub last_nc_mode: Option<c_long>,
    pub window_id: WindowId,
    pub last_window_geom: WindowGeom,
    
    pub ime_spot: DVec2,
    pub current_cursor: MouseCursor,
    pub last_mouse_pos: DVec2,
}
/*
#[derive(Clone)]
pub struct XlibChildWindow {
    pub window: c_ulong,
    visible: bool,
    x: i32,
    y: i32,
    w: u32,
    h: u32
}*/

impl XlibWindow {
    
    pub fn new(window_id: WindowId) -> XlibWindow {
        
        XlibWindow {
            window: None,
            xic: None,
            attributes: None,
            visual_info: None,
            //child_windows: Vec::new(),
            window_id,
            last_window_geom: WindowGeom::default(),
            last_nc_mode: None,
            ime_spot: DVec2::default(),
            current_cursor: MouseCursor::Default,
            last_mouse_pos: DVec2::default(),
        }
    }
    
    pub fn init(&mut self, title: &str, size: DVec2, position: Option<DVec2>, visual_info: x11_sys::XVisualInfo, custom_window_chrome: bool) {
        unsafe {
            let display = get_xlib_app_global().display;
            
            // The default screen of the display
            let default_screen = x11_sys::XDefaultScreen(display);
            
            // The root window of the default screen
            let root_window = x11_sys::XRootWindow(display, default_screen);
            
            let mut attributes = mem::zeroed::<x11_sys::XSetWindowAttributes>();
            
            attributes.border_pixel = 0;
            //attributes.override_redirect = 1;
            attributes.colormap =
            x11_sys::XCreateColormap(display, root_window, visual_info.visual, x11_sys::AllocNone as i32);
            attributes.event_mask = (
                x11_sys::ExposureMask
                    | x11_sys::StructureNotifyMask
                    | x11_sys::ButtonMotionMask
                    | x11_sys::PointerMotionMask
                    | x11_sys::ButtonPressMask
                    | x11_sys::ButtonReleaseMask
                    | x11_sys::KeyPressMask
                    | x11_sys::KeyReleaseMask
                    | x11_sys::VisibilityChangeMask
                    | x11_sys::FocusChangeMask
                    | x11_sys::EnterWindowMask
                    | x11_sys::LeaveWindowMask
            ) as c_long;
            
            let dpi_factor = self.get_dpi_factor();
            // Create a window
            let window = x11_sys::XCreateWindow(
                display,
                root_window,
                if position.is_some() {position.unwrap().x}else {150.0} as i32,
                if position.is_some() {position.unwrap().y}else {60.0} as i32,
                (size.x * dpi_factor) as u32,
                (size.y * dpi_factor) as u32,
                0,
                visual_info.depth,
                x11_sys::InputOutput as u32,
                visual_info.visual,
                (x11_sys::CWBorderPixel | x11_sys::CWColormap | x11_sys::CWEventMask) as c_ulong, // | X11_sys::CWOverrideRedirect,
                &mut attributes,
            );
            
            // Tell the window manager that we want to be notified when the window is closed
            x11_sys::XSetWMProtocols(display, window, &mut get_xlib_app_global().atoms.wm_delete_window, 1);
            
            if custom_window_chrome {
                let hints = MwmHints {
                    flags: MWM_HINTS_DECORATIONS,
                    functions: 0,
                    decorations: 0,
                    input_mode: 0,
                    status: 0,
                };
                
                let atom_motif_wm_hints = get_xlib_app_global().atoms.motif_wm_hints;
                
                x11_sys::XChangeProperty(
                    display,
                    window,
                    atom_motif_wm_hints,
                    atom_motif_wm_hints,
                    32,
                    x11_sys::PropModeReplace as i32,
                    &hints as *const _ as *const u8,
                    5
                );
            }
            
            get_xlib_app_global().dnd.enable_for_window(window);
            
            // Map the window to the screen
            x11_sys::XMapWindow(display, window);
            x11_sys::XFlush(display);
            
            let title_bytes = format!("{}\0", title);
            x11_sys::XStoreName(display, window, title_bytes.as_bytes().as_ptr() as *const ::std::os::raw::c_char);
            
            let xic = x11_sys::XCreateIC(
                get_xlib_app_global().xim,
                x11_sys::XNInputStyle.as_ptr(),
                (x11_sys::XIMPreeditNothing | x11_sys::XIMStatusNothing) as i32,
                x11_sys::XNClientWindow.as_ptr(),
                window,
                x11_sys::XNFocusWindow.as_ptr(),
                window,
                ptr::null_mut() as *mut c_void
            );
            
            // Create a window
            get_xlib_app_global().window_map.insert(window, self);
            
            self.attributes = Some(attributes);
            self.visual_info = Some(visual_info);
            self.window = Some(window);
            self.xic = Some(xic);
            self.last_window_geom = self.get_window_geom();
            
            let new_geom = self.get_window_geom();
            self.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent {
                    window_id: self.window_id,
                    old_geom: new_geom.clone(),
                    new_geom: new_geom
                })
            );
        }
    }
    /*
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
    }*/
    /*
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
    */
    
    pub fn on_mouse_move(&self) {
    }
    
    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {
    }
    
    fn restore_or_maximize(&self, add_remove: c_long) {
        unsafe {
            let default_screen = x11_sys::XDefaultScreen(get_xlib_app_global().display);
            let root_window = x11_sys::XRootWindow(get_xlib_app_global().display, default_screen);
            let mut xclient = x11_sys::XClientMessageEvent {
                type_: x11_sys::ClientMessage as i32,
                serial: 0,
                send_event: 0,
                display: get_xlib_app_global().display,
                window: self.window.unwrap(),
                message_type: get_xlib_app_global().atoms.net_wm_state,
                format: 32,
                data: {
                    let mut msg = mem::zeroed::<x11_sys::XClientMessageEvent__bindgen_ty_1>();
                    msg.l[0] = add_remove;
                    msg.l[1] = get_xlib_app_global().atoms.new_wm_state_maximized_horz as c_long;
                    msg.l[2] = get_xlib_app_global().atoms.new_wm_state_maximized_vert as c_long;
                    msg
                }
            };
            x11_sys::XSendEvent(
                get_xlib_app_global().display,
                root_window,
                0,
                (x11_sys::SubstructureNotifyMask | x11_sys::SubstructureRedirectMask) as c_long,
                &mut xclient as *mut _ as *mut x11_sys::XEvent
            );
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
            x11_sys::XDestroyWindow(get_xlib_app_global().display, self.window.unwrap());
            self.window = None;
            // lets remove us from the mapping
            
        }
    }
    
    pub fn minimize(&self) {
        unsafe {
            let default_screen = x11_sys::XDefaultScreen(get_xlib_app_global().display);
            x11_sys::XIconifyWindow(get_xlib_app_global().display, self.window.unwrap(), default_screen);
            x11_sys::XFlush(get_xlib_app_global().display);
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
            let mut prop_type = mem::MaybeUninit::uninit();
            let mut format = mem::MaybeUninit::uninit();
            let mut n_item = mem::MaybeUninit::uninit();
            let mut bytes_after = mem::MaybeUninit::uninit();
            let mut properties = mem::MaybeUninit::uninit();
            let result = x11_sys::XGetWindowProperty(
                get_xlib_app_global().display,
                self.window.unwrap(),
                get_xlib_app_global().atoms.net_wm_state,
                0,
                !0,
                0,
                x11_sys::AnyPropertyType as c_ulong,
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
                    if *item == get_xlib_app_global().atoms.new_wm_state_maximized_horz
                        || *item == get_xlib_app_global().atoms.new_wm_state_maximized_vert {
                        maximized = true;
                        break;
                    }
                }
                x11_sys::XFree(properties as *mut _);
            }
        }
        maximized
    }
    
    pub fn set_ime_spot(&mut self, spot: DVec2) {
        self.ime_spot = spot;
    }
    
    pub fn get_position(&self) -> DVec2 {
        unsafe {
            let mut xwa = mem::MaybeUninit::uninit();
            let display = get_xlib_app_global().display;
            x11_sys::XGetWindowAttributes(
                display,
                self.window.unwrap(),
                xwa.as_mut_ptr()
            );
            let xwa = xwa.assume_init();
            return DVec2 {x: xwa.x as f64, y: xwa.y as f64}
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
    
    pub fn get_inner_size(&self) -> DVec2 {
        let dpi_factor = self.get_dpi_factor();
        unsafe {
            let mut xwa = mem::MaybeUninit::uninit();
            let display = get_xlib_app_global().display;
            x11_sys::XGetWindowAttributes(display, self.window.unwrap(), xwa.as_mut_ptr());
            let xwa = xwa.assume_init();
            return DVec2 {x: xwa.width as f64 / dpi_factor, y: xwa.height as f64 / dpi_factor}
        }
    }
    
    pub fn get_outer_size(&self) -> DVec2 {
        unsafe {
            let mut xwa = mem::MaybeUninit::uninit();
            let display = get_xlib_app_global().display;
            x11_sys::XGetWindowAttributes(display, self.window.unwrap(), xwa.as_mut_ptr());
            let xwa = xwa.assume_init();
            return DVec2 {x: xwa.width as f64, y: xwa.height as f64}
        }
    }
    
    pub fn set_position(&mut self, _pos: DVec2) {
    }
    
    pub fn set_outer_size(&self, _size: DVec2) {
    }
    
    pub fn set_inner_size(&self, _size: DVec2) {
    }
    
    pub fn get_dpi_factor(&self) -> f64 {
        unsafe {
            //return 2.0;
            let display = get_xlib_app_global().display;
            let resource_string = x11_sys::XResourceManagerString(display);
            if resource_string == std::ptr::null_mut() {
                return 1.0
            }
            let db = x11_sys::XrmGetStringDatabase(resource_string);
            let mut ty = mem::MaybeUninit::uninit();
            let mut value = mem::MaybeUninit::uninit();
            x11_sys::XrmGetResource(
                db,
                "Xft.dpi\0".as_ptr() as * const _,
                "String\0".as_ptr() as * const _,
                ty.as_mut_ptr(),
                value.as_mut_ptr()
            );
            //let ty = ty.assume_init();
            let value = value.assume_init();
            if value.addr == std::ptr::null_mut() {
                return 1.0; // TODO find some other way to figure it out
            }
            else {
                let dpi: f64 = CStr::from_ptr(value.addr).to_str().unwrap().parse().unwrap();
                return dpi / 96.0;
            }
        }
    }
    
    pub fn time_now(&self) -> f64 {
         get_xlib_app_global().time_now()
    }

    pub fn do_callback(&mut self, event: XlibEvent) {
        unsafe {
            get_xlib_app_global().do_callback(event);
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
        
        self.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent {
            window_id: self.window_id,
            old_geom: old_geom,
            new_geom: new_geom
        }));
        self.do_callback(XlibEvent::Paint);
    }
    
    pub fn send_focus_event(&mut self) {
        self.do_callback(XlibEvent::AppGotFocus);
    }
    
    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(XlibEvent::AppLostFocus);
    }
    
    pub fn send_mouse_down(&mut self, button: usize, modifiers: KeyModifiers) {
        self.do_callback(XlibEvent::MouseDown(MouseDownEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
    }
    
    pub fn send_mouse_up(&mut self, button: usize, modifiers: KeyModifiers) {
        self.do_callback(XlibEvent::MouseUp(MouseUpEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now()
        }));
    }
    
    pub fn send_mouse_move(&mut self, pos: DVec2, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        self.do_callback(XlibEvent::MouseMove(MouseMoveEvent {
            window_id: self.window_id,
            abs: pos,
            modifiers: modifiers,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
        
    }
    
    pub fn send_close_requested_event(&mut self) -> bool {
        let accept_close = Rc::new(Cell::new(true));
        self.do_callback(XlibEvent::WindowCloseRequested(WindowCloseRequestedEvent {
            window_id: self.window_id,
            accept_close: accept_close.clone()
        }));
        if !accept_close.get() {
            return false
        }
        true
    }
    
    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(XlibEvent::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last
        }))
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

pub const MWM_HINTS_FUNCTIONS: c_ulong = 1 << 0;
pub const MWM_HINTS_DECORATIONS: c_ulong = 1 << 1;

pub const MWM_FUNC_ALL: c_ulong = 1 << 0;
pub const MWM_FUNC_RESIZE: c_ulong = 1 << 1;
pub const MWM_FUNC_MOVE: c_ulong = 1 << 2;
pub const MWM_FUNC_MINIMIZE: c_ulong = 1 << 3;
pub const MWM_FUNC_MAXIMIZE: c_ulong = 1 << 4;
pub const MWM_FUNC_CLOSE: c_ulong = 1 << 5;
pub const _NET_WM_MOVERESIZE_SIZE_TOPLEFT: c_long = 0;
pub const _NET_WM_MOVERESIZE_SIZE_TOP: c_long = 1;
pub const _NET_WM_MOVERESIZE_SIZE_TOPRIGHT: c_long = 2;
pub const _NET_WM_MOVERESIZE_SIZE_RIGHT: c_long = 3;
pub const _NET_WM_MOVERESIZE_SIZE_BOTTOMRIGHT: c_long = 4;
pub const _NET_WM_MOVERESIZE_SIZE_BOTTOM: c_long = 5;
pub const _NET_WM_MOVERESIZE_SIZE_BOTTOMLEFT: c_long = 6;
pub const _NET_WM_MOVERESIZE_SIZE_LEFT: c_long = 7;
pub const _NET_WM_MOVERESIZE_MOVE: c_long = 8;/* movement only */
pub const _NET_WM_MOVERESIZE_SIZE_KEYBOARD: c_long = 9;/* size via keyboard */
pub const _NET_WM_MOVERESIZE_MOVE_KEYBOARD: c_long = 10;

pub const _NET_WM_STATE_REMOVE: c_long = 0;/* remove/unset property */
pub const _NET_WM_STATE_ADD: c_long = 1;/* add/set property */
pub const _NET_WM_STATE_TOGGLE: c_long = 2;/* toggle property  */

/* move via keyboard */

pub struct Dnd {
    pub atoms: DndAtoms,
    pub display: *mut x11_sys::Display,
    pub type_list: Option<Vec<x11_sys::Atom >>,
    pub selection: Option<CString>,
}

impl Dnd {
    pub unsafe fn new(display: *mut x11_sys::Display) -> Dnd {
        Dnd {
            atoms: DndAtoms::new(display),
            display,
            type_list: None,
            selection: None,
        }
    }
    
    /// Enables drag-and-drop for the given window.
    pub unsafe fn enable_for_window(&mut self, window: x11_sys::Window) {
        // To enable drag-and-drop for a window, we need to set the XDndAware property of the window
        // to the version of XDnd we support.
        
        // I took this value from the Winit source code. Apparently, this is the latest version, and
        // hasn't changed since 2002.
        let version = 5 as c_ulong;
        
        x11_sys::XChangeProperty(
            self.display,
            window,
            self.atoms.aware,
            4, // XA_ATOM
            32,
            x11_sys::PropModeReplace as std::os::raw::c_int,
            &version as *const c_ulong as *const std::os::raw::c_uchar,
            1
        );
    }
    
    /// Handles a XDndEnter event.
    pub unsafe fn handle_enter_event(&mut self, event: &x11_sys::XClientMessageEvent) {
        // The XDndEnter event is sent by the source window when a drag begins. That is, the mouse
        // enters the client rectangle of the target window. The target window is supposed to
        // respond to this by requesting the list of types supported by the source.
        
        let source_window = event.data.l[0] as x11_sys::Window;
        let has_more_types = event.data.l[1] & (1 << 0) != 0;
        
        // If the has_more_types flags is set, we have to obtain the list of supported types from
        // the XDndTypeList property. Otherwise, we can obtain the list of supported types from the
        // event itself.
        self.type_list = Some(if has_more_types {
            self.get_type_list_property(source_window)
        } else {
            event.data.l[2..4]
                .iter()
                .map( | &l | l as x11_sys::Atom)
                .filter( | &atom | atom != x11_sys::None as x11_sys::Atom)
                .collect()
        });
    }
    
    /// Handles a XDndDrop event.
    pub unsafe fn handle_drop_event(&mut self, event: &x11_sys::XClientMessageEvent) {
        // The XDndLeave event is sent by the source window when a drag is confirmed. That is, the
        // mouse button is released while the mouse is inside the client rectangle of the target
        // window. The target window is supposed to respond to this by requesting that the selection
        // representing the thing being dragged is converted to the appropriate data type (in our
        // case, a URI list). The source window, in turn, is supposed to respond this by sending a
        // selection event containing the data to the source window.
        
        let target_window = event.window as x11_sys::Window;
        self.convert_selection(target_window);
        self.type_list = None;
    }
    
    /// Handles a XDndLeave event.
    pub unsafe fn handle_leave_event(&mut self, _event: &x11_sys::XClientMessageEvent) {
        // The XDndLeave event is sent by the source window when a drag is canceled. That is, the
        // mouse leaves the client rectangle of the target window. The target window is supposed to
        // repsond this this by pretending the drag never happened.
        
        self.type_list = None;
    }
    
    /// Handles a XDndPosition event.
    pub unsafe fn handle_position_event(&mut self, event: &x11_sys::XClientMessageEvent) {
        // The XDndPosition event is sent by the source window after the XDndEnter event, every time
        // the mouse is moved. The target window is supposed to respond to this by sending a status
        // event to the source window notifying whether it can accept the drag at this position.
        
        let target_window = event.window as x11_sys::Window;
        let source_window = event.data.l[0] as x11_sys::Window;
        
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
    pub unsafe fn handle_selection_event(&mut self, _event: &x11_sys::XSelectionEvent) {
        // The XSelectionEvent is sent by the source window in response to a request by the source
        // window to convert the selection representing the thing being dragged to the appropriate
        // data type. This request is always sent in response to a XDndDrop event, so this event
        // should only be received after a drop operation has completed.
        
        //let source_window = event.requestor;
        //let selection = CString::new(self.get_selection_property(source_window)).unwrap();
        
        // TODO: Actually use the selection
    }
    
    /// Gets the XDndSelection property from the source window.
    pub unsafe fn get_selection_property(&mut self, source_window: x11_sys::Window) -> Vec< std::os::raw::c_uchar> {
        let mut selection = Vec::new();
        let mut offset = 0;
        let length = 1024;
        let mut actual_type = 0;
        let mut actual_format = 0;
        let mut nitems = 0;
        let mut bytes_after = 0;
        let mut prop = ptr::null_mut();
        loop {
            x11_sys::XGetWindowProperty(
                self.display,
                source_window,
                self.atoms.selection,
                offset,
                length,
                x11_sys::False as std::os::raw::c_int,
                self.atoms.uri_list,
                &mut actual_type,
                &mut actual_format,
                &mut nitems,
                &mut bytes_after,
                &mut prop,
            );
            selection.extend_from_slice(std::slice::from_raw_parts(prop as *mut  std::os::raw::c_uchar, nitems as usize));
            x11_sys::XFree(prop as *mut c_void);
            if bytes_after == 0 {
                break;
            }
            offset += length;
        };
        selection
    }
    
    /// Gets the XDndTypeList property from the source window.
    pub unsafe fn get_type_list_property(&mut self, source_window: x11_sys::Window) -> Vec<x11_sys::Atom> {
        let mut type_list = Vec::new();
        let mut offset = 0;
        let length = 1024;
        let mut actual_type = 0;
        let mut actual_format = 0;
        let mut nitems = 0;
        let mut bytes_after = 0;
        let mut prop = ptr::null_mut();
        loop {
            x11_sys::XGetWindowProperty(
                self.display,
                source_window,
                self.atoms.type_list,
                offset,
                length,
                x11_sys::False as  std::os::raw::c_int,
                4, // XA_ATOM,
                &mut actual_type,
                &mut actual_format,
                &mut nitems,
                &mut bytes_after,
                &mut prop,
            );
            type_list.extend_from_slice(std::slice::from_raw_parts(prop as *mut x11_sys::Atom, nitems as usize));
            x11_sys::XFree(prop as *mut c_void);
            if bytes_after == 0 {
                break;
            }
            offset += length;
        };
        type_list
    }
    
    /// Sends a XDndStatus event to the target window.
    pub unsafe fn send_status_event(&mut self, source_window: x11_sys::Window, target_window: x11_sys::Window, accepted: bool) {
        x11_sys::XSendEvent(
            self.display,
            source_window,
            x11_sys::False as  std::os::raw::c_int,
            x11_sys::NoEventMask as  std::os::raw::c_long,
            &mut x11_sys::XClientMessageEvent {
                type_: x11_sys::ClientMessage as  std::os::raw::c_int,
                serial: 0,
                send_event: 0,
                display: self.display,
                window: source_window,
                message_type: self.atoms.status,
                format: 32,
                data: {
                    let mut data = mem::zeroed::<x11_sys::XClientMessageEvent__bindgen_ty_1>();
                    data.l[0] = target_window as c_long;
                    data.l[1] = if accepted {1 << 0} else {0};
                    data.l[2] = 0;
                    data.l[3] = 0;
                    data.l[4] = if accepted {self.atoms.action_private} else {self.atoms.none} as c_long;
                    data
                }
            } as *mut x11_sys::XClientMessageEvent as *mut x11_sys::XEvent
        );
        x11_sys::XFlush(self.display);
    }
    
    // Requests that the selection representing the thing being dragged is converted to the
    // appropriate data type (in our case, a URI list).
    pub unsafe fn convert_selection(&self, target_window: x11_sys::Window) {
        x11_sys::XConvertSelection(
            self.display,
            self.atoms.selection,
            self.atoms.uri_list,
            self.atoms.selection,
            target_window,
            x11_sys::CurrentTime as x11_sys::Time,
        );
    }
}

struct DndAtoms {
    pub action_private: x11_sys::Atom,
    pub aware: x11_sys::Atom,
    pub drop: x11_sys::Atom,
    pub enter: x11_sys::Atom,
    pub leave: x11_sys::Atom,
    pub none: x11_sys::Atom,
    pub position: x11_sys::Atom,
    pub selection: x11_sys::Atom,
    pub status: x11_sys::Atom,
    pub type_list: x11_sys::Atom,
    pub uri_list: x11_sys::Atom,
}

impl DndAtoms {
    pub unsafe fn new(display: *mut x11_sys::Display) -> DndAtoms {
        DndAtoms {
            action_private: x11_sys::XInternAtom(display, "XdndActionPrivate\0".as_ptr() as *const _, 0),
            aware: x11_sys::XInternAtom(display, "XdndAware\0".as_ptr() as *const _, 0),
            drop: x11_sys::XInternAtom(display, "XdndDrop\0".as_ptr() as *const _, 0),
            enter: x11_sys::XInternAtom(display, "XdndEnter\0".as_ptr() as *const _, 0),
            leave: x11_sys::XInternAtom(display, "XdndLeave\0".as_ptr() as *const _, 0),
            none: x11_sys::XInternAtom(display, "None\0".as_ptr() as *const _, 0),
            position: x11_sys::XInternAtom(display, "XdndPosition\0".as_ptr() as *const _, 0),
            selection: x11_sys::XInternAtom(display, "XdndSelection\0".as_ptr() as *const _, 0),
            status: x11_sys::XInternAtom(display, "XdndStatus\0".as_ptr() as *const _, 0),
            type_list: x11_sys::XInternAtom(display, "XdndTypeList\0".as_ptr() as *const _, 0),
            uri_list: x11_sys::XInternAtom(display, "text/uri-list\0".as_ptr() as *const _, 0),
        }
    }
}
