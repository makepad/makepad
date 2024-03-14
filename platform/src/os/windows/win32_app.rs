use {
    std::{
        ffi::OsStr,
        os::windows::ffi::OsStrExt,
        mem,
        cell::{
            Cell,
            RefCell,
        },
    },
    crate::{
        log,
        error,
        windows::{
            core::HRESULT,
            core::PCWSTR,
            core::PCSTR,
            core::IntoParam,
            Win32::{
                UI::{
                    WindowsAndMessaging::{
                        WNDCLASSEXW,
                        PM_REMOVE,
                        LoadIconW,
                        RegisterClassExW,
                        IsGUIThread,
                        GetMessageW,
                        TranslateMessage,
                        DispatchMessageW,
                        PeekMessageW,
                        SetTimer,
                        KillTimer,
                        ShowCursor,
                        SetCursor,
                        LoadCursorW,
                        IsProcessDPIAware,
                        IDC_ARROW,
                        IDC_CROSS,
                        IDC_HAND,
                        IDC_SIZEALL,
                        IDC_IBEAM,
                        IDC_HELP,
                        IDC_NO,
                        IDC_SIZEWE,
                        IDC_SIZENS,
                        IDC_SIZENESW,
                        IDC_SIZENWSE,
                        WM_QUIT,
                        CS_HREDRAW,
                        CS_VREDRAW,
                        CS_OWNDC,
                        IDI_WINLOGO,
                    },
                    HiDpi::{
                        PROCESS_DPI_AWARENESS,
                        DPI_AWARENESS_CONTEXT,
                        MONITOR_DPI_TYPE,
                        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
                        PROCESS_PER_MONITOR_DPI_AWARE,
                        MDT_EFFECTIVE_DPI
                    },
                },
                Graphics::Gdi::{
                    CreateSolidBrush,
                    HMONITOR,
                    GetDC,
                    MonitorFromWindow,
                    GetDeviceCaps,
                    MONITOR_DEFAULTTONEAREST,
                    LOGPIXELSX
                },
                Foundation::{
                    COLORREF,
                    S_OK,
                    HWND,
                    BOOL,
                    FARPROC,
                    DRAGDROP_S_DROP,
                    DRAGDROP_S_CANCEL,
                },
                System::{
                    Threading::ExitProcess,
                    LibraryLoader::{
                        GetModuleHandleW,
                        LoadLibraryA,
                        GetProcAddress,
                    },
                    Performance::{
                        QueryPerformanceCounter,
                        QueryPerformanceFrequency,
                    },
                    //Com::IDataObject,
                    Ole::{
                        OleInitialize,
                        //DoDragDrop,
                        IDropSource,
                        DROPEFFECT,
                        DROPEFFECT_COPY,
                        DROPEFFECT_MOVE,
                    },
                },
            },
        },
        event::*,
        cursor::MouseCursor,
        os::{
            cx_native::EventFlow,
            windows::{
                dropsource::*,
                dataobject::*,
                win32_event::Win32Event,
                win32_window::Win32Window,
            },
        },
        window::WindowId,
    },
};
pub const FALSE: BOOL = BOOL(0);
pub const TRUE: BOOL = BOOL(1);

static mut WIN32_APP: Option<RefCell<Win32App >> = None;

pub fn get_win32_app_global() -> std::cell::RefMut<'static, Win32App> {
    unsafe {
        WIN32_APP.as_mut().unwrap().borrow_mut()
    }
}

pub fn init_win32_app_global(event_callback: Box<dyn FnMut(Win32Event) -> EventFlow>) {
    unsafe {
        WIN32_APP = Some(RefCell::new(Win32App::new(event_callback)));
    }
}

// copied from Microsoft so it refers to the right IDataObject
#[allow(non_snake_case)]
pub unsafe fn DoDragDrop<P0, P1>(pdataobj: P0, pdropsource: P1, dwokeffects: DROPEFFECT, pdweffect: *mut DROPEFFECT) -> HRESULT
where
P0: IntoParam<IDataObject>,
P1: IntoParam<IDropSource>,
{
    ::windows_targets::link!("ole32.dll" "system" fn DoDragDrop(pdataobj: *mut::core::ffi::c_void, pdropsource: *mut::core::ffi::c_void, dwokeffects: DROPEFFECT, pdweffect: *mut DROPEFFECT) -> HRESULT);
    DoDragDrop(pdataobj.into_param().abi(), pdropsource.into_param().abi(), dwokeffects, pdweffect)
}

