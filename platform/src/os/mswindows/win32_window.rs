use {
    std::{
        cell::{RefCell,Cell},
        rc::Rc,
        ptr,
        ffi::OsStr,
        os::windows::ffi::OsStrExt,
        mem,
        collections::{HashSet}
    },
    crate::{
        event::*,
        area::Area,
        os::mswindows::win32_sys,
        os::mswindows::win32_sys::{
            HWND,
            WPARAM,
            LPARAM,
            LRESULT,
            RECT,
            GlobalLock,
            GlobalAlloc,
            GlobalSize,
            GlobalUnlock,
            OpenClipboard,
            EmptyClipboard,
            GetClipboardData,
            SetClipboardData,
            CloseClipboard,
            CreateWindowExW,
            SetWindowLongPtrW,
            GetWindowLongPtrW,
            GetWindowLongW,
            DefWindowProcW,
            ShowWindow,
            PostMessageW,
            GetWindowRect,
            DestroyWindow,
            SetWindowPos,
            GetWindowPlacement,
            WINDOWPLACEMENT,
            GetClientRect,
            MoveWindow,
            MARGINS,
            ReleaseCapture,
            SetCapture,
            TrackMouseEvent,
            GetKeyState, 
            TRACKMOUSEEVENT,
            DwmExtendFrameIntoClientArea,
            GetModuleHandleW
        },
        os::mswindows::win32_event::*,
        os::mswindows::win32_app::Win32App,
        os::mswindows::win32_app::get_win32_app_global,
        window::{WindowId},
        cx::*,
        cursor::MouseCursor,
    },
};

#[derive(Clone)]
pub struct Win32Window {
    pub window_id: WindowId,
    pub last_window_geom: WindowGeom,
    
    pub mouse_buttons_down: usize,
    pub last_key_mod: KeyModifiers,
    pub ime_spot: DVec2,
    pub current_cursor: MouseCursor,
    pub last_mouse_pos: DVec2,
    pub ignore_wmsize: usize,
    pub hwnd: Option<HWND>,
    pub track_mouse_event: bool
}

impl Win32Window {
    
    pub fn new(win32_app: &mut Win32App, window_id: usize) -> Win32Window {
        Win32Window {
            window_id: window_id,
            win32_app: win32_app,
            last_window_geom: WindowGeom::default(),
            time_start: win32_app.time_start,
            last_key_mod: KeyModifiers::default(),
            ime_spot: Vec2::default(),
            current_cursor: MouseCursor::Default,
            last_mouse_pos: Vec2::default(),
            ignore_wmsize: 0,
            hwnd: None,
            track_mouse_event: false
        }
    }
    
    pub fn init(&mut self, title: &str, size: Vec2, position: Option<Vec2>) {
        
        let style = win32_sys::WS_SIZEBOX
            | win32_sys::WS_MAXIMIZEBOX
            | win32_sys::WS_MINIMIZEBOX
            | win32_sys::WS_POPUP
            | win32_sys::WS_CLIPSIBLINGS
            | win32_sys::WS_CLIPCHILDREN
            | win32_sys::WS_SYSMENU;
        
        let style_ex = win32_sys::WS_EX_WINDOWEDGE
            | win32_sys::WS_EX_APPWINDOW
            | win32_sys::WS_EX_ACCEPTFILES;
        
        unsafe {
            
            let (x, y) = if let Some(position) = position {
                (position.x as i32, position.y as i32)
            }
            else {
                (win32_sys::CW_USEDEFAULT, win32_sys::CW_USEDEFAULT)
            };
            
            let hwnd = CreateWindowExW(
                style_ex,
                get_win32_app_global().class_name_wstr.as_ptr(),
                title,
                style,
                x,
                y,
                win32_sys::CW_USEDEFAULT,
                win32_sys::CW_USEDEFAULT,
                ptr::null_mut(),
                ptr::null_mut(),
                GetModuleHandleW(ptr::null()),
                ptr::null_mut(),
            );
            
            self.hwnd = Some(hwnd);
            
            
            SetWindowLongPtrW(hwnd, win32_sys::GWLP_USERDATA, self as *const _ as isize);
            
            self.set_outer_size(size);
            
            ShowWindow(hwnd, win32_sys::SW_SHOW);
            
            get_win32_app_global().dpi_functions.enable_non_client_dpi_scaling(self.hwnd.unwrap());
            
            if let Ok(mut sigs) = get_win32_app_global().race_signals.lock() {
                get_win32_app_global().all_windows.push(hwnd);
                for sig in sigs.iter() {
                    PostMessageW(hwnd, win32_sys::WM_USER, sig.0, sig.1 as isize);
                }
                sigs.clear();
            }
            
        }
    }
    
