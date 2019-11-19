use crate::cx::*;
use time::precise_time_ns;
use std::{ptr};
use winapi::um::{libloaderapi, winuser, winbase, dwmapi};
use winapi::shared::minwindef::{LPARAM, LRESULT, DWORD, WPARAM, BOOL, UINT, FALSE};
use winapi::shared::ntdef::{NULL};
use winapi::um::winnt::{LPCWSTR, HRESULT, LPCSTR};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::mem;
use std::os::raw::c_void;
use winapi::shared::basetsd::{UINT_PTR};
use winapi::shared::windef::{RECT, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE, HMONITOR, HWND};
use winapi::shared::winerror::S_OK;
use winapi::um::libloaderapi::{GetProcAddress, LoadLibraryA};
use winapi::um::shellscalingapi::{MDT_EFFECTIVE_DPI, MONITOR_DPI_TYPE, PROCESS_DPI_AWARENESS, PROCESS_PER_MONITOR_DPI_AWARE,};
use winapi::um::wingdi::{GetDeviceCaps, LOGPIXELSX};
use winapi::um::winuser::{MONITOR_DEFAULTTONEAREST, TRACKMOUSEEVENT};
use winapi::um::uxtheme::MARGINS;

static mut GLOBAL_WIN32_APP: *mut Win32App = 0 as *mut _;

pub struct Win32App {
    pub time_start: u64,
    pub event_callback: Option<*mut dyn FnMut(&mut Win32App, &mut Vec<Event>) -> bool>,
    pub event_recur_block: bool,
    pub event_loop_running: bool,
    pub class_name_wstr: Vec<u16>,
    pub all_windows: Vec<HWND>,
    pub timers: Vec<Win32Timer>,
    pub free_timers: Vec<usize>,
    
    pub loop_block: bool,
    pub dpi_functions: DpiFunctions,
    pub current_cursor: MouseCursor,
}

#[derive(Clone)]
pub struct Win32Window {
    pub window_id: usize,
    pub win32_app: *mut Win32App,
    pub last_window_geom: WindowGeom,
    
    pub time_start: u64,
    
    pub last_key_mod: KeyModifiers,
    pub ime_spot: Vec2,
    pub current_cursor: MouseCursor,
    pub last_mouse_pos: Vec2,
    pub fingers_down: Vec<bool>,
    pub ignore_wmsize: usize,
    pub hwnd: Option<HWND>,
    pub track_mouse_event: bool
}

#[derive(Clone)]
pub enum Win32Timer {
    Free,
    Timer {win32_id: UINT_PTR, timer_id: u64, interval: f64, repeats: bool},
    Resize {win32_id: UINT_PTR},
}


