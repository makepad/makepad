#![allow(non_camel_case_types)]

use std::ffi::c_void;

pub type HWND = isize;
pub type LPARAM = isize;
pub type LRESULT = isize;
pub type WPARAM = usize;
pub type HINSTANCE = isize;
pub type HICON = isize;
pub type HCURSOR = isize;
pub type HBRUSH = isize;
pub type HMONITOR = isize;
pub type HDC = isize;
pub type HANDLE = isize;
pub type HMENU = isize;

#[repr(C)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}
pub type HRESULT = i32;
pub type HSTRING = *mut c_void;
pub type IUnknown = *mut c_void;
pub type PSTR = *mut u8;
pub type PWSTR = *mut u16;
pub type PCSTR = *const u8;
pub type PCWSTR = *const u16;
pub type BSTR = *const u16;
pub type BOOL = i32;

pub const S_OK: HRESULT = 0i32;

pub type WNDPROC = Option<unsafe extern "system" fn(param0: HWND, param1: u32, param2: WPARAM, param3: LPARAM) -> LRESULT>;
pub type TIMERPROC = Option<unsafe extern "system" fn(param0: HWND, param1: u32, param2: usize, param3: u32)>;

pub type WNDCLASS_STYLES = u32;
#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: WNDCLASS_STYLES,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: HICON,
    pub hCursor: HCURSOR,
    pub hbrBackground: HBRUSH,
    pub lpszMenuName: PCWSTR,
    pub lpszClassName: PCWSTR,
    pub hIconSm: HICON,
}

pub type WINDOWPLACEMENT_FLAGS = u32;
pub type SHOW_WINDOW_CMD = u32;
#[repr(C)]
pub struct WINDOWPLACEMENT {
    pub length: u32,
    pub flags: WINDOWPLACEMENT_FLAGS,
    pub showCmd: SHOW_WINDOW_CMD,
    pub ptMinPosition: POINT,
    pub ptMaxPosition: POINT,
    pub rcNormalPosition: RECT,
}

#[repr(C)]
pub struct MARGINS {
    pub cxLeftWidth: i32,
    pub cxRightWidth: i32,
    pub cyTopHeight: i32,
    pub cyBottomHeight: i32,
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
}

#[repr(C)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}


pub type TRACKMOUSEEVENT_FLAGS = u32;
#[repr(C)]
pub struct TRACKMOUSEEVENT {
    pub cbSize: u32,
    pub dwFlags: TRACKMOUSEEVENT_FLAGS,
    pub hwndTrack: HWND,
    pub dwHoverTime: u32,
}

pub type FARPROC = Option<unsafe extern "system" fn() -> isize>;

pub type PEEK_MESSAGE_REMOVE_TYPE = u32;

pub const SW_SHOW: SHOW_WINDOW_CMD = 5u32;

pub const CS_VREDRAW: WNDCLASS_STYLES = 1u32;
pub const CS_HREDRAW: WNDCLASS_STYLES = 2u32;
pub const CS_OWNDC: WNDCLASS_STYLES = 32u32;

pub const CW_USEDEFAULT: i32 = -2147483648i32;

pub const GWLP_USERDATA: WINDOW_LONG_PTR_INDEX = -21i32;

pub type WINDOW_EX_STYLE = u32;
pub type WINDOW_STYLE = u32;

pub const WS_SIZEBOX: WINDOW_STYLE = 262144u32;
pub const WS_MINIMIZEBOX: WINDOW_STYLE = 131072u32;
pub const WS_MAXIMIZEBOX: WINDOW_STYLE = 65536u32;
pub const WS_POPUP: WINDOW_STYLE = 2147483648u32;
pub const WS_CLIPSIBLINGS: WINDOW_STYLE = 67108864u32;
pub const WS_CLIPCHILDREN: WINDOW_STYLE = 33554432u32;
pub const WS_SYSMENU: WINDOW_STYLE = 524288u32;

pub const WS_EX_WINDOWEDGE: WINDOW_EX_STYLE = 256u32;
pub const WS_EX_APPWINDOW: WINDOW_EX_STYLE = 262144u32;
pub const WS_EX_ACCEPTFILES: WINDOW_EX_STYLE = 16u32;
pub const WS_EX_TOPMOST: WINDOW_EX_STYLE = 8u32;

