#![allow(non_snake_case)]

use {
    crate::{
        area::Area,
        cursor::MouseCursor,
        event::{PopupDismissReason, PopupDismissedEvent, *},
        makepad_math::*,
        os::windows::{
            droptarget::*,
            win32_app::{encode_wide, with_win32_app, Win32App},
            win32_event::*,
        },
        window::{WindowBackdrop, WindowId, WindowVisuals},
        windows::{
            core::PCWSTR,
            //core::IntoParam,
            //core::Result as coreResult,
            //core::HRESULT,
            Win32::{
                Foundation::{
                    COLORREF, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, POINTL, RECT, WPARAM,
                },
                Graphics::{
                    Dwm::{
                        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_MAINWINDOW,
                        DWMSBT_NONE, DWMSBT_TABBEDWINDOW, DWMSBT_TRANSIENTWINDOW,
                        DWMWA_SYSTEMBACKDROP_TYPE,
                    },
                    Gdi::ScreenToClient,
                },
                System::{
                    DataExchange::{
                        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard,
                        SetClipboardData,
                    },
                    LibraryLoader::GetModuleHandleW,
                    Memory::{
                        GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GLOBAL_ALLOC_FLAGS,
                    },
                    Ole::{
                        IDropTarget, RegisterDragDrop, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY,
                        DROPEFFECT_LINK, DROPEFFECT_MOVE,
                    },
                    SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS},
                    WindowsProgramming::GMEM_DDESHARE,
                },
                UI::{
                    Controls::{MARGINS, WM_MOUSELEAVE},
                    Input::{
                        Ime::{
                            ImmAssociateContext, ImmGetCompositionStringW, ImmGetContext,
                            ImmReleaseContext, ImmSetCompositionWindow, CFS_POINT, COMPOSITIONFORM,
                            GCS_COMPSTR, GCS_RESULTSTR, HIMC,
                        },
                        KeyboardAndMouse::{
                            GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE,
                            TRACKMOUSEEVENT, VIRTUAL_KEY, VK_0, VK_1, VK_2, VK_3, VK_4, VK_5, VK_6,
                            VK_7, VK_8, VK_9, VK_A, VK_ADD, VK_B, VK_BACK, VK_C, VK_CAPITAL,
                            VK_CONTROL, VK_D, VK_DECIMAL, VK_DELETE, VK_DIVIDE, VK_DOWN, VK_E,
                            VK_END, VK_ESCAPE, VK_F, VK_F1, VK_F10, VK_F11, VK_F12, VK_F2, VK_F3,
                            VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_G, VK_H, VK_HOME, VK_I,
                            VK_INSERT, VK_J, VK_K, VK_L, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT,
                            VK_LWIN, VK_M, VK_MENU, VK_MULTIPLY, VK_N, VK_NEXT, VK_NUMLOCK,
                            VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5,
                            VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_O, VK_OEM_1,
                            VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7,
                            VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_P, VK_PAUSE,
                            VK_PRIOR, VK_Q, VK_R, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU,
                            VK_RSHIFT, VK_RWIN, VK_S, VK_SCROLL, VK_SHIFT, VK_SNAPSHOT, VK_SPACE,
                            VK_SUBTRACT, VK_T, VK_TAB, VK_U, VK_UP, VK_V, VK_W, VK_X, VK_Y, VK_Z,
                        },
                    },
                    WindowsAndMessaging::{
                        CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
                        GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, MoveWindow,
                        PostMessageW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
                        ShowWindow, CW_USEDEFAULT, GWLP_USERDATA, GWL_EXSTYLE, HTBOTTOM,
                        HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT,
                        HTSYSMENU, HTTOP, HTTOPLEFT, HTTOPRIGHT, HWND_NOTOPMOST, HWND_TOPMOST,
                        LWA_ALPHA, SWP_NOMOVE, SWP_NOSIZE, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
                        SW_SHOW, WA_ACTIVE, WINDOWPLACEMENT, WM_ACTIVATE, WM_CHAR, WM_CLOSE,
                        WM_DESTROY, WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_ERASEBKGND,
                        WM_EXITSIZEMOVE, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION,
                        WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP,
                        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
                        WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCHITTEST, WM_RBUTTONDOWN, WM_RBUTTONUP,
                        WM_SIZE, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
                        WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_ACCEPTFILES, WS_EX_APPWINDOW,
                        WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_WINDOWEDGE,
                        WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SIZEBOX, WS_SYSMENU,
                    },
                },
            },
        },
    },
    std::{
        cell::{Cell, RefCell},
        ffi::{c_void, OsStr},
        mem,
        os::windows::ffi::OsStrExt,
        rc::Rc,
        sync::Arc,
        sync::Mutex,
    },
};

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WindowCompositionAttribData {
    attrib: u32,
    pv_data: *mut c_void,
    cb_data: usize,
}

#[inline]
unsafe fn SetWindowCompositionAttribute(
    hwnd: HWND,
    data: *mut WindowCompositionAttribData,
) -> windows_core::BOOL {
    windows_core::link!("user32.dll" "system" fn SetWindowCompositionAttribute(hwnd : HWND, data : *mut WindowCompositionAttribData) -> windows_core::BOOL);
    unsafe { SetWindowCompositionAttribute(hwnd, data) }
}

// IME candidate-window positioning (not generated in the vendored `windows`
// bindings). `CFS_EXCLUDE` tells the IME to keep its candidate list out of
// `rc_area` (the current text line), so it appears directly above or below the
// line rather than on top of it.
const CFS_EXCLUDE: u32 = 0x0080;

#[repr(C)]
#[derive(Clone, Copy)]
struct CANDIDATEFORM {
    dw_index: u32,
    dw_style: u32,
    pt_current_pos: POINT,
    rc_area: RECT,
}

#[inline]
unsafe fn ImmSetCandidateWindow(himc: HIMC, lpcandidate: *const CANDIDATEFORM) -> windows_core::BOOL {
    windows_core::link!("imm32.dll" "system" fn ImmSetCandidateWindow(himc : HIMC, lpcandidate : *const CANDIDATEFORM) -> windows_core::BOOL);
    unsafe { ImmSetCandidateWindow(himc, lpcandidate) }
}