impl Win32App {
    pub fn new() -> Win32App {
        
        let class_name_wstr: Vec<u16> = OsStr::new("MakepadWindow").encode_wide().chain(Some(0).into_iter()).collect();
        
        let class = winuser::WNDCLASSEXW {
            cbSize: mem::size_of::<winuser::WNDCLASSEXW>() as UINT,
            style: winuser::CS_HREDRAW | winuser::CS_VREDRAW | winuser::CS_OWNDC,
            lpfnWndProc: Some(Win32Window::window_class_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: unsafe {libloaderapi::GetModuleHandleW(ptr::null())},
            hIcon: unsafe {winuser::LoadIconW(ptr::null_mut(), winuser::IDI_WINLOGO)}, //h_icon,
            hCursor: ptr::null_mut(), //unsafe {winuser::LoadCursorW(ptr::null_mut(), winuser::IDC_ARROW)}, // must be null in order for cursor state to work properly
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name_wstr.as_ptr(),
            hIconSm: ptr::null_mut(),
        };
        
        unsafe {
            winuser::RegisterClassExW(&class);
            winuser::IsGUIThread(1);
        }
        
        let win32_app = Win32App {
            class_name_wstr: class_name_wstr,
            time_start: precise_time_ns(),
            event_callback: None,
            event_recur_block: false,
            event_loop_running: true,
            loop_block: false,
            all_windows: Vec::new(),
            timers: Vec::new(),
            free_timers: Vec::new(),
            dpi_functions: DpiFunctions::new(),
            current_cursor: MouseCursor::Default
        };
        
        win32_app.dpi_functions.become_dpi_aware();
        
        win32_app
    }
    
    pub fn init(&mut self) {
        unsafe {
            GLOBAL_WIN32_APP = self;
        }
    }
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Win32App, &mut Vec<Event>) -> bool,
    {
        unsafe {
            self.event_callback = Some(
                &mut event_handler as *const dyn FnMut(&mut Win32App, &mut Vec<Event>) -> bool
                as *mut dyn FnMut(&mut Win32App, &mut Vec<Event>) -> bool
            );
            
            while self.event_loop_running {
                let mut msg = mem::uninitialized();
                
                if self.loop_block {
                    if winuser::GetMessageW(&mut msg, ptr::null_mut(), 0, 0) == 0 {
                        // Only happens if the message is `WM_QUIT`.
                        debug_assert_eq!(msg.message, winuser::WM_QUIT);
                        self.event_loop_running = false;
                    }
                    else {
                        winuser::TranslateMessage(&msg);
                        winuser::DispatchMessageW(&msg);
                        self.do_callback(&mut vec![Event::Paint]);
                    }
                }
                else {
                    if winuser::PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, 1) == 0 {
                        self.do_callback(&mut vec![Event::Paint])
                    }
                    else {
                        winuser::TranslateMessage(&msg);
                        winuser::DispatchMessageW(&msg);
                    }
                }
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
    
    pub unsafe extern "system" fn timer_proc(_hwnd: HWND, _arg1: UINT, in_win32_id: UINT_PTR, _arg2: DWORD) {
        let win32_app = &mut (*GLOBAL_WIN32_APP);
        let hit_timer = {
            let mut hit_timer = None;
            for slot in 0..win32_app.timers.len() {
                match win32_app.timers[slot] {
                    Win32Timer::Timer {win32_id, repeats, ..} => if win32_id == in_win32_id {
                        hit_timer = Some(win32_app.timers[slot].clone());
                        if !repeats {
                            winuser::KillTimer(NULL as HWND, in_win32_id);
                            win32_app.timers[slot] = Win32Timer::Free;
                            win32_app.free_timers.push(slot);
                        }
                        break;
                    },
                    Win32Timer::Resize {win32_id, ..} => if win32_id == in_win32_id {
                        hit_timer = Some(win32_app.timers[slot].clone());
                        break;
                    },
                    _ => ()
                }
            };
            hit_timer
        };
        // call the dependencies
        if let Some(hit_timer) = hit_timer {
            match hit_timer {
                Win32Timer::Timer {timer_id, ..} => {
                    win32_app.do_callback(&mut vec![Event::Timer(TimerEvent {timer_id: timer_id})]);
                },
                Win32Timer::Resize {..} => {
                    win32_app.do_callback(&mut vec![Event::Paint]);
                },
                _ => ()
            }
        }
    }
    
    pub fn get_free_timer_slot(&mut self) -> usize {
        if self.free_timers.len()>0 {
            self.free_timers.pop().unwrap()
        }
        else {
            let slot = self.timers.len();
            self.timers.push(Win32Timer::Free);
            slot
        }
    }
    
    pub fn start_timer(&mut self, timer_id: u64, interval: f64, repeats: bool) {
        let slot = self.get_free_timer_slot();
        let win32_id = unsafe {winuser::SetTimer(NULL as HWND, 0, (interval * 1000.0) as u32, Some(Self::timer_proc))};
        self.timers[slot] = Win32Timer::Timer {
            timer_id: timer_id,
            win32_id: win32_id,
            interval: interval,
            repeats: repeats
        };
    }
    
    pub fn stop_timer(&mut self, which_timer_id: u64) {
        for slot in 0..self.timers.len() {
            if let Win32Timer::Timer {win32_id, timer_id, ..} = self.timers[slot] {
                if timer_id == which_timer_id {
                    self.timers[slot] = Win32Timer::Free;
                    self.free_timers.push(slot);
                    unsafe {winuser::KillTimer(NULL as HWND, win32_id);}
                }
            }
        }
    }
    
    pub fn start_resize(&mut self) {
        let slot = self.get_free_timer_slot();
        let win32_id = unsafe {winuser::SetTimer(NULL as HWND, 0, 8 as u32, Some(Self::timer_proc))};
        self.timers[slot] = Win32Timer::Resize {win32_id: win32_id};
    }
    
    pub fn stop_resize(&mut self) {
        for slot in 0..self.timers.len() {
            if let Win32Timer::Resize {win32_id} = self.timers[slot] {
                self.timers[slot] = Win32Timer::Free;
                self.free_timers.push(slot);
                unsafe {winuser::KillTimer(NULL as HWND, win32_id);}
            }
        }
    }
    
    pub fn post_signal(signal_id: usize, value: usize) {
        unsafe {
            let win32_app = &mut (*GLOBAL_WIN32_APP);
            if win32_app.all_windows.len()>0 {
                winuser::PostMessageW(win32_app.all_windows[0], winuser::WM_USER, signal_id as usize, value as isize);
            }
        }
    }
    
    pub fn terminate_event_loop(&mut self) {
        unsafe {
            if self.all_windows.len()>0 {
                winuser::PostMessageW(self.all_windows[0], winuser::WM_QUIT, 0, 0);
            }
        }
        self.event_loop_running = false;
    }
    
    pub fn time_now(&self) -> f64 {
        let time_now = precise_time_ns();
        (time_now - self.time_start) as f64 / 1_000_000_000.0
    }
    
    
    pub fn set_mouse_cursor(&mut self, cursor: MouseCursor) {
        if self.current_cursor != cursor {
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
                MouseCursor::NotAllowed => winuser::IDC_NO,

                MouseCursor::EResize =>  winuser::IDC_SIZEWE,
                MouseCursor::NResize => winuser::IDC_SIZENS,
                MouseCursor::NeResize => winuser::IDC_SIZENESW,
                MouseCursor::NwResize => winuser::IDC_SIZENWSE,
                MouseCursor::SResize => winuser::IDC_SIZENS,
                MouseCursor::SeResize => winuser::IDC_SIZENWSE,
                MouseCursor::SwResize => winuser::IDC_SIZENESW,
                MouseCursor::WResize =>  winuser::IDC_SIZEWE,
                
                
                MouseCursor::NsResize => winuser::IDC_SIZENS,
                MouseCursor::NeswResize => winuser::IDC_SIZENESW,
                MouseCursor::EwResize => winuser::IDC_SIZEWE,
                MouseCursor::NwseResize => winuser::IDC_SIZENWSE,
                
                MouseCursor::ColResize => winuser::IDC_SIZEWE,
                MouseCursor::RowResize => winuser::IDC_SIZENS,
            };
            self.current_cursor = cursor;
            unsafe {
                if win32_cursor == ptr::null() {
                    winuser::ShowCursor(0);
                }
                else {
                    winuser::SetCursor(winuser::LoadCursorW(ptr::null_mut(), win32_cursor));
                    winuser::ShowCursor(1);
                }
            }
            //TODO
        }
    }
}

impl Win32Window {
    