pub const HTTOPLEFT: u32 = 13u32;
pub const HTBOTTOMLEFT: u32 = 16u32;
pub const HTLEFT: u32 = 10u32;
pub const HTTOPRIGHT: u32 = 14u32;
pub const HTBOTTOMRIGHT: u32 = 17u32;
pub const HTRIGHT: u32 = 11u32;
pub const HTTOP: u32 = 12u32;
pub const HTBOTTOM: u32 = 15u32;
pub const HTCLIENT: u32 = 1u32;
pub const HTCAPTION: u32 = 2u32;
pub const HTSYSMENU: u32 = 3u32;

pub const IDI_WINLOGO: PCWSTR = 32517u32 as _;

pub const WM_QUIT: u32 = 18u32;
pub const WM_USER: u32 = 1024u32;
pub const WM_ACTIVATE: u32 = 6u32;
pub const WM_NCCALCSIZE: u32 = 131u32;
pub const WM_NCHITTEST: u32 = 132u32;
pub const WM_ERASEBKGND: u32 = 20u32;
pub const WM_MOUSEMOVE: u32 = 512u32;
pub const WM_MOUSELEAVE: u32 = 675u32;
pub const WM_MOUSEWHEEL: u32 = 522u32;
pub const WM_LBUTTONDOWN: u32 = 513u32;
pub const WM_LBUTTONUP: u32 = 514u32;
pub const WM_RBUTTONDOWN: u32 = 516u32;
pub const WM_RBUTTONUP: u32 = 517u32;
pub const WM_MBUTTONDOWN: u32 = 519u32;
pub const WM_MBUTTONUP: u32 = 520u32;
pub const WM_KEYDOWN: u32 = 256u32;
pub const WM_SYSKEYDOWN: u32 = 260u32;
pub const WM_CLOSE: u32 = 16u32;
pub const WM_KEYUP: u32 = 257u32;
pub const WM_SYSKEYUP: u32 = 261u32;
pub const WM_CHAR: u32 = 258u32;
pub const WM_ENTERSIZEMOVE: u32 = 561u32;
pub const WM_EXITSIZEMOVE: u32 = 562u32;
pub const WM_SIZE: u32 = 5u32;
pub const WM_DPICHANGED: u32 = 736u32;
pub const WM_DESTROY: u32 = 2u32;