    pub unsafe extern "system" fn window_class_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,) -> LRESULT {
        
        let user_data = GetWindowLongPtrW(hwnd, win32_sys::GWLP_USERDATA);
        if user_data == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        
        let window = &mut (*(user_data as *mut Win32Window));
        match msg {
            win32_sys::WM_ACTIVATE => {
                if wparam & 0xffff == win32_sys::WA_ACTIVE as usize {
                    window.do_callback(&mut vec![Win32Event::AppFocus]);
                }
                else {
                    window.do_callback(&mut vec![Win32Event::AppFocusLost]);
                }
            },
            win32_sys::WM_NCCALCSIZE => {
                // check if we are maximised
                if window.get_is_maximized() {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                if wparam == 1 {
                    let margins = MARGINS {
                        cxLeftWidth: 0,
                        cxRightWidth: 0,
                        cyTopHeight: 0,
                        cyBottomHeight: 1
                    };
                    DwmExtendFrameIntoClientArea(hwnd, &margins);
                    return 0
                }
            },
            win32_sys::WM_NCHITTEST => {
                let ycoord = (lparam >> 16) as u16 as i16 as i32;
                let xcoord = (lparam & 0xffff) as u16 as i16 as i32;
                let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
                const EDGE: i32 = 8;
                GetWindowRect(hwnd, &mut rect);
                if xcoord < rect.left + EDGE {
                    (*window.win32_app).current_cursor = MouseCursor::Hidden;
                    if ycoord < rect.top + EDGE {
                        window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NwseResize)]);
                        return win32_sys::HTTOPLEFT;
                    }
                    if ycoord > rect.bottom - EDGE {
                        window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NeswResize)]);
                        return win32_sys::HTBOTTOMLEFT;
                    }
                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::EwResize)]);
                    return win32_sys::HTLEFT;
                }
                if xcoord > rect.right - EDGE {
                    (*window.win32_app).current_cursor = MouseCursor::Hidden;
                    if ycoord < rect.top + EDGE {
                        window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NeswResize)]);
                        return win32_sys::HTTOPRIGHT;
                    }
                    if ycoord > rect.bottom - EDGE {
                        window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NwseResize)]);
                        return win32_sys::HTBOTTOMRIGHT;
                    }
                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::EwResize)]);
                    return win32_sys::HTRIGHT;
                }
                if ycoord < rect.top + EDGE {
                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NsResize)]);
                    return win32_sys::HTTOP;
                }
                if ycoord > rect.bottom - EDGE {
                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NsResize)]);
                    return win32_sys::HTBOTTOM;
                }
                let mut events = vec![
                    Event::WindowDragQuery(WindowDragQueryEvent {
                        window_id: window.window_id,
                        abs: window.get_mouse_pos_from_lparam(lparam),
                        response: WindowDragQueryResponse::NoAnswer
                    })
                ];
                window.do_callback(&mut events);
                match &events[0] {
                    Event::WindowDragQuery(wd) => match &wd.response {
                        WindowDragQueryResponse::Client => {
                            return win32_sys::HTCLIENT
                        }
                        WindowDragQueryResponse::Caption => {
                            window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::Default)]);
                            return win32_sys::HTCAPTION
                        },
                        WindowDragQueryResponse::SysMenu => {
                            window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::Default)]);
                            return win32_sys::HTSYSMENU
                        }
                        _ => ()
                    },
                    _ => ()
                }
                if ycoord < rect.top + 50 && xcoord < rect.left + 50 {
                    return win32_sys::HTSYSMENU;
                }
                if ycoord < rect.top + 50 && xcoord < rect.right - 300 {
                    return win32_sys::HTCAPTION;
                }
                return win32_sys::HTCLIENT;
            },
            win32_sys::WM_ERASEBKGND => {
                return 1
            },
            win32_sys::WM_MOUSEMOVE => {
                if !window.track_mouse_event {
                    window.track_mouse_event = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: win32_sys::TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0
                    };
                    TrackMouseEvent(&mut tme);
                }
                window.send_mouse_move(
                    window.get_mouse_pos_from_lparam(lparam),
                    Self::get_key_modifiers()
                )
            },
            win32_sys::WM_MOUSELEAVE => {
                window.track_mouse_event = false;
                window.send_mouse_move(
                    window.last_mouse_pos,
                    Self::get_key_modifiers()
                );
                get_win32_app_global().current_cursor = MouseCursor::Hidden;
            },
            win32_sys::WM_MOUSEWHEEL => {
                let delta = (wparam >> 16) as u16 as i16 as f32;
                window.send_scroll(Vec2 {x: 0.0, y: -delta}, Self::get_key_modifiers(), true);
            },
            win32_sys::WM_LBUTTONDOWN => window.send_mouse_down(0, Self::get_key_modifiers()),
            win32_sys::WM_LBUTTONUP => window.send_mouse_up(0, Self::get_key_modifiers()),
            win32_sys::WM_RBUTTONDOWN => window.send_mouse_down(1, Self::get_key_modifiers()),
            win32_sys::WM_RBUTTONUP => window.send_mouse_up(1, Self::get_key_modifiers()),
            win32_sys::WM_MBUTTONDOWN => window.send_mouse_down(2, Self::get_key_modifiers()),
            win32_sys::WM_MBUTTONUP => window.send_mouse_up(2, Self::get_key_modifiers()),
            win32_sys::WM_KEYDOWN | win32_sys::WM_SYSKEYDOWN => {
                // detect control/cmd - c / v / x
                let modifiers = Self::get_key_modifiers();
                let key_code = Self::virtual_key_to_key_code(wparam);
                if modifiers.alt && key_code == KeyCode::F4 {
                    PostMessageW(hwnd, win32_sys::WM_CLOSE, 0, 0);
                }
                if modifiers.control || modifiers.logo {
                    match key_code {
                        KeyCode::KeyV => { // paste
                            if OpenClipboard(ptr::null_mut()) != 0 {
                                let mut data: Vec<u16> = Vec::new();
                                let h_clipboard_data = GetClipboardData(win32_sys::CF_UNICODETEXT);
                                let h_clipboard_ptr = GlobalLock(h_clipboard_data) as *mut u16;
                                let clipboard_size = GlobalSize(h_clipboard_data);
                                if clipboard_size > 2 {
                                    data.resize((clipboard_size >> 1) - 1, 0);
                                    std::ptr::copy_nonoverlapping(h_clipboard_ptr, data.as_mut_ptr(), data.len());
                                    GlobalUnlock(h_clipboard_data);
                                    CloseClipboard();
                                    if let Ok(utf8) = String::from_utf16(&data) {
                                        window.do_callback(&mut vec![
                                            Win32Event::TextInput(TextInputEvent {
                                                input: utf8,
                                                was_paste: true,
                                                replace_last: false
                                            })
                                        ]);
                                    }
                                }
                                else {
                                    GlobalUnlock(h_clipboard_data);
                                    CloseClipboard();
                                }
                            }
                        }
                        KeyCode::KeyX | KeyCode::KeyC => {
                            let response = Rc::new(RefCell::new(None));
                            window.do_callback(vec![
                                Win32Event::TextCopy(TextCopyEvent {
                                    response: response.clone()
                                })
                            ]);
                            let response = response.borrow();
                            if let Some(response) = response.as_ref(){
                                // plug it into the windows clipboard
                                // make utf16 dta
                                if OpenClipboard(ptr::null_mut()) != 0 {
                                    EmptyClipboard();
                                    
                                    let data: Vec<u16> = OsStr::new(response).encode_wide().chain(Some(0).into_iter()).collect();
                                    
                                    let h_clipboard_data = GlobalAlloc(win32_sys::GMEM_DDESHARE, 2 * data.len());
                                    
                                    let h_clipboard_ptr = GlobalLock(h_clipboard_data) as *mut u16;
                                    
                                    std::ptr::copy_nonoverlapping(data.as_ptr(), h_clipboard_ptr, data.len());
                                    
                                    GlobalUnlock(h_clipboard_data);
                                    SetClipboardData(win32_sys::CF_UNICODETEXT, h_clipboard_data);
                                    CloseClipboard();
                                }
                            };
                        }
                        _ => ()
                    }
                }
                window.do_callback(&vec![
                    Event::KeyDown(KeyEvent {
                        key_code: key_code,
                        is_repeat: lparam & 0x7fff>0,
                        modifiers: modifiers,
                        time: window.time_now()
                    })
                ]);
            },
            win32_sys::WM_KEYUP | win32_sys::WM_SYSKEYUP => {
                window.do_callback(&vec![
                    Win32Event::KeyUp(KeyEvent {
                        key_code: Self::virtual_key_to_key_code(wparam),
                        is_repeat: lparam & 0x7fff>0,
                        modifiers: Self::get_key_modifiers(),
                        time: window.time_now()
                    })
                ]);
                
            },
            win32_sys::WM_CHAR => {
                if let Ok(utf8) = String::from_utf16(&[wparam as u16]) {
                    let char_code = utf8.chars().next().unwrap();
                    if char_code >= ' ' {
                        window.do_callback(&vec![
                            Win32Event::TextInput(TextInputEvent {
                                input: utf8,
                                was_paste: false,
                                replace_last: false
                            })
                        ]);
                    }
                }
            },
            win32_sys::WM_ENTERSIZEMOVE => {
                get_win32_app_global().start_resize();
                window.do_callback(&vec![Win32Event::WindowResizeLoopStart(window.window_id)]);
            }
            win32_sys::WM_EXITSIZEMOVE => {
                get_win32_app_global().stop_resize();
                window.do_callback(&vec![Win32Event::WindowResizeLoopStop(window.window_id)]);
            },
            win32_sys::WM_SIZE | win32_sys::WM_DPICHANGED => {
                window.send_change_event();
            },
            win32_sys::WM_USER => {
                let mut signals = HashSet::new();
                signals.insert(Signal {signal_id: wparam as usize});
                window.do_callback(&mut vec![
                    Win32Event::Signal(SignalEvent {signals})
                ]);
            },
            win32_sys::WM_CLOSE => { // close requested
                let accept_close = Rc::new(Cell::new(true));
                window.do_callback(vec![Win32Event::WindowCloseRequested(WindowCloseRequestedEvent {
                    window_id: window.window_id,
                    accept_close: accept_close.clone()
                })]);
                if accept_close.get() {
                    DestroyWindow(hwnd);
                }
            },
            win32_sys::WM_DESTROY => { // window actively destroyed
                get_win32_app_global().event_recur_block = false; //exception case
                window.do_callback(&mut vec![
                    Event::WindowClosed(WindowClosedEvent {
                        window_id: window.window_id,
                    })
                ]);
            },
            _ => {
                return DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        return 1
        // lets get the window
        // Unwinding into foreign code is undefined behavior. So we catch any panics that occur in our
        // code, and if a panic happens we cancel any future operations.
        //run_catch_panic(-1, || callback_inner(window, msg, wparam, lparam))
    }
    
    pub fn get_mouse_pos_from_lparam(&self, lparam: LPARAM) -> DVec2 {
        let dpi = self.get_dpi_factor();
        let ycoord = (lparam >> 16) as u16 as i16 as f64;
        let xcoord = (lparam & 0xffff) as u16 as i16 as f64;
        Vec2 {x: xcoord / dpi, y: ycoord / dpi}
    }
    
    pub fn get_key_modifiers() -> KeyModifiers {
        unsafe {
            KeyModifiers {
                control: GetKeyState(win32_sys::VK_CONTROL) & 0x80>0,
                shift: GetKeyState(win32_sys::VK_SHIFT) & 0x80>0,
                alt: GetKeyState(win32_sys::VK_MENU) & 0x80>0,
                logo: GetKeyState(win32_sys::VK_LWIN) & 0x80>0
                    || GetKeyState(win32_sys::VK_RWIN) & 0x80>0,
            }
        }
    }
    
    pub fn on_mouse_move(&self) {
    }
    
    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {
    }
    
    pub fn restore(&self) {
        unsafe {
            ShowWindow(self.hwnd.unwrap(), win32_sys::SW_RESTORE);
            PostMessageW(self.hwnd.unwrap(), win32_sys::WM_SIZE, 0, 0);
        }
    }
    
    pub fn maximize(&self) {
        unsafe {
            ShowWindow(self.hwnd.unwrap(), win32_sys::SW_MAXIMIZE);
            PostMessageW(self.hwnd.unwrap(), win32_sys::WM_SIZE, 0, 0);
        }
    }
    
    pub fn close_window(&self) {
        unsafe {
            DestroyWindow(self.hwnd.unwrap());
        }
    }
    
    pub fn minimize(&self) {
        unsafe {
            ShowWindow(self.hwnd.unwrap(), win32_sys::SW_MINIMIZE);
        }
    }
    
    pub fn set_topmost(&self, topmost: bool) {
        unsafe {
            if topmost {
                SetWindowPos(
                    self.hwnd.unwrap(),
                    win32_sys::HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    win32_sys::SWP_NOMOVE | win32_sys::SWP_NOSIZE
                );
            }
            else {
                SetWindowPos(
                    self.hwnd.unwrap(),
                    win32_sys::HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    win32_sys::SWP_NOMOVE | win32_sys::SWP_NOSIZE
                );
            }
        }
    }
    
    pub fn get_is_topmost(&self) -> bool {
        unsafe {
            let ex_style = GetWindowLongW(self.hwnd.unwrap(), win32_sys::GWL_EXSTYLE) as u32;
            if (ex_style & win32_sys::WS_EX_TOPMOST) != 0 {
                return true
            }
            return false
        }
    }
    
    pub fn get_window_geom(&self) -> WindowGeom {
        WindowGeom {
            xr_can_present: false,
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
        unsafe {
            let wp: mem::MaybeUninit<WINDOWPLACEMENT> = mem::MaybeUninit::uninit();
            let mut wp = wp.assume_init();
            wp.length = mem::size_of::<WINDOWPLACEMENT>() as u32;
            GetWindowPlacement(self.hwnd.unwrap(), &mut wp);
            if wp.showCmd as i32 == win32_sys::SW_MAXIMIZE {
                return true
            }
            return false
        }
    }
    
    pub fn time_now(&self) -> f64 {
        get_win32_app_global().time_now()
    }
    
    pub fn set_ime_spot(&mut self, spot: Vec2) {
        self.ime_spot = spot;
    }
    
    pub fn get_position(&self) -> DVec2 {
        unsafe {
            let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetWindowRect(self.hwnd.unwrap(), &mut rect);
            DVec2 {x: rect.left as f64, y: rect.top as f64}
        }
    }
    
    pub fn get_inner_size(&self) -> DVec2 {
        unsafe {
            let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetClientRect(self.hwnd.unwrap(), &mut rect);
            let dpi = self.get_dpi_factor();
            DVec2 {x: (rect.right - rect.left) as f64 / dpi, y: (rect.bottom - rect.top)as f64 / dpi}
        }
    }
    
    pub fn get_outer_size(&self) -> DVec2 {
        unsafe {
            let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetWindowRect(self.hwnd.unwrap(), &mut rect);
            DVec2 {x: (rect.right - rect.left) as f64, y: (rect.bottom - rect.top)as f64}
        }
    }
    
    pub fn set_position(&mut self, pos: DVec2) {
        unsafe {
            let mut window_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetWindowRect(self.hwnd.unwrap(), &mut window_rect);
            let dpi = self.get_dpi_factor();
            MoveWindow(
                self.hwnd.unwrap(),
                (pos.x * dpi) as i32,
                (pos.y * dpi) as i32,
                window_rect.right - window_rect.left,
                window_rect.bottom - window_rect.top,
                false
            );
        }
    }
    
    pub fn set_outer_size(&self, size: DVec2) {
        unsafe {
            let mut window_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetWindowRect(self.hwnd.unwrap(), &mut window_rect);
            let dpi = self.get_dpi_factor();
            MoveWindow(
                self.hwnd.unwrap(),
                window_rect.left,
                window_rect.top,
                (size.x * dpi) as i32,
                (size.y * dpi) as i32,
                false
            );
        }
    }
    
    pub fn set_inner_size(&self, size: DVec2) {
        unsafe {
            let mut window_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetWindowRect(self.hwnd.unwrap(), &mut window_rect);
            let mut client_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetClientRect(self.hwnd.unwrap(), &mut client_rect);
            let dpi = self.get_dpi_factor();
            MoveWindow(
                self.hwnd.unwrap(),
                window_rect.left,
                window_rect.top,
                (size.x * dpi) as i32
                    + ((window_rect.right - window_rect.left) - (client_rect.right - client_rect.left)),
                (size.y * dpi) as i32
                    + ((window_rect.bottom - window_rect.top) - (client_rect.bottom - client_rect.top)),
                false
            );
        }
    }
    
    pub fn get_dpi_factor(&self) -> f64 {
        unsafe {
            get_win32_app_global().dpi_functions.hwnd_dpi_factor(self.hwnd.unwrap()) as f64
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Win32Event>) {
        unsafe {
            get_win32_app_global().do_callback(events);
        }
    }
    
    pub fn send_change_event(&mut self) {
        
        let new_geom = self.get_window_geom();
        let old_geom = self.last_window_geom.clone();
        self.last_window_geom = new_geom.clone();
        
        self.do_callback(&mut vec![
            Win32Event::WindowGeomChange(WindowGeomChangeEvent {
                window_id: self.window_id,
                old_geom: old_geom,
                new_geom: new_geom
            }),
            Win32Event::Paint
        ]);
    }
    
    pub fn send_focus_event(&mut self) {
        self.do_callback(&mut vec![Win32Event::AppFocus]);
    }
    
    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(&mut vec![Win32Event::AppFocusLost]);
    }
    
    pub fn send_mouse_down(&mut self, button: usize, modifiers: KeyModifiers) {
        if self.mouse_buttons_down == 0 {
            unsafe {SetCapture(self.hwnd.unwrap());}
        }
        self.mouse_buttons_down += 1;
        self.do_callback(vec![Win32Event::MouseDown(MouseDownEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        })]);
    }
    
    pub fn send_mouse_up(&mut self, button: usize, modifiers: KeyModifiers) {
        if self.mouse_buttons_down > 1 {
            self.mouse_buttons_down -= 1;
        }
        else {
            unsafe {ReleaseCapture();}
            self.mouse_buttons_down = 0;
        }
        self.do_callback(vec![Win32Event::MouseUp(MouseUpEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now()
        })]);
    }
    
    pub fn send_mouse_move(&mut self, pos: DVec2, modifiers: KeyModifiers) {
        self.do_callback(vec![Win32Event::MouseMove(MouseMoveEvent {
            window_id: self.window_id,
            abs: pos,
            modifiers: modifiers,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        })]);
    }
    
    pub fn send_scroll(&mut self, scroll: DVec2, modifiers: KeyModifiers, is_mouse: bool) {
        self.do_callback(vec![
            Win32Event::Scroll(ScrollEvent {
                window_id: self.window_id,
                scroll,
                abs: self.last_mouse_pos,
                modifiers,
                time: self.time_now(),
                is_mouse,
                handled_x: Cell::new(false),
                handled_y: Cell::new(false),
            })
        ]);
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
    
    pub fn virtual_key_to_key_code(wparam: WPARAM) -> KeyCode {
        match wparam as i32 {
            win32_sys::VK_ESCAPE => KeyCode::Escape,
            win32_sys::VK_OEM_3 => KeyCode::Backtick,
            win32_sys::VK_0 => KeyCode::Key0,
            win32_sys::VK_1 => KeyCode::Key1,
            win32_sys::VK_2 => KeyCode::Key2,
            win32_sys::VK_3 => KeyCode::Key3,
            win32_sys::VK_4 => KeyCode::Key4,
            win32_sys::VK_5 => KeyCode::Key5,
            win32_sys::VK_6 => KeyCode::Key6,
            win32_sys::VK_7 => KeyCode::Key7,
            win32_sys::VK_8 => KeyCode::Key8,
            win32_sys::VK_9 => KeyCode::Key9,
            win32_sys::VK_OEM_MINUS => KeyCode::Minus,
            win32_sys::VK_OEM_PLUS => KeyCode::Equals,
            win32_sys::VK_BACK => KeyCode::Backspace,
            win32_sys::VK_TAB => KeyCode::Tab,
            win32_sys::VK_Q => KeyCode::KeyQ,
            win32_sys::VK_W => KeyCode::KeyW,
            win32_sys::VK_E => KeyCode::KeyE,
            win32_sys::VK_R => KeyCode::KeyR,
            win32_sys::VK_T => KeyCode::KeyT,
            win32_sys::VK_Y => KeyCode::KeyY,
            win32_sys::VK_U => KeyCode::KeyU,
            win32_sys::VK_I => KeyCode::KeyI,
            win32_sys::VK_O => KeyCode::KeyO,
            win32_sys::VK_P => KeyCode::KeyP,
            win32_sys::VK_OEM_4 => KeyCode::LBracket,
            win32_sys::VK_OEM_6 => KeyCode::RBracket,
            win32_sys::VK_RETURN => KeyCode::Return,
            win32_sys::VK_A => KeyCode::KeyA,
            win32_sys::VK_S => KeyCode::KeyS,
            win32_sys::VK_D => KeyCode::KeyD,
            win32_sys::VK_F => KeyCode::KeyF,
            win32_sys::VK_G => KeyCode::KeyG,
            win32_sys::VK_H => KeyCode::KeyH,
            win32_sys::VK_J => KeyCode::KeyJ,
            win32_sys::VK_K => KeyCode::KeyK,
            win32_sys::VK_L => KeyCode::KeyL,
            win32_sys::VK_OEM_1 => KeyCode::Semicolon,
            win32_sys::VK_OEM_7 => KeyCode::Quote,
            win32_sys::VK_OEM_5 => KeyCode::Backslash,
            win32_sys::VK_Z => KeyCode::KeyZ,
            win32_sys::VK_X => KeyCode::KeyX,
            win32_sys::VK_C => KeyCode::KeyC,
            win32_sys::VK_V => KeyCode::KeyV,
            win32_sys::VK_B => KeyCode::KeyB,
            win32_sys::VK_N => KeyCode::KeyN,
            win32_sys::VK_M => KeyCode::KeyM,
            win32_sys::VK_OEM_COMMA => KeyCode::Comma,
            win32_sys::VK_OEM_PERIOD => KeyCode::Period,
            win32_sys::VK_OEM_2 => KeyCode::Slash,
            win32_sys::VK_LCONTROL => KeyCode::Control,
            win32_sys::VK_RCONTROL => KeyCode::Control,
            win32_sys::VK_CONTROL => KeyCode::Control,
            win32_sys::VK_LMENU => KeyCode::Alt,
            win32_sys::VK_RMENU => KeyCode::Alt,
            win32_sys::VK_MENU => KeyCode::Alt,
            win32_sys::VK_LSHIFT => KeyCode::Shift,
            win32_sys::VK_RSHIFT => KeyCode::Shift,
            win32_sys::VK_SHIFT => KeyCode::Shift,
            win32_sys::VK_LWIN => KeyCode::Logo,
            win32_sys::VK_RWIN => KeyCode::Logo,
            win32_sys::VK_SPACE => KeyCode::Space,
            win32_sys::VK_CAPITAL => KeyCode::Capslock,
            win32_sys::VK_F1 => KeyCode::F1,
            win32_sys::VK_F2 => KeyCode::F2,
            win32_sys::VK_F3 => KeyCode::F3,
            win32_sys::VK_F4 => KeyCode::F4,
            win32_sys::VK_F5 => KeyCode::F5,
            win32_sys::VK_F6 => KeyCode::F6,
            win32_sys::VK_F7 => KeyCode::F7,
            win32_sys::VK_F8 => KeyCode::F8,
            win32_sys::VK_F9 => KeyCode::F9,
            win32_sys::VK_F10 => KeyCode::F10,
            win32_sys::VK_F11 => KeyCode::F11,
            win32_sys::VK_F12 => KeyCode::F12,
            win32_sys::VK_SNAPSHOT => KeyCode::PrintScreen,
            win32_sys::VK_SCROLL => KeyCode::Scrolllock,
            win32_sys::VK_PAUSE => KeyCode::Pause,
            win32_sys::VK_INSERT => KeyCode::Insert,
            win32_sys::VK_DELETE => KeyCode::Delete,
            win32_sys::VK_HOME => KeyCode::Home,
            win32_sys::VK_END => KeyCode::End,
            win32_sys::VK_PRIOR => KeyCode::PageUp,
            win32_sys::VK_NEXT => KeyCode::PageDown,
            win32_sys::VK_NUMPAD0 => KeyCode::Numpad0,
            win32_sys::VK_NUMPAD1 => KeyCode::Numpad1,
            win32_sys::VK_NUMPAD2 => KeyCode::Numpad2,
            win32_sys::VK_NUMPAD3 => KeyCode::Numpad3,
            win32_sys::VK_NUMPAD4 => KeyCode::Numpad4,
            win32_sys::VK_NUMPAD5 => KeyCode::Numpad5,
            win32_sys::VK_NUMPAD6 => KeyCode::Numpad6,
            win32_sys::VK_NUMPAD7 => KeyCode::Numpad7,
            win32_sys::VK_NUMPAD8 => KeyCode::Numpad8,
            win32_sys::VK_NUMPAD9 => KeyCode::Numpad9,
            //winuser::VK_BACK => KeyCode::NumpadEquals,
            win32_sys::VK_SUBTRACT => KeyCode::NumpadSubtract,
            win32_sys::VK_ADD => KeyCode::NumpadAdd,
            win32_sys::VK_DECIMAL => KeyCode::NumpadDecimal,
            win32_sys::VK_MULTIPLY => KeyCode::NumpadMultiply,
            win32_sys::VK_DIVIDE => KeyCode::NumpadDivide,
            win32_sys::VK_NUMLOCK => KeyCode::Numlock,
            //winuser::VK_BACK => KeyCode::NumpadEnter,
            win32_sys::VK_UP => KeyCode::ArrowUp,
            win32_sys::VK_DOWN => KeyCode::ArrowDown,
            win32_sys::VK_LEFT => KeyCode::ArrowLeft,
            win32_sys::VK_RIGHT => KeyCode::ArrowRight,
            _ => KeyCode::Unknown
        }
    }
}