/*
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
*/
//#[derive(Clone)]
pub struct Win32Window {
    pub window_id: WindowId,
    pub last_window_geom: WindowGeom,

    pub mouse_buttons_down: usize,
    pub last_key_mod: KeyModifiers,
    // Caret/composition line rect in window-relative logical points (size
    // includes the line height); used to keep the IME candidate off the line.
    pub ime_rect: Rect,
    pub current_cursor: MouseCursor,
    pub last_mouse_pos: Vec2d,
    pub ignore_wmsize: usize,
    pub hwnd: HWND,
    pub track_mouse_event: bool,
    pub is_fullscreen: bool,
    pub is_popup: bool,
    ime_saved_himc: HIMC,
}

impl Win32Window {
    // 2-stage initialization (new and init) to connect GWLP_USERDATA

    // create window structure and register drag/drop
    pub fn new(
        window_id: WindowId,
        title: &str,
        position: Option<Vec2d>,
        is_fullscreen: bool,
    ) -> Win32Window {
        let title = encode_wide(title);

        let style = WS_SIZEBOX
            | WS_MAXIMIZEBOX
            | WS_MINIMIZEBOX
            | WS_POPUP
            | WS_CLIPSIBLINGS
            | WS_CLIPCHILDREN
            | WS_SYSMENU;

        let style_ex = WS_EX_WINDOWEDGE | WS_EX_APPWINDOW | WS_EX_ACCEPTFILES;

        let (x, y) = if let Some(position) = position {
            (position.x as i32, position.y as i32)
        } else {
            (CW_USEDEFAULT, CW_USEDEFAULT)
        };

        let hwnd = unsafe {
            CreateWindowExW(
                style_ex,
                PCWSTR(with_win32_app(|app| app.window_class_name.as_ptr())),
                PCWSTR(title.as_ptr()),
                style,
                x,
                y,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(GetModuleHandleW(None).unwrap().into()),
                None,
            )
            .unwrap()
        };

        // create DropTarget object that accesses the same data object, convert to COM and give to Microsoft
        let drop_target: IDropTarget = DropTarget {
            drag_item: RefCell::new(None),
            hwnd,
        }
        .into();
        unsafe { RegisterDragDrop(hwnd, &drop_target).unwrap() };

        Win32Window {
            window_id,
            mouse_buttons_down: 0,
            last_window_geom: WindowGeom::default(),
            last_key_mod: KeyModifiers::default(),
            ime_rect: Rect::default(),
            current_cursor: MouseCursor::Default,
            last_mouse_pos: Vec2d::default(),
            ignore_wmsize: 0,
            hwnd,
            track_mouse_event: false,
            is_fullscreen,
            is_popup: false,
            ime_saved_himc: HIMC::default(),
        }
    }

    pub fn new_popup(window_id: WindowId, position: Vec2d, size: Vec2d) -> Win32Window {
        let title = encode_wide("Makepad Popup");

        let style = WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
        let style_ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW;

        let dpi = with_win32_app(|app| app.dpi_functions.system_dpi_factor() as f64);
        let x = (position.x * dpi) as i32;
        let y = (position.y * dpi) as i32;
        let w = (size.x * dpi) as i32;
        let h = (size.y * dpi) as i32;

        let hwnd = unsafe {
            CreateWindowExW(
                style_ex,
                PCWSTR(with_win32_app(|app| app.window_class_name.as_ptr())),
                PCWSTR(title.as_ptr()),
                style,
                x,
                y,
                w,
                h,
                None,
                None,
                Some(GetModuleHandleW(None).unwrap().into()),
                None,
            )
            .unwrap()
        };

        Win32Window {
            window_id,
            mouse_buttons_down: 0,
            last_window_geom: WindowGeom::default(),
            last_key_mod: KeyModifiers::default(),
            ime_rect: Rect::default(),
            current_cursor: MouseCursor::Default,
            last_mouse_pos: Vec2d::default(),
            ignore_wmsize: 0,
            hwnd,
            track_mouse_event: false,
            is_fullscreen: false,
            is_popup: true,
            ime_saved_himc: HIMC::default(),
        }
    }

    // initialize GWLP_USERDATA and registration of global stuff, and set outer size
    pub fn init(&mut self, size: Vec2d) {
        unsafe { SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, self as *const _ as isize) };