pub type VIRTUAL_KEY = u16;
pub const VK_0: VIRTUAL_KEY = 48u16;
pub const VK_1: VIRTUAL_KEY = 49u16;
pub const VK_2: VIRTUAL_KEY = 50u16;
pub const VK_3: VIRTUAL_KEY = 51u16;
pub const VK_4: VIRTUAL_KEY = 52u16;
pub const VK_5: VIRTUAL_KEY = 53u16;
pub const VK_6: VIRTUAL_KEY = 54u16;
pub const VK_7: VIRTUAL_KEY = 55u16;
pub const VK_8: VIRTUAL_KEY = 56u16;
pub const VK_9: VIRTUAL_KEY = 57u16;
pub const VK_A: VIRTUAL_KEY = 65u16;
pub const VK_B: VIRTUAL_KEY = 66u16;
pub const VK_C: VIRTUAL_KEY = 67u16;
pub const VK_D: VIRTUAL_KEY = 68u16;
pub const VK_E: VIRTUAL_KEY = 69u16;
pub const VK_F: VIRTUAL_KEY = 70u16;
pub const VK_G: VIRTUAL_KEY = 71u16;
pub const VK_H: VIRTUAL_KEY = 72u16;
pub const VK_I: VIRTUAL_KEY = 73u16;
pub const VK_J: VIRTUAL_KEY = 74u16;
pub const VK_K: VIRTUAL_KEY = 75u16;
pub const VK_L: VIRTUAL_KEY = 76u16;
pub const VK_M: VIRTUAL_KEY = 77u16;
pub const VK_N: VIRTUAL_KEY = 78u16;
pub const VK_O: VIRTUAL_KEY = 79u16;
pub const VK_P: VIRTUAL_KEY = 80u16;
pub const VK_Q: VIRTUAL_KEY = 81u16;
pub const VK_R: VIRTUAL_KEY = 82u16;
pub const VK_S: VIRTUAL_KEY = 83u16;
pub const VK_T: VIRTUAL_KEY = 84u16;
pub const VK_U: VIRTUAL_KEY = 85u16;
pub const VK_V: VIRTUAL_KEY = 86u16;
pub const VK_W: VIRTUAL_KEY = 87u16;
pub const VK_X: VIRTUAL_KEY = 88u16;
pub const VK_Y: VIRTUAL_KEY = 89u16;
pub const VK_Z: VIRTUAL_KEY = 90u16;
pub const VK_CONTROL: VIRTUAL_KEY = 17u16;
pub const VK_SHIFT: VIRTUAL_KEY = 16u16;
pub const VK_MENU: VIRTUAL_KEY = 18u16;
pub const VK_LWIN: VIRTUAL_KEY = 91u16;
pub const VK_RWIN: VIRTUAL_KEY = 92u16;
pub const VK_ESCAPE: VIRTUAL_KEY = 27u16;
pub const VK_OEM_3: VIRTUAL_KEY = 192u16;
pub const VK_OEM_MINUS: VIRTUAL_KEY = 189u16;
pub const VK_OEM_PLUS: VIRTUAL_KEY = 187u16;
pub const VK_BACK: VIRTUAL_KEY = 8u16;
pub const VK_TAB: VIRTUAL_KEY = 9u16;
pub const VK_OEM_4: VIRTUAL_KEY = 219u16;
pub const VK_OEM_6: VIRTUAL_KEY = 221u16;
pub const VK_OEM_1: VIRTUAL_KEY = 186u16;
pub const VK_OEM_7: VIRTUAL_KEY = 222u16;
pub const VK_OEM_5: VIRTUAL_KEY = 220u16;
pub const VK_OEM_COMMA: VIRTUAL_KEY = 188u16;
pub const VK_OEM_PERIOD: VIRTUAL_KEY = 190u16;
pub const VK_OEM_2: VIRTUAL_KEY = 191u16;
pub const VK_LCONTROL: VIRTUAL_KEY = 162u16;
pub const VK_RCONTROL: VIRTUAL_KEY = 163u16;
pub const VK_LMENU: VIRTUAL_KEY = 164u16;
pub const VK_RMENU: VIRTUAL_KEY = 165u16;
pub const VK_LSHIFT: VIRTUAL_KEY = 160u16;
pub const VK_RSHIFT: VIRTUAL_KEY = 161u16;
pub const VK_SPACE: VIRTUAL_KEY = 32u16;
pub const VK_CAPITAL: VIRTUAL_KEY = 20u16;
pub const VK_F1: VIRTUAL_KEY = 112u16;
pub const VK_F2: VIRTUAL_KEY = 113u16;
pub const VK_F3: VIRTUAL_KEY = 114u16;
pub const VK_F4: VIRTUAL_KEY = 115u16;
pub const VK_F5: VIRTUAL_KEY = 116u16;
pub const VK_F6: VIRTUAL_KEY = 117u16;
pub const VK_F7: VIRTUAL_KEY = 118u16;
pub const VK_F8: VIRTUAL_KEY = 119u16;
pub const VK_F9: VIRTUAL_KEY = 120u16;
pub const VK_F10: VIRTUAL_KEY = 121u16;
pub const VK_F11: VIRTUAL_KEY = 122u16;
pub const VK_F12: VIRTUAL_KEY = 123u16;
pub const VK_SNAPSHOT: VIRTUAL_KEY = 44u16;
pub const VK_SCROLL: VIRTUAL_KEY = 145u16;
pub const VK_PAUSE: VIRTUAL_KEY = 19u16;
pub const VK_INSERT: VIRTUAL_KEY = 45u16;
pub const VK_DELETE: VIRTUAL_KEY = 46u16;
pub const VK_HOME: VIRTUAL_KEY = 36u16;
pub const VK_END: VIRTUAL_KEY = 35u16;
pub const VK_PRIOR: VIRTUAL_KEY = 33u16;
pub const VK_NEXT: VIRTUAL_KEY = 34u16;
pub const VK_NUMPAD0: VIRTUAL_KEY = 96u16;
pub const VK_NUMPAD1: VIRTUAL_KEY = 97u16;
pub const VK_NUMPAD2: VIRTUAL_KEY = 98u16;
pub const VK_NUMPAD3: VIRTUAL_KEY = 99u16;
pub const VK_NUMPAD4: VIRTUAL_KEY = 100u16;
pub const VK_NUMPAD5: VIRTUAL_KEY = 101u16;
pub const VK_NUMPAD6: VIRTUAL_KEY = 102u16;
pub const VK_NUMPAD7: VIRTUAL_KEY = 103u16;
pub const VK_NUMPAD8: VIRTUAL_KEY = 104u16;
pub const VK_NUMPAD9: VIRTUAL_KEY = 105u16;
pub const VK_SUBTRACT: VIRTUAL_KEY = 109u16;
pub const VK_ADD: VIRTUAL_KEY = 107u16;
pub const VK_DECIMAL: VIRTUAL_KEY = 110u16;
pub const VK_MULTIPLY: VIRTUAL_KEY = 106u16;
pub const VK_DIVIDE: VIRTUAL_KEY = 111u16;
pub const VK_NUMLOCK: VIRTUAL_KEY = 144u16;
pub const VK_LEFT: VIRTUAL_KEY = 37u16;
pub const VK_UP: VIRTUAL_KEY = 38u16;
pub const VK_RIGHT: VIRTUAL_KEY = 39u16;
pub const VK_DOWN: VIRTUAL_KEY = 40u16;
pub const VK_RETURN: VIRTUAL_KEY = 13u16;