    pub fn new(win32_app: &mut Win32App, window_id: usize) -> Win32Window {
        let mut fingers_down = Vec::new();
        fingers_down.resize(NUM_FINGERS, false);
        
        Win32Window {
            window_id: window_id,
            win32_app: win32_app,
            last_window_geom: WindowGeom::default(),
            time_start: win32_app.time_start,
            last_key_mod: KeyModifiers::default(),
            ime_spot: Vec2::zero(),
            current_cursor: MouseCursor::Default,
            last_mouse_pos: Vec2::zero(),
            fingers_down: fingers_down,
            ignore_wmsize: 0,
            hwnd: None,
            track_mouse_event: false
        }
    }
    
    pub fn init(&mut self, title: &str, size: Vec2, position: Option<Vec2>) {
        
        let style = winuser::WS_SIZEBOX | winuser::WS_MAXIMIZEBOX | winuser::WS_MINIMIZEBOX | winuser::WS_POPUP
            | winuser::WS_CLIPSIBLINGS | winuser::WS_CLIPCHILDREN | winuser::WS_SYSMENU;
        
        let style_ex = winuser::WS_EX_WINDOWEDGE | winuser::WS_EX_APPWINDOW | winuser::WS_EX_ACCEPTFILES;
        
        unsafe {
            let title_wstr: Vec<_> = OsStr::new(title).encode_wide().chain(Some(0).into_iter()).collect();
            
            let (x, y) = if let Some(position) = position {
                (position.x as i32, position.y as i32)
            }
            else {
                (winuser::CW_USEDEFAULT, winuser::CW_USEDEFAULT)
            };
            
            let hwnd = winuser::CreateWindowExW(
                style_ex,
                (*self.win32_app).class_name_wstr.as_ptr(),
                title_wstr.as_ptr() as LPCWSTR,
                style,
                x,
                y,
                winuser::CW_USEDEFAULT,
                winuser::CW_USEDEFAULT,
                ptr::null_mut(),
                ptr::null_mut(),
                libloaderapi::GetModuleHandleW(ptr::null()),
                ptr::null_mut(),
            );
            
            self.hwnd = Some(hwnd);
            
            // TODO remove it as well
            (*self.win32_app).all_windows.push(hwnd);
            
            winuser::SetWindowLongPtrW(hwnd, winuser::GWLP_USERDATA, self as *const _ as isize);
            
            self.set_outer_size(size);
            
            winuser::ShowWindow(hwnd, winuser::SW_SHOW);
            
            (*self.win32_app).dpi_functions.enable_non_client_dpi_scaling(self.hwnd.unwrap());
        }
    }
    