        with_win32_app(|app| app.dpi_functions.enable_non_client_dpi_scaling(self.hwnd));
        with_win32_app(|app| app.all_windows.push(self.hwnd));
        self.set_outer_size(size);
        if self.is_fullscreen {
            self.maximize();
        }
    }

    /// Reads an IME composition string (`GCS_COMPSTR` for the in-progress
    /// preedit, or `GCS_RESULTSTR` for the committed text) from the input
    /// context as a Rust `String`. Returns `Some("")` for an empty string and
    /// `None` only on error.
    unsafe fn imm_get_composition_string(himc: HIMC, index: u32) -> Option<String> {
        // A null buffer makes ImmGetCompositionStringW return the required byte
        // length (the W variant returns UTF-16 code units, i.e. 2 bytes each).
        let byte_len = ImmGetCompositionStringW(himc, index, std::ptr::null_mut(), 0);
        if byte_len < 0 {
            return None;
        }
        if byte_len == 0 {
            return Some(String::new());
        }
        let mut buf = vec![0u16; byte_len as usize / 2];
        let written = ImmGetCompositionStringW(
            himc,
            index,
            buf.as_mut_ptr() as *mut c_void,
            byte_len as u32,
        );
        if written <= 0 {
            return Some(String::new());
        }
        let len = (written as usize / 2).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    pub unsafe extern "system" fn window_class_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let user_data = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if user_data == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };

        let window = &mut (*(user_data as *mut Win32Window));
        match msg {
            WM_ACTIVATE => {
                if wparam.0 & 0xffff == WA_ACTIVE as usize {
                    window.do_callback(Win32Event::WindowGotFocus(window.window_id));
                } else {
                    if window.is_popup {
                        window.do_callback(Win32Event::PopupDismissed(PopupDismissedEvent {
                            window_id: window.window_id,
                            reason: PopupDismissReason::FocusLost,
                        }));
                    } else {
                        window.do_callback(Win32Event::WindowLostFocus(window.window_id));
                    }
                }
            }
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
                        cyBottomHeight: 1,
                    };
                    DwmExtendFrameIntoClientArea(hwnd, &margins).unwrap();
                    return LRESULT(0);
                }
            }
            WM_NCHITTEST => {
                //let ycoord = (lparam.0 >> 16) as u16 as i16 as i32;
                //let xcoord = (lparam.0 & 0xffff) as u16 as i16 as i32;
                let abs = window.get_mouse_pos_from_lparam(lparam);
                let mut rect = RECT {
                    left: 0,
                    top: 0,
                    bottom: 0,
                    right: 0,
                };
                const EDGE: f64 = 4.0;
                let dpi = window.get_dpi_factor();
                GetWindowRect(hwnd, &mut rect).unwrap();
                let rect = Rect {
                    pos: dvec2(rect.left as f64 / dpi, rect.top as f64 / dpi),
                    size: dvec2(
                        (rect.right - rect.left) as f64 / dpi,
                        (rect.bottom - rect.top) as f64 / dpi,
                    ),
                };
                if abs.x < rect.pos.x + EDGE {
                    if abs.y < rect.pos.y + EDGE {
                        with_win32_app(|app| app.set_mouse_cursor(MouseCursor::NwseResize));
                        return LRESULT(HTTOPLEFT as isize);
                    }
                    if abs.y > rect.pos.y + rect.size.y - EDGE {
                        with_win32_app(|app| app.set_mouse_cursor(MouseCursor::NeswResize));
                        return LRESULT(HTBOTTOMLEFT as isize);
                    }
                    with_win32_app(|app| app.set_mouse_cursor(MouseCursor::EwResize));
                    return LRESULT(HTLEFT as isize);
                }
                if abs.x > rect.pos.x + rect.size.x - EDGE {
                    if abs.y < rect.pos.y + EDGE {
                        with_win32_app(|app| app.set_mouse_cursor(MouseCursor::NeswResize));
                        return LRESULT(HTTOPRIGHT as isize);
                    }
                    if abs.y > rect.pos.y + rect.size.y - EDGE {
                        with_win32_app(|app| app.set_mouse_cursor(MouseCursor::NwseResize));
                        return LRESULT(HTBOTTOMRIGHT as isize);
                    }
                    with_win32_app(|app| app.set_mouse_cursor(MouseCursor::EwResize));
                    return LRESULT(HTRIGHT as isize);
                }
                if abs.y < rect.pos.y + EDGE {
                    with_win32_app(|app| app.set_mouse_cursor(MouseCursor::NsResize));
                    return LRESULT(HTTOP as isize);
                }
                if abs.y > rect.pos.y + rect.size.y - EDGE {
                    with_win32_app(|app| app.set_mouse_cursor(MouseCursor::NsResize));
                    return LRESULT(HTBOTTOM as isize);
                }
                let response = Rc::new(Cell::new(WindowDragQueryResponse::NoAnswer));
                window.do_callback(Win32Event::WindowDragQuery(WindowDragQueryEvent {
                    window_id: window.window_id,
                    abs: window.get_mouse_pos_from_lparam(lparam) - rect.pos,
                    response: response.clone(),
                }));
                match response.get() {
                    WindowDragQueryResponse::Client => {
                        return LRESULT(HTCLIENT as isize);
                    }
                    WindowDragQueryResponse::Caption => {
                        with_win32_app(|app| app.set_mouse_cursor(MouseCursor::Default));
                        return LRESULT(HTCAPTION as isize);
                    }
                    WindowDragQueryResponse::SysMenu => {
                        with_win32_app(|app| app.set_mouse_cursor(MouseCursor::Default));
                        return LRESULT(HTSYSMENU as isize);
                    }
                    _ => (),
                }
                return LRESULT(HTCLIENT as isize);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_MOUSEMOVE => {
                if with_win32_app(|app| app.start_dragging_items.is_some()) {
                    return LRESULT(0);
                }
                if !window.track_mouse_event {
                    window.track_mouse_event = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut tme).unwrap();
                }
                window.send_mouse_move(
                    window.get_mouse_pos_from_lparam(lparam),
                    Self::get_key_modifiers(),
                )
            }
            WM_MOUSELEAVE => {
                if with_win32_app(|app| app.start_dragging_items.is_some()) {
                    return LRESULT(0);
                }
                window.track_mouse_event = false;
                window.send_mouse_leave(window.last_mouse_pos, Self::get_key_modifiers());
                with_win32_app(|app| app.current_cursor = Some(MouseCursor::Hidden));
            }
            WM_MOUSEWHEEL => {
                let delta = (wparam.0 >> 16) as u16 as i16 as f64;
                window.send_scroll(Vec2d { x: 0.0, y: -delta }, Self::get_key_modifiers(), true);
            }
            WM_LBUTTONDOWN => {
                // hack for drag/drop: save which window was last clicked on in win32_app
                with_win32_app(|app| app.currently_clicked_window_id = Some(window.window_id));
                window.send_mouse_down(MouseButton::PRIMARY, Self::get_key_modifiers());
            }
            WM_LBUTTONUP => window.send_mouse_up(MouseButton::PRIMARY, Self::get_key_modifiers()),
            WM_RBUTTONDOWN => {
                window.send_mouse_down(MouseButton::SECONDARY, Self::get_key_modifiers())
            }
            WM_RBUTTONUP => window.send_mouse_up(MouseButton::SECONDARY, Self::get_key_modifiers()),
            WM_MBUTTONDOWN => {
                window.send_mouse_down(MouseButton::MIDDLE, Self::get_key_modifiers())
            }
            WM_MBUTTONUP => window.send_mouse_up(MouseButton::MIDDLE, Self::get_key_modifiers()),
            // All other mouse buttons are handled as "XBUTTON"s.
            // Their specific button value is obtained via the "hiword" (bits 16..32) of the `wparam` value.
            // The back mouse button is XBUTTON1 (value 0x1); the forward button is XBUTTON2 (value 0x2).
            // Thus, we add `2` to the XBUTTON value in order to get the `MouseButton` value of `3` for BACK and `4` for FORWARD.
            WM_XBUTTONDOWN => {
                let wparam_hiword = (wparam.0 >> 16) & 0xFFFF;
                let raw_button = wparam_hiword + 2;
                window.send_mouse_down(
                    MouseButton::from_raw_button(raw_button),
                    Self::get_key_modifiers(),
                );
            }
            WM_XBUTTONUP => {
                let wparam_hiword = (wparam.0 >> 16) & 0xFFFF;
                let raw_button = wparam_hiword + 2;
                window.send_mouse_up(
                    MouseButton::from_raw_button(raw_button),
                    Self::get_key_modifiers(),
                );
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                // detect control/cmd - c / v / x
                let modifiers = Self::get_key_modifiers();
                let key_code = Self::virtual_key_to_key_code(wparam);
                if window.is_popup && key_code == KeyCode::Escape {
                    window.do_callback(Win32Event::PopupDismissed(PopupDismissedEvent {
                        window_id: window.window_id,
                        reason: PopupDismissReason::Escape,
                    }));
                    return LRESULT(0);
                }
                if modifiers.alt && key_code == KeyCode::F4 {
                    PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)).unwrap();
                }
                if modifiers.control || modifiers.logo {
                    match key_code {
                        KeyCode::KeyV => {
                            // paste
                            if let Ok(()) = OpenClipboard(None) {
                                let mut data: Vec<u16> = Vec::new();
                                let h_clipboard_data =
                                    GetClipboardData(CF_UNICODETEXT.0 as u32).unwrap();
                                let h_clipboard_ptr =
                                    GlobalLock(std::mem::transmute::<_, HGLOBAL>(h_clipboard_data))
                                        as *mut u16;
                                let clipboard_size =
                                    GlobalSize(std::mem::transmute::<_, HGLOBAL>(h_clipboard_data));
                                if clipboard_size > 2 {
                                    data.resize((clipboard_size >> 1) - 1, 0);
                                    std::ptr::copy_nonoverlapping(
                                        h_clipboard_ptr,
                                        data.as_mut_ptr(),
                                        data.len(),
                                    );
                                    GlobalUnlock(std::mem::transmute::<_, HGLOBAL>(
                                        h_clipboard_data,
                                    ))
                                    .unwrap();
                                    CloseClipboard().unwrap();
                                    if let Ok(utf8) = String::from_utf16(&data) {
                                        window.do_callback(Win32Event::TextInput(TextInputEvent {
                                            input: utf8,
                                            was_paste: true,
                                            replace_last: false,
                                            ..Default::default()
                                        }));
                                    }
                                } else {
                                    GlobalUnlock(std::mem::transmute::<_, HGLOBAL>(
                                        h_clipboard_data,
                                    ))
                                    .unwrap();
                                    CloseClipboard().unwrap();
                                }
                            }
                        }
                        KeyCode::KeyC => {
                            let response = Rc::new(RefCell::new(None));
                            window.do_callback(Win32Event::TextCopy(TextClipboardEvent {
                                response: response.clone(),
                            }));
                            let response = response.borrow();
                            if let Some(response) = response.as_ref() {
                                Self::copy_to_clipboard(response);
                            }
                        }
                        KeyCode::KeyX => {
                            let response = Rc::new(RefCell::new(None));
                            window.do_callback(Win32Event::TextCut(TextClipboardEvent {
                                response: response.clone(),
                            }));
                            let response = response.borrow();
                            if let Some(response) = response.as_ref() {
                                Self::copy_to_clipboard(response);
                            }
                        }
                        _ => (),
                    }
                }
                window.do_callback(Win32Event::KeyDown(KeyEvent {
                    key_code: key_code,
                    // lParam bit 30 is the previous key state: set means this is an auto-repeat.
                    is_repeat: (lparam.0 & 0x4000_0000) != 0,
                    modifiers: modifiers,
                    time: window.time_now(),
                }));
            }
            WM_KEYUP | WM_SYSKEYUP => {
                window.do_callback(Win32Event::KeyUp(KeyEvent {
                    key_code: Self::virtual_key_to_key_code(wparam),
                    is_repeat: false,
                    modifiers: Self::get_key_modifiers(),
                    time: window.time_now(),
                }));
            }
            WM_CHAR => {
                if let Ok(utf8) = String::from_utf16(&[wparam.0 as u16]) {
                    let char_code = utf8.chars().next().unwrap();
                    if char_code >= ' ' {
                        window.do_callback(Win32Event::TextInput(TextInputEvent {
                            input: utf8,
                            was_paste: false,
                            replace_last: false,
                            ..Default::default()
                        }));
                    }
                }
            }
            WM_IME_STARTCOMPOSITION => {
                let rect = window.ime_rect;
                if rect.size.y > 0.0 {
                    let himc = ImmGetContext(hwnd);
                    if !himc.is_invalid() {
                        let dpi_factor = window.get_dpi_factor();
                        let left = (rect.pos.x * dpi_factor) as i32;
                        let top = (rect.pos.y * dpi_factor) as i32;
                        let right = ((rect.pos.x + rect.size.x) * dpi_factor) as i32;
                        let bottom = ((rect.pos.y + rect.size.y) * dpi_factor) as i32;
                        // Inflate the excluded line vertically (by a fraction of the
                        // line height) so the candidate list keeps a gap from the
                        // text rather than hugging it. Matches the macOS clearance.
                        let clearance = (rect.size.y * dpi_factor * 0.6) as i32;
                        // Anchor the (makepad-drawn) composition string at the caret.
                        let caret = POINT { x: left, y: bottom };
                        let _ = ImmSetCompositionWindow(
                            himc,
                            &COMPOSITIONFORM {
                                dwStyle: CFS_POINT,
                                ptCurrentPos: caret,
                                rcArea: RECT::default(),
                            },
                        );
                        // Exclude the whole text line so the candidate list pops up
                        // directly above or below it instead of covering the text.
                        let _ = ImmSetCandidateWindow(
                            himc,
                            &CANDIDATEFORM {
                                dw_index: 0,
                                dw_style: CFS_EXCLUDE,
                                pt_current_pos: caret,
                                rc_area: RECT {
                                    left,
                                    top: top - clearance,
                                    right,
                                    bottom: bottom + clearance,
                                },
                            },
                        );
                        let _ = ImmReleaseContext(hwnd, himc);
                    }
                }
            }
            WM_IME_COMPOSITION => {
                let himc = ImmGetContext(hwnd);
                if !himc.is_invalid() {
                    let flags = lparam.0 as u32;
                    // GCS_RESULTSTR: the finalized text. Commit it with
                    // `replace_last = false`, which replaces any active composition
                    // preview and then clears the composition. We commit here (and
                    // consume the message below) so DefWindowProc does NOT also
                    // synthesize WM_CHAR for the same result and double-insert.
                    if flags & GCS_RESULTSTR != 0 {
                        if let Some(result) =
                            Self::imm_get_composition_string(himc, GCS_RESULTSTR)
                        {
                            if !result.is_empty() {
                                window.do_callback(Win32Event::TextInput(TextInputEvent {
                                    input: result,
                                    was_paste: false,
                                    replace_last: false,
                                    ..Default::default()
                                }));
                            }
                        }
                    }
                    // GCS_COMPSTR: the in-progress preedit. Show it inline with
                    // `replace_last = true`; an empty string clears the preview.
                    if flags & GCS_COMPSTR != 0 {
                        let comp = Self::imm_get_composition_string(himc, GCS_COMPSTR)
                            .unwrap_or_default();
                        window.do_callback(Win32Event::TextInput(TextInputEvent {
                            input: comp,
                            was_paste: false,
                            replace_last: true,
                            ..Default::default()
                        }));
                    }
                    let _ = ImmReleaseContext(hwnd, himc);
                }
                // Falls through to `return LRESULT(1)`, consuming the message so
                // DefWindowProc draws no default composition window and synthesizes
                // no WM_CHAR/WM_IME_CHAR for the result handled above.
            }
            WM_IME_ENDCOMPOSITION => {
                // Composition finished or was cancelled. Clear any leftover inline
                // preview (a no-op if it was already committed/cleared). This
                // handles IMEs that end composition without first sending an empty
                // GCS_COMPSTR (e.g. some Escape/cancel paths).
                window.do_callback(Win32Event::TextInput(TextInputEvent {
                    input: String::new(),
                    was_paste: false,
                    replace_last: true,
                    ..Default::default()
                }));
            }
            WM_ENTERSIZEMOVE => {
                with_win32_app(|app| app.start_resize());
                window.do_callback(Win32Event::WindowResizeLoopStart(window.window_id));
            }
            WM_EXITSIZEMOVE => {
                with_win32_app(|app| app.stop_resize());
                window.do_callback(Win32Event::WindowResizeLoopStop(window.window_id));
            }
            // WM_SIZING (0x0214) fires BEFORE the window is resized with
            // the proposed new rect. By pre-rendering at this size, the
            // swap chain frame is ready when DWM composites the window at
            // the new size, eliminating the empty gap at growing edges.
            0x0214 => {
                let proposed_rect = &*(lparam.0 as *const RECT);
                window.send_sizing_event(proposed_rect);
            }
            WM_SIZE | WM_DPICHANGED => {
                window.send_change_event();
            }
            WM_CLOSE => {
                // close requested
                let accept_close = Rc::new(Cell::new(true));
                window.do_callback(Win32Event::WindowCloseRequested(
                    WindowCloseRequestedEvent {
                        window_id: window.window_id,
                        accept_close: accept_close.clone(),
                    },
                ));
                if accept_close.get() {
                    DestroyWindow(hwnd).unwrap();
                }
            }
            WM_DESTROY => {
                // window actively destroyed
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                window.do_callback(Win32Event::WindowClosed(WindowClosedEvent {
                    window_id: window.window_id,
                }));
            }

            // from DropTarget
            WM_DROPTARGET => {
                // restore the Box<>
                let message = unsafe { Box::from_raw(lparam.0 as *mut DropTargetMessage) };

                match *message {
                    DropTargetMessage::Leave => {
                        if with_win32_app(|app| app.is_dragging_internal.get()) {
                            // TODO: cancel DoDragDrop somehow
                            window.do_callback(Win32Event::DragEnd);
                        }
                    }
                    DropTargetMessage::Enter(flags, mut point, effect, drag_item)
                    | DropTargetMessage::Over(flags, mut point, effect, drag_item) => {
                        // decode message
                        let _ = unsafe {
                            ScreenToClient(window.hwnd, &mut point as *mut POINTL as *mut POINT)
                        };
                        let response = if (effect & DROPEFFECT_LINK) != DROPEFFECT(0) {
                            DragResponse::Link
                        } else if (effect & DROPEFFECT_MOVE) != DROPEFFECT(0) {
                            DragResponse::Move
                        } else if (effect & DROPEFFECT_COPY) != DROPEFFECT(0) {
                            DragResponse::Copy
                        } else {
                            DragResponse::None
                        };

                        let dpi_factor = window.get_dpi_factor();

                        // send to makepad
                        window.do_callback(Win32Event::Drag(DragEvent {
                            modifiers: KeyModifiers {
                                shift: (flags & MK_SHIFT) != MODIFIERKEYS_FLAGS(0),
                                control: (flags & MK_CONTROL) != MODIFIERKEYS_FLAGS(0),
                                alt: false,  // TODO
                                logo: false, // Windows doesn't have a logo button
                            },
                            handled: Arc::new(Mutex::new(false)),
                            abs: Vec2d {
                                x: point.x as f64 / dpi_factor,
                                y: point.y as f64 / dpi_factor,
                            },
                            items: Arc::new(vec![drag_item]),
                            response: Arc::new(Mutex::new(response)),
                        }));
                    }

                    DropTargetMessage::Drop(flags, mut point, _effect, drag_item) => {
                        // decode message
                        let _ = unsafe {
                            ScreenToClient(window.hwnd, &mut point as *mut POINTL as *mut POINT)
                        };

                        //log!("dropping at ({},{}), flags: {:04X}, response: {:?}, drag_item: {:?}",point.x,point.y,flags.0,response,drag_item);
                        let dpi_factor = window.get_dpi_factor();

                        // send to makepad
                        window.do_callback(Win32Event::Drop(DropEvent {
                            modifiers: KeyModifiers {
                                shift: (flags & MK_SHIFT) != MODIFIERKEYS_FLAGS(0),
                                control: (flags & MK_CONTROL) != MODIFIERKEYS_FLAGS(0),
                                alt: false,  // TODO
                                logo: false, // Windows doesn't have a logo button
                            },
                            handled: Arc::new(Mutex::new(false)),
                            abs: Vec2d {
                                x: point.x as f64 / dpi_factor,
                                y: point.y as f64 / dpi_factor,
                            },
                            items: Arc::new(vec![drag_item]),
                        }));

                        window.do_callback(Win32Event::DragEnd);
                    }
                }
            }

            _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
        }
        return LRESULT(1);
        // lets get the window
        // Unwinding into foreign code is undefined behavior. So we catch any panics that occur in our
        // code, and if a panic happens we cancel any future operations.
        //run_catch_panic(-1, || callback_inner(window, msg, wparam, lparam))
    }

    pub unsafe fn copy_to_clipboard(text: &String) {
        // plug it into the windows clipboard
        // make utf16 dta
        if let Ok(()) = OpenClipboard(None) {
            EmptyClipboard().unwrap();

            let data: Vec<u16> = OsStr::new(text)
                .encode_wide()
                .chain(Some(0).into_iter())
                .collect();

            let h_clipboard_data = GlobalAlloc(GLOBAL_ALLOC_FLAGS(GMEM_DDESHARE), 2 * data.len())
                .expect("GlobalAlloc for clipboard failed");

            let h_clipboard_ptr = GlobalLock(h_clipboard_data) as *mut u16;

            std::ptr::copy_nonoverlapping(data.as_ptr(), h_clipboard_ptr, data.len());

            GlobalUnlock(h_clipboard_data).unwrap();
            SetClipboardData(
                CF_UNICODETEXT.0 as u32,
                Some(std::mem::transmute::<_, HANDLE>(h_clipboard_data)),
            )
            .unwrap();
            CloseClipboard().unwrap();
        }
    }

    pub fn get_mouse_pos_from_lparam(&self, lparam: LPARAM) -> Vec2d {
        let dpi = self.get_dpi_factor();
        let ycoord = (lparam.0 >> 16) as u16 as i16 as f64;
        let xcoord = (lparam.0 & 0xffff) as u16 as i16 as f64;
        Vec2d {
            x: xcoord / dpi,
            y: ycoord / dpi,
        }
    }

    pub fn get_key_modifiers() -> KeyModifiers {
        unsafe {
            KeyModifiers {
                control: GetKeyState(VK_CONTROL.0 as i32) & 0x80 > 0,
                shift: GetKeyState(VK_SHIFT.0 as i32) & 0x80 > 0,
                alt: GetKeyState(VK_MENU.0 as i32) & 0x80 > 0,
                logo: GetKeyState(VK_LWIN.0 as i32) & 0x80 > 0
                    || GetKeyState(VK_RWIN.0 as i32) & 0x80 > 0,
            }
        }
    }

    pub fn on_mouse_move(&self) {}

    pub fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {}

    pub fn restore(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_RESTORE);
            PostMessageW(Some(self.hwnd), WM_SIZE, WPARAM(0), LPARAM(0)).unwrap();
        }
    }

    pub fn maximize(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_MAXIMIZE);
            PostMessageW(Some(self.hwnd), WM_SIZE, WPARAM(0), LPARAM(0)).unwrap();
        }
    }

    pub fn close_window(&self) {
        unsafe {
            DestroyWindow(self.hwnd).unwrap();
        }
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
        }
    }

    pub fn minimize(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
        }
    }

    pub fn set_topmost(&self, topmost: bool) {
        unsafe {
            if topmost {
                SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE,
                )
                .unwrap();
            } else {
                SetWindowPos(
                    self.hwnd,
                    Some(HWND_NOTOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE,
                )
                .unwrap();
            }
        }
    }

    pub fn get_is_topmost(&self) -> bool {
        unsafe {
            let ex_style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            if ex_style as u32 & WS_EX_TOPMOST.0 != 0 {
                return true;
            }
            return false;
        }
    }

    pub fn get_window_geom(&self) -> WindowGeom {
        // Three caption buttons (minimize / maximize / close), each 46 × 29 logical px,
        // right-aligned at the top of the caption bar.
        const BUTTON_W: f64 = 46.0;
        const BUTTON_H: f64 = 29.0;
        const BUTTON_COUNT: f64 = 3.0;
        const BUTTONS_W: f64 = BUTTON_W * BUTTON_COUNT;
        let inner_size = if self.get_is_maximized() {
            self.get_outer_size()
        } else {
            self.get_inner_size()
        };
        WindowGeom {
            xr_is_presenting: false,
            can_fullscreen: false,
            is_topmost: self.get_is_topmost(),
            is_fullscreen: self.get_is_maximized(),
            inner_size,
            outer_size: self.get_outer_size(),
            dpi_factor: self.get_dpi_factor(),
            position: self.get_position(),
            window_chrome_buttons: Rect {
                pos: Vec2d {
                    x: inner_size.x - BUTTONS_W,
                    y: 0.0,
                },
                size: Vec2d {
                    x: BUTTONS_W,
                    y: BUTTON_H,
                },
            },
            ..Default::default()
        }
    }

    pub fn get_is_maximized(&self) -> bool {
        unsafe {
            let wp: mem::MaybeUninit<WINDOWPLACEMENT> = mem::MaybeUninit::uninit();
            let mut wp = wp.assume_init();
            wp.length = mem::size_of::<WINDOWPLACEMENT>() as u32;
            GetWindowPlacement(self.hwnd, &mut wp).unwrap();
            if wp.showCmd == SW_MAXIMIZE.0 as u32 {
                return true;
            }
            return false;
        }
    }

    pub fn time_now(&self) -> f64 {
        with_win32_app(|app| app.time_now())
    }

    pub fn set_ime_rect(&mut self, rect: Rect) {
        self.ime_rect = rect;
    }

    pub fn get_position(&self) -> Vec2d {
        unsafe {
            let mut rect = RECT {
                left: 0,
                top: 0,
                bottom: 0,
                right: 0,
            };
            GetWindowRect(self.hwnd, &mut rect).unwrap();
            Vec2d {
                x: rect.left as f64,
                y: rect.top as f64,
            }
        }
    }

    pub fn get_inner_size(&self) -> Vec2d {
        unsafe {
            let mut rect = RECT {
                left: 0,
                top: 0,
                bottom: 0,
                right: 0,
            };
            GetClientRect(self.hwnd, &mut rect).unwrap();
            let dpi = self.get_dpi_factor();
            Vec2d {
                x: (rect.right - rect.left) as f64 / dpi,
                y: (rect.bottom - rect.top) as f64 / dpi,
            }
        }
    }

    pub fn get_outer_size(&self) -> Vec2d {
        unsafe {
            let mut rect = RECT {
                left: 0,
                top: 0,
                bottom: 0,
                right: 0,
            };
            GetWindowRect(self.hwnd, &mut rect).unwrap();
            let dpi = self.get_dpi_factor();
            Vec2d {
                x: (rect.right - rect.left) as f64 / dpi,
                y: (rect.bottom - rect.top) as f64 / dpi,
            }
        }
    }

    pub fn set_position(&mut self, pos: Vec2d) {
        unsafe {
            let mut window_rect = RECT {
                left: 0,
                top: 0,
                bottom: 0,
                right: 0,
            };
            GetWindowRect(self.hwnd, &mut window_rect).unwrap();
            let dpi = self.get_dpi_factor();
            MoveWindow(
                self.hwnd,
                (pos.x * dpi) as i32,
                (pos.y * dpi) as i32,
                window_rect.right - window_rect.left,
                window_rect.bottom - window_rect.top,
                false,
            )
            .unwrap();
        }
    }

    pub fn set_outer_size(&self, size: Vec2d) {
        unsafe {
            let mut window_rect = RECT {
                left: 0,
                top: 0,
                bottom: 0,
                right: 0,
            };
            GetWindowRect(self.hwnd, &mut window_rect).unwrap();
            let dpi = self.get_dpi_factor();
            MoveWindow(
                self.hwnd,
                window_rect.left,
                window_rect.top,
                (size.x * dpi) as i32,
                (size.y * dpi) as i32,
                false,
            )
            .unwrap();
        }
    }

    pub fn apply_window_visuals(&mut self, visuals: WindowVisuals) {
        const WCA_ACCENT_POLICY: u32 = 19;
        const ACCENT_DISABLED: u32 = 0;
        const ACCENT_ENABLE_BLURBEHIND: u32 = 3;

        let intensity = visuals.backdrop_intensity.clamp(0.0, 1.0);
        let accent_alpha = (intensity * 255.0).round() as u32;
        let accent_color = accent_alpha << 24;

        let backdrop = match visuals.backdrop {
            WindowBackdrop::None => DWMSBT_NONE,
            WindowBackdrop::Auto | WindowBackdrop::Mica => DWMSBT_MAINWINDOW,
            WindowBackdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
            WindowBackdrop::Vibrancy | WindowBackdrop::Blur => DWMSBT_TABBEDWINDOW,
        };

        unsafe {
            let mut ex_style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
            if visuals.transparent {
                ex_style |= WS_EX_LAYERED.0;
            } else {
                ex_style &= !WS_EX_LAYERED.0;
            }
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex_style as isize);
            let _ = SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 255, LWA_ALPHA);

            let margins = if visuals.transparent {
                MARGINS {
                    cxLeftWidth: -1,
                    cxRightWidth: -1,
                    cyTopHeight: -1,
                    cyBottomHeight: -1,
                }
            } else {
                MARGINS {
                    cxLeftWidth: 0,
                    cxRightWidth: 0,
                    cyTopHeight: 0,
                    cyBottomHeight: 0,
                }
            };
            DwmExtendFrameIntoClientArea(self.hwnd, &margins).unwrap();
        }

        let hr = unsafe {
            DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &(backdrop.0) as *const _ as *const c_void,
                std::mem::size_of::<i32>() as u32,
            )
        };

        let accent_state = if visuals.transparent || visuals.backdrop != WindowBackdrop::None {
            ACCENT_ENABLE_BLURBEHIND
        } else {
            ACCENT_DISABLED
        };
        let gradient_color = if visuals.backdrop == WindowBackdrop::None {
            0
        } else {
            accent_color
        };
        let mut accent = AccentPolicy {
            accent_state,
            accent_flags: if hr.is_ok() { 0x20 } else { 0 },
            gradient_color,
            animation_id: 0,
        };
        let mut data = WindowCompositionAttribData {
            attrib: WCA_ACCENT_POLICY,
            pv_data: &mut accent as *mut _ as *mut c_void,
            cb_data: std::mem::size_of::<AccentPolicy>(),
        };
        unsafe {
            let _ = SetWindowCompositionAttribute(self.hwnd, &mut data);
        }
    }

    pub fn set_inner_size(&self, size: Vec2d) {
        unsafe {
            let mut window_rect = RECT {
                left: 0,
                top: 0,
                bottom: 0,
                right: 0,
            };
            GetWindowRect(self.hwnd, &mut window_rect).unwrap();
            let mut client_rect = RECT {
                left: 0,
                top: 0,
                bottom: 0,
                right: 0,
            };
            GetClientRect(self.hwnd, &mut client_rect).unwrap();
            let dpi = self.get_dpi_factor();
            MoveWindow(
                self.hwnd,
                window_rect.left,
                window_rect.top,
                (size.x * dpi) as i32
                    + ((window_rect.right - window_rect.left)
                        - (client_rect.right - client_rect.left)),
                (size.y * dpi) as i32
                    + ((window_rect.bottom - window_rect.top)
                        - (client_rect.bottom - client_rect.top)),
                false,
            )
            .unwrap();
        }
    }

    pub fn get_dpi_factor(&self) -> f64 {
        with_win32_app(|app| app.dpi_functions.hwnd_dpi_factor(self.hwnd) as f64)
    }

    pub fn do_callback(&mut self, event: Win32Event) {
        Win32App::do_callback(event);
    }

    pub fn send_change_event(&mut self) {
        let new_geom = self.get_window_geom();
        let old_geom = self.last_window_geom.clone();
        self.last_window_geom = new_geom.clone();

        self.do_callback(Win32Event::WindowGeomChange(WindowGeomChangeEvent {
            window_id: self.window_id,
            old_geom: old_geom,
            new_geom: new_geom,
        }));
        self.do_callback(Win32Event::Paint);
    }

    /// Pre-render at a proposed window size from WM_SIZING. This fires
    /// BEFORE the window is actually resized, so the swap chain frame is
    /// ready when DWM composites the window at the new size — eliminating
    /// the empty-edge gap that appears when growing the window.
    pub fn send_sizing_event(&mut self, proposed_rect: &RECT) {
        let dpi = self.get_dpi_factor();
        let proposed_size = Vec2d {
            x: (proposed_rect.right - proposed_rect.left) as f64 / dpi,
            y: (proposed_rect.bottom - proposed_rect.top) as f64 / dpi,
        };

        let mut new_geom = self.last_window_geom.clone();
        // For custom chrome, inner size == outer size.
        new_geom.inner_size = proposed_size;
        new_geom.outer_size = proposed_size;
        new_geom.position = Vec2d {
            x: proposed_rect.left as f64,
            y: proposed_rect.top as f64,
        };

        let old_geom = self.last_window_geom.clone();
        if old_geom.inner_size == new_geom.inner_size {
            return; // Size didn't change (e.g. just a move), nothing to pre-render.
        }
        // Skip degenerate sizes — ResizeBuffers rejects zero dimensions.
        if proposed_size.x < 1.0 || proposed_size.y < 1.0 {
            return;
        }
        self.last_window_geom = new_geom.clone();

        self.do_callback(Win32Event::WindowGeomChange(WindowGeomChangeEvent {
            window_id: self.window_id,
            old_geom,
            new_geom,
        }));
        self.do_callback(Win32Event::Paint);
    }

    pub fn send_focus_event(&mut self) {
        self.do_callback(Win32Event::WindowGotFocus(self.window_id));
    }

    pub fn send_focus_lost_event(&mut self) {
        self.do_callback(Win32Event::WindowLostFocus(self.window_id));
    }

    pub fn send_mouse_down(&mut self, button: MouseButton, modifiers: KeyModifiers) {
        if self.mouse_buttons_down == 0 {
            unsafe {
                SetCapture(self.hwnd);
            }
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

    pub fn send_mouse_up(&mut self, button: MouseButton, modifiers: KeyModifiers) {
        if self.mouse_buttons_down > 1 {
            self.mouse_buttons_down -= 1;
        } else {
            unsafe {
                ReleaseCapture().unwrap();
            }
            self.mouse_buttons_down = 0;
        }
        self.do_callback(Win32Event::MouseUp(MouseUpEvent {
            button,
            modifiers,
            window_id: self.window_id,
            abs: self.last_mouse_pos,
            time: self.time_now(),
        }));
    }

    pub fn send_mouse_move(&mut self, pos: Vec2d, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        self.do_callback(Win32Event::MouseMove(MouseMoveEvent {
            window_id: self.window_id,
            abs: pos,
            modifiers: modifiers,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
    }

    pub fn send_mouse_leave(&mut self, pos: Vec2d, modifiers: KeyModifiers) {
        self.last_mouse_pos = pos;
        self.do_callback(Win32Event::MouseLeave(MouseLeaveEvent {
            window_id: self.window_id,
            abs: pos,
            modifiers: modifiers,
            time: self.time_now(),
            handled: Cell::new(Area::Empty),
        }));
    }

    pub fn send_scroll(&mut self, scroll: Vec2d, modifiers: KeyModifiers, is_mouse: bool) {
        self.do_callback(Win32Event::Scroll(ScrollEvent {
            window_id: self.window_id,
            scroll,
            abs: self.last_mouse_pos,
            modifiers,
            time: self.time_now(),
            is_mouse,
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
        }));
    }

    pub fn send_close_requested_event(&mut self) -> bool {
        let accept_close = Rc::new(Cell::new(true));
        self.do_callback(Win32Event::WindowCloseRequested(
            WindowCloseRequestedEvent {
                window_id: self.window_id,
                accept_close: accept_close.clone(),
            },
        ));
        if !accept_close.get() {
            return false;
        }
        true
    }

    pub fn send_text_input(&mut self, input: String, replace_last: bool) {
        self.do_callback(Win32Event::TextInput(TextInputEvent {
            input: input,
            was_paste: false,
            replace_last: replace_last,
            ..Default::default()
        }))
    }

    pub fn set_ime_active(&mut self, active: bool) {
        if active {
            if !self.ime_saved_himc.is_invalid() {
                unsafe { ImmAssociateContext(self.hwnd, self.ime_saved_himc) };
                self.ime_saved_himc = HIMC::default();
            }
        } else {
            if self.ime_saved_himc.is_invalid() {
                self.ime_saved_himc = unsafe { ImmAssociateContext(self.hwnd, HIMC::default()) };
            }
        }
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
            _ => KeyCode::Unknown,
        }
    }
}