pub struct Win32App {
    pub time_start: i64,
    pub time_freq: i64,
    event_callback: Option<Box<dyn FnMut(Win32Event) -> EventFlow >>,
    pub window_class_name: Vec<u16>,
    pub all_windows: Vec<HWND>,
    pub timers: Vec<Win32Timer>,
    pub was_signal_poll: bool,
    pub event_flow: EventFlow,
    pub dpi_functions: DpiFunctions,
    pub current_cursor: Option<MouseCursor>,
    pub currently_clicked_window_id: Option<WindowId >,
    pub start_dragging_items: Option<Vec<DragItem >>,
    pub is_dragging_internal: Cell<bool>,
}

#[derive(Clone)]
pub enum Win32Timer {
    Free,
    Timer {win32_id: usize, timer_id: u64, interval: f64, repeats: bool},
    Resize {win32_id: usize},
    DragDrop {win32_id: usize},
    SignalPoll {win32_id: usize},
}

impl Win32App {
    pub fn new(event_callback: Box<dyn FnMut(Win32Event) -> EventFlow>) -> Win32App {
        
        let window_class_name = encode_wide("MakepadWindow\0");
        let class = WNDCLASSEXW {
            cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW
                | CS_VREDRAW
                | CS_OWNDC,
            lpfnWndProc: Some(Win32Window::window_class_proc),
            hInstance: unsafe {GetModuleHandleW(None).unwrap().into()},
            hIcon: unsafe {LoadIconW(None, IDI_WINLOGO).unwrap()}, //h_icon,
            lpszClassName: PCWSTR(window_class_name.as_ptr()),
            hbrBackground: unsafe {CreateSolidBrush(COLORREF(0x3f3f3f3f))},
            ..Default::default()
            /*            
            cbClsExtra: 0,
            cbWndExtra: 0,
            hCursor: Default::default(), //unsafe {winuser::LoadCursorW(ptr::null_mut(), winuser::IDC_ARROW)}, // must be null in order for cursor state to work properly
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            hIconSm: Default::default(),*/
        };
        
        unsafe {
            RegisterClassExW(&class);
            IsGUIThread(TRUE);
            
            // initialize COM using OleInitialize to allow Drag&Drop and other shell features
            OleInitialize(None).unwrap();
        }
        
        let mut time_start = 0i64;
        unsafe {QueryPerformanceCounter(&mut time_start).unwrap()};
        
        let mut time_freq = 0i64;
        unsafe {QueryPerformanceFrequency(&mut time_freq).unwrap()};
        
        let win32_app = Win32App {
            start_dragging_items: None,
            window_class_name,
            was_signal_poll: false,
            time_start,
            time_freq,
            event_callback: Some(event_callback),
            event_flow: EventFlow::Poll,
            all_windows: Vec::new(),
            timers: Vec::new(),
            dpi_functions: DpiFunctions::new(),
            current_cursor: None,
            currently_clicked_window_id: None,
            is_dragging_internal: Cell::new(false),
        };
        win32_app.dpi_functions.become_dpi_aware();
        
        win32_app
    }
    
    pub fn event_loop() {
        unsafe {
            loop {
                let event_flow = get_win32_app_global().event_flow.clone();
                match event_flow {
                    EventFlow::Wait => {
                        let mut msg = std::mem::MaybeUninit::uninit();
                        let ret = GetMessageW(msg.as_mut_ptr(), None, 0, 0);
                        let msg = msg.assume_init();
                        if ret == FALSE {
                            // Only happens if the message is `WM_QUIT`.
                            debug_assert_eq!(msg.message, WM_QUIT);
                            get_win32_app_global().event_flow = EventFlow::Exit;
                        }
                        else {
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                            if !get_win32_app_global().was_signal_poll() {
                                Win32App::do_callback(Win32Event::Paint);
                            }
                        }
                    }
                    EventFlow::Poll => {
                        let mut msg = std::mem::MaybeUninit::uninit();
                        let ret = PeekMessageW(msg.as_mut_ptr(), None, 0, 0, PM_REMOVE);
                        let msg = msg.assume_init();
                        if ret == FALSE {
                            Win32App::do_callback(Win32Event::Paint)
                        }
                        else {
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                    EventFlow::Exit => panic!()
                }
                Win32App::poll_start_drag_drop();
            }
        }
    }
    
    pub fn do_callback(event: Win32Event) {
        let cb = get_win32_app_global().event_callback.take();
        if let Some(mut callback) = cb {
            let event_flow = callback(event);
            get_win32_app_global().event_flow = event_flow;
            if let EventFlow::Exit = event_flow {
                unsafe {ExitProcess(0);}
            }
            get_win32_app_global().event_callback = Some(callback);
        }
    }
    
    pub unsafe extern "system" fn timer_proc(_hwnd: HWND, _arg1: u32, in_win32_id: usize, _arg2: u32) {
        let hit_timer = {
            let mut win32_app = get_win32_app_global();
            {
                let mut hit_timer = None;
                for slot in 0..win32_app.timers.len() {
                    match win32_app.timers[slot] {
                        Win32Timer::Timer {win32_id, repeats, ..} => if win32_id == in_win32_id {
                            hit_timer = Some(win32_app.timers[slot].clone());
                            if !repeats {
                                KillTimer(None, in_win32_id).unwrap();
                                win32_app.timers[slot] = Win32Timer::Free;
                            }
                            break;
                        },
                        Win32Timer::DragDrop {win32_id, ..} => if win32_id == in_win32_id {
                            hit_timer = Some(win32_app.timers[slot].clone());
                            break;
                        },
                        Win32Timer::Resize {win32_id, ..} => if win32_id == in_win32_id {
                            hit_timer = Some(win32_app.timers[slot].clone());
                            break;
                        },
                        Win32Timer::SignalPoll {win32_id, ..} => if win32_id == in_win32_id {
                            hit_timer = Some(win32_app.timers[slot].clone());
                            break;
                        }
                        _ => ()
                    }
                };
                hit_timer
            }
        };
        // call the dependencies
        let time =get_win32_app_global().time_now();
        if let Some(hit_timer) = hit_timer {
            match hit_timer {
                Win32Timer::Timer {timer_id, ..} => {
                    Win32App::do_callback(Win32Event::Timer(TimerEvent {
                        time: Some(time),
                        timer_id: timer_id
                    }));
                },
                Win32Timer::Resize {..} => {
                    Win32App::do_callback(Win32Event::Paint);
                },
                Win32Timer::DragDrop {..} => {
                    Win32App::do_callback(Win32Event::Paint);
                },
                Win32Timer::SignalPoll {..} => {
                    Win32App::do_callback(
                        Win32Event::Signal
                    );
                    get_win32_app_global().was_signal_poll = true;
                }
                _ => ()
            }
        }
    }
    
    pub fn was_signal_poll(&mut self) -> bool {
        if self.was_signal_poll {
            self.was_signal_poll = false;
            true
        }
        else {
            false
        }
    }
    
    pub fn get_free_timer_slot(&mut self) -> usize {
        //let win32_app = get_win32_app_global();
        for slot in 0..self.timers.len() {
            if let Win32Timer::Free = self.timers[slot] {
                return slot
            }
        }
        let slot = self.timers.len();
        self.timers.push(Win32Timer::Free);
        slot
    }
    
    pub fn start_timer(&mut self, timer_id: u64, interval: f64, repeats: bool) {
        let slot = self.get_free_timer_slot();
        let win32_id = unsafe {SetTimer(None, 0, (interval * 1000.0) as u32, Some(Self::timer_proc))};
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
                    unsafe {KillTimer(None, win32_id).unwrap();}
                }
            }
        }
    }
    
    pub fn start_resize(&mut self) {
        let slot = self.get_free_timer_slot();
        let win32_id = unsafe {SetTimer(None, 0, 8 as u32, Some(Self::timer_proc))};
        self.timers[slot] = Win32Timer::Resize {win32_id: win32_id};
    }
    
    pub fn poll_start_drag_drop() {
        let items = get_win32_app_global().start_dragging_items.take();
        if let Some(items) = items {
            {
                let mut win32_app = get_win32_app_global();
                let slot = win32_app.get_free_timer_slot();
                let win32_id = unsafe {SetTimer(None, 0, 8 as u32, Some(Self::timer_proc))};
                win32_app.timers[slot] = Win32Timer::DragDrop {win32_id: win32_id};
            }
            
            if items.len() > 1 {
                error!("multi-item drag/drop operation not supported");
            }
            match &items[0] {
                DragItem::FilePath {path, internal_id,} => {
                    
                    //log!("win32: about to drag path \"{}\" with internal ID {:?}", path, internal_id);
                    
                    // only drag if something is there
                    if (path.len() > 0) || internal_id.is_some() {
                        
                        // create COM IDataObject that hosts the drag item
                        let data_object: IDataObject = DragItem::FilePath {path: path.clone(), internal_id: internal_id.clone(),}.into();
                        
                        // create COM IDropSource to indicate when to stop dragging
                        let drop_source: IDropSource = DropSource {}.into();
                        
                        get_win32_app_global().is_dragging_internal.replace(true);
                        let mut effect = DROPEFFECT(0);
                        match unsafe {DoDragDrop(&data_object, &drop_source, DROPEFFECT_COPY | DROPEFFECT_MOVE, &mut effect)} {
                            DRAGDROP_S_DROP => {/*log!("DoDragDrop: succesful")*/},
                            DRAGDROP_S_CANCEL => {/*log!("DoDragDrop: canceled")*/},
                            _ => {log!("DoDragDrop: failed for some reason")},
                        }
                        get_win32_app_global().is_dragging_internal.replace(false);
                    }
                },
                _ => {
                    error!("Only DragItem::FilePath supported");
                }
            }
            {
                let mut win32_app = get_win32_app_global();
                for slot in 0..win32_app.timers.len() {
                    if let Win32Timer::DragDrop {win32_id} = win32_app.timers[slot] {
                        win32_app.timers[slot] = Win32Timer::Free;
                        unsafe {KillTimer(None, win32_id).unwrap();}
                    }
                }
            }
        }
    }
    
    pub fn start_signal_poll(&mut self) {
        let slot = self.get_free_timer_slot();
        let win32_id = unsafe {SetTimer(None, 0, 8 as u32, Some(Self::timer_proc))};
        self.timers[slot] = Win32Timer::SignalPoll {win32_id: win32_id};
    }
    
    pub fn stop_resize(&mut self) {
        for slot in 0..self.timers.len() {
            if let Win32Timer::Resize {win32_id} = self.timers[slot] {
                self.timers[slot] = Win32Timer::Free;
                unsafe {KillTimer(None, win32_id).unwrap();}
            }
        }
    }
    
    pub fn start_dragging(&mut self, items: Vec<DragItem>) {
        self.start_dragging_items = Some(items);
    }
    
    pub fn time_now(&self) -> f64 {
        unsafe {
            let mut time_now = 0i64;
            QueryPerformanceCounter(&mut time_now).unwrap();
            (time_now - self.time_start) as f64 / self.time_freq as f64
        }
    }
    
    pub fn set_mouse_cursor(&mut self, cursor: MouseCursor) {
        if self.current_cursor.is_none() || self.current_cursor.unwrap() != cursor {
            let win32_cursor = match cursor {
                MouseCursor::Hidden => {
                    PCWSTR::null()
                },
                MouseCursor::Default => IDC_ARROW,
                MouseCursor::Crosshair => IDC_CROSS,
                MouseCursor::Hand => IDC_HAND,
                MouseCursor::Arrow => IDC_ARROW,
                MouseCursor::Move => IDC_SIZEALL,
                MouseCursor::Text => IDC_IBEAM,
                MouseCursor::Wait => IDC_ARROW,
                MouseCursor::Help => IDC_HELP,
                MouseCursor::NotAllowed => IDC_NO,
                
                MouseCursor::EResize => IDC_SIZEWE,
                MouseCursor::NResize => IDC_SIZENS,
                MouseCursor::NeResize => IDC_SIZENESW,
                MouseCursor::NwResize => IDC_SIZENWSE,
                MouseCursor::SResize => IDC_SIZENS,
                MouseCursor::SeResize => IDC_SIZENWSE,
                MouseCursor::SwResize => IDC_SIZENESW,
                MouseCursor::WResize => IDC_SIZEWE,
                
                
                MouseCursor::NsResize => IDC_SIZENS,
                MouseCursor::NeswResize => IDC_SIZENESW,
                MouseCursor::EwResize => IDC_SIZEWE,
                MouseCursor::NwseResize => IDC_SIZENWSE,
                
                MouseCursor::ColResize => IDC_SIZEWE,
                MouseCursor::RowResize => IDC_SIZENS,
            };
            self.current_cursor = Some(cursor);
            unsafe {
                if win32_cursor == PCWSTR::null() {
                    ShowCursor(FALSE);
                }
                else {
                    SetCursor(LoadCursorW(None, win32_cursor).unwrap());
                    ShowCursor(TRUE);
                }
            }
            //TODO
        }
    }
}

// reworked from winit windows platform https://github.com/rust-windowing/winit/blob/eventloop-2.0/src/platform_impl/windows/dpi.rs

type SetProcessDPIAware = unsafe extern "system" fn () -> BOOL;
type SetProcessDpiAwareness = unsafe extern "system" fn (value: PROCESS_DPI_AWARENESS,) -> HRESULT;
type SetProcessDpiAwarenessContext = unsafe extern "system" fn (value: DPI_AWARENESS_CONTEXT,) -> BOOL;
type GetDpiForWindow = unsafe extern "system" fn (hwnd: HWND) -> u32;
type GetDpiForMonitor = unsafe extern "system" fn (hmonitor: HMONITOR, dpi_type: MONITOR_DPI_TYPE, dpi_x: *mut u32, dpi_y: *mut u32) -> HRESULT;
type EnableNonClientDpiScaling = unsafe extern "system" fn (hwnd: HWND) -> BOOL;

// Helper function to dynamically load function pointer.
// `library` and `function` must be zero-terminated.
fn get_function_impl(library: &str, function: &str) -> FARPROC {
    // Library names we will use are ASCII so we can use the A version to avoid string conversion.
    
    let module = unsafe {LoadLibraryA(PCSTR::from_raw(library.as_ptr()))};
    if module.is_err() {
        return None;
    }
    
    let function_ptr = unsafe {GetProcAddress(module.unwrap(), PCSTR::from_raw(function.as_ptr()))};
    if function_ptr.is_none() {
        return None;
    }
    
    function_ptr
}

macro_rules!get_function {
    ( $ lib: expr, $ func: ident) => {
        get_function_impl(concat!( $ lib, '\0'), concat!(stringify!( $ func), '\0'))
            .map( | f | unsafe {mem::transmute::<_, $ func>(f)})
    }
}

pub fn encode_wide(string: impl AsRef<OsStr>) -> Vec<u16> {
    string.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/*
pub fn post_signal_to_hwnd(hwnd:HWND, signal:Signal){
    unsafe{PostMessageW(
        hwnd,
        WM_USER,
        WPARAM(((signal.0.0)&0xffff_ffff) as usize),
        LPARAM(((signal.0.0>>32)&0xffff_ffff) as isize),
    )};
}
*/
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
                set_process_dpi_awareness(PROCESS_PER_MONITOR_DPI_AWARE).unwrap();
            }
            else if let Some(set_process_dpi_aware) = self.set_process_dpi_aware {
                // We are on Vista or later.
                set_process_dpi_aware().unwrap();
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
            let hdc = GetDC(hwnd);
            if hdc.is_invalid() {
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
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                if monitor.is_invalid() {
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
                if IsProcessDPIAware() == TRUE {
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