pub const SW_RESTORE: SHOW_WINDOW_CMD = 9u32;
pub const SW_MAXIMIZE: SHOW_WINDOW_CMD = 3u32;
pub const SW_MINIMIZE: SHOW_WINDOW_CMD = 6u32;

pub const HWND_TOPMOST: HWND = -1i32 as _;
pub const HWND_NOTOPMOST: HWND = -2i32 as _;

pub type SET_WINDOW_POS_FLAGS = u32;
pub const SWP_NOMOVE: SET_WINDOW_POS_FLAGS = 2u32;
pub const SWP_NOSIZE: SET_WINDOW_POS_FLAGS = 1u32;

pub type WINDOW_LONG_PTR_INDEX = i32;
pub const GWL_EXSTYLE: WINDOW_LONG_PTR_INDEX = -20i32;

pub const WA_ACTIVE: u32 = 1u32;

pub type CLIPBOARD_FORMATS = u32;
pub const CF_UNICODETEXT: CLIPBOARD_FORMATS = 13u32;
pub const GMEM_DDESHARE: u32 = 8192u32;

pub const IDC_ARROW: PCWSTR = 32512i32 as _;
pub const IDC_CROSS: PCWSTR = 32515i32 as _;
pub const IDC_HAND: PCWSTR = 32649i32 as _;
pub const IDC_HELP: PCWSTR = 32651i32 as _;
pub const IDC_IBEAM: PCWSTR = 32513i32 as _;
pub const IDC_ICON: PCWSTR = 32641i32 as _;
pub const IDC_NO: PCWSTR = 32648i32 as _;
pub const IDC_PERSON: PCWSTR = 32672i32 as _;
pub const IDC_PIN: PCWSTR = 32671i32 as _;
pub const IDC_SIZE: PCWSTR = 32640i32 as _;
pub const IDC_SIZEALL: PCWSTR = 32646i32 as _;
pub const IDC_SIZENESW: PCWSTR = 32643i32 as _;
pub const IDC_SIZENS: PCWSTR = 32645i32 as _;
pub const IDC_SIZENWSE: PCWSTR = 32642i32 as _;
pub const IDC_SIZEWE: PCWSTR = 32644i32 as _;
pub const IDC_UPARROW: PCWSTR = 32516i32 as _;
pub const IDC_WAIT: PCWSTR = 32514i32 as _;

pub type MONITOR_FROM_FLAGS = u32;
pub const MONITOR_DEFAULTTONEAREST: MONITOR_FROM_FLAGS = 2u32;
pub type GET_DEVICE_CAPS_INDEX = u32;

pub type DPI_AWARENESS_CONTEXT = isize;
pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE: DPI_AWARENESS_CONTEXT = -3i32 as _;
pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: DPI_AWARENESS_CONTEXT = -4i32 as _;

pub type PROCESS_DPI_AWARENESS = i32;
pub const PROCESS_PER_MONITOR_DPI_AWARE: PROCESS_DPI_AWARENESS = 2i32;

pub type MONITOR_DPI_TYPE = i32;
pub const MDT_EFFECTIVE_DPI: MONITOR_DPI_TYPE = 0i32;

pub const LOGPIXELSX: GET_DEVICE_CAPS_INDEX = 88u32;

pub type GLOBAL_ALLOC_FLAGS = u32;

pub const TME_LEAVE: TRACKMOUSEEVENT_FLAGS = 2u32;

pub use SetWindowLongW as SetWindowLongPtrW;
pub use GetWindowLongW as GetWindowLongPtrW;