    pub unsafe extern "system" fn window_class_proc(hwnd: HWND, msg: UINT, wparam: WPARAM, lparam: LPARAM,) -> LRESULT {
        
        let user_data = winuser::GetWindowLongPtrW(hwnd, winuser::GWLP_USERDATA);
        if user_data == 0 {
            return winuser::DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        
        let window = &mut (*(user_data as *mut Win32Window));
        match msg {
            winuser::WM_ACTIVATE=>{
                if wparam&0xffff == winuser::WA_ACTIVE as usize{
                     window.do_callback(&mut vec![Event::AppFocus]);
                }
                else{
                     window.do_callback(&mut vec![Event::AppFocusLost]);
                }
            },
            winuser::WM_NCCALCSIZE => {
                // check if we are maximised
                if window.get_is_maximized() {
                    return winuser::DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                if wparam == 1 {
                    let margins = MARGINS {
                        cxLeftWidth: 0,
                        cxRightWidth: 0,
                        cyTopHeight: 0,
                        cyBottomHeight: 1
                    };
                    dwmapi::DwmExtendFrameIntoClientArea(hwnd, &margins);
                    return 0
                }
            },
            winuser::WM_NCHITTEST => {
                let ycoord = (lparam >> 16) as u16 as i16 as i32;
                let xcoord = (lparam & 0xffff) as u16 as i16 as i32;
                let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
                const EDGE: i32 = 8;
                winuser::GetWindowRect(hwnd, &mut rect);
                if xcoord < rect.left + EDGE {
                    (*window.win32_app).current_cursor = MouseCursor::Hidden;
                    if ycoord < rect.top + EDGE {
                        window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NwseResize)]);
                        return winuser::HTTOPLEFT;
                    }
                    if ycoord > rect.bottom - EDGE {
                        window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NeswResize)]);
                        return winuser::HTBOTTOMLEFT;
                    }
                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::EwResize)]);
                    return winuser::HTLEFT;
                }
                if xcoord > rect.right - EDGE {
                    (*window.win32_app).current_cursor = MouseCursor::Hidden;
                    if ycoord < rect.top + EDGE {
                        window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NeswResize)]);
                        return winuser::HTTOPRIGHT;
                    }
                    if ycoord > rect.bottom - EDGE {
                        window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NwseResize)]);
                        return winuser::HTBOTTOMRIGHT;
                    }
                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::EwResize)]);
                    return winuser::HTRIGHT;
                }
                if ycoord < rect.top + EDGE {
                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NsResize)]);
                    return winuser::HTTOP;
                }
                if ycoord > rect.bottom - EDGE {
                    window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::NsResize)]);
                    return winuser::HTBOTTOM;
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
                            return winuser::HTCLIENT
                        }
                        WindowDragQueryResponse::Caption => {
                            window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::Default)]);
                            return winuser::HTCAPTION
                        },
                        WindowDragQueryResponse::SysMenu => {
                            window.do_callback(&mut vec![Event::WindowSetHoverCursor(MouseCursor::Default)]);
                            return winuser::HTSYSMENU
                        }
                        _ => ()
                    },
                    _ => ()
                }
                if ycoord < rect.top + 50 && xcoord < rect.left + 50 {
                    return winuser::HTSYSMENU;
                }
                if ycoord < rect.top + 50 && xcoord < rect.right - 300 {
                    return winuser::HTCAPTION;
                }
                return winuser::HTCLIENT;
            },
            winuser::WM_ERASEBKGND => {
                return 1
            },
            winuser::WM_MOUSEMOVE => {
                if !window.track_mouse_event {
                    window.track_mouse_event = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: winuser::TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0
                    };
                    winuser::TrackMouseEvent(&mut tme);
                }
                window.send_finger_hover_and_move(
                    window.get_mouse_pos_from_lparam(lparam),
                    Self::get_key_modifiers()
                )
            },
            winuser::WM_MOUSELEAVE => {
                window.track_mouse_event = false;
                window.do_callback(&mut vec![Event::FingerHover(FingerHoverEvent {
                    window_id: window.window_id,
                    any_down: false,
                    abs: window.last_mouse_pos,
                    rel: window.last_mouse_pos,
                    rect: Rect::zero(),
                    handled: false,
                    hover_state: HoverState::Out,
                    modifiers: Self::get_key_modifiers(),
                    time: window.time_now()
                })]);
                (*window.win32_app).current_cursor = MouseCursor::Hidden;
            },
            winuser::WM_MOUSEWHEEL => {
                let delta = (wparam>>16) as u16 as i16 as f32;
                window.do_callback(&mut vec![
                    Event::FingerScroll(FingerScrollEvent {
                        window_id: window.window_id,
                        scroll: Vec2 {
                            x: 0.0,
                            y: delta
                        },
                        abs: window.last_mouse_pos,
                        rel: window.last_mouse_pos,
                        rect: Rect::zero(),
                        is_wheel: true,
                        modifiers: Self::get_key_modifiers(),
                        handled: false,
                        time: window.time_now()
                    })
                ]);
            },
            winuser::WM_LBUTTONDOWN => window.send_finger_down(0, Self::get_key_modifiers()),
            winuser::WM_LBUTTONUP => window.send_finger_up(0, Self::get_key_modifiers()),
            winuser::WM_RBUTTONDOWN => window.send_finger_down(1, Self::get_key_modifiers()),
            winuser::WM_RBUTTONUP => window.send_finger_up(1, Self::get_key_modifiers()),
            winuser::WM_MBUTTONDOWN => window.send_finger_down(2, Self::get_key_modifiers()),
            winuser::WM_MBUTTONUP => window.send_finger_up(2, Self::get_key_modifiers()),
            winuser::WM_KEYDOWN | winuser::WM_SYSKEYDOWN => {
                // detect control/cmd - c / v / x
                let modifiers = Self::get_key_modifiers();
                let key_code = Self::virtual_key_to_key_code(wparam);
                if modifiers.alt && key_code == KeyCode::F4 {
                    winuser::PostMessageW(hwnd, winuser::WM_CLOSE, 0, 0);
                }
                if modifiers.control || modifiers.logo {
                    match key_code {
                        KeyCode::KeyV => { // paste
                            if winuser::OpenClipboard(ptr::null_mut()) != 0 {
                                let mut data: Vec<u16> = Vec::new();
                                let h_clipboard_data = winuser::GetClipboardData(winuser::CF_UNICODETEXT);
                                let h_clipboard_ptr = winbase::GlobalLock(h_clipboard_data) as *mut u16;
                                let clipboard_size = winbase::GlobalSize(h_clipboard_data);
                                if clipboard_size > 2 {
                                    data.resize((clipboard_size>>1) - 1, 0);
                                    std::ptr::copy_nonoverlapping(h_clipboard_ptr, data.as_mut_ptr(), data.len());
                                    winbase::GlobalUnlock(h_clipboard_data);
                                    winuser::CloseClipboard();
                                    if let Ok(utf8) = String::from_utf16(&data) {
                                        window.do_callback(&mut vec![
                                            Event::TextInput(TextInputEvent {
                                                input: utf8,
                                                was_paste: true,
                                                replace_last: false
                                            })
                                        ]);
                                    }
                                }
                                else {
                                    winbase::GlobalUnlock(h_clipboard_data);
                                    winuser::CloseClipboard();
                                }
                            }
                        }
                        KeyCode::KeyX | KeyCode::KeyC => {
                            let mut events = vec![
                                Event::TextCopy(TextCopyEvent {
                                    response: None
                                })
                            ];
                            window.do_callback(&mut events);
                            match &events[0] {
                                Event::TextCopy(req) => if let Some(response) = &req.response {
                                    // plug it into the windows clipboard
                                    // make utf16 dta
                                    if winuser::OpenClipboard(ptr::null_mut()) != 0 {
                                        winuser::EmptyClipboard();
                                        
                                        let data: Vec<u16> = OsStr::new(response).encode_wide().chain(Some(0).into_iter()).collect();
                                        
                                        let h_clipboard_data = winbase::GlobalAlloc(winbase::GMEM_DDESHARE, 2 * data.len());
                                        
                                        let h_clipboard_ptr = winbase::GlobalLock(h_clipboard_data) as *mut u16;
                                        
                                        std::ptr::copy_nonoverlapping(data.as_ptr(), h_clipboard_ptr, data.len());
                                        
                                        winbase::GlobalUnlock(h_clipboard_data);
                                        winuser::SetClipboardData(winuser::CF_UNICODETEXT, h_clipboard_data);
                                        winuser::CloseClipboard();
                                    }
                                    
                                },
                                _ => ()
                            };
                        }
                        _ => ()
                    }
                }
                window.do_callback(&mut vec![
                    Event::KeyDown(KeyEvent {
                        key_code: key_code,
                        is_repeat: lparam & 0x7fff>0,
                        modifiers: modifiers,
                        time: window.time_now()
                    })
                ]);
            },
            winuser::WM_KEYUP | winuser::WM_SYSKEYUP => {
                window.do_callback(&mut vec![
                    Event::KeyUp(KeyEvent {
                        key_code: Self::virtual_key_to_key_code(wparam),
                        is_repeat: lparam & 0x7fff>0,
                        modifiers: Self::get_key_modifiers(),
                        time: window.time_now()
                    })
                ]);
                
            },
            winuser::WM_CHAR => {
                if let Ok(utf8) = String::from_utf16(&[wparam as u16]) {
                    let char_code = utf8.chars().next().unwrap();
                    if char_code >= ' ' {
                        window.do_callback(&mut vec![
                            Event::TextInput(TextInputEvent {
                                input: utf8,
                                was_paste: false,
                                replace_last: false
                            })
                        ]);
                    }
                }
            },
            winuser::WM_ENTERSIZEMOVE => {
                (*window.win32_app).start_resize();
                window.do_callback(&mut vec![Event::WindowResizeLoop(WindowResizeLoopEvent {
                    was_started: true,
                    window_id: window.window_id
                })]);
            }
            winuser::WM_EXITSIZEMOVE => {
                (*window.win32_app).stop_resize();
                window.do_callback(&mut vec![Event::WindowResizeLoop(WindowResizeLoopEvent {
                    was_started: false,
                    window_id: window.window_id
                })]);
            },
            winuser::WM_SIZE | winuser::WM_DPICHANGED => {
                //if window.ignore_wmsize > 1{
                window.send_change_event();
                // }
                //else{
                //    window.ignore_wmsize += 1;
                // }
            },
            winuser::WM_USER => {
                window.do_callback(&mut vec![
                    Event::Signal(SignalEvent {
                        signal_id: wparam as usize,
                        value: lparam as usize
                    })
                ]);
            },
            winuser::WM_CLOSE => { // close requested
                let mut events = vec![Event::WindowCloseRequested(WindowCloseRequestedEvent {
                    window_id: window.window_id,
                    accept_close: true
                })];
                window.do_callback(&mut events);
                if let Event::WindowCloseRequested(cre) = &events[0] {
                    if cre.accept_close {
                        winuser::DestroyWindow(hwnd);
                    }
                }
            },
            winuser::WM_DESTROY => { // window actively destroyed
                (*window.win32_app).event_recur_block = false; //exception case
                window.do_callback(&mut vec![
                    Event::WindowClosed(WindowClosedEvent {
                        window_id: window.window_id,
                    })
                ]);
            },
            _ => {
                return winuser::DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        return 1
        // lets get the window
        // Unwinding into foreign code is undefined behavior. So we catch any panics that occur in our
        // code, and if a panic happens we cancel any future operations.
        //run_catch_panic(-1, || callback_inner(window, msg, wparam, lparam))
    }
    
    pub fn get_mouse_pos_from_lparam(&self, lparam: LPARAM) -> Vec2 {
        let dpi = self.get_dpi_factor();
        let ycoord = (lparam >> 16) as u16 as i16 as f32;
        let xcoord = (lparam & 0xffff) as u16 as i16 as f32;
        Vec2 {x: xcoord / dpi, y: ycoord / dpi}
    }
    
    pub fn get_key_modifiers() -> KeyModifiers {
        unsafe {
            KeyModifiers {
                control: winuser::GetKeyState(winuser::VK_CONTROL) & 0x80>0,
                shift: winuser::GetKeyState(winuser::VK_SHIFT) & 0x80>0,
                alt: winuser::GetKeyState(winuser::VK_MENU) & 0x80>0,
                logo: winuser::GetKeyState(winuser::VK_LWIN) & 0x80>0
                    || winuser::GetKeyState(winuser::VK_RWIN) & 0x80>0,
            }
        }
    }
    
    pub fn update_ptrs(&mut self) {
        unsafe {
            winuser::SetWindowLongPtrW(self.hwnd.unwrap(), winuser::GWLP_USERDATA, self as *const _ as isize);
        }
    }
    
    pub fn on_mouse_move(&self) {
    }
    
    
    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {
    }
    
    pub fn restore(&self) {
        unsafe {
            winuser::ShowWindow(self.hwnd.unwrap(), winuser::SW_RESTORE);
            winuser::PostMessageW(self.hwnd.unwrap(), winuser::WM_SIZE, 0, 0);
        }
    }
    
    pub fn maximize(&self) {
        unsafe {
            winuser::ShowWindow(self.hwnd.unwrap(), winuser::SW_MAXIMIZE);
            winuser::PostMessageW(self.hwnd.unwrap(), winuser::WM_SIZE, 0, 0);
        }
    }
    
    pub fn close_window(&self) {
        unsafe {
            winuser::DestroyWindow(self.hwnd.unwrap());
        }
    }
    
    pub fn minimize(&self) {
        unsafe {
            winuser::ShowWindow(self.hwnd.unwrap(), winuser::SW_MINIMIZE);
        }
    }
    
    pub fn set_topmost(&self, topmost: bool) {
        unsafe {
            if topmost {
                winuser::SetWindowPos(
                    self.hwnd.unwrap(),
                    winuser::HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    winuser::SWP_NOMOVE | winuser::SWP_NOSIZE
                );
            }
            else {
                winuser::SetWindowPos(
                    self.hwnd.unwrap(),
                    winuser::HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    winuser::SWP_NOMOVE | winuser::SWP_NOSIZE
                );
            }
        }
    }
    
    pub fn get_is_topmost(&self) -> bool {
        unsafe {
            let ex_style = winuser::GetWindowLongW(self.hwnd.unwrap(), winuser::GWL_EXSTYLE) as u32;
            if (ex_style & winuser::WS_EX_TOPMOST) != 0 {
                return true
            }
            return false
        }
    }
    
    pub fn get_window_geom(&self) -> WindowGeom {
        WindowGeom {
            vr_is_presenting: false,
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
            let mut wp: winuser::WINDOWPLACEMENT = mem::uninitialized();
            wp.length = mem::size_of::<winuser::WINDOWPLACEMENT>() as u32;
            winuser::GetWindowPlacement(self.hwnd.unwrap(), &mut wp);
            if wp.showCmd as i32 == winuser::SW_MAXIMIZE {
                return true
            }
            return false
        }
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
            let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            winuser::GetWindowRect(self.hwnd.unwrap(), &mut rect);
            Vec2 {x: rect.left as f32, y: rect.top as f32}
        }
    }
    
    pub fn get_inner_size(&self) -> Vec2 {
        unsafe {
            let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            winuser::GetClientRect(self.hwnd.unwrap(), &mut rect);
            let dpi = self.get_dpi_factor();
            Vec2 {x: (rect.right - rect.left) as f32 / dpi, y: (rect.bottom - rect.top)as f32 / dpi}
        }
    }
    
    pub fn get_outer_size(&self) -> Vec2 {
        unsafe {
            let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            winuser::GetWindowRect(self.hwnd.unwrap(), &mut rect);
            Vec2 {x: (rect.right - rect.left) as f32, y: (rect.bottom - rect.top)as f32}
        }
    }
    
    pub fn set_position(&mut self, pos: Vec2) {
        unsafe {
            let mut window_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            winuser::GetWindowRect(self.hwnd.unwrap(), &mut window_rect);
            let dpi = self.get_dpi_factor();
            winuser::MoveWindow(
                self.hwnd.unwrap(),
                (pos.x * dpi) as i32,
                (pos.y * dpi) as i32,
                window_rect.right - window_rect.left,
                window_rect.bottom - window_rect.top,
                FALSE
            );
        }
    }
    
    pub fn set_outer_size(&self, size: Vec2) {
        unsafe {
            let mut window_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            winuser::GetWindowRect(self.hwnd.unwrap(), &mut window_rect);
            let dpi = self.get_dpi_factor();
            winuser::MoveWindow(
                self.hwnd.unwrap(),
                window_rect.left,
                window_rect.top,
                (size.x * dpi) as i32,
                (size.y * dpi) as i32,
                FALSE
            );
        }
    }
    
    pub fn set_inner_size(&self, size: Vec2) {
        unsafe {
            let mut window_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            winuser::GetWindowRect(self.hwnd.unwrap(), &mut window_rect);
            let mut client_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            winuser::GetClientRect(self.hwnd.unwrap(), &mut client_rect);
            let dpi = self.get_dpi_factor();
            winuser::MoveWindow(
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
    
    pub fn get_dpi_factor(&self) -> f32 {
        unsafe {
            (*self.win32_app).dpi_functions.hwnd_dpi_factor(self.hwnd.unwrap())
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Event>) {
        unsafe {
            (*self.win32_app).do_callback(events);
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
            unsafe {winuser::SetCapture(self.hwnd.unwrap());}
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
            unsafe {winuser::ReleaseCapture();}
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
    
    pub fn virtual_key_to_key_code(wparam: WPARAM) -> KeyCode {
        match wparam as i32 {
            winuser::VK_ESCAPE => KeyCode::Escape,
            winuser::VK_OEM_3 => KeyCode::Backtick,
            0x30 => KeyCode::Key0,
            0x31 => KeyCode::Key1,
            0x32 => KeyCode::Key2,
            0x33 => KeyCode::Key3,
            0x34 => KeyCode::Key4,
            0x35 => KeyCode::Key5,
            0x36 => KeyCode::Key6,
            0x37 => KeyCode::Key7,
            0x38 => KeyCode::Key8,
            0x39 => KeyCode::Key9,
            winuser::VK_OEM_MINUS => KeyCode::Minus,
            winuser::VK_OEM_PLUS => KeyCode::Equals,
            winuser::VK_BACK => KeyCode::Backspace,
            winuser::VK_TAB => KeyCode::Tab,
            0x51 => KeyCode::KeyQ,
            0x57 => KeyCode::KeyW,
            0x45 => KeyCode::KeyE,
            0x52 => KeyCode::KeyR,
            0x54 => KeyCode::KeyT,
            0x59 => KeyCode::KeyY,
            0x55 => KeyCode::KeyU,
            0x49 => KeyCode::KeyI,
            0x4f => KeyCode::KeyO,
            0x50 => KeyCode::KeyP,
            winuser::VK_OEM_4 => KeyCode::LBracket,
            winuser::VK_OEM_6 => KeyCode::RBracket,
            winuser::VK_RETURN => KeyCode::Return,
            0x41 => KeyCode::KeyA,
            0x53 => KeyCode::KeyS,
            0x44 => KeyCode::KeyD,
            0x46 => KeyCode::KeyF,
            0x47 => KeyCode::KeyG,
            0x48 => KeyCode::KeyH,
            0x4a => KeyCode::KeyJ,
            0x4b => KeyCode::KeyK,
            0x4c => KeyCode::KeyL,
            winuser::VK_OEM_1 => KeyCode::Semicolon,
            winuser::VK_OEM_7 => KeyCode::Quote,
            winuser::VK_OEM_5 => KeyCode::Backslash,
            0x5a => KeyCode::KeyZ,
            0x58 => KeyCode::KeyX,
            0x43 => KeyCode::KeyC,
            0x56 => KeyCode::KeyV,
            0x42 => KeyCode::KeyB,
            0x4e => KeyCode::KeyN,
            0x4d => KeyCode::KeyM,
            winuser::VK_OEM_COMMA => KeyCode::Comma,
            winuser::VK_OEM_PERIOD => KeyCode::Period,
            winuser::VK_OEM_2 => KeyCode::Slash,
            winuser::VK_LCONTROL => KeyCode::Control,
            winuser::VK_RCONTROL => KeyCode::Control,
            winuser::VK_CONTROL => KeyCode::Control,
            winuser::VK_LMENU => KeyCode::Alt,
            winuser::VK_RMENU => KeyCode::Alt,
            winuser::VK_MENU => KeyCode::Alt,
            winuser::VK_LSHIFT => KeyCode::Shift,
            winuser::VK_RSHIFT => KeyCode::Shift,
            winuser::VK_SHIFT => KeyCode::Shift,
            winuser::VK_LWIN => KeyCode::Logo,
            winuser::VK_RWIN => KeyCode::Logo,
            winuser::VK_SPACE => KeyCode::Space,
            winuser::VK_CAPITAL => KeyCode::Capslock,
            winuser::VK_F1 => KeyCode::F1,
            winuser::VK_F2 => KeyCode::F2,
            winuser::VK_F3 => KeyCode::F3,
            winuser::VK_F4 => KeyCode::F4,
            winuser::VK_F5 => KeyCode::F5,
            winuser::VK_F6 => KeyCode::F6,
            winuser::VK_F7 => KeyCode::F7,
            winuser::VK_F8 => KeyCode::F8,
            winuser::VK_F9 => KeyCode::F9,
            winuser::VK_F10 => KeyCode::F10,
            winuser::VK_F11 => KeyCode::F11,
            winuser::VK_F12 => KeyCode::F12,
            winuser::VK_SNAPSHOT => KeyCode::PrintScreen,
            winuser::VK_SCROLL => KeyCode::Scrolllock,
            winuser::VK_PAUSE => KeyCode::Pause,
            winuser::VK_INSERT => KeyCode::Insert,
            winuser::VK_DELETE => KeyCode::Delete,
            winuser::VK_HOME => KeyCode::Home,
            winuser::VK_END => KeyCode::End,
            winuser::VK_PRIOR => KeyCode::PageUp,
            winuser::VK_NEXT => KeyCode::PageDown,
            winuser::VK_NUMPAD0 => KeyCode::Numpad0,
            winuser::VK_NUMPAD1 => KeyCode::Numpad1,
            winuser::VK_NUMPAD2 => KeyCode::Numpad2,
            winuser::VK_NUMPAD3 => KeyCode::Numpad3,
            winuser::VK_NUMPAD4 => KeyCode::Numpad4,
            winuser::VK_NUMPAD5 => KeyCode::Numpad5,
            winuser::VK_NUMPAD6 => KeyCode::Numpad6,
            winuser::VK_NUMPAD7 => KeyCode::Numpad7,
            winuser::VK_NUMPAD8 => KeyCode::Numpad8,
            winuser::VK_NUMPAD9 => KeyCode::Numpad9,
            //winuser::VK_BACK => KeyCode::NumpadEquals,
            winuser::VK_SUBTRACT => KeyCode::NumpadSubtract,
            winuser::VK_ADD => KeyCode::NumpadAdd,
            winuser::VK_DECIMAL => KeyCode::NumpadDecimal,
            winuser::VK_MULTIPLY => KeyCode::NumpadMultiply,
            winuser::VK_DIVIDE => KeyCode::NumpadDivide,
            winuser::VK_NUMLOCK => KeyCode::Numlock,
            //winuser::VK_BACK => KeyCode::NumpadEnter,
            winuser::VK_UP => KeyCode::ArrowUp,
            winuser::VK_DOWN => KeyCode::ArrowDown,
            winuser::VK_LEFT => KeyCode::ArrowLeft,
            winuser::VK_RIGHT => KeyCode::ArrowRight,
            _ => KeyCode::Unknown
        }
    }
}

// reworked from winit windows platform https://github.com/rust-windowing/winit/blob/eventloop-2.0/src/platform_impl/windows/dpi.rs

const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: DPI_AWARENESS_CONTEXT = -4isize as _;
type SetProcessDPIAware = unsafe extern "system" fn () -> BOOL;
type SetProcessDpiAwareness = unsafe extern "system" fn (value: PROCESS_DPI_AWARENESS,) -> HRESULT;
type SetProcessDpiAwarenessContext = unsafe extern "system" fn (value: DPI_AWARENESS_CONTEXT,) -> BOOL;
type GetDpiForWindow = unsafe extern "system" fn (hwnd: HWND) -> UINT;
type GetDpiForMonitor = unsafe extern "system" fn (hmonitor: HMONITOR, dpi_type: MONITOR_DPI_TYPE, dpi_x: *mut UINT, dpi_y: *mut UINT,) -> HRESULT;
type EnableNonClientDpiScaling = unsafe extern "system" fn (hwnd: HWND) -> BOOL;

// Helper function to dynamically load function pointer.
// `library` and `function` must be zero-terminated.
fn get_function_impl(library: &str, function: &str) -> Option<*const c_void> {
    // Library names we will use are ASCII so we can use the A version to avoid string conversion.
    let module = unsafe {LoadLibraryA(library.as_ptr() as LPCSTR)};
    if module.is_null() {
        return None;
    }
    
    let function_ptr = unsafe {GetProcAddress(module, function.as_ptr() as LPCSTR)};
    if function_ptr.is_null() {
        return None;
    }
    
    Some(function_ptr as _)
}

macro_rules!get_function {
    ( $ lib: expr, $ func: ident) => {
        get_function_impl(concat!( $ lib, '\0'), concat!(stringify!( $ func), '\0'))
            .map( | f | unsafe {mem::transmute::<*const _, $ func>(f)})
    }
}

pub struct DpiFunctions {
    get_dpi_for_window: Option<GetDpiForWindow>,
    get_dpi_for_monitor: Option<GetDpiForMonitor>,
    enable_nonclient_dpi_scaling: Option<EnableNonClientDpiScaling>,
    set_process_dpi_awareness_context: Option<SetProcessDpiAwarenessContext>,
    set_process_dpi_awareness: Option<SetProcessDpiAwareness>,
    set_process_dpi_aware: Option<SetProcessDPIAware>
}

const BASE_DPI: u32 = 96;

impl DpiFunctions {
    fn new() -> DpiFunctions {
        DpiFunctions {
            get_dpi_for_window: get_function!("user32.dll", GetDpiForWindow),
            get_dpi_for_monitor: get_function!("shcore.dll", GetDpiForMonitor),
            enable_nonclient_dpi_scaling: get_function!("user32.dll", EnableNonClientDpiScaling),
            set_process_dpi_awareness_context: get_function!("user32.dll", SetProcessDpiAwarenessContext),
            set_process_dpi_awareness: get_function!("shcore.dll", SetProcessDpiAwareness),
            set_process_dpi_aware: get_function!("user32.dll", SetProcessDPIAware)
        }
    }
    
    fn become_dpi_aware(&self) {
        unsafe {
            if let Some(set_process_dpi_awareness_context) = self.set_process_dpi_awareness_context {
                // We are on Windows 10 Anniversary Update (1607) or later.
                if set_process_dpi_awareness_context(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == FALSE {
                    // V2 only works with Windows 10 Creators Update (1703). Try using the older
                    // V1 if we can't set V2.
                    set_process_dpi_awareness_context(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE);
                }
            }
            else if let Some(set_process_dpi_awareness) = self.set_process_dpi_awareness {
                // We are on Windows 8.1 or later.
                set_process_dpi_awareness(PROCESS_PER_MONITOR_DPI_AWARE);
            }
            else if let Some(set_process_dpi_aware) = self.set_process_dpi_aware {
                // We are on Vista or later.
                set_process_dpi_aware();
            }
        }
    }
    
    pub fn enable_non_client_dpi_scaling(&self, hwnd: HWND) {
        unsafe {
            if let Some(enable_nonclient_dpi_scaling) = self.enable_nonclient_dpi_scaling {
                enable_nonclient_dpi_scaling(hwnd);
            }
        }
    }
    /*
    pub fn get_monitor_dpi(hmonitor: HMONITOR) -> Option<u32> {
        unsafe {
            if let Some(GetDpiForMonitor) = *GET_DPI_FOR_MONITOR {
                // We are on Windows 8.1 or later.
                let mut dpi_x = 0;
                let mut dpi_y = 0;
                if GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) == S_OK {
                    // MSDN says that "the values of *dpiX and *dpiY are identical. You only need to
                    // record one of the values to determine the DPI and respond appropriately".
                    // https://msdn.microsoft.com/en-us/library/windows/desktop/dn280510(v=vs.85).aspx
                    return Some(dpi_x as u32)
                }
            }
        }
        None
    }*/
    
    pub fn hwnd_dpi_factor(&self, hwnd: HWND) -> f32 {
        unsafe {
            let hdc = winuser::GetDC(hwnd);
            if hdc.is_null() {
                panic!("`GetDC` returned null!");
            }
            let dpi = if let Some(get_dpi_for_window) = self.get_dpi_for_window {
                // We are on Windows 10 Anniversary Update (1607) or later.
                match get_dpi_for_window(hwnd) {
                    0 => BASE_DPI, // 0 is returned if hwnd is invalid
                    dpi => dpi as u32,
                }
            }
            else if let Some(get_dpi_for_monitor) = self.get_dpi_for_monitor {
                // We are on Windows 8.1 or later.
                let monitor = winuser::MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                if monitor.is_null() {
                    BASE_DPI
                }
                else {
                    let mut dpi_x = 0;
                    let mut dpi_y = 0;
                    if get_dpi_for_monitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) == S_OK {
                        dpi_x as u32
                    } else {
                        BASE_DPI
                    }
                }
            }
            else {
                // We are on Vista or later.
                if winuser::IsProcessDPIAware() != FALSE {
                    // If the process is DPI aware, then scaling must be handled by the application using
                    // this DPI value.
                    GetDeviceCaps(hdc, LOGPIXELSX) as u32
                } else {
                    // If the process is DPI unaware, then scaling is performed by the OS; we thus return
                    // 96 (scale factor 1.0) to prevent the window from being re-scaled by both the
                    // application and the WM.
                    BASE_DPI
                }
            };
            dpi as f32 / BASE_DPI as f32
        }
    }
    
}
