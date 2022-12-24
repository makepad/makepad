use {
    std::{
        cell::{RefCell, Cell},
        rc::Rc,
        ffi::OsStr,
        os::windows::ffi::OsStrExt,
        mem,
        collections::{HashSet}
    },
    
    crate::{
        windows_crate::{
            core::PCWSTR,
            Win32::Foundation::HWND,
            Win32::Foundation::HANDLE,
            Win32::Foundation::WPARAM,
            Win32::Foundation::LPARAM,
            Win32::Foundation::LRESULT,
            Win32::Foundation::RECT,
            Win32::System::Memory::GlobalLock,
            Win32::System::Memory::GlobalAlloc,
            Win32::System::Memory::GlobalSize,
            Win32::System::Memory::GlobalUnlock,
            Win32::System::Memory::GLOBAL_ALLOC_FLAGS,
            Win32::System::SystemServices,
            Win32::System::WindowsProgramming,
            Win32::System::DataExchange::OpenClipboard,
            Win32::System::DataExchange::EmptyClipboard,
            Win32::System::DataExchange::GetClipboardData,
            Win32::System::DataExchange::SetClipboardData,
            Win32::System::DataExchange::CloseClipboard,
            Win32::UI::WindowsAndMessaging,
            Win32::UI::WindowsAndMessaging::CreateWindowExW,
            Win32::UI::WindowsAndMessaging::SetWindowLongPtrW,
            Win32::UI::WindowsAndMessaging::GetWindowLongPtrW,
            Win32::UI::WindowsAndMessaging::DefWindowProcW,
            Win32::UI::WindowsAndMessaging::ShowWindow,
            Win32::UI::WindowsAndMessaging::PostMessageW,
            Win32::UI::WindowsAndMessaging::GetWindowRect,
            Win32::UI::WindowsAndMessaging::DestroyWindow,
            Win32::UI::WindowsAndMessaging::SetWindowPos,
            Win32::UI::WindowsAndMessaging::GetWindowPlacement,
            Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT,
            Win32::UI::WindowsAndMessaging::GetClientRect,
            Win32::UI::WindowsAndMessaging::MoveWindow,
            Win32::UI::Controls::MARGINS,
            Win32::UI::Controls,
            Win32::UI::Input::KeyboardAndMouse,
            Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
            Win32::UI::Input::KeyboardAndMouse::ReleaseCapture,
            Win32::UI::Input::KeyboardAndMouse::SetCapture,
            Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent,
            Win32::UI::Input::KeyboardAndMouse::GetKeyState,
            Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT,
            Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea,
            Win32::System::LibraryLoader::GetModuleHandleW,
        },
        
        event::*,
        area::Area,
        makepad_live_id::*,
        os::mswindows::win32_app::encode_wide,
        os::mswindows::win32_app::{TRUE, FALSE, post_signal_to_hwnd},
        os::mswindows::win32_event::*,
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
    
    pub fn new(window_id: WindowId) -> Win32Window {
        Win32Window {
            window_id,
            mouse_buttons_down: 0,
            last_window_geom: WindowGeom::default(),
            last_key_mod: KeyModifiers::default(),
            ime_spot: DVec2::default(),
            current_cursor: MouseCursor::Default,
            last_mouse_pos: DVec2::default(),
            ignore_wmsize: 0,
            hwnd: None,
            track_mouse_event: false
        }
    }
    
    pub fn init(&mut self, title: &str, size: DVec2, position: Option<DVec2>) {
        let title = encode_wide(title);
        
        let style = WindowsAndMessaging::WS_SIZEBOX
            | WindowsAndMessaging::WS_MAXIMIZEBOX
            | WindowsAndMessaging::WS_MINIMIZEBOX
            | WindowsAndMessaging::WS_POPUP
            | WindowsAndMessaging::WS_CLIPSIBLINGS
            | WindowsAndMessaging::WS_CLIPCHILDREN
            | WindowsAndMessaging::WS_SYSMENU;
        
        let style_ex = WindowsAndMessaging::WS_EX_WINDOWEDGE
            | WindowsAndMessaging::WS_EX_APPWINDOW
            | WindowsAndMessaging::WS_EX_ACCEPTFILES;
        
        unsafe {
            
            let (x, y) = if let Some(position) = position {
                (position.x as i32, position.y as i32)
            }
            else {
                (WindowsAndMessaging::CW_USEDEFAULT, WindowsAndMessaging::CW_USEDEFAULT)
            };
            
            let hwnd = CreateWindowExW(
                style_ex,
                PCWSTR(b"MakepadWindow\0".as_ptr() as _),
                PCWSTR(title.as_ptr()),
                style,
                x,
                y,
                WindowsAndMessaging::CW_USEDEFAULT,
                WindowsAndMessaging::CW_USEDEFAULT,
                None,
                None,
                GetModuleHandleW(None).unwrap(),
                None,
            );
            
            self.hwnd = Some(hwnd);
            
            SetWindowLongPtrW(hwnd, WindowsAndMessaging::GWLP_USERDATA, self as *const _ as isize);
            
            self.set_outer_size(size);
            
            ShowWindow(hwnd, WindowsAndMessaging::SW_SHOW);
            
            get_win32_app_global().dpi_functions.enable_non_client_dpi_scaling(self.hwnd.unwrap());
            
            if let Ok(mut sigs) = get_win32_app_global().race_signals.lock() {
                get_win32_app_global().all_windows.push(hwnd);
                for signal in sigs.iter() {
                    post_signal_to_hwnd(hwnd, *signal);
                }
                sigs.clear();
            }
            
        }
    }
    
    pub unsafe extern "system" fn window_class_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,) -> LRESULT {
        
        let user_data = GetWindowLongPtrW(hwnd, WindowsAndMessaging::GWLP_USERDATA);
        if user_data == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        
        let window = &mut (*(user_data as *mut Win32Window));
        match msg {
            WindowsAndMessaging::WM_ACTIVATE => {
                if wparam.0 & 0xffff == WindowsAndMessaging::WA_ACTIVE as usize {
                    window.do_callback(vec![Win32Event::AppGotFocus]);
                }
                else {
                    window.do_callback(vec![Win32Event::AppLostFocus]);
                }
            },
            WindowsAndMessaging::WM_NCCALCSIZE => {
                // check if we are maximised
                if window.get_is_maximized() {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                if wparam == WPARAM(1) {
                    let margins = MARGINS {
                        cxLeftWidth: 0,
                        cxRightWidth: 0,
                        cyTopHeight: 0,
                        cyBottomHeight: 1
                    };
                    DwmExtendFrameIntoClientArea(hwnd, &margins).unwrap();
                    return LRESULT(0)
                }
            },
            WindowsAndMessaging::WM_NCHITTEST => {
                let ycoord = (lparam.0 >> 16) as u16 as i16 as i32;
                let xcoord = (lparam.0 & 0xffff) as u16 as i16 as i32;
                let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
                const EDGE: i32 = 8;
                GetWindowRect(hwnd, &mut rect);
                if xcoord < rect.left + EDGE {
                    if ycoord < rect.top + EDGE {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::NwseResize);
                        return LRESULT(WindowsAndMessaging::HTTOPLEFT as isize);
                    }
                    if ycoord > rect.bottom - EDGE {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::NeswResize);
                        return LRESULT(WindowsAndMessaging::HTBOTTOMLEFT as isize);
                    }
                    get_win32_app_global().set_mouse_cursor(MouseCursor::EwResize);
                    return LRESULT(WindowsAndMessaging::HTLEFT as isize);
                }
                if xcoord > rect.right - EDGE {
                    if ycoord < rect.top + EDGE {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::NeswResize);
                        return LRESULT(WindowsAndMessaging::HTTOPRIGHT as isize);
                    }
                    if ycoord > rect.bottom - EDGE {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::NwseResize);
                        return LRESULT(WindowsAndMessaging::HTBOTTOMRIGHT as isize);
                    }
                    get_win32_app_global().set_mouse_cursor(MouseCursor::EwResize);
                    return LRESULT(WindowsAndMessaging::HTRIGHT as isize);
                }
                if ycoord < rect.top + EDGE {
                    get_win32_app_global().set_mouse_cursor(MouseCursor::NsResize);
                    return LRESULT(WindowsAndMessaging::HTTOP as isize);
                }
                if ycoord > rect.bottom - EDGE {
                    get_win32_app_global().set_mouse_cursor(MouseCursor::NsResize);
                    return LRESULT(WindowsAndMessaging::HTBOTTOM as isize);
                }
                let response = Rc::new(Cell::new(WindowDragQueryResponse::NoAnswer));
                window.do_callback(vec![
                    Win32Event::WindowDragQuery(WindowDragQueryEvent {
                        window_id: window.window_id,
                        abs: window.get_mouse_pos_from_lparam(lparam),
                        response: response.clone()
                    })
                ]);
                match response.get() {
                    WindowDragQueryResponse::Client => {
                        return LRESULT(WindowsAndMessaging::HTCLIENT as isize);
                    }
                    WindowDragQueryResponse::Caption => {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::Default);
                        return LRESULT(WindowsAndMessaging::HTCAPTION as isize);
                    },
                    WindowDragQueryResponse::SysMenu => {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::Default);
                        return LRESULT(WindowsAndMessaging::HTSYSMENU as isize);
                    }
                    _ => ()
                }
                if ycoord < rect.top + 50 && xcoord < rect.left + 50 {
                    return LRESULT(WindowsAndMessaging::HTSYSMENU as isize);
                }
                if ycoord < rect.top + 50 && xcoord < rect.right - 300 {
                    return LRESULT(WindowsAndMessaging::HTCAPTION as isize);
                }
                return LRESULT(WindowsAndMessaging::HTCLIENT as isize);
            },
            WindowsAndMessaging::WM_ERASEBKGND => {
                return LRESULT(1)
            },
            WindowsAndMessaging::WM_MOUSEMOVE => {
                if !window.track_mouse_event {
                    window.track_mouse_event = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: KeyboardAndMouse::TME_LEAVE,
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
            Controls::WM_MOUSELEAVE => {
                window.track_mouse_event = false;
                window.send_mouse_move(
                    window.last_mouse_pos,
                    Self::get_key_modifiers()
                );
                get_win32_app_global().current_cursor = MouseCursor::Hidden;
            },
            WindowsAndMessaging::WM_MOUSEWHEEL => {
                let delta = (wparam.0 >> 16) as u16 as i16 as f64;
                window.send_scroll(DVec2 {x: 0.0, y: -delta}, Self::get_key_modifiers(), true);
            },
            WindowsAndMessaging::WM_LBUTTONDOWN => window.send_mouse_down(0, Self::get_key_modifiers()),
            WindowsAndMessaging::WM_LBUTTONUP => window.send_mouse_up(0, Self::get_key_modifiers()),
            WindowsAndMessaging::WM_RBUTTONDOWN => window.send_mouse_down(1, Self::get_key_modifiers()),
            WindowsAndMessaging::WM_RBUTTONUP => window.send_mouse_up(1, Self::get_key_modifiers()),
            WindowsAndMessaging::WM_MBUTTONDOWN => window.send_mouse_down(2, Self::get_key_modifiers()),
            WindowsAndMessaging::WM_MBUTTONUP => window.send_mouse_up(2, Self::get_key_modifiers()),
            WindowsAndMessaging::WM_KEYDOWN | WindowsAndMessaging::WM_SYSKEYDOWN => {
                // detect control/cmd - c / v / x
                let modifiers = Self::get_key_modifiers();
                let key_code = Self::virtual_key_to_key_code(wparam);
                if modifiers.alt && key_code == KeyCode::F4 {
                    PostMessageW(hwnd, WindowsAndMessaging::WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                if modifiers.control || modifiers.logo {
                    match key_code {
                        KeyCode::KeyV => { // paste
                            if OpenClipboard(None) == TRUE{
                                let mut data: Vec<u16> = Vec::new();
                                let h_clipboard_data = GetClipboardData(SystemServices::CF_UNICODETEXT.0).unwrap();
                                let h_clipboard_ptr = GlobalLock(h_clipboard_data.0) as *mut u16;
                                let clipboard_size = GlobalSize(h_clipboard_data.0);
                                if clipboard_size > 2 {
                                    data.resize((clipboard_size >> 1) - 1, 0);
                                    std::ptr::copy_nonoverlapping(h_clipboard_ptr, data.as_mut_ptr(), data.len());
                                    GlobalUnlock(h_clipboard_data.0);
                                    CloseClipboard();
                                    if let Ok(utf8) = String::from_utf16(&data) {
                                        window.do_callback(vec![
                                            Win32Event::TextInput(TextInputEvent {
                                                input: utf8,
                                                was_paste: true,
                                                replace_last: false
                                            })
                                        ]);
                                    }
                                }
                                else {
                                    GlobalUnlock(h_clipboard_data.0);
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
                            if let Some(response) = response.as_ref() {
                                // plug it into the windows clipboard
                                // make utf16 dta
                                if OpenClipboard(None) == TRUE{
                                    EmptyClipboard();
                                    
                                    let data: Vec<u16> = OsStr::new(response).encode_wide().chain(Some(0).into_iter()).collect();
                                    
                                    let h_clipboard_data = GlobalAlloc(GLOBAL_ALLOC_FLAGS(WindowsProgramming::GMEM_DDESHARE), 2 * data.len());
                                    
                                    let h_clipboard_ptr = GlobalLock(h_clipboard_data) as *mut u16;
                                    
                                    std::ptr::copy_nonoverlapping(data.as_ptr(), h_clipboard_ptr, data.len());
                                    
                                    GlobalUnlock(h_clipboard_data);
                                    SetClipboardData(SystemServices::CF_UNICODETEXT.0, HANDLE(h_clipboard_data)).unwrap();
                                    CloseClipboard();
                                }
                            };
                        }
                        _ => ()
                    }
                }
                window.do_callback(vec![
                    Win32Event::KeyDown(KeyEvent {
                        key_code: key_code,
                        is_repeat: lparam.0 & 0x7fff>0,
                        modifiers: modifiers,
                        time: window.time_now()
                    })
                ]);
            },
            WindowsAndMessaging::WM_KEYUP | WindowsAndMessaging::WM_SYSKEYUP => {
                window.do_callback(vec![
                    Win32Event::KeyUp(KeyEvent {
                        key_code: Self::virtual_key_to_key_code(wparam),
                        is_repeat: lparam.0 & 0x7fff>0,
                        modifiers: Self::get_key_modifiers(),
                        time: window.time_now()
                    })
                ]);
                
            },
            WindowsAndMessaging::WM_CHAR => {
                if let Ok(utf8) = String::from_utf16(&[wparam.0 as u16]) {
                    let char_code = utf8.chars().next().unwrap();
                    if char_code >= ' ' {
                        window.do_callback(vec![
                            Win32Event::TextInput(TextInputEvent {
                                input: utf8,
                                was_paste: false,
                                replace_last: false
                            })
                        ]);
                    }
                }
            },
            WindowsAndMessaging::WM_ENTERSIZEMOVE => {
                get_win32_app_global().start_resize();
                window.do_callback(vec![Win32Event::WindowResizeLoopStart(window.window_id)]);
            }
            WindowsAndMessaging::WM_EXITSIZEMOVE => {
                get_win32_app_global().stop_resize();
                window.do_callback(vec![Win32Event::WindowResizeLoopStop(window.window_id)]);
            },
            WindowsAndMessaging::WM_SIZE | WindowsAndMessaging::WM_DPICHANGED => {
                window.send_change_event();
            },
            WindowsAndMessaging::WM_USER => {
                let mut signals = HashSet::new();
                signals.insert(Signal(LiveId(((wparam.0 as u64) << 32) | (lparam.0 as u64))));
                window.do_callback(vec![
                    Win32Event::Signal(SignalEvent {signals})
                ]);
            },
            WindowsAndMessaging::WM_CLOSE => { // close requested
                let accept_close = Rc::new(Cell::new(true));
                window.do_callback(vec![Win32Event::WindowCloseRequested(WindowCloseRequestedEvent {
                    window_id: window.window_id,
                    accept_close: accept_close.clone()
                })]);
                if accept_close.get() {
                    DestroyWindow(hwnd);
                }
            },
            WindowsAndMessaging::WM_DESTROY => { // window actively destroyed
                window.do_callback(vec![
                    Win32Event::WindowClosed(WindowClosedEvent {
                        window_id: window.window_id,
                    })
                ]);
            },
            _ => {
                return DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        return LRESULT(1)
        // lets get the window
        // Unwinding into foreign code is undefined behavior. So we catch any panics that occur in our
        // code, and if a panic happens we cancel any future operations.
        //run_catch_panic(-1, || callback_inner(window, msg, wparam, lparam))
    }
    
    pub fn get_mouse_pos_from_lparam(&self, lparam: LPARAM) -> DVec2 {
        let dpi = self.get_dpi_factor();
        let ycoord = (lparam.0 >> 16) as u16 as i16 as f64;
        let xcoord = (lparam.0 & 0xffff) as u16 as i16 as f64;
        DVec2 {x: xcoord / dpi, y: ycoord / dpi}
    }
    
    pub fn get_key_modifiers() -> KeyModifiers {
        unsafe {
            KeyModifiers {
                control: GetKeyState(KeyboardAndMouse::VK_CONTROL.0 as i32) & 0x80>0,
                shift: GetKeyState(KeyboardAndMouse::VK_SHIFT.0 as i32) & 0x80>0,
                alt: GetKeyState(KeyboardAndMouse::VK_MENU.0 as i32) & 0x80>0,
                logo: GetKeyState(KeyboardAndMouse::VK_LWIN.0 as i32) & 0x80>0
                    || GetKeyState(KeyboardAndMouse::VK_RWIN.0 as i32) & 0x80>0,
            }
        }
    }
    
    pub fn on_mouse_move(&self) {
    }
    
    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {
    }
    
    pub fn restore(&self) {
        unsafe {
            ShowWindow(self.hwnd.unwrap(), WindowsAndMessaging::SW_RESTORE);
            PostMessageW(self.hwnd.unwrap(), WindowsAndMessaging::WM_SIZE, WPARAM(0), LPARAM(0));
        }
    }
    
    pub fn maximize(&self) {
        unsafe {
            ShowWindow(self.hwnd.unwrap(), WindowsAndMessaging::SW_MAXIMIZE);
            PostMessageW(self.hwnd.unwrap(), WindowsAndMessaging::WM_SIZE, WPARAM(0), LPARAM(0));
        }
    }
    
    pub fn close_window(&self) {
        unsafe {
            DestroyWindow(self.hwnd.unwrap());
        }
    }
    
    pub fn minimize(&self) {
        unsafe {
            ShowWindow(self.hwnd.unwrap(), WindowsAndMessaging::SW_MINIMIZE);
        }
    }
    
    pub fn set_topmost(&self, topmost: bool) {
        unsafe {
            if topmost {
                SetWindowPos(
                    self.hwnd.unwrap(),
                    WindowsAndMessaging::HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    WindowsAndMessaging::SWP_NOMOVE | WindowsAndMessaging::SWP_NOSIZE
                );
            }
            else {
                SetWindowPos(
                    self.hwnd.unwrap(),
                    WindowsAndMessaging::HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    WindowsAndMessaging::SWP_NOMOVE | WindowsAndMessaging::SWP_NOSIZE
                );
            }
        }
    }
    
    pub fn get_is_topmost(&self) -> bool {
        unsafe {
            let ex_style = GetWindowLongPtrW(self.hwnd.unwrap(), WindowsAndMessaging::GWL_EXSTYLE);
            if ex_style as u32 & WindowsAndMessaging::WS_EX_TOPMOST.0 != 0 {
                return true
            }
            return false
        }
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
        unsafe {
            let wp: mem::MaybeUninit<WINDOWPLACEMENT> = mem::MaybeUninit::uninit();
            let mut wp = wp.assume_init();
            wp.length = mem::size_of::<WINDOWPLACEMENT>() as u32;
            GetWindowPlacement(self.hwnd.unwrap(), &mut wp);
            if wp.showCmd == WindowsAndMessaging::SW_MAXIMIZE {
                return true
            }
            return false
        }
    }
    
    pub fn time_now(&self) -> f64 {
        get_win32_app_global().time_now()
    }
    
    pub fn set_ime_spot(&mut self, spot: DVec2) {
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
                FALSE
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
                FALSE
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
                FALSE
            );
        }
    }
    
    pub fn get_dpi_factor(&self) -> f64 {
        get_win32_app_global().dpi_functions.hwnd_dpi_factor(self.hwnd.unwrap()) as f64
    }
    
    pub fn do_callback(&mut self, events: Vec<Win32Event>) {
        get_win32_app_global().do_callback(events);
    }
    
    pub fn send_change_event(&mut self) {
        
        let new_geom = self.get_window_geom();
        let old_geom = self.last_window_geom.clone();
        self.last_window_geom = new_geom.clone();
        
        self.do_callback(vec![
            Win32Event::WindowGeomChange(WindowGeomChangeEvent {
                window_id: self.window_id,
                old_geom: old_geom,
                new_geom: new_geom
            }),
            Win32Event::Paint
        ]);
    }
    
    pub fn send_focus_event(&mut self) {
        self.do_callback(vec![Win32Event::AppGotFocus]);
    }
    
    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(vec![Win32Event::AppLostFocus]);
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
        let accept_close = Rc::new(Cell::new(true));
        self.do_callback(vec![Win32Event::WindowCloseRequested(WindowCloseRequestedEvent {
            window_id: self.window_id,
            accept_close: accept_close.clone()
        })]);
        if !accept_close.get() {
            return false
        }
        true
    }
    
    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(vec![Win32Event::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last
        })])
    }
    
    pub fn virtual_key_to_key_code(wparam: WPARAM) -> KeyCode {
        match VIRTUAL_KEY(wparam.0 as u16) {
            KeyboardAndMouse::VK_ESCAPE => KeyCode::Escape,
            KeyboardAndMouse::VK_OEM_3 => KeyCode::Backtick,
            KeyboardAndMouse::VK_0 => KeyCode::Key0,
            KeyboardAndMouse::VK_1 => KeyCode::Key1,
            KeyboardAndMouse::VK_2 => KeyCode::Key2,
            KeyboardAndMouse::VK_3 => KeyCode::Key3,
            KeyboardAndMouse::VK_4 => KeyCode::Key4,
            KeyboardAndMouse::VK_5 => KeyCode::Key5,
            KeyboardAndMouse::VK_6 => KeyCode::Key6,
            KeyboardAndMouse::VK_7 => KeyCode::Key7,
            KeyboardAndMouse::VK_8 => KeyCode::Key8,
            KeyboardAndMouse::VK_9 => KeyCode::Key9,
            KeyboardAndMouse::VK_OEM_MINUS => KeyCode::Minus,
            KeyboardAndMouse::VK_OEM_PLUS => KeyCode::Equals,
            KeyboardAndMouse::VK_BACK => KeyCode::Backspace,
            KeyboardAndMouse::VK_TAB => KeyCode::Tab,
            KeyboardAndMouse::VK_Q => KeyCode::KeyQ,
            KeyboardAndMouse::VK_W => KeyCode::KeyW,
            KeyboardAndMouse::VK_E => KeyCode::KeyE,
            KeyboardAndMouse::VK_R => KeyCode::KeyR,
            KeyboardAndMouse::VK_T => KeyCode::KeyT,
            KeyboardAndMouse::VK_Y => KeyCode::KeyY,
            KeyboardAndMouse::VK_U => KeyCode::KeyU,
            KeyboardAndMouse::VK_I => KeyCode::KeyI,
            KeyboardAndMouse::VK_O => KeyCode::KeyO,
            KeyboardAndMouse::VK_P => KeyCode::KeyP,
            KeyboardAndMouse::VK_OEM_4 => KeyCode::LBracket,
            KeyboardAndMouse::VK_OEM_6 => KeyCode::RBracket,
            KeyboardAndMouse::VK_RETURN => KeyCode::ReturnKey,
            KeyboardAndMouse::VK_A => KeyCode::KeyA,
            KeyboardAndMouse::VK_S => KeyCode::KeyS,
            KeyboardAndMouse::VK_D => KeyCode::KeyD,
            KeyboardAndMouse::VK_F => KeyCode::KeyF,
            KeyboardAndMouse::VK_G => KeyCode::KeyG,
            KeyboardAndMouse::VK_H => KeyCode::KeyH,
            KeyboardAndMouse::VK_J => KeyCode::KeyJ,
            KeyboardAndMouse::VK_K => KeyCode::KeyK,
            KeyboardAndMouse::VK_L => KeyCode::KeyL,
            KeyboardAndMouse::VK_OEM_1 => KeyCode::Semicolon,
            KeyboardAndMouse::VK_OEM_7 => KeyCode::Quote,
            KeyboardAndMouse::VK_OEM_5 => KeyCode::Backslash,
            KeyboardAndMouse::VK_Z => KeyCode::KeyZ,
            KeyboardAndMouse::VK_X => KeyCode::KeyX,
            KeyboardAndMouse::VK_C => KeyCode::KeyC,
            KeyboardAndMouse::VK_V => KeyCode::KeyV,
            KeyboardAndMouse::VK_B => KeyCode::KeyB,
            KeyboardAndMouse::VK_N => KeyCode::KeyN,
            KeyboardAndMouse::VK_M => KeyCode::KeyM,
            KeyboardAndMouse::VK_OEM_COMMA => KeyCode::Comma,
            KeyboardAndMouse::VK_OEM_PERIOD => KeyCode::Period,
            KeyboardAndMouse::VK_OEM_2 => KeyCode::Slash,
            KeyboardAndMouse::VK_LCONTROL => KeyCode::Control,
            KeyboardAndMouse::VK_RCONTROL => KeyCode::Control,
            KeyboardAndMouse::VK_CONTROL => KeyCode::Control,
            KeyboardAndMouse::VK_LMENU => KeyCode::Alt,
            KeyboardAndMouse::VK_RMENU => KeyCode::Alt,
            KeyboardAndMouse::VK_MENU => KeyCode::Alt,
            KeyboardAndMouse::VK_LSHIFT => KeyCode::Shift,
            KeyboardAndMouse::VK_RSHIFT => KeyCode::Shift,
            KeyboardAndMouse::VK_SHIFT => KeyCode::Shift,
            KeyboardAndMouse::VK_LWIN => KeyCode::Logo,
            KeyboardAndMouse::VK_RWIN => KeyCode::Logo,
            KeyboardAndMouse::VK_SPACE => KeyCode::Space,
            KeyboardAndMouse::VK_CAPITAL => KeyCode::Capslock,
            KeyboardAndMouse::VK_F1 => KeyCode::F1,
            KeyboardAndMouse::VK_F2 => KeyCode::F2,
            KeyboardAndMouse::VK_F3 => KeyCode::F3,
            KeyboardAndMouse::VK_F4 => KeyCode::F4,
            KeyboardAndMouse::VK_F5 => KeyCode::F5,
            KeyboardAndMouse::VK_F6 => KeyCode::F6,
            KeyboardAndMouse::VK_F7 => KeyCode::F7,
            KeyboardAndMouse::VK_F8 => KeyCode::F8,
            KeyboardAndMouse::VK_F9 => KeyCode::F9,
            KeyboardAndMouse::VK_F10 => KeyCode::F10,
            KeyboardAndMouse::VK_F11 => KeyCode::F11,
            KeyboardAndMouse::VK_F12 => KeyCode::F12,
            KeyboardAndMouse::VK_SNAPSHOT => KeyCode::PrintScreen,
            KeyboardAndMouse::VK_SCROLL => KeyCode::ScrollLock,
            KeyboardAndMouse::VK_PAUSE => KeyCode::Pause,
            KeyboardAndMouse::VK_INSERT => KeyCode::Insert,
            KeyboardAndMouse::VK_DELETE => KeyCode::Delete,
            KeyboardAndMouse::VK_HOME => KeyCode::Home,
            KeyboardAndMouse::VK_END => KeyCode::End,
            KeyboardAndMouse::VK_PRIOR => KeyCode::PageUp,
            KeyboardAndMouse::VK_NEXT => KeyCode::PageDown,
            KeyboardAndMouse::VK_NUMPAD0 => KeyCode::Numpad0,
            KeyboardAndMouse::VK_NUMPAD1 => KeyCode::Numpad1,
            KeyboardAndMouse::VK_NUMPAD2 => KeyCode::Numpad2,
            KeyboardAndMouse::VK_NUMPAD3 => KeyCode::Numpad3,
            KeyboardAndMouse::VK_NUMPAD4 => KeyCode::Numpad4,
            KeyboardAndMouse::VK_NUMPAD5 => KeyCode::Numpad5,
            KeyboardAndMouse::VK_NUMPAD6 => KeyCode::Numpad6,
            KeyboardAndMouse::VK_NUMPAD7 => KeyCode::Numpad7,
            KeyboardAndMouse::VK_NUMPAD8 => KeyCode::Numpad8,
            KeyboardAndMouse::VK_NUMPAD9 => KeyCode::Numpad9,
            //winuser::VK_BACK => KeyCode::NumpadEquals,
            KeyboardAndMouse::VK_SUBTRACT => KeyCode::NumpadSubtract,
            KeyboardAndMouse::VK_ADD => KeyCode::NumpadAdd,
            KeyboardAndMouse::VK_DECIMAL => KeyCode::NumpadDecimal,
            KeyboardAndMouse::VK_MULTIPLY => KeyCode::NumpadMultiply,
            KeyboardAndMouse::VK_DIVIDE => KeyCode::NumpadDivide,
            KeyboardAndMouse::VK_NUMLOCK => KeyCode::Numlock,
            //winuser::VK_BACK => KeyCode::NumpadEnter,
            KeyboardAndMouse::VK_UP => KeyCode::ArrowUp,
            KeyboardAndMouse::VK_DOWN => KeyCode::ArrowDown,
            KeyboardAndMouse::VK_LEFT => KeyCode::ArrowLeft,
            KeyboardAndMouse::VK_RIGHT => KeyCode::ArrowRight,
            _ => KeyCode::Unknown
        }
    }
}