#[cfg_attr(windows, link(name = "windows"))]
extern "system" {
    pub fn LoadIconW(hinstance: HINSTANCE, lpiconname: PCWSTR) -> HICON;
    pub fn RegisterClassExW(param0: *const WNDCLASSEXW) -> u16;
    pub fn IsGUIThread(bconvert: BOOL) -> BOOL;
    pub fn GetMessageW(lpmsg: *mut MSG, hwnd: HWND, wmsgfiltermin: u32, wmsgfiltermax: u32) -> BOOL;
    pub fn TranslateMessage(lpmsg: *const MSG) -> BOOL;
    pub fn DispatchMessageW(lpmsg: *const MSG) -> LRESULT;
    pub fn PeekMessageW(lpmsg: *mut MSG, hwnd: HWND, wmsgfiltermin: u32, wmsgfiltermax: u32, wremovemsg: PEEK_MESSAGE_REMOVE_TYPE) -> super::super::Foundation::BOOL;
    pub fn SetTimer(hwnd: HWND, nidevent: usize, uelapse: u32, lptimerfunc: TIMERPROC) -> usize;
    pub fn KillTimer(hwnd: HWND, uidevent: usize) -> BOOL;
    pub fn PostMessageW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> BOOL;
    pub fn ShowCursor(bshow: BOOL) -> i32;
    pub fn SetCursor(hcursor: HCURSOR) -> HCURSOR;
    pub fn LoadCursorW(hinstance: HINSTANCE, lpcursorname: PCWSTR) -> HCURSOR;
    pub fn IsProcessDPIAware() -> BOOL;
    pub fn GetDC(hwnd: HWND) -> HDC;
    pub fn MonitorFromWindow(hwnd: HWND, dwflags: MONITOR_FROM_FLAGS) -> HMONITOR;
    pub fn GetDeviceCaps(hdc: HDC, index: GET_DEVICE_CAPS_INDEX) -> i32;
    pub fn GetModuleHandleW(lpmodulename: PCWSTR) -> HINSTANCE;
    pub fn LoadLibraryA(lplibfilename: PCSTR) -> HINSTANCE;
    pub fn GetProcAddress(hmodule: HINSTANCE, lpprocname:PCSTR) -> FARPROC;
    pub fn QueryPerformanceCounter(lpperformancecount: *mut i64) -> BOOL;
    pub fn QueryPerformanceFrequency(lpfrequency: *mut i64) -> BOOL;
    pub fn GlobalLock(hmem: isize) -> *mut c_void;
    pub fn GlobalAlloc(uflags: GLOBAL_ALLOC_FLAGS, dwbytes: usize) -> isize;
    pub fn GlobalSize(hmem: isize) -> usize;
    pub fn GlobalUnlock(hmem: isize) -> BOOL;
    pub fn OpenClipboard(hwndnewowner: HWND) -> BOOL;
    pub fn EmptyClipboard() -> BOOL;
    pub fn GetClipboardData(uformat: u32) -> HANDLE;
    pub fn SetClipboardData(uformat: u32, hmem: HANDLE) -> HANDLE;
    pub fn CloseClipboard() -> BOOL;
    pub fn CreateWindowExW(dwexstyle: WINDOW_EX_STYLE, lpclassname:PCWSTR, lpwindowname: PCWSTR, dwstyle: WINDOW_STYLE, x: i32, y: i32, nwidth: i32, nheight: i32, hwndparent: HWND, hmenu: HMENU, hinstance: HINSTANCE, lpparam: *const c_void) -> HWND;
    pub fn SetWindowLongW(hwnd: HWND, nindex: WINDOW_LONG_PTR_INDEX, dwnewlong: i32) -> i32;
    pub fn GetWindowLongW(hwnd: HWND, nindex: WINDOW_LONG_PTR_INDEX) -> i32;
    pub fn DefWindowProcW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    pub fn ShowWindow(hwnd: HWND, ncmdshow: SHOW_WINDOW_CMD) -> BOOL;
    pub fn GetWindowRect(hwnd: HWND, lprect: *mut RECT) -> BOOL;
    pub fn DestroyWindow(hwnd: HWND) -> BOOL;
    pub fn SetWindowPos(hwnd: HWND, hwndinsertafter: HWND, x: i32, y: i32, cx: i32, cy: i32, uflags: SET_WINDOW_POS_FLAGS) -> BOOL;
    pub fn GetWindowPlacement(hwnd: HWND, lpwndpl: *mut WINDOWPLACEMENT) -> BOOL;
    pub fn GetClientRect(hwnd: HWND, lprect: *mut RECT) -> BOOL;
    pub fn MoveWindow(hwnd: HWND, x: i32, y: i32, nwidth: i32, nheight: i32, brepaint: BOOL) -> BOOL;
    pub fn ReleaseCapture() -> BOOL;
    pub fn SetCapture(hwnd: HWND) -> HWND;
    pub fn TrackMouseEvent(lpeventtrack: *mut TRACKMOUSEEVENT) -> BOOL;
    pub fn GetKeyState(nvirtkey: i32) -> i16;
    pub fn DwmExtendFrameIntoClientArea(hwnd: HWND, pmarinset: *const MARGINS) -> HRESULT;
    
}
