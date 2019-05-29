use crate::cx::*;
use time::precise_time_ns;
use std::{ptr};
use winapi::um::{libloaderapi, winuser};
use winapi::shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::windef::{HWND};
use winapi::um::winnt:: {LPCWSTR};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::mem;

#[derive(Clone, Default)]
pub struct WindowsWindow {
    pub last_window_geom: WindowGeom,
    
    pub time_start: u64,
    pub last_key_mod: KeyModifiers,
    pub ime_spot: Vec2,
    
    pub current_cursor: MouseCursor,
    pub last_mouse_pos: Vec2,
    pub fingers_down: Vec<bool>,
    pub hwnd: Option<HWND>,
    pub event_callback: Option<*mut FnMut(&mut Vec<Event>)>
}

impl WindowsWindow {
    
    pub unsafe extern "system" fn window_proc(hwnd: HWND, msg: UINT, wparam: WPARAM, lparam: LPARAM,) -> LRESULT {
        
        let user_data = winuser::GetWindowLongPtrW(hwnd, winuser::GWLP_USERDATA);
        if user_data == 0 {
            return winuser::DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        
        let window = &(*(user_data as *mut WindowsWindow));
        match msg {
            winuser::WM_MOUSEMOVE => {
                window.on_mouse_move();
            },
            _ => ()
        }
        // lets get the window
        // Unwinding into foreign code is undefined behavior. So we catch any panics that occur in our
        // code, and if a panic happens we cancel any future operations.
        //run_catch_panic(-1, || callback_inner(window, msg, wparam, lparam))
        return winuser::DefWindowProcW(hwnd, msg, wparam, lparam)
    }
    
    pub fn on_mouse_move(&self) {
        println!("WE IZ MOUSEMOVING");
    }
    
    pub fn init(&mut self, title: &str) {
        self.time_start = precise_time_ns();
        for _i in 0..10 {
            self.fingers_down.push(false);
        }
        
        let class_name_wstr: Vec<_> = OsStr::new("MakepadWindow").encode_wide().chain(Some(0).into_iter()).collect();
        
        let class = winuser::WNDCLASSEXW {
            cbSize: mem::size_of::<winuser::WNDCLASSEXW>() as UINT,
            style: winuser::CS_HREDRAW | winuser::CS_VREDRAW | winuser::CS_OWNDC,
            lpfnWndProc: Some(WindowsWindow::window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: unsafe {libloaderapi::GetModuleHandleW(ptr::null())},
            hIcon: unsafe{winuser::LoadIconW(ptr::null_mut(),winuser::IDI_WINLOGO)}, //h_icon,
            hCursor: unsafe{winuser::LoadCursorW(ptr::null_mut(),winuser::IDC_ARROW)}, // must be null in order for cursor state to work properly
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name_wstr.as_ptr(),
            hIconSm: ptr::null_mut(),
        };
        
        unsafe {winuser::RegisterClassExW(&class);}
        
        let style = winuser::WS_SIZEBOX | winuser::WS_MAXIMIZEBOX | winuser::WS_CAPTION
            | winuser::WS_MINIMIZEBOX | winuser::WS_BORDER | winuser::WS_VISIBLE
            | winuser::WS_CLIPSIBLINGS | winuser::WS_CLIPCHILDREN | winuser::WS_SYSMENU;
        
        let style_ex = winuser::WS_EX_WINDOWEDGE | winuser::WS_EX_APPWINDOW | winuser::WS_EX_ACCEPTFILES;
        
        unsafe {
            // lets store the window
            winuser::IsGUIThread(1);
            
            let title_wstr: Vec<_> = OsStr::new(title).encode_wide().chain(Some(0).into_iter()).collect();
            
            let hwnd = winuser::CreateWindowExW(
                style_ex,
                class_name_wstr.as_ptr(),
                title_wstr.as_ptr() as LPCWSTR,
                style,
                winuser::CW_USEDEFAULT,
                winuser::CW_USEDEFAULT,
                winuser::CW_USEDEFAULT,
                winuser::CW_USEDEFAULT,
                ptr::null_mut(),
                ptr::null_mut(),
                libloaderapi::GetModuleHandleW(ptr::null()),
                ptr::null_mut(),
            );
            self.hwnd = Some(hwnd);
            winuser::SetWindowLongPtrW(hwnd, winuser::GWLP_USERDATA, &self as *const _ as isize);
        }
    }
    
    pub fn poll_events<F>(&mut self, first_block: bool, mut event_handler: F)
    where F: FnMut(&mut Vec<Event>),
    {
        unsafe {
            self.event_callback = Some(&mut event_handler as *const FnMut(&mut Vec<Event>) as *mut FnMut(&mut Vec<Event>));
            let mut msg = mem::uninitialized();
            loop {
                if first_block {
                    if winuser::GetMessageW(&mut msg, ptr::null_mut(), 0, 0) == 0 {
                        // Only happens if the message is `WM_QUIT`.
                        debug_assert_eq!(msg.message, winuser::WM_QUIT);
                        break;
                    }
                }
                else {
                    if winuser::PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, 1) == 0 {
                        break;
                    }
                }
                // Calls `callback` below.
                winuser::TranslateMessage(&msg);
                winuser::DispatchMessageW(&msg);
            }
            self.event_callback = None;
        }
    }
    
    pub fn do_callback(&mut self, events: &mut Vec<Event>) {
        unsafe {
            if self.event_callback.is_none() {
                return
            };
            let callback = self.event_callback.unwrap();
            (*callback)(events);
        }
    }
    
    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {
    }
    
    pub fn get_window_geom(&self) -> WindowGeom {
        WindowGeom {..Default::default()}
    }
    
    
    pub fn time_now(&self) -> f64 {
        let time_now = precise_time_ns();
        (time_now - self.time_start) as f64 / 1_000_000_000.0
    }
    
    pub fn set_position(&mut self, _pos: Vec2) {
    }
    
    pub fn get_position(&self) -> Vec2 {
        Vec2::zero()
    }
    
    fn get_ime_origin(&self) -> Vec2 {
        Vec2::zero()
    }
    
    pub fn get_inner_size(&self) -> Vec2 {
        Vec2::zero()
    }
    
    pub fn get_outer_size(&self) -> Vec2 {
        Vec2::zero()
    }
    
    pub fn set_outer_size(&self, _size: Vec2) {
    }
    
    pub fn get_dpi_factor(&self) -> f32 {
        1.0
    }
    
    
    pub fn start_timer(&mut self, _timer_id: u64, _interval: f64, _repeats: bool) {
    }
    
    pub fn stop_timer(&mut self, _timer_id: u64) {
    }
    
    pub fn post_signal(_signal_id: u64, _value: u64) {
    }
    
    pub fn send_change_event(&mut self) {
        
        let new_geom = self.get_window_geom();
        let old_geom = self.last_window_geom.clone();
        self.last_window_geom = new_geom.clone();
        
        self.do_callback(&mut vec![Event::WindowChange(WindowChangeEvent {
            old_geom: old_geom,
            new_geom: new_geom
        })]);
    }
    
    pub fn send_focus_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocus]);
    }
    
    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(&mut vec![Event::AppFocusLost]);
    }
    
    pub fn send_finger_down(&mut self, digit: usize, modifiers: KeyModifiers) {
        self.fingers_down[digit] = true;
        self.do_callback(&mut vec![Event::FingerDown(FingerDownEvent {
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
        self.do_callback(&mut vec![Event::FingerUp(FingerUpEvent {
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
            abs: pos,
            rel: pos,
            rect: Rect::zero(),
            handled: false,
            hover_state: HoverState::Over,
            modifiers: modifiers,
            time: self.time_now()
        }));
        self.do_callback(&mut events);
    }
    
    pub fn send_close_requested_event(&mut self) {
        self.do_callback(&mut vec![Event::CloseRequested])
    }
    
    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(&mut vec![Event::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last
        })])
    }
}