use {
    std::{
        cell::{
            RefCell,
            Cell,
        },
        rc::Rc,
        ffi::OsStr,
        os::windows::ffi::OsStrExt,
        mem,
    },
    
    crate::{
        windows::{
            core::PCWSTR,
            core::IntoParam,
            core::Result as coreResult,
            core::HRESULT,
            Win32::{
                Foundation::{
                    HWND,
                    HANDLE,
                    HGLOBAL,
                    WPARAM,
                    LPARAM,
                    LRESULT,
                    RECT,
                    POINT,
                    POINTL,
                },
                System::{
                    Memory::{
                        GlobalLock,
                        GlobalAlloc,
                        GlobalSize,
                        GlobalUnlock,
                        GLOBAL_ALLOC_FLAGS,
                    },
                    Ole::{
                        CF_UNICODETEXT,
                        //RegisterDragDrop,
                        //IDropTarget,
                        DROPEFFECT,
                        DROPEFFECT_COPY,
                        DROPEFFECT_MOVE,
                        DROPEFFECT_LINK,
                    },
                    SystemServices::{
                        MODIFIERKEYS_FLAGS,
                        MK_CONTROL,
                        MK_SHIFT,
                    },
                    WindowsProgramming::GMEM_DDESHARE,
                    DataExchange::{
                        OpenClipboard,
                        EmptyClipboard,
                        GetClipboardData,
                        SetClipboardData,
                        CloseClipboard,
                    },
                    LibraryLoader::GetModuleHandleW,
                },
                UI::{
                    WindowsAndMessaging::{
                        CreateWindowExW,
                        SetWindowLongPtrW,
                        GetWindowLongPtrW,
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
                        GWL_EXSTYLE,
                        HWND_TOPMOST,
                        HWND_NOTOPMOST,
                        WS_SIZEBOX,
                        WS_MAXIMIZEBOX,
                        WS_MINIMIZEBOX,
                        WS_POPUP,
                        WS_CLIPSIBLINGS,
                        WS_CLIPCHILDREN,
                        WS_SYSMENU,
                        WS_EX_WINDOWEDGE,
                        WS_EX_APPWINDOW,
                        WS_EX_ACCEPTFILES,
                        WS_EX_TOPMOST,
                        CW_USEDEFAULT,
                        GWLP_USERDATA,
                        SW_SHOW,
                        SW_RESTORE,
                        SW_MAXIMIZE,
                        SW_MINIMIZE,
                        SWP_NOMOVE,
                        SWP_NOSIZE,
                        WM_ACTIVATE,
                        WM_NCCALCSIZE,
                        WM_NCHITTEST,
                        WA_ACTIVE,
                        WM_ERASEBKGND,
                        WM_MOUSEMOVE,
                        WM_MOUSEWHEEL,
                        WM_LBUTTONDOWN,
                        WM_LBUTTONUP,
                        WM_RBUTTONDOWN,
                        WM_RBUTTONUP,
                        WM_MBUTTONDOWN,
                        WM_MBUTTONUP,
                        WM_KEYDOWN,
                        WM_SYSKEYDOWN,
                        WM_CLOSE,
                        WM_KEYUP,
                        WM_SYSKEYUP,
                        WM_CHAR,
                        WM_ENTERSIZEMOVE,
                        WM_EXITSIZEMOVE,
                        WM_SIZE,
                        WM_DPICHANGED,
                        WM_DESTROY,
                        HTTOPLEFT,
                        HTBOTTOMLEFT,
                        HTLEFT,
                        HTTOPRIGHT,
                        HTBOTTOMRIGHT,
                        HTRIGHT,
                        HTTOP,
                        HTBOTTOM,
                        HTCLIENT,
                        HTCAPTION,
                        HTSYSMENU
                    },
                    Controls::{
                        MARGINS,
                        WM_MOUSELEAVE
                    },
                    Input::KeyboardAndMouse::{
                        VIRTUAL_KEY,
                        ReleaseCapture,
                        SetCapture,
                        TrackMouseEvent,
                        GetKeyState,
                        TRACKMOUSEEVENT,
                        TME_LEAVE,
                        VK_CONTROL,
                        VK_SHIFT,
                        VK_MENU,
                        VK_LWIN,
                        VK_RWIN,
                        VK_ESCAPE,
                        VK_OEM_3,
                        VK_0,
                        VK_1,
                        VK_2,
                        VK_3,
                        VK_4,
                        VK_5,
                        VK_6,
                        VK_7,
                        VK_8,
                        VK_9,
                        VK_OEM_MINUS,
                        VK_OEM_PLUS,
                        VK_BACK,
                        VK_TAB,
                        VK_Q,
                        VK_W,
                        VK_E,
                        VK_R,
                        VK_T,
                        VK_Y,
                        VK_U,
                        VK_I,
                        VK_O,
                        VK_P,
                        VK_OEM_4,
                        VK_OEM_6,
                        VK_RETURN,
                        VK_A,
                        VK_S,
                        VK_D,
                        VK_F,
                        VK_G,
                        VK_H,
                        VK_J,
                        VK_K,
                        VK_L,
                        VK_OEM_1,
                        VK_OEM_7,
                        VK_OEM_5,
                        VK_Z,
                        VK_X,
                        VK_C,
                        VK_V,
                        VK_B,
                        VK_N,
                        VK_M,
                        VK_OEM_COMMA,
                        VK_OEM_PERIOD,
                        VK_OEM_2,
                        VK_LCONTROL,
                        VK_RCONTROL,
                        VK_LMENU,
                        VK_RMENU,
                        VK_LSHIFT,
                        VK_RSHIFT,
                        VK_SPACE,
                        VK_CAPITAL,
                        VK_F1,
                        VK_F2,
                        VK_F3,
                        VK_F4,
                        VK_F5,
                        VK_F6,
                        VK_F7,
                        VK_F8,
                        VK_F9,
                        VK_F10,
                        VK_F11,
                        VK_F12,
                        VK_SNAPSHOT,
                        VK_SCROLL,
                        VK_PAUSE,
                        VK_INSERT,
                        VK_DELETE,
                        VK_HOME,
                        VK_END,
                        VK_PRIOR,
                        VK_NEXT,
                        VK_NUMPAD0,
                        VK_NUMPAD1,
                        VK_NUMPAD2,
                        VK_NUMPAD3,
                        VK_NUMPAD4,
                        VK_NUMPAD5,
                        VK_NUMPAD6,
                        VK_NUMPAD7,
                        VK_NUMPAD8,
                        VK_NUMPAD9,
                        VK_SUBTRACT,
                        VK_ADD,
                        VK_DECIMAL,
                        VK_MULTIPLY,
                        VK_DIVIDE,
                        VK_NUMLOCK,
                        VK_UP,
                        VK_DOWN,
                        VK_LEFT,
                        VK_RIGHT,
                    },
                },
                Graphics::{
                    Dwm::DwmExtendFrameIntoClientArea,
                    Gdi::ScreenToClient,
                },
            },
        },
        event::*,
        area::Area,
        os::windows::{
            win32_app::{
                Win32App,
                encode_wide,
                FALSE,
                get_win32_app_global,
            },
            win32_event::*,
            droptarget::*,
        },
        window::WindowId,
        makepad_math::*,
        cursor::MouseCursor,
    },
};

// Copied from Microsoft so it refers to the right IDropTarget
#[allow(non_snake_case)]
pub unsafe fn RegisterDragDrop<P0, P1>(hwnd: P0, pdroptarget: P1) -> coreResult<()>
where
    P0: IntoParam<HWND>,
    P1: IntoParam<IDropTarget>,
{
    ::windows_targets::link!("ole32.dll" "system" fn RegisterDragDrop(hwnd : HWND, pdroptarget : * mut::core::ffi::c_void) -> HRESULT);
    RegisterDragDrop(hwnd.into_param().abi(), pdroptarget.into_param().abi()).ok()
}

//#[derive(Clone)]
pub struct Win32Window {
    pub window_id: WindowId,
    pub last_window_geom: WindowGeom,
    
    pub mouse_buttons_down: usize,
    pub last_key_mod: KeyModifiers,
    pub ime_spot: DVec2,
    pub current_cursor: MouseCursor,
    pub last_mouse_pos: DVec2,
    pub ignore_wmsize: usize,
    pub hwnd: HWND,
    pub track_mouse_event: bool,
}

impl Win32Window {
    
    // 2-stage initialization (new and init) to connect GWLP_USERDATA 

    // create window structure and register drag/drop
    pub fn new(window_id: WindowId,title: &str, position: Option<DVec2>) -> Win32Window {

        let title = encode_wide(title);
        
        let style = WS_SIZEBOX
            | WS_MAXIMIZEBOX
            | WS_MINIMIZEBOX
            | WS_POPUP
            | WS_CLIPSIBLINGS
            | WS_CLIPCHILDREN
            | WS_SYSMENU;
        
        let style_ex = WS_EX_WINDOWEDGE
            | WS_EX_APPWINDOW
            | WS_EX_ACCEPTFILES;
        
        let (x, y) = if let Some(position) = position {
            (position.x as i32, position.y as i32)
        }
        else {
            (CW_USEDEFAULT, CW_USEDEFAULT)
        };

        let hwnd = unsafe { CreateWindowExW(
            style_ex,
            PCWSTR(get_win32_app_global().window_class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            style,
            x,
            y,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            GetModuleHandleW(None).unwrap(),
            None,
        ) };

        // create DropTarget object that accesses the same data object, convert to COM and give to Microsoft
        let drop_target: IDropTarget = DropTarget { drag_item: RefCell::new(None),hwnd, }.into();
        unsafe { RegisterDragDrop(hwnd, &drop_target).unwrap() };

        Win32Window {
            window_id,
            mouse_buttons_down: 0,
            last_window_geom: WindowGeom::default(),
            last_key_mod: KeyModifiers::default(),
            ime_spot: DVec2::default(),
            current_cursor: MouseCursor::Default,
            last_mouse_pos: DVec2::default(),
            ignore_wmsize: 0,
            hwnd,
            track_mouse_event: false,
        }
    }

    // initialize GWLP_USERDATA and registration of global stuff, and set outer size
    pub fn init(&mut self,size: DVec2) {

        unsafe { SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, self as *const _ as isize) };

        get_win32_app_global().dpi_functions.enable_non_client_dpi_scaling(self.hwnd);
        get_win32_app_global().all_windows.push(self.hwnd);

        self.set_outer_size(size);
    }
    
    pub unsafe extern "system" fn window_class_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,) -> LRESULT {
        
        let user_data = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if user_data == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        
        let window = &mut (*(user_data as *mut Win32Window));
        match msg {
            WM_ACTIVATE => {
                if wparam.0 & 0xffff == WA_ACTIVE as usize {
                    window.do_callback(Win32Event::AppGotFocus);
                }
                else {
                    window.do_callback(Win32Event::AppLostFocus);
                }
            },
            WM_NCCALCSIZE => {
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
            WM_NCHITTEST => {
                //let ycoord = (lparam.0 >> 16) as u16 as i16 as i32;
                //let xcoord = (lparam.0 & 0xffff) as u16 as i16 as i32;
                let abs = window.get_mouse_pos_from_lparam(lparam);
                let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
                const EDGE: f64 = 4.0; 
                let dpi = window.get_dpi_factor();
                GetWindowRect(hwnd, &mut rect).unwrap();
                let rect = Rect{pos:dvec2(rect.left as f64 / dpi, rect.top as f64 / dpi),
                size:dvec2( (rect.right-rect.left) as f64 / dpi,  (rect.bottom-rect.top) as f64/ dpi) 
                }; 
                if abs.x < rect.pos.x + EDGE {
                    if abs.y < rect.pos.y  + EDGE {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::NwseResize);
                        return LRESULT(HTTOPLEFT as isize);
                    }
                    if abs.y > rect.pos.y + rect.size.y - EDGE {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::NeswResize);
                        return LRESULT(HTBOTTOMLEFT as isize);
                    }
                    get_win32_app_global().set_mouse_cursor(MouseCursor::EwResize);
                    return LRESULT(HTLEFT as isize);
                }
                if abs.x > rect.pos.x+rect.size.x - EDGE {
                    if abs.y < rect.pos.y  + EDGE {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::NeswResize);
                        return LRESULT(HTTOPRIGHT as isize);
                    }
                    if abs.y > rect.pos.y + rect.size.y - EDGE {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::NwseResize);
                        return LRESULT(HTBOTTOMRIGHT as isize);
                    }
                    get_win32_app_global().set_mouse_cursor(MouseCursor::EwResize);
                    return LRESULT(HTRIGHT as isize);
                }
                if abs.y < rect.pos.y + EDGE {
                    get_win32_app_global().set_mouse_cursor(MouseCursor::NsResize);
                    return LRESULT(HTTOP as isize);
                }
                if abs.y > rect.pos.y+rect.size.y - EDGE {
                    get_win32_app_global().set_mouse_cursor(MouseCursor::NsResize);
                    return LRESULT(HTBOTTOM as isize);
                }
                let response = Rc::new(Cell::new(WindowDragQueryResponse::NoAnswer));
                window.do_callback(
                    Win32Event::WindowDragQuery(WindowDragQueryEvent {
                        window_id: window.window_id,
                        abs: window.get_mouse_pos_from_lparam(lparam)- rect.pos,
                        response: response.clone()
                    })
                );
                match response.get() {
                    WindowDragQueryResponse::Client => {
                        return LRESULT(HTCLIENT as isize);
                    }
                    WindowDragQueryResponse::Caption => {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::Default);
                        return LRESULT(HTCAPTION as isize);
                    },
                    WindowDragQueryResponse::SysMenu => {
                        get_win32_app_global().set_mouse_cursor(MouseCursor::Default);
                        return LRESULT(HTSYSMENU as isize);
                    }
                    _ => ()
                }
                return LRESULT(HTCLIENT as isize);
            },
            WM_ERASEBKGND => {
                return LRESULT(1)
            },
            WM_MOUSEMOVE => {
                if get_win32_app_global().start_dragging_items.is_some(){
                    return LRESULT(0)
                }
                if !window.track_mouse_event {
                    window.track_mouse_event = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0
                    };
                    TrackMouseEvent(&mut tme).unwrap();
                }
                window.send_mouse_move(
                    window.get_mouse_pos_from_lparam(lparam),
                    Self::get_key_modifiers()
                )
            },
            WM_MOUSELEAVE => {
                if get_win32_app_global().start_dragging_items.is_some() {
                    return LRESULT(0)
                }
                window.track_mouse_event = false;
                window.send_mouse_leave(
                    window.last_mouse_pos,
                    Self::get_key_modifiers()
                );
                get_win32_app_global().current_cursor = Some(MouseCursor::Hidden);
            },
            WM_MOUSEWHEEL => {
                let delta = (wparam.0 >> 16) as u16 as i16 as f64;
                window.send_scroll(DVec2 {x: 0.0, y: -delta}, Self::get_key_modifiers(), true);
            },
            WM_LBUTTONDOWN => {
                // hack for drag/drop: save which window was last clicked on in win32_app
                get_win32_app_global().currently_clicked_window_id = Some(window.window_id);
                window.send_mouse_down(0, Self::get_key_modifiers());
            },
            WM_LBUTTONUP => window.send_mouse_up(0, Self::get_key_modifiers()),
            WM_RBUTTONDOWN => window.send_mouse_down(1, Self::get_key_modifiers()),
            WM_RBUTTONUP => window.send_mouse_up(1, Self::get_key_modifiers()),
            WM_MBUTTONDOWN => window.send_mouse_down(2, Self::get_key_modifiers()),
            WM_MBUTTONUP => window.send_mouse_up(2, Self::get_key_modifiers()),
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                // detect control/cmd - c / v / x
                let modifiers = Self::get_key_modifiers();
                let key_code = Self::virtual_key_to_key_code(wparam);
                if modifiers.alt && key_code == KeyCode::F4 {
                    PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).unwrap();
                }
                if modifiers.control || modifiers.logo {
                    match key_code {
                        KeyCode::KeyV => { // paste
                            if let Ok(()) = OpenClipboard(None) {
                                let mut data: Vec<u16> = Vec::new();
                                let h_clipboard_data = GetClipboardData(CF_UNICODETEXT.0 as u32).unwrap();
                                let h_clipboard_ptr = GlobalLock(std::mem::transmute::<_,HGLOBAL>(h_clipboard_data)) as *mut u16;
                                let clipboard_size = GlobalSize(std::mem::transmute::<_,HGLOBAL>(h_clipboard_data));
                                if clipboard_size > 2 {
                                    data.resize((clipboard_size >> 1) - 1, 0);
                                    std::ptr::copy_nonoverlapping(h_clipboard_ptr, data.as_mut_ptr(), data.len());
                                    GlobalUnlock(std::mem::transmute::<_,HGLOBAL>(h_clipboard_data)).unwrap();
                                    CloseClipboard().unwrap();
                                    if let Ok(utf8) = String::from_utf16(&data) {
                                        window.do_callback(
                                            Win32Event::TextInput(TextInputEvent {
                                                input: utf8,
                                                was_paste: true,
                                                replace_last: false
                                            })
                                        );
                                    }
                                }
                                else {
                                    GlobalUnlock(std::mem::transmute::<_,HGLOBAL>(h_clipboard_data)).unwrap();
                                    CloseClipboard().unwrap();
                                }
                            }
                        }
                        KeyCode::KeyC => {
                            let response = Rc::new(RefCell::new(None));
                            window.do_callback(
                                Win32Event::TextCut(TextClipboardEvent {
                                    response: response.clone()
                                })
                            );
                            let response = response.borrow();
                            if let Some(response) = response.as_ref() {
                                Self::copy_to_clipboard(response);
                            }
                        },
                        KeyCode::KeyX => {
                            let response = Rc::new(RefCell::new(None));
                            window.do_callback(
                                Win32Event::TextCut(TextClipboardEvent {
                                    response: response.clone()
                                })
                            );
                            let response = response.borrow();
                            if let Some(response) = response.as_ref() {
                                Self::copy_to_clipboard(response);
                            }
                        }
                        _ => ()
                    }
                }
                window.do_callback(
                    Win32Event::KeyDown(KeyEvent {
                        key_code: key_code,
                        is_repeat: (lparam.0 & 0x7000_0000)>0,
                        modifiers: modifiers,
                        time: window.time_now()
                    })
                );
            },
            WM_KEYUP | WM_SYSKEYUP => {
                window.do_callback(
                    Win32Event::KeyUp(KeyEvent {
                        key_code: Self::virtual_key_to_key_code(wparam),
                        is_repeat: lparam.0 & 0x7fff>0,
                        modifiers: Self::get_key_modifiers(),
                        time: window.time_now()
                    })
                );
                
            },
            WM_CHAR => {
                if let Ok(utf8) = String::from_utf16(&[wparam.0 as u16]) {
                    let char_code = utf8.chars().next().unwrap();
                    if char_code >= ' ' {
                        window.do_callback(
                            Win32Event::TextInput(TextInputEvent {
                                input: utf8,
                                was_paste: false,
                                replace_last: false
                            })
                        );
                    }
                }
            },
            WM_ENTERSIZEMOVE => {
                get_win32_app_global().start_resize();
                window.do_callback(Win32Event::WindowResizeLoopStart(window.window_id));
            }
            WM_EXITSIZEMOVE => {
                get_win32_app_global().stop_resize();
                window.do_callback(Win32Event::WindowResizeLoopStop(window.window_id));
            },
            WM_SIZE | WM_DPICHANGED => {
                window.send_change_event();
            },
            WM_CLOSE => { // close requested
                let accept_close = Rc::new(Cell::new(true));
                window.do_callback(Win32Event::WindowCloseRequested(WindowCloseRequestedEvent {
                    window_id: window.window_id,
                    accept_close: accept_close.clone()
                }));
                if accept_close.get() {
                    DestroyWindow(hwnd).unwrap();
                }
            },
            WM_DESTROY => { // window actively destroyed
                window.do_callback(
                    Win32Event::WindowClosed(WindowClosedEvent {
                        window_id: window.window_id,
                    })
                );
            },

            // from DropTarget
            WM_DROPTARGET => {

                // restore the Box<>
                let message = unsafe { Box::from_raw(lparam.0 as *mut DropTargetMessage) };

                match *message {

                    DropTargetMessage::Leave => {
                        if get_win32_app_global().is_dragging_internal.get() {
                            // TODO: cancel DoDragDrop somehow
                            window.do_callback(Win32Event::DragEnd);
                        }
                    },
                    DropTargetMessage::Enter(flags,mut point,effect,drag_item) |
                    DropTargetMessage::Over(flags,mut point,effect,drag_item) => {
                        
                        // decode message
                        unsafe { ScreenToClient(window.hwnd,&mut point as *mut POINTL as *mut POINT) };
                        let response = if (effect & DROPEFFECT_LINK) != DROPEFFECT(0) { DragResponse::Link }
                        else if (effect & DROPEFFECT_MOVE) != DROPEFFECT(0) { DragResponse::Move }
                        else if (effect & DROPEFFECT_COPY) != DROPEFFECT(0) { DragResponse::Copy }
                        else { DragResponse::None };

                        let dpi_factor = window.get_dpi_factor();

                        // send to makepad
                        window.do_callback(
                            Win32Event::Drag(
                                DragEvent {
                                    modifiers: KeyModifiers {
                                        shift: (flags & MK_SHIFT) != MODIFIERKEYS_FLAGS(0),
                                        control: (flags & MK_CONTROL) != MODIFIERKEYS_FLAGS(0),
                                        alt: false,  // TODO
                                        logo: false,  // Windows doesn't have a logo button
                                    },
                                    handled: Cell::new(false),
                                    abs: DVec2 { x: point.x as f64 / dpi_factor,y: point.y as f64 / dpi_factor, },
                                    items: Rc::new(vec![drag_item]),
                                    response: Rc::new(Cell::new(response)),
                                }
                            )
                        );        
                    }, 

                    DropTargetMessage::Drop(flags,mut point,_effect,drag_item) => {

                        // decode message
                        unsafe { ScreenToClient(window.hwnd,&mut point as *mut POINTL as *mut POINT) };

                        //log!("dropping at ({},{}), flags: {:04X}, response: {:?}, drag_item: {:?}",point.x,point.y,flags.0,response,drag_item);
                        let dpi_factor = window.get_dpi_factor();

                        // send to makepad
                        window.do_callback(
                            Win32Event::Drop(
                                DropEvent {
                                    modifiers: KeyModifiers {
                                        shift: (flags & MK_SHIFT) != MODIFIERKEYS_FLAGS(0),
                                        control: (flags & MK_CONTROL) != MODIFIERKEYS_FLAGS(0),
                                        alt: false,  // TODO
                                        logo: false,  // Windows doesn't have a logo button
                                    },
                                    handled: Cell::new(false),
                                    abs: DVec2 { x: point.x as f64 / dpi_factor,y: point.y as f64 / dpi_factor, },
                                    items: Rc::new(vec![drag_item]),
                                }
                            )
                        );

                        window.do_callback(
                            Win32Event::DragEnd
                        );        
                    },
                }
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

    unsafe fn copy_to_clipboard(text: &String) {
        // plug it into the windows clipboard
        // make utf16 dta
        if let Ok(()) = OpenClipboard(None) {
            EmptyClipboard().unwrap();

            let data: Vec<u16> = OsStr::new(text).encode_wide().chain(Some(0).into_iter()).collect();

            let h_clipboard_data = GlobalAlloc(GLOBAL_ALLOC_FLAGS(GMEM_DDESHARE), 2 * data.len()).expect("GlobalAlloc for clipboard failed");

            let h_clipboard_ptr = GlobalLock(h_clipboard_data) as *mut u16;

            std::ptr::copy_nonoverlapping(data.as_ptr(), h_clipboard_ptr, data.len());

            GlobalUnlock(h_clipboard_data).unwrap();
            SetClipboardData(CF_UNICODETEXT.0 as u32, std::mem::transmute::<_,HANDLE>(h_clipboard_data)).unwrap();
            CloseClipboard().unwrap();
        }
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
                control: GetKeyState(VK_CONTROL.0 as i32) & 0x80>0,
                shift: GetKeyState(VK_SHIFT.0 as i32) & 0x80>0,
                alt: GetKeyState(VK_MENU.0 as i32) & 0x80>0,
                logo: GetKeyState(VK_LWIN.0 as i32) & 0x80>0
                    || GetKeyState(VK_RWIN.0 as i32) & 0x80>0,
            }
        }
    }
    
    pub fn on_mouse_move(&self) {
    }
    
    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {
    }
    
    pub fn restore(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_RESTORE);
            PostMessageW(self.hwnd, WM_SIZE, WPARAM(0), LPARAM(0)).unwrap();
        }
    }
    
    pub fn maximize(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_MAXIMIZE);
            PostMessageW(self.hwnd, WM_SIZE, WPARAM(0), LPARAM(0)).unwrap();
        }
    }
    
    pub fn close_window(&self) {
        unsafe {
            DestroyWindow(self.hwnd).unwrap();
        }
    }
    
    pub fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOW);
        }
    }
    
    pub fn minimize(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_MINIMIZE);
        }
    }
    
    pub fn set_topmost(&self, topmost: bool) {
        unsafe {
            if topmost {
                SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE
                ).unwrap();
            }
            else {
                SetWindowPos(
                    self.hwnd,
                    HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE
                ).unwrap();
            }
        }
    }
    
    pub fn get_is_topmost(&self) -> bool {
        unsafe {
            let ex_style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            if ex_style as u32 & WS_EX_TOPMOST.0 != 0 {
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
            inner_size: if self.get_is_maximized(){self.get_outer_size()}else{self.get_inner_size()},
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
            GetWindowPlacement(self.hwnd, &mut wp).unwrap();
            if wp.showCmd == SW_MAXIMIZE.0 as u32 {
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
            GetWindowRect(self.hwnd, &mut rect).unwrap();
            DVec2 {x: rect.left as f64, y: rect.top as f64}
        }
    }
    
    pub fn get_inner_size(&self) -> DVec2 {
        unsafe {
            let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetClientRect(self.hwnd, &mut rect).unwrap();
            let dpi = self.get_dpi_factor();
            DVec2 {x: (rect.right - rect.left) as f64 / dpi, y: (rect.bottom - rect.top)as f64 / dpi}
        }
    }
    
    pub fn get_outer_size(&self) -> DVec2 {
        unsafe {
            let mut rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetWindowRect(self.hwnd, &mut rect).unwrap();
            let dpi = self.get_dpi_factor();
            DVec2 {x: (rect.right - rect.left) as f64/ dpi, y: (rect.bottom - rect.top)as f64/ dpi}
        }
    }
    
    pub fn set_position(&mut self, pos: DVec2) {
        unsafe {
            let mut window_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetWindowRect(self.hwnd, &mut window_rect).unwrap();
            let dpi = self.get_dpi_factor();
            MoveWindow(
                self.hwnd,
                (pos.x * dpi) as i32,
                (pos.y * dpi) as i32,
                window_rect.right - window_rect.left,
                window_rect.bottom - window_rect.top,
                FALSE
            ).unwrap();
        }
    }
    
    pub fn set_outer_size(&self, size: DVec2) {
        unsafe {
            let mut window_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetWindowRect(self.hwnd, &mut window_rect).unwrap();
            let dpi = self.get_dpi_factor();
            MoveWindow(
                self.hwnd,
                window_rect.left,
                window_rect.top,
                (size.x * dpi) as i32,
                (size.y * dpi) as i32,
                FALSE
            ).unwrap();
        }
    }
    
    pub fn set_inner_size(&self, size: DVec2) {
        unsafe {
            let mut window_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetWindowRect(self.hwnd, &mut window_rect).unwrap();
            let mut client_rect = RECT {left: 0, top: 0, bottom: 0, right: 0};
            GetClientRect(self.hwnd, &mut client_rect).unwrap();
            let dpi = self.get_dpi_factor();
            MoveWindow(
                self.hwnd,
                window_rect.left,
                window_rect.top,
                (size.x * dpi) as i32
                    + ((window_rect.right - window_rect.left) - (client_rect.right - client_rect.left)),
                (size.y * dpi) as i32
                    + ((window_rect.bottom - window_rect.top) - (client_rect.bottom - client_rect.top)),
                FALSE
            ).unwrap();
        }
    }
    
    pub fn get_dpi_factor(&self) -> f64 {
        get_win32_app_global().dpi_functions.hwnd_dpi_factor(self.hwnd) as f64
    }
    
    pub fn do_callback(&mut self, event: Win32Event) {
        Win32App::do_callback(event);
    }
    
    pub fn send_change_event(&mut self) {
        
        let new_geom = self.get_window_geom();
        let old_geom = self.last_window_geom.clone();
        self.last_window_geom = new_geom.clone();
        
        self.do_callback(
            Win32Event::WindowGeomChange(WindowGeomChangeEvent {
                window_id: self.window_id,
                old_geom: old_geom,
                new_geom: new_geom
            }),
        );
        self.do_callback(
            Win32Event::Paint
        );
    }
    
    pub fn send_focus_event(&mut self) {
        self.do_callback(Win32Event::AppGotFocus);
    }
    
    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(Win32Event::AppLostFocus);
    }
    
    pub fn send_mouse_down(&mut self, button: usize, modifiers: KeyModifiers) {
        if self.mouse_buttons_down == 0 {
            unsafe {SetCapture(self.hwnd);}
        }
        self.mouse_buttons_down += 1;
        self.do_callback(Win32Event::MouseDown(MouseDownEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
    }
    
    pub fn send_mouse_up(&mut self, button: usize, modifiers: KeyModifiers) {
        if self.mouse_buttons_down > 1 {
            self.mouse_buttons_down -= 1;
        }
        else {
            unsafe { ReleaseCapture().unwrap(); }
            self.mouse_buttons_down = 0;
        }
        self.do_callback(Win32Event::MouseUp(MouseUpEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now()
        }));
    }
    
    pub fn send_mouse_move(&mut self, pos: DVec2, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        self.do_callback(Win32Event::MouseMove(MouseMoveEvent {
            window_id: self.window_id,
            abs: pos,
            modifiers: modifiers,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
    }

    pub fn send_mouse_leave(&mut self, pos: DVec2, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        self.do_callback(Win32Event::MouseLeave(MouseLeaveEvent {
            window_id: self.window_id,
            abs: pos,
            modifiers: modifiers,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
    }
    
    pub fn send_scroll(&mut self, scroll: DVec2, modifiers: KeyModifiers, is_mouse: bool) {
        self.do_callback(
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
        );
    }
    
    pub fn send_close_requested_event(&mut self) -> bool {
        let accept_close = Rc::new(Cell::new(true));
        self.do_callback(Win32Event::WindowCloseRequested(WindowCloseRequestedEvent {
            window_id: self.window_id,
            accept_close: accept_close.clone()
        }));
        if !accept_close.get() {
            return false
        }
        true
    }
    
    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(Win32Event::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last
        }))
    }
    
    pub fn virtual_key_to_key_code(wparam: WPARAM) -> KeyCode {
        match VIRTUAL_KEY(wparam.0 as u16) {
            VK_ESCAPE => KeyCode::Escape,
            VK_OEM_3 => KeyCode::Backtick,
            VK_0 => KeyCode::Key0,
            VK_1 => KeyCode::Key1,
            VK_2 => KeyCode::Key2,
            VK_3 => KeyCode::Key3,
            VK_4 => KeyCode::Key4,
            VK_5 => KeyCode::Key5,
            VK_6 => KeyCode::Key6,
            VK_7 => KeyCode::Key7,
            VK_8 => KeyCode::Key8,
            VK_9 => KeyCode::Key9,
            VK_OEM_MINUS => KeyCode::Minus,
            VK_OEM_PLUS => KeyCode::Equals,
            VK_BACK => KeyCode::Backspace,
            VK_TAB => KeyCode::Tab,
            VK_Q => KeyCode::KeyQ,
            VK_W => KeyCode::KeyW,
            VK_E => KeyCode::KeyE,
            VK_R => KeyCode::KeyR,
            VK_T => KeyCode::KeyT,
            VK_Y => KeyCode::KeyY,
            VK_U => KeyCode::KeyU,
            VK_I => KeyCode::KeyI,
            VK_O => KeyCode::KeyO,
            VK_P => KeyCode::KeyP,
            VK_OEM_4 => KeyCode::LBracket,
            VK_OEM_6 => KeyCode::RBracket,
            VK_RETURN => KeyCode::ReturnKey,
            VK_A => KeyCode::KeyA,
            VK_S => KeyCode::KeyS,
            VK_D => KeyCode::KeyD,
            VK_F => KeyCode::KeyF,
            VK_G => KeyCode::KeyG,
            VK_H => KeyCode::KeyH,
            VK_J => KeyCode::KeyJ,
            VK_K => KeyCode::KeyK,
            VK_L => KeyCode::KeyL,
            VK_OEM_1 => KeyCode::Semicolon,
            VK_OEM_7 => KeyCode::Quote,
            VK_OEM_5 => KeyCode::Backslash,
            VK_Z => KeyCode::KeyZ,
            VK_X => KeyCode::KeyX,
            VK_C => KeyCode::KeyC,
            VK_V => KeyCode::KeyV,
            VK_B => KeyCode::KeyB,
            VK_N => KeyCode::KeyN,
            VK_M => KeyCode::KeyM,
            VK_OEM_COMMA => KeyCode::Comma,
            VK_OEM_PERIOD => KeyCode::Period,
            VK_OEM_2 => KeyCode::Slash,
            VK_LCONTROL => KeyCode::Control,
            VK_RCONTROL => KeyCode::Control,
            VK_CONTROL => KeyCode::Control,
            VK_LMENU => KeyCode::Alt,
            VK_RMENU => KeyCode::Alt,
            VK_MENU => KeyCode::Alt,
            VK_LSHIFT => KeyCode::Shift,
            VK_RSHIFT => KeyCode::Shift,
            VK_SHIFT => KeyCode::Shift,
            VK_LWIN => KeyCode::Logo,
            VK_RWIN => KeyCode::Logo,
            VK_SPACE => KeyCode::Space,
            VK_CAPITAL => KeyCode::Capslock,
            VK_F1 => KeyCode::F1,
            VK_F2 => KeyCode::F2,
            VK_F3 => KeyCode::F3,
            VK_F4 => KeyCode::F4,
            VK_F5 => KeyCode::F5,
            VK_F6 => KeyCode::F6,
            VK_F7 => KeyCode::F7,
            VK_F8 => KeyCode::F8,
            VK_F9 => KeyCode::F9,
            VK_F10 => KeyCode::F10,
            VK_F11 => KeyCode::F11,
            VK_F12 => KeyCode::F12,
            VK_SNAPSHOT => KeyCode::PrintScreen,
            VK_SCROLL => KeyCode::ScrollLock,
            VK_PAUSE => KeyCode::Pause,
            VK_INSERT => KeyCode::Insert,
            VK_DELETE => KeyCode::Delete,
            VK_HOME => KeyCode::Home,
            VK_END => KeyCode::End,
            VK_PRIOR => KeyCode::PageUp,
            VK_NEXT => KeyCode::PageDown,
            VK_NUMPAD0 => KeyCode::Numpad0,
            VK_NUMPAD1 => KeyCode::Numpad1,
            VK_NUMPAD2 => KeyCode::Numpad2,
            VK_NUMPAD3 => KeyCode::Numpad3,
            VK_NUMPAD4 => KeyCode::Numpad4,
            VK_NUMPAD5 => KeyCode::Numpad5,
            VK_NUMPAD6 => KeyCode::Numpad6,
            VK_NUMPAD7 => KeyCode::Numpad7,
            VK_NUMPAD8 => KeyCode::Numpad8,
            VK_NUMPAD9 => KeyCode::Numpad9,
            VK_SUBTRACT => KeyCode::NumpadSubtract,
            VK_ADD => KeyCode::NumpadAdd,
            VK_DECIMAL => KeyCode::NumpadDecimal,
            VK_MULTIPLY => KeyCode::NumpadMultiply,
            VK_DIVIDE => KeyCode::NumpadDivide,
            VK_NUMLOCK => KeyCode::Numlock,
            VK_UP => KeyCode::ArrowUp,
            VK_DOWN => KeyCode::ArrowDown,
            VK_LEFT => KeyCode::ArrowLeft,
            VK_RIGHT => KeyCode::ArrowRight,
            _ => KeyCode::Unknown
        }
    }
}

