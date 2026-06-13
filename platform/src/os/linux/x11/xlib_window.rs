use {
    self::super::{x11_sys, xlib_app::*, xlib_event::XlibEvent},
    crate::{area::Area, cursor::MouseCursor, event::*, makepad_math::{Rect, Vec2d}, window::WindowId},
    std::{
        cell::Cell,
        ffi::{CStr, CString, OsStr},
        mem,
        os::raw::{c_char, c_int, c_long, c_uint, c_ulong, c_void},
        ptr,
        rc::Rc,
    },
};

#[derive(Clone)]
pub struct XlibWindow {
    pub window: Option<c_ulong>,
    pub xic: Option<XimInputContext>,
    pub attributes: Option<x11_sys::XSetWindowAttributes>,
    pub visual_info: Option<x11_sys::XVisualInfo>,
    //pub child_windows: Vec<XlibChildWindow>,
    pub last_nc_mode: Option<c_long>,
    pub window_id: WindowId,
    pub last_window_geom: WindowGeom,

    // Caret/composition line and current-line area in window-relative native
    // points; fed to the IM as spot plus clipping area.
    pub ime_rect: Rect,
    pub ime_area_rect: Rect,
    ime_popup_last_adjust_time: f64,
    ime_popup_last_miss_log_time: f64,
    pub current_cursor: MouseCursor,
    pub last_mouse_pos: Vec2d,
    // When ime_active is false, XSetICFocus is not used so the IME candidate window does not show.
    pub ime_active: bool,
    pub is_popup: bool,
    pub popup_parent: Option<WindowId>,
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

const X11_IS_VIEWABLE: c_int = 2;
const X11_IME_POPUP_ADJUST_INTERVAL: f64 = 1.0 / 30.0;
const X11_IME_POPUP_SCAN_MAX_DEPTH: usize = 5;
const X11_IME_POPUP_SCAN_MAX_WINDOWS: usize = 1500;

struct ImePopupCandidate {
    window: x11_sys::Window,
    class_name: String,
    class_match: bool,
    override_redirect: bool,
    ancestor_override_redirect: bool,
    depth: usize,
    x: c_int,
    y: c_int,
    parent_root_x: c_int,
    parent_root_y: c_int,
    width: c_int,
    height: c_int,
    score: f64,
}

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
            ime_rect: Rect::default(),
            ime_area_rect: Rect::default(),
            ime_popup_last_adjust_time: 0.0,
            ime_popup_last_miss_log_time: 0.0,
            current_cursor: MouseCursor::Default,
            last_mouse_pos: Vec2d::default(),
            ime_active: false,
            is_popup: false,
            popup_parent: None,
        }
    }

    pub fn init(
        &mut self,
        title: &str,
        size: Vec2d,
        position: Option<Vec2d>,
        is_fullscreen: bool,
        visual_info: x11_sys::XVisualInfo,
        custom_window_chrome: bool,
    ) {
        self.is_popup = false;
        self.popup_parent = None;
        unsafe {
            let display = get_xlib_app_global().display;

            // The default screen of the display
            let default_screen = x11_sys::XDefaultScreen(display);

            // The root window of the default screen
            let root_window = x11_sys::XRootWindow(display, default_screen);

            let mut attributes = mem::zeroed::<x11_sys::XSetWindowAttributes>();

            attributes.border_pixel = 0;
            //attributes.override_redirect = 1;
            attributes.colormap = x11_sys::XCreateColormap(
                display,
                root_window,
                visual_info.visual,
                x11_sys::AllocNone as i32,
            );
            attributes.event_mask = (x11_sys::ExposureMask
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
                | x11_sys::LeaveWindowMask) as c_long;

            let dpi_factor = self.get_dpi_factor();
            // Create a window
            let window = x11_sys::XCreateWindow(
                display,
                root_window,
                if position.is_some() {
                    position.unwrap().x
                } else {
                    150.0
                } as i32,
                if position.is_some() {
                    position.unwrap().y
                } else {
                    60.0
                } as i32,
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
            x11_sys::XSetWMProtocols(
                display,
                window,
                &mut get_xlib_app_global().atoms.wm_delete_window,
                1,
            );

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
                    5,
                );
            }

            get_xlib_app_global().dnd.enable_for_window(window);

            // The title should be set prior to mapping the window.
            let title_bytes = format!("{}\0", title);
            let title_ptr = title_bytes.as_ptr() as *mut c_char;

            // Set USPosition so the WM (e.g. GNOME/Mutter) honors our requested position.
            // Without this hint many WMs ignore the position from XCreateWindow and apply
            // their own smart-placement algorithm instead.
            let mut size_hints = mem::zeroed::<x11_sys::XSizeHints>();
            if let Some(pos) = position {
                size_hints.flags = x11_sys::USPosition | x11_sys::PPosition;
                size_hints.x = pos.x as c_int;
                size_hints.y = pos.y as c_int;
            }

            x11_sys::Xutf8SetWMProperties(
                display,
                window,
                title_ptr,
                title_ptr,
                ptr::null_mut(),
                0,
                &mut size_hints,
                ptr::null_mut(),
                ptr::null_mut(),
            );

            // Set the WM_CLASS before mapping the window.
            // Based on <https://www.x.org/releases/X11R7.5/doc/man/man3/XSetWMProperties.3.html>
            {
                // Use the binary name by default (the first arg).
                let class = std::env::args_os()
                    .next()
                    .as_ref()
                    .and_then(|arg0| std::path::Path::new(arg0).file_name())
                    .and_then(OsStr::to_str)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| String::from("Makepad"));
                let instance = std::env::var("RESOURCE_NAME")
                    .ok()
                    .unwrap_or_else(|| class.clone());
                let wm_class = format!("{instance}\0{class}\0");

                x11_sys::XChangeProperty(
                    display,
                    window,
                    get_xlib_app_global().atoms.wm_class,
                    get_xlib_app_global().atoms.string,
                    // the wm_class is passed in as a string of bytes
                    (core::mem::size_of::<u8>() * 8) as i32,
                    x11_sys::PropModeReplace as i32,
                    wm_class.as_ptr(),
                    wm_class.len() as i32,
                );
            }

            // Set window icon via _NET_WM_ICON
            Self::set_x11_icon(display, window);

            // Reinforce the requested position before mapping. Calling XMoveWindow before
            // XMapWindow ensures the coordinates are root-relative (the window's parent is
            // still the root window at this point). After XMapWindow the WM reparents the
            // window into a decoration frame, and any subsequent XMoveWindow would be
            // interpreted relative to that frame rather than the root, which can cause the
            // client area to end up at an unexpected position inside the WM frame.
            if let Some(pos) = position {
                x11_sys::XMoveWindow(display, window, pos.x as i32, pos.y as i32);
            }

            // Map the window to the screen
            x11_sys::XMapWindow(display, window);
            x11_sys::XFlush(display);

            let xic = create_xim_input_context(get_xlib_app_global().xim, window);

            // Create a window
            get_xlib_app_global().window_map.insert(window, self);

            self.attributes = Some(attributes);
            self.visual_info = Some(visual_info);
            self.window = Some(window);
            self.xic = xic;
            self.last_window_geom = self.get_window_geom();

            let new_geom = self.get_window_geom();
            self.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent {
                window_id: self.window_id,
                old_geom: new_geom.clone(),
                new_geom: new_geom,
            }));
            if is_fullscreen {
                self.maximize();
            }
        }
    }

    pub fn init_popup(
        &mut self,
        parent_window_id: WindowId,
        size: Vec2d,
        position: Vec2d,
        visual_info: x11_sys::XVisualInfo,
    ) {
        self.is_popup = true;
        self.popup_parent = Some(parent_window_id);
        unsafe {
            let display = get_xlib_app_global().display;
            let default_screen = x11_sys::XDefaultScreen(display);
            let root_window = x11_sys::XRootWindow(display, default_screen);

            let mut attributes = mem::zeroed::<x11_sys::XSetWindowAttributes>();
            attributes.border_pixel = 0;
            attributes.override_redirect = 1;
            attributes.colormap = x11_sys::XCreateColormap(
                display,
                root_window,
                visual_info.visual,
                x11_sys::AllocNone as i32,
            );
            attributes.event_mask = (x11_sys::ExposureMask
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
                | x11_sys::LeaveWindowMask) as c_long;

            let dpi_factor = self.get_dpi_factor();
            let window = x11_sys::XCreateWindow(
                display,
                root_window,
                position.x as i32,
                position.y as i32,
                (size.x * dpi_factor) as u32,
                (size.y * dpi_factor) as u32,
                0,
                visual_info.depth,
                x11_sys::InputOutput as u32,
                visual_info.visual,
                (x11_sys::CWBorderPixel
                    | x11_sys::CWColormap
                    | x11_sys::CWEventMask
                    | x11_sys::CWOverrideRedirect) as c_ulong,
                &mut attributes,
            );

            x11_sys::XChangeProperty(
                display,
                window,
                get_xlib_app_global().atoms.net_wm_window_type,
                get_xlib_app_global().atoms.atom,
                32,
                x11_sys::PropModeReplace as i32,
                &get_xlib_app_global().atoms.net_wm_window_type_popup_menu as *const _ as *const u8,
                1,
            );

            x11_sys::XMapRaised(display, window);
            x11_sys::XFlush(display);

            let xic = create_xim_input_context(get_xlib_app_global().xim, window);

            get_xlib_app_global().window_map.insert(window, self);

            self.attributes = Some(attributes);
            self.visual_info = Some(visual_info);
            self.window = Some(window);
            self.xic = xic;
            self.last_window_geom = self.get_window_geom();

            let new_geom = self.get_window_geom();
            self.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent {
                window_id: self.window_id,
                old_geom: new_geom.clone(),
                new_geom,
            }));
        }
    }

    /// Set `_NET_WM_ICON` from the default Makepad icon (RGBA8 → ARGB u32 array).
    unsafe fn set_x11_icon(display: *mut x11_sys::Display, window: c_ulong) {
        let icon = crate::app_icon::window_icon();
        let buf = match icon.buffers.first() {
            Some(b) => b,
            None => return,
        };
        // _NET_WM_ICON format: [width, height, pixel_data...] as u32 ARGB
        let pixel_count = (buf.width * buf.height) as usize;
        let mut data: Vec<c_ulong> = Vec::with_capacity(2 + pixel_count);
        data.push(buf.width as c_ulong);
        data.push(buf.height as c_ulong);
        for chunk in buf.data.chunks_exact(4) {
            let r = chunk[0] as c_ulong;
            let g = chunk[1] as c_ulong;
            let b = chunk[2] as c_ulong;
            let a = chunk[3] as c_ulong;
            data.push((a << 24) | (r << 16) | (g << 8) | b);
        }
        x11_sys::XChangeProperty(
            display,
            window,
            get_xlib_app_global().atoms.net_wm_icon,
            get_xlib_app_global().atoms.cardinal,
            32,
            x11_sys::PropModeReplace as i32,
            data.as_ptr() as *const u8,
            data.len() as i32,
        );
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
                },
            };
            x11_sys::XSendEvent(
                get_xlib_app_global().display,
                root_window,
                0,
                (x11_sys::SubstructureNotifyMask | x11_sys::SubstructureRedirectMask) as c_long,
                &mut xclient as *mut _ as *mut x11_sys::XEvent,
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
        if let Some(window) = self.window.take() {
            unsafe {
                let xlib_app = get_xlib_app_global();
                if xlib_app.active_popup == Some(window) {
                    xlib_app.release_popup_grab(window);
                }
                xlib_app.window_map.remove(&window);
                x11_sys::XDestroyWindow(xlib_app.display, window);
            }
        }
    }

    pub fn minimize(&self) {
        unsafe {
            let default_screen = x11_sys::XDefaultScreen(get_xlib_app_global().display);
            x11_sys::XIconifyWindow(
                get_xlib_app_global().display,
                self.window.unwrap(),
                default_screen,
            );
            x11_sys::XFlush(get_xlib_app_global().display);
        }
    }

    pub fn set_topmost(&self, _topmost: bool) {}

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
            position: self.get_position(),
            ..Default::default()
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
                properties.as_mut_ptr(),
            );
            //let prop_type = prop_type.assume_init();
            //let format = format.assume_init();
            let n_item = n_item.assume_init();
            //let bytes_after = bytes_after.assume_init();
            let properties = properties.assume_init();
            if result == 0 && properties != ptr::null_mut() {
                let items =
                    std::slice::from_raw_parts::<c_ulong>(properties as *mut _, n_item as usize);
                for item in items {
                    if *item == get_xlib_app_global().atoms.new_wm_state_maximized_horz
                        || *item == get_xlib_app_global().atoms.new_wm_state_maximized_vert
                    {
                        maximized = true;
                        break;
                    }
                }
                x11_sys::XFree(properties as *mut _);
            }
        }
        maximized
    }

    unsafe fn create_position_xic_with_spot(
        &self,
        preferred_status_style: c_ulong,
        spot_px: x11_sys::XPoint,
        area_px: x11_sys::XRectangle,
    ) -> Option<XimInputContext> {
        let window = self.window?;
        let xim = get_xlib_app_global().xim;
        if let Some(context) = create_xim_position_input_context_with_spot(
            xim,
            window,
            preferred_status_style,
            spot_px,
            area_px,
        ) {
            return Some(context);
        }
        for status_style in xim_status_candidates() {
            if status_style == preferred_status_style {
                continue;
            }
            if let Some(context) = create_xim_position_input_context_with_spot(
                xim,
                window,
                status_style,
                spot_px,
                area_px,
            ) {
                return Some(context);
            }
        }
        None
    }

    unsafe fn window_wm_class(
        display: *mut x11_sys::Display,
        window: x11_sys::Window,
        wm_class_atom: x11_sys::Atom,
    ) -> Option<String> {
        let mut actual_type = 0;
        let mut actual_format = 0;
        let mut nitems = 0;
        let mut bytes_after = 0;
        let mut prop = ptr::null_mut();
        let result = x11_sys::XGetWindowProperty(
            display,
            window,
            wm_class_atom,
            0,
            1024,
            x11_sys::False as c_int,
            x11_sys::AnyPropertyType as c_ulong,
            &mut actual_type,
            &mut actual_format,
            &mut nitems,
            &mut bytes_after,
            &mut prop,
        );
        if result != 0 || prop.is_null() || actual_format != 8 || nitems == 0 {
            if !prop.is_null() {
                x11_sys::XFree(prop as *mut c_void);
            }
            return None;
        }
        let bytes = std::slice::from_raw_parts(prop as *const u8, nitems as usize);
        let class_name = String::from_utf8_lossy(bytes)
            .replace('\0', " ")
            .to_lowercase();
        x11_sys::XFree(prop as *mut c_void);
        Some(class_name)
    }

    unsafe fn window_or_child_wm_class(
        display: *mut x11_sys::Display,
        window: x11_sys::Window,
        wm_class_atom: x11_sys::Atom,
        depth: usize,
    ) -> Option<String> {
        if let Some(class_name) = Self::window_wm_class(display, window, wm_class_atom) {
            return Some(class_name);
        }
        if depth == 0 {
            return None;
        }

        let mut root_return = 0;
        let mut parent_return = 0;
        let mut children: *mut x11_sys::Window = ptr::null_mut();
        let mut child_count: c_uint = 0;
        if x11_sys::XQueryTree(
            display,
            window,
            &mut root_return,
            &mut parent_return,
            &mut children,
            &mut child_count,
        ) == 0
        {
            return None;
        }

        let mut class_name = None;
        if !children.is_null() {
            for child_window in
                std::slice::from_raw_parts(children, child_count as usize).iter().copied()
            {
                if let Some(child_class) =
                    Self::window_or_child_wm_class(display, child_window, wm_class_atom, depth - 1)
                {
                    class_name = Some(child_class);
                    break;
                }
            }
            x11_sys::XFree(children as *mut c_void);
        }
        class_name
    }

    unsafe fn ime_popup_candidate(
        &self,
        display: *mut x11_sys::Display,
        root_window: x11_sys::Window,
        window: x11_sys::Window,
        wm_class_atom: x11_sys::Atom,
        line_rect_root_px: Rect,
        ancestor_override_redirect: bool,
        depth: usize,
    ) -> Option<ImePopupCandidate> {
        if Some(window) == self.window {
            return None;
        }
        let mut xwa = mem::MaybeUninit::uninit();
        if x11_sys::XGetWindowAttributes(display, window, xwa.as_mut_ptr()) == 0 {
            return None;
        }
        let xwa = xwa.assume_init();
        if xwa.map_state != X11_IS_VIEWABLE || xwa.width <= 0 || xwa.height <= 0 {
            return None;
        }
        let class_name = Self::window_or_child_wm_class(display, window, wm_class_atom, 2)
            .unwrap_or_else(|| "<none>".to_string());
        let class_match = class_name.contains("ibus") || class_name.contains("fcitx");
        let override_redirect = xwa.override_redirect != 0;
        let fallback_allowed = override_redirect || ancestor_override_redirect;
        if !class_match && !fallback_allowed {
            return None;
        }

        let mut root_x = 0;
        let mut root_y = 0;
        let mut child = 0;
        if x11_sys::XTranslateCoordinates(
            display,
            window,
            root_window,
            0,
            0,
            &mut root_x,
            &mut root_y,
            &mut child,
        ) == 0
        {
            return None;
        }

        let mut parent_root_x = 0;
        let mut parent_root_y = 0;
        let mut root_return = 0;
        let mut parent_window = 0;
        let mut children: *mut x11_sys::Window = ptr::null_mut();
        let mut child_count: c_uint = 0;
        if x11_sys::XQueryTree(
            display,
            window,
            &mut root_return,
            &mut parent_window,
            &mut children,
            &mut child_count,
        ) != 0
        {
            if !children.is_null() {
                x11_sys::XFree(children as *mut c_void);
            }
            if parent_window != 0 && parent_window != root_window {
                let mut ignored_child = 0;
                let _ = x11_sys::XTranslateCoordinates(
                    display,
                    parent_window,
                    root_window,
                    0,
                    0,
                    &mut parent_root_x,
                    &mut parent_root_y,
                    &mut ignored_child,
                );
            }
        }

        // Keep this scoped to plausible candidate popups near the focused line;
        // ibus/fcitx may also expose panels or tray windows with the same class.
        let width = xwa.width;
        let height = xwa.height;
        if width < 20 || height < 12 || width > 1400 || height > 700 {
            return None;
        }
        if !class_match && (width > 1200 || height > 320) {
            return None;
        }
        let line_left = line_rect_root_px.pos.x;
        let line_top = line_rect_root_px.pos.y;
        let line_right = line_rect_root_px.pos.x + line_rect_root_px.size.x;
        let line_bottom = line_rect_root_px.pos.y + line_rect_root_px.size.y;
        let popup_left = root_x as f64;
        let popup_top = root_y as f64;
        let popup_right = popup_left + width as f64;
        let popup_bottom = popup_top + height as f64;
        let vertical_gap = if popup_bottom < line_top {
            line_top - popup_bottom
        } else if popup_top > line_bottom {
            popup_top - line_bottom
        } else {
            0.0
        };
        let horizontal_gap = if popup_right < line_left {
            line_left - popup_right
        } else if popup_left > line_right {
            popup_left - line_right
        } else {
            0.0
        };
        if !class_match && (vertical_gap > 220.0 || horizontal_gap > 160.0) {
            return None;
        }
        if vertical_gap > 500.0 || horizontal_gap > 500.0 {
            return None;
        }
        Some(ImePopupCandidate {
            window,
            class_name,
            class_match,
            override_redirect,
            ancestor_override_redirect,
            depth,
            x: root_x,
            y: root_y,
            parent_root_x,
            parent_root_y,
            width,
            height,
            score: vertical_gap + horizontal_gap * 0.5,
        })
    }

    unsafe fn ime_debug_window_summary(
        display: *mut x11_sys::Display,
        root_window: x11_sys::Window,
        window: x11_sys::Window,
        wm_class_atom: x11_sys::Atom,
        line_rect_root_px: Rect,
        depth: usize,
        ancestor_override_redirect: bool,
    ) -> Option<(f64, String)> {
        let mut xwa = mem::MaybeUninit::uninit();
        if x11_sys::XGetWindowAttributes(display, window, xwa.as_mut_ptr()) == 0 {
            return None;
        }
        let xwa = xwa.assume_init();
        if xwa.map_state != X11_IS_VIEWABLE || xwa.width <= 0 || xwa.height <= 0 {
            return None;
        }

        let mut root_x = 0;
        let mut root_y = 0;
        let mut child = 0;
        if x11_sys::XTranslateCoordinates(
            display,
            window,
            root_window,
            0,
            0,
            &mut root_x,
            &mut root_y,
            &mut child,
        ) == 0
        {
            return None;
        }

        let line_left = line_rect_root_px.pos.x;
        let line_top = line_rect_root_px.pos.y;
        let line_right = line_rect_root_px.pos.x + line_rect_root_px.size.x;
        let line_bottom = line_rect_root_px.pos.y + line_rect_root_px.size.y;
        let popup_left = root_x as f64;
        let popup_top = root_y as f64;
        let popup_right = popup_left + xwa.width as f64;
        let popup_bottom = popup_top + xwa.height as f64;
        let vertical_gap = if popup_bottom < line_top {
            line_top - popup_bottom
        } else if popup_top > line_bottom {
            popup_top - line_bottom
        } else {
            0.0
        };
        let horizontal_gap = if popup_right < line_left {
            line_left - popup_right
        } else if popup_left > line_right {
            popup_left - line_right
        } else {
            0.0
        };
        if vertical_gap > 900.0 || horizontal_gap > 900.0 {
            return None;
        }

        let class_name = Self::window_or_child_wm_class(display, window, wm_class_atom, 2)
            .unwrap_or_else(|| "<none>".to_string());
        let score =
            vertical_gap + horizontal_gap * 0.5 + (xwa.width * xwa.height) as f64 / 1_000_000.0;
        Some((
            score,
            format!(
                "win={} depth={} class={:?} override={} ancestor_override={} pos=({}, {}) size=({}, {}) gap=({}, {})",
                window,
                depth,
                class_name,
                xwa.override_redirect != 0,
                ancestor_override_redirect,
                root_x,
                root_y,
                xwa.width,
                xwa.height,
                horizontal_gap,
                vertical_gap
            ),
        ))
    }

    unsafe fn scan_ime_popup_windows(
        &self,
        display: *mut x11_sys::Display,
        root_window: x11_sys::Window,
        window: x11_sys::Window,
        wm_class_atom: x11_sys::Atom,
        line_rect_root_px: Rect,
        depth: usize,
        ancestor_override_redirect: bool,
        want_miss_log: bool,
        visited_count: &mut usize,
        nearby_windows: &mut Vec<(f64, String)>,
        best: &mut Option<ImePopupCandidate>,
    ) {
        if *visited_count >= X11_IME_POPUP_SCAN_MAX_WINDOWS {
            return;
        }
        *visited_count += 1;

        let mut xwa = mem::MaybeUninit::uninit();
        if x11_sys::XGetWindowAttributes(display, window, xwa.as_mut_ptr()) == 0 {
            return;
        }
        let xwa = xwa.assume_init();
        let next_ancestor_override = ancestor_override_redirect || xwa.override_redirect != 0;

        if want_miss_log {
            if let Some(summary) = Self::ime_debug_window_summary(
                display,
                root_window,
                window,
                wm_class_atom,
                line_rect_root_px,
                depth,
                ancestor_override_redirect,
            ) {
                nearby_windows.push(summary);
            }
        }

        if let Some(candidate) = self.ime_popup_candidate(
            display,
            root_window,
            window,
            wm_class_atom,
            line_rect_root_px,
            ancestor_override_redirect,
            depth,
        ) {
            if best
                .as_ref()
                .map(|best| candidate.score < best.score)
                .unwrap_or(true)
            {
                *best = Some(candidate);
            }
        }

        if depth >= X11_IME_POPUP_SCAN_MAX_DEPTH {
            return;
        }

        let mut root_return = 0;
        let mut parent_return = 0;
        let mut children: *mut x11_sys::Window = ptr::null_mut();
        let mut child_count: c_uint = 0;
        if x11_sys::XQueryTree(
            display,
            window,
            &mut root_return,
            &mut parent_return,
            &mut children,
            &mut child_count,
        ) == 0
        {
            return;
        }

        if !children.is_null() {
            for child_window in
                std::slice::from_raw_parts(children, child_count as usize).iter().copied()
            {
                self.scan_ime_popup_windows(
                    display,
                    root_window,
                    child_window,
                    wm_class_atom,
                    line_rect_root_px,
                    depth + 1,
                    next_ancestor_override,
                    want_miss_log,
                    visited_count,
                    nearby_windows,
                    best,
                );
                if *visited_count >= X11_IME_POPUP_SCAN_MAX_WINDOWS {
                    break;
                }
            }
            x11_sys::XFree(children as *mut c_void);
        }
    }

    pub fn adjust_ime_candidate_popup(&mut self) {
        if !self.ime_active || self.ime_rect.size.y <= 0.0 {
            return;
        }
        let now = self.time_now();
        if now - self.ime_popup_last_adjust_time < X11_IME_POPUP_ADJUST_INTERVAL {
            return;
        }
        self.ime_popup_last_adjust_time = now;

        unsafe {
            let Some(window) = self.window else {
                return;
            };
            let display = get_xlib_app_global().display;
            let default_screen = x11_sys::XDefaultScreen(display);
            let root_window = x11_sys::XRootWindow(display, default_screen);
            let mut root_x = 0;
            let mut root_y = 0;
            let mut child = 0;
            if x11_sys::XTranslateCoordinates(
                display,
                window,
                root_window,
                0,
                0,
                &mut root_x,
                &mut root_y,
                &mut child,
            ) == 0
            {
                return;
            }
            let dpi_factor = self.get_dpi_factor();
            let line_rect_root_px = Rect {
                pos: crate::makepad_math::dvec2(
                    root_x as f64 + self.ime_rect.pos.x * dpi_factor,
                    root_y as f64 + self.ime_rect.pos.y * dpi_factor,
                ),
                size: self.ime_rect.size * dpi_factor,
            };

            let mut root_return = 0;
            let mut parent_return = 0;
            let mut children: *mut x11_sys::Window = ptr::null_mut();
            let mut child_count: c_uint = 0;
            if x11_sys::XQueryTree(
                display,
                root_window,
                &mut root_return,
                &mut parent_return,
                &mut children,
                &mut child_count,
            ) == 0
            {
                return;
            }

            let want_miss_log =
                x11_ime_debug_enabled() && now - self.ime_popup_last_miss_log_time >= 1.0;
            let mut nearby_windows = Vec::new();
            let mut best: Option<ImePopupCandidate> = None;
            let mut visited_count = 0;
            if !children.is_null() {
                for child_window in
                    std::slice::from_raw_parts(children, child_count as usize).iter().copied()
                {
                    self.scan_ime_popup_windows(
                        display,
                        root_window,
                        child_window,
                        get_xlib_app_global().atoms.wm_class,
                        line_rect_root_px,
                        0,
                        false,
                        want_miss_log,
                        &mut visited_count,
                        &mut nearby_windows,
                        &mut best,
                    );
                    if visited_count >= X11_IME_POPUP_SCAN_MAX_WINDOWS {
                        break;
                    }
                }
                x11_sys::XFree(children as *mut c_void);
            }

            let Some(candidate) = best else {
                if want_miss_log {
                    self.ime_popup_last_miss_log_time = now;
                    nearby_windows
                        .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                    let nearby = nearby_windows
                        .iter()
                        .take(8)
                        .map(|(_, summary)| summary.as_str())
                        .collect::<Vec<_>>()
                        .join(" | ");
                    crate::log!(
                        "X11 IME: no ibus/fcitx candidate popup found root_children={} visited={} line_root=({}, {}, {}, {}) nearby=[{}]",
                        child_count,
                        visited_count,
                        line_rect_root_px.pos.x,
                        line_rect_root_px.pos.y,
                        line_rect_root_px.size.x,
                        line_rect_root_px.size.y,
                        nearby
                    );
                }
                return;
            };
            let mut root_attrs = mem::MaybeUninit::uninit();
            if x11_sys::XGetWindowAttributes(display, root_window, root_attrs.as_mut_ptr()) == 0 {
                return;
            }
            let root_attrs = root_attrs.assume_init();
            let line_top = line_rect_root_px.pos.y;
            let line_bottom = line_rect_root_px.pos.y + line_rect_root_px.size.y;
            let line_center = (line_top + line_bottom) * 0.5;
            let popup_center = candidate.y as f64 + candidate.height as f64 * 0.5;
            let gap = (line_rect_root_px.size.y * 0.8).max(14.0).min(32.0);
            let place_above = popup_center < line_center;
            let desired_y = if place_above {
                line_top - gap - candidate.height as f64
            } else {
                line_bottom + gap
            }
            .round()
            .max(0.0)
            .min((root_attrs.height - candidate.height).max(0) as f64)
                as c_int;

            if (desired_y - candidate.y).abs() <= 1 {
                return;
            }
            x11_sys::XMoveWindow(
                display,
                candidate.window,
                candidate.x - candidate.parent_root_x,
                desired_y - candidate.parent_root_y,
            );
            x11_sys::XFlush(display);
            if x11_ime_debug_enabled() {
                crate::log!(
                    "X11 IME: adjusted candidate popup window={} depth={} class={:?} class_match={} override_redirect={} ancestor_override={} side={} from_root=({}, {}) to_root=({}, {}) parent_root=({}, {}) size=({}, {}) line_root=({}, {}, {}, {}) gap={}",
                    candidate.window,
                    candidate.depth,
                    candidate.class_name,
                    candidate.class_match,
                    candidate.override_redirect,
                    candidate.ancestor_override_redirect,
                    if place_above { "above" } else { "below" },
                    candidate.x,
                    candidate.y,
                    candidate.x,
                    desired_y,
                    candidate.parent_root_x,
                    candidate.parent_root_y,
                    candidate.width,
                    candidate.height,
                    line_rect_root_px.pos.x,
                    line_rect_root_px.pos.y,
                    line_rect_root_px.size.x,
                    line_rect_root_px.size.y,
                    gap
                );
            }
        }
    }

    pub fn set_ime_rect(&mut self, rect: Rect, area_rect: Rect) {
        if self.ime_rect == rect && self.ime_area_rect == area_rect {
            return;
        }
        self.ime_rect = rect;
        self.ime_area_rect = area_rect;
        let Some(mut xim_context) = self.xic else {
            if x11_ime_debug_enabled() {
                crate::log!(
                    "X11 IME: set rect skipped; no XIC rect=({}, {}, {}, {}) line_area=({}, {}, {}, {})",
                    rect.pos.x,
                    rect.pos.y,
                    rect.size.x,
                    rect.size.y,
                    area_rect.pos.x,
                    area_rect.pos.y,
                    area_rect.size.x,
                    area_rect.size.y
                );
            }
            return;
        };
        let dpi_factor = self.get_dpi_factor();
        // XIM defines XNSpotLocation.y as the current text line baseline. We do
        // not have the real font baseline here, so approximate it from the line
        // rect and give the IM the padded current-line bounds as XNArea. ibus'
        // XIM bridge currently forwards only XNSpotLocation to its candidate UI,
        // so this area is only useful for XIM implementations that honor it.
        let line_height_px = rect.size.y * dpi_factor;
        let line_top_px = rect.pos.y * dpi_factor;
        let baseline_px = line_top_px + line_height_px * 0.85;
        let line_area = if area_rect.size.x > 0.0 && area_rect.size.y > 0.0 {
            area_rect
        } else {
            rect
        };
        let (padding_x_px, padding_y_px) = if line_height_px > 0.0 {
            (
                (line_height_px * 0.25).max(3.0),
                (line_height_px * 1.25).max(20.0),
            )
        } else {
            (0.0, 0.0)
        };
        let area_line_left_px = line_area.pos.x * dpi_factor;
        let area_line_top_px = line_area.pos.y * dpi_factor;
        let area_line_right_px = (line_area.pos.x + line_area.size.x) * dpi_factor;
        let area_line_bottom_px = (line_area.pos.y + line_area.size.y) * dpi_factor;
        let area_left_px = (area_line_left_px - padding_x_px).max(0.0);
        let area_top_px = (area_line_top_px - padding_y_px).max(0.0);
        let area_right_px = area_line_right_px + padding_x_px;
        let area_bottom_px = area_line_bottom_px + padding_y_px;
        let spot_px = x11_sys::XPoint {
            x: (rect.pos.x * dpi_factor) as i16,
            y: baseline_px as i16,
        };
        let area_px = x11_sys::XRectangle {
            x: area_left_px as i16,
            y: area_top_px as i16,
            width: (area_right_px - area_left_px).max(1.0) as u16,
            height: (area_bottom_px - area_top_px).max(1.0) as u16,
        };
        unsafe {
            let mut xic = xim_context.xic;
            if xim_context.preedit_style == XimPreeditStyle::Position
                && !xim_context.spot_initialized_at_creation
            {
                if let Some(new_context) = self.create_position_xic_with_spot(
                    xim_context.status_style(),
                    spot_px,
                    area_px,
                ) {
                    if x11_ime_debug_enabled() {
                        crate::log!(
                            "X11 IME: recreated position XIC with initial spot=({}, {}) area=({}, {}, {}, {})",
                            spot_px.x,
                            spot_px.y,
                            area_px.x,
                            area_px.y,
                            area_px.width,
                            area_px.height
                        );
                    }
                    x11_sys::XDestroyIC(xic);
                    self.xic = Some(new_context);
                    xim_context = new_context;
                    xic = new_context.xic;
                    if self.ime_active {
                        x11_sys::XSetICFocus(xic);
                    }
                } else if x11_ime_debug_enabled() {
                    crate::log!(
                        "X11 IME: failed to recreate position XIC with initial spot=({}, {})",
                        spot_px.x,
                        spot_px.y
                    );
                }
            }

            let preedit_attr = x11_sys::XVaCreateNestedList(
                0,
                x11_sys::XNSpotLocation.as_ptr(),
                &spot_px,
                x11_sys::XNArea.as_ptr(),
                &area_px,
                ptr::null_mut::<c_void>(),
            );
            if preedit_attr.is_null() {
                if x11_ime_debug_enabled() {
                    crate::log!(
                        "X11 IME: set rect aborted; XVaCreateNestedList failed spot=({}, {}) area=({}, {}, {}, {})",
                        spot_px.x,
                        spot_px.y,
                        area_px.x,
                        area_px.y,
                        area_px.width,
                        area_px.height
                    );
                }
                return;
            }

            if x11_ime_debug_enabled() {
                crate::log!(
                    "X11 IME: setting rect window={:?} style={} input_style=0x{:x} spot=({}, {}) area=({}, {}, {}, {})",
                    self.window,
                    xim_preedit_style_name(xim_context.preedit_style),
                    xim_context.input_style,
                    spot_px.x,
                    spot_px.y,
                    area_px.x,
                    area_px.y,
                    area_px.width,
                    area_px.height
                );
            }
            let mut failed_attr = x11_sys::XSetICValues(
                xic,
                x11_sys::XNPreeditAttributes.as_ptr(),
                preedit_attr,
                ptr::null_mut::<c_void>(),
            );
            let mut fallback_note = "";
            if !failed_attr.is_null() && xim_context.preedit_style != XimPreeditStyle::Position {
                if let Some(new_context) = self.create_position_xic_with_spot(
                    xim_context.status_style(),
                    spot_px,
                    area_px,
                ) {
                    if x11_ime_debug_enabled() {
                        crate::log!(
                            "X11 IME: callback rect update failed_attr={}; switching to position XIC input_style=0x{:x}",
                            x11_ime_failed_attr_name(failed_attr),
                            new_context.input_style
                        );
                    }
                    x11_sys::XDestroyIC(xic);
                    self.xic = Some(new_context);
                    xim_context = new_context;
                    xic = new_context.xic;
                    if self.ime_active {
                        x11_sys::XSetICFocus(xic);
                    }
                    failed_attr = x11_sys::XSetICValues(
                        xic,
                        x11_sys::XNPreeditAttributes.as_ptr(),
                        preedit_attr,
                        ptr::null_mut::<c_void>(),
                    );
                    fallback_note = " after callback-to-position retry";
                } else if x11_ime_debug_enabled() {
                    crate::log!(
                        "X11 IME: callback rect update failed_attr={}; position XIC fallback creation failed",
                        x11_ime_failed_attr_name(failed_attr)
                    );
                }
            }
            if x11_ime_debug_enabled() {
                crate::log!(
                    "X11 IME: set rect returned window={:?} style={} input_style=0x{:x} dpi={} rect=({}, {}, {}, {}) line_area=({}, {}, {}, {}) spot=({}, {}) area=({}, {}, {}, {}) padding=({}, {}) baseline_y={} failed_attr={}{}",
                    self.window,
                    xim_preedit_style_name(xim_context.preedit_style),
                    xim_context.input_style,
                    dpi_factor,
                    rect.pos.x,
                    rect.pos.y,
                    rect.size.x,
                    rect.size.y,
                    area_rect.pos.x,
                    area_rect.pos.y,
                    area_rect.size.x,
                    area_rect.size.y,
                    spot_px.x,
                    spot_px.y,
                    area_px.x,
                    area_px.y,
                    area_px.width,
                    area_px.height,
                    padding_x_px,
                    padding_y_px,
                    baseline_px,
                    x11_ime_failed_attr_name(failed_attr),
                    fallback_note
                );
            }
            x11_sys::XFree(preedit_attr);
        }
    }

    pub fn set_ime_active(&mut self, active: bool) {
        if self.ime_active == active {
            return;
        }
        self.ime_active = active;
        if let Some(xim_context) = self.xic {
            if self.ime_active {
                unsafe { x11_sys::XSetICFocus(xim_context.xic) };
                if self.ime_rect != Rect::default() {
                    let ime_rect = self.ime_rect;
                    let ime_area_rect = self.ime_area_rect;
                    self.ime_rect = Rect::default();
                    self.ime_area_rect = Rect::default();
                    self.set_ime_rect(ime_rect, ime_area_rect);
                }
            } else {
                unsafe { x11_sys::XUnsetICFocus(xim_context.xic) };
            }
        }
    }

    pub fn get_position(&self) -> Vec2d {
        unsafe {
            let display = get_xlib_app_global().display;
            let default_screen = x11_sys::XDefaultScreen(display);
            let root_window = x11_sys::XRootWindow(display, default_screen);
            let mut x: c_int = 0;
            let mut y: c_int = 0;
            let mut child = mem::MaybeUninit::uninit();
            // XGetWindowAttributes returns position relative to the parent window,
            // which after WM reparenting is the decoration frame (not the root).
            // XTranslateCoordinates gives the correct root-relative (screen) position.
            x11_sys::XTranslateCoordinates(
                display,
                self.window.unwrap(),
                root_window,
                0,
                0,
                &mut x,
                &mut y,
                child.as_mut_ptr(),
            );
            Vec2d {
                x: x as f64,
                y: y as f64,
            }
        }
    }

    pub fn get_inner_size(&self) -> Vec2d {
        let dpi_factor = self.get_dpi_factor();
        unsafe {
            let mut xwa = mem::MaybeUninit::uninit();
            let display = get_xlib_app_global().display;
            x11_sys::XGetWindowAttributes(display, self.window.unwrap(), xwa.as_mut_ptr());
            let xwa = xwa.assume_init();
            return Vec2d {
                x: xwa.width as f64 / dpi_factor,
                y: xwa.height as f64 / dpi_factor,
            };
        }
    }

    pub fn get_outer_size(&self) -> Vec2d {
        unsafe {
            let mut xwa = mem::MaybeUninit::uninit();
            let display = get_xlib_app_global().display;
            x11_sys::XGetWindowAttributes(display, self.window.unwrap(), xwa.as_mut_ptr());
            let xwa = xwa.assume_init();
            return Vec2d {
                x: xwa.width as f64,
                y: xwa.height as f64,
            };
        }
    }

    pub fn set_position(&mut self, pos: Vec2d) {
        unsafe {
            let display = get_xlib_app_global().display;
            let dpi_factor = self.get_dpi_factor();
            x11_sys::XMoveWindow(
                display,
                self.window.unwrap(),
                (pos.x * dpi_factor) as i32,
                (pos.y * dpi_factor) as i32,
            );
            x11_sys::XFlush(display);
            self.last_window_geom.position = pos;
        }
    }

    pub fn set_outer_size(&self, _size: Vec2d) {}

    pub fn set_inner_size(&self, _size: Vec2d) {}

    pub fn get_dpi_factor(&self) -> f64 {
        unsafe {
            //return 2.0;
            let display = get_xlib_app_global().display;
            let resource_string = x11_sys::XResourceManagerString(display);
            if resource_string == std::ptr::null_mut() {
                return 1.0;
            }
            let db = x11_sys::XrmGetStringDatabase(resource_string);
            let mut ty = mem::MaybeUninit::uninit();
            let mut value = mem::MaybeUninit::uninit();
            x11_sys::XrmGetResource(
                db,
                "Xft.dpi\0".as_ptr() as *const _,
                "String\0".as_ptr() as *const _,
                ty.as_mut_ptr(),
                value.as_mut_ptr(),
            );
            //let ty = ty.assume_init();
            let value = value.assume_init();
            if value.addr == std::ptr::null_mut() {
                return 1.0; // TODO find some other way to figure it out
            } else {
                let dpi: f64 = CStr::from_ptr(value.addr)
                    .to_str()
                    .unwrap()
                    .parse()
                    .unwrap();
                return dpi / 96.0;
            }
        }
    }

    pub fn time_now(&self) -> f64 {
        get_xlib_app_global().time_now()
    }

    pub fn do_callback(&mut self, event: XlibEvent) {
        get_xlib_app_global().do_callback(event);
    }

    pub fn send_change_event(&mut self) {
        let mut new_geom = self.get_window_geom();
        if new_geom.inner_size.x < self.last_window_geom.inner_size.x
            || new_geom.inner_size.y < self.last_window_geom.inner_size.y
        {
            new_geom.is_fullscreen = false;
        }
        let old_geom = self.last_window_geom.clone();
        self.last_window_geom = new_geom.clone();

        self.do_callback(XlibEvent::WindowGeomChange(WindowGeomChangeEvent {
            window_id: self.window_id,
            old_geom: old_geom,
            new_geom: new_geom,
        }));
        self.do_callback(XlibEvent::Paint);
    }

    pub fn send_focus_event(&mut self) {
        self.do_callback(XlibEvent::WindowGotFocus(self.window_id));
    }

    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(XlibEvent::WindowLostFocus(self.window_id));
    }

    pub fn contains_root_pos(&self, root_x: i32, root_y: i32) -> bool {
        unsafe {
            let mut xwa = mem::MaybeUninit::uninit();
            x11_sys::XGetWindowAttributes(
                get_xlib_app_global().display,
                self.window.unwrap(),
                xwa.as_mut_ptr(),
            );
            let xwa = xwa.assume_init();
            root_x >= xwa.x
                && root_y >= xwa.y
                && root_x <= xwa.x + xwa.width
                && root_y <= xwa.y + xwa.height
        }
    }

    pub fn send_mouse_down(&mut self, button: MouseButton, modifiers: KeyModifiers) {
        self.do_callback(XlibEvent::MouseDown(MouseDownEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
    }

    pub fn send_mouse_up(&mut self, button: MouseButton, modifiers: KeyModifiers) {
        self.do_callback(XlibEvent::MouseUp(MouseUpEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now(),
        }));
    }

    pub fn send_mouse_move(&mut self, pos: Vec2d, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        self.do_callback(XlibEvent::MouseMove(MouseMoveEvent {
            window_id: self.window_id,
            abs: pos,
            modifiers,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
    }

    pub fn send_close_requested_event(&mut self) -> bool {
        let accept_close = Rc::new(Cell::new(true));
        self.do_callback(XlibEvent::WindowCloseRequested(WindowCloseRequestedEvent {
            window_id: self.window_id,
            accept_close: accept_close.clone(),
        }));
        if !accept_close.get() {
            return false;
        }
        true
    }

    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(XlibEvent::TextInput(TextInputEvent {
            input,
            was_paste: false,
            replace_last,
            ..Default::default()
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
pub const _NET_WM_MOVERESIZE_MOVE: c_long = 8; /* movement only */
pub const _NET_WM_MOVERESIZE_SIZE_KEYBOARD: c_long = 9; /* size via keyboard */
pub const _NET_WM_MOVERESIZE_MOVE_KEYBOARD: c_long = 10;

pub const _NET_WM_STATE_REMOVE: c_long = 0; /* remove/unset property */
pub const _NET_WM_STATE_ADD: c_long = 1; /* add/set property */
pub const _NET_WM_STATE_TOGGLE: c_long = 2; /* toggle property  */

/* move via keyboard */

pub struct Dnd {
    pub atoms: DndAtoms,
    pub display: *mut x11_sys::Display,
    pub type_list: Option<Vec<x11_sys::Atom>>,
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
            1,
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
                .map(|&l| l as x11_sys::Atom)
                .filter(|&atom| atom != x11_sys::None as x11_sys::Atom)
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
        let accepted = self
            .type_list
            .as_ref()
            .map_or(false, |type_list| type_list.contains(&self.atoms.uri_list));

        // Notify the source window whether we can accept the drag at this position.
        self.send_status_event(source_window, target_window, accepted);

        // If this is the first time we've accepted the drag, request that the drag-and-drop
        // selection be converted to a URI list. The target window is supposed to respond to this by
        // sending a XSelectionEvent containing the URI list.

        // Since this is an asynchronous operation, its possible for another XDndPosition event to
        // come in before the response to the first conversion request has been received. In this
        // case, a second conversion request will be sent, the response to which will be ignored.
        if accepted && self.selection.is_none() {}
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
    pub unsafe fn get_selection_property(
        &mut self,
        source_window: x11_sys::Window,
    ) -> Vec<std::os::raw::c_uchar> {
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
            selection.extend_from_slice(std::slice::from_raw_parts(
                prop as *mut std::os::raw::c_uchar,
                nitems as usize,
            ));
            x11_sys::XFree(prop as *mut c_void);
            if bytes_after == 0 {
                break;
            }
            offset += length;
        }
        selection
    }

    /// Gets the XDndTypeList property from the source window.
    pub unsafe fn get_type_list_property(
        &mut self,
        source_window: x11_sys::Window,
    ) -> Vec<x11_sys::Atom> {
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
                x11_sys::False as std::os::raw::c_int,
                4, // XA_ATOM,
                &mut actual_type,
                &mut actual_format,
                &mut nitems,
                &mut bytes_after,
                &mut prop,
            );
            type_list.extend_from_slice(std::slice::from_raw_parts(
                prop as *mut x11_sys::Atom,
                nitems as usize,
            ));
            x11_sys::XFree(prop as *mut c_void);
            if bytes_after == 0 {
                break;
            }
            offset += length;
        }
        type_list
    }

    /// Sends a XDndStatus event to the target window.
    pub unsafe fn send_status_event(
        &mut self,
        source_window: x11_sys::Window,
        target_window: x11_sys::Window,
        accepted: bool,
    ) {
        x11_sys::XSendEvent(
            self.display,
            source_window,
            x11_sys::False as std::os::raw::c_int,
            x11_sys::NoEventMask as std::os::raw::c_long,
            &mut x11_sys::XClientMessageEvent {
                type_: x11_sys::ClientMessage as std::os::raw::c_int,
                serial: 0,
                send_event: 0,
                display: self.display,
                window: source_window,
                message_type: self.atoms.status,
                format: 32,
                data: {
                    let mut data = mem::zeroed::<x11_sys::XClientMessageEvent__bindgen_ty_1>();
                    data.l[0] = target_window as c_long;
                    data.l[1] = if accepted { 1 << 0 } else { 0 };
                    data.l[2] = 0;
                    data.l[3] = 0;
                    data.l[4] = if accepted {
                        self.atoms.action_private
                    } else {
                        self.atoms.none
                    } as c_long;
                    data
                },
            } as *mut x11_sys::XClientMessageEvent as *mut x11_sys::XEvent,
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

pub struct DndAtoms {
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
            action_private: x11_sys::XInternAtom(
                display,
                "XdndActionPrivate\0".as_ptr() as *const _,
                0,
            ),
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
