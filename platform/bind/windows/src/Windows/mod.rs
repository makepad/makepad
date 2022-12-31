#![allow(non_camel_case_types)]
pub mod Win32{
pub mod Foundation{
#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct WPARAM(pub usize);
impl ::core::marker::Copy for WPARAM {}
impl ::core::clone::Clone for WPARAM {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for WPARAM {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for WPARAM {
    type Abi = Self;
}
impl ::core::fmt::Debug for WPARAM {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("WPARAM").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<WPARAM>> for WPARAM {
    fn from(optional: ::core::option::Option<WPARAM>) -> WPARAM {
        optional.unwrap_or_default()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct LPARAM(pub isize);
impl ::core::marker::Copy for LPARAM {}
impl ::core::clone::Clone for LPARAM {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for LPARAM {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for LPARAM {
    type Abi = Self;
}
impl ::core::fmt::Debug for LPARAM {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("LPARAM").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<LPARAM>> for LPARAM {
    fn from(optional: ::core::option::Option<LPARAM>) -> LPARAM {
        optional.unwrap_or_default()
    }
}

pub const S_OK: ::windows::core::HRESULT = ::windows::core::HRESULT(0i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct HWND(pub isize);
impl ::core::marker::Copy for HWND {}
impl ::core::clone::Clone for HWND {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for HWND {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for HWND {
    type Abi = Self;
}
impl ::core::fmt::Debug for HWND {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("HWND").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<HWND>> for HWND {
    fn from(optional: ::core::option::Option<HWND>) -> HWND {
        optional.unwrap_or_default()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct BOOL(pub i32);
impl BOOL {
    #[inline]
    pub fn as_bool(self) -> bool {
        self.0 != 0
    }
    #[inline]
    pub fn ok(self) -> ::windows::core::Result<()> {
        if self.as_bool() {
            Ok(())
        } else {
            Err(::windows::core::Error::from_win32())
        }
    }
    #[inline]
    #[track_caller]
    pub fn unwrap(self) {
        self.ok().unwrap();
    }
    #[inline]
    #[track_caller]
    pub fn expect(self, msg: &str) {
        self.ok().expect(msg);
    }
}
impl ::core::marker::Copy for BOOL {}
impl ::core::clone::Clone for BOOL {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for BOOL {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for BOOL {
    type Abi = Self;
}
impl ::core::fmt::Debug for BOOL {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("BOOL").field(&self.0).finish()
    }
}
impl ::core::ops::Not for BOOL {
    type Output = Self;
    fn not(self) -> Self::Output {
        if self.as_bool() {
            Self(0)
        } else {
            Self(1)
        }
    }
}
impl ::core::convert::From<::core::option::Option<BOOL>> for BOOL {
    fn from(optional: ::core::option::Option<BOOL>) -> BOOL {
        optional.unwrap_or_default()
    }
}

pub type FARPROC = ::core::option::Option<unsafe extern "system" fn() -> isize>;

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct HANDLE(pub isize);
impl HANDLE {
    pub fn is_invalid(&self) -> bool {
        self.0 == -1 || self.0 == 0
    }
}
impl ::core::marker::Copy for HANDLE {}
impl ::core::clone::Clone for HANDLE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for HANDLE {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for HANDLE {
    type Abi = Self;
}
impl ::core::fmt::Debug for HANDLE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("HANDLE").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<HANDLE>> for HANDLE {
    fn from(optional: ::core::option::Option<HANDLE>) -> HANDLE {
        optional.unwrap_or_default()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct LRESULT(pub isize);
impl ::core::marker::Copy for LRESULT {}
impl ::core::clone::Clone for LRESULT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for LRESULT {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for LRESULT {
    type Abi = Self;
}
impl ::core::fmt::Debug for LRESULT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("LRESULT").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<LRESULT>> for LRESULT {
    fn from(optional: ::core::option::Option<LRESULT>) -> LRESULT {
        optional.unwrap_or_default()
    }
}

#[repr(C)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl ::core::marker::Copy for RECT {}
impl ::core::cmp::Eq for RECT {}
impl ::core::cmp::PartialEq for RECT {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<RECT>()) == 0 }
    }
}
impl ::core::clone::Clone for RECT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for RECT {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for RECT {
    type Abi = Self;
}
impl ::core::fmt::Debug for RECT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("RECT").field("left", &self.left).field("top", &self.top).field("right", &self.right).field("bottom", &self.bottom).finish()
    }
}

#[repr(C)]
pub struct LUID {
    pub LowPart: u32,
    pub HighPart: i32,
}
impl ::core::marker::Copy for LUID {}
impl ::core::cmp::Eq for LUID {}
impl ::core::cmp::PartialEq for LUID {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<LUID>()) == 0 }
    }
}
impl ::core::clone::Clone for LUID {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for LUID {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for LUID {
    type Abi = Self;
}
impl ::core::fmt::Debug for LUID {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("LUID").field("LowPart", &self.LowPart).field("HighPart", &self.HighPart).finish()
    }
}

#[repr(C)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}
impl ::core::marker::Copy for POINT {}
impl ::core::cmp::Eq for POINT {}
impl ::core::cmp::PartialEq for POINT {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<POINT>()) == 0 }
    }
}
impl ::core::clone::Clone for POINT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for POINT {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for POINT {
    type Abi = Self;
}
impl ::core::fmt::Debug for POINT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("POINT").field("x", &self.x).field("y", &self.y).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct HINSTANCE(pub isize);
impl HINSTANCE {
    pub fn is_invalid(&self) -> bool {
        self.0 == 0
    }
}
impl ::core::marker::Copy for HINSTANCE {}
impl ::core::clone::Clone for HINSTANCE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for HINSTANCE {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for HINSTANCE {
    type Abi = Self;
}
impl ::core::fmt::Debug for HINSTANCE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("HINSTANCE").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<HINSTANCE>> for HINSTANCE {
    fn from(optional: ::core::option::Option<HINSTANCE>) -> HINSTANCE {
        optional.unwrap_or_default()
    }
}

}
pub mod UI{
pub mod WindowsAndMessaging{
#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: WNDCLASS_STYLES,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: super::super::Foundation::HINSTANCE,
    pub hIcon: HICON,
    pub hCursor: HCURSOR,
    pub hbrBackground: super::super::Graphics::Gdi::HBRUSH,
    pub lpszMenuName: ::windows::core::PCWSTR,
    pub lpszClassName: ::windows::core::PCWSTR,
    pub hIconSm: HICON,
}
impl ::core::marker::Copy for WNDCLASSEXW {}
impl ::core::cmp::Eq for WNDCLASSEXW {}
impl ::core::cmp::PartialEq for WNDCLASSEXW {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<WNDCLASSEXW>()) == 0 }
    }
}
impl ::core::clone::Clone for WNDCLASSEXW {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for WNDCLASSEXW {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for WNDCLASSEXW {
    type Abi = Self;
}
impl ::core::fmt::Debug for WNDCLASSEXW {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("WNDCLASSEXW")
            .field("cbSize", &self.cbSize)
            .field("style", &self.style)
            .field("lpfnWndProc", &self.lpfnWndProc.map(|f| f as usize))
            .field("cbClsExtra", &self.cbClsExtra)
            .field("cbWndExtra", &self.cbWndExtra)
            .field("hInstance", &self.hInstance)
            .field("hIcon", &self.hIcon)
            .field("hCursor", &self.hCursor)
            .field("hbrBackground", &self.hbrBackground)
            .field("lpszMenuName", &self.lpszMenuName)
            .field("lpszClassName", &self.lpszClassName)
            .field("hIconSm", &self.hIconSm)
            .finish()
    }
}

pub const PM_REMOVE: PEEK_MESSAGE_REMOVE_TYPE = PEEK_MESSAGE_REMOVE_TYPE(1u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct PEEK_MESSAGE_REMOVE_TYPE(pub u32);
impl ::core::marker::Copy for PEEK_MESSAGE_REMOVE_TYPE {}
impl ::core::clone::Clone for PEEK_MESSAGE_REMOVE_TYPE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for PEEK_MESSAGE_REMOVE_TYPE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for PEEK_MESSAGE_REMOVE_TYPE {
    type Abi = Self;
}
impl ::core::fmt::Debug for PEEK_MESSAGE_REMOVE_TYPE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("PEEK_MESSAGE_REMOVE_TYPE").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for PEEK_MESSAGE_REMOVE_TYPE {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for PEEK_MESSAGE_REMOVE_TYPE {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for PEEK_MESSAGE_REMOVE_TYPE {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for PEEK_MESSAGE_REMOVE_TYPE {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for PEEK_MESSAGE_REMOVE_TYPE {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub unsafe fn LoadIconW<'a, P0, P1>(hinstance: P0, lpiconname: P1) -> ::windows::core::Result<HICON>
where
    P0: ::std::convert::Into<super::super::Foundation::HINSTANCE>,
    P1: ::std::convert::Into<::windows::core::PCWSTR>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn LoadIconW(hinstance: super::super::Foundation::HINSTANCE, lpiconname: ::windows::core::PCWSTR) -> HICON;
    }
    let result__ = LoadIconW(hinstance.into(), lpiconname.into());
    (!result__.is_invalid()).then(|| result__).ok_or_else(::windows::core::Error::from_win32)
}

pub unsafe fn RegisterClassExW(param0: *const WNDCLASSEXW) -> u16 {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn RegisterClassExW(param0: *const WNDCLASSEXW) -> u16;
    }
    RegisterClassExW(::core::mem::transmute(param0))
}

pub unsafe fn IsGUIThread<'a, P0>(bconvert: P0) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::BOOL>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn IsGUIThread(bconvert: super::super::Foundation::BOOL) -> super::super::Foundation::BOOL;
    }
    IsGUIThread(bconvert.into())
}

pub unsafe fn GetMessageW<'a, P0>(lpmsg: *mut MSG, hwnd: P0, wmsgfiltermin: u32, wmsgfiltermax: u32) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetMessageW(lpmsg: *mut MSG, hwnd: super::super::Foundation::HWND, wmsgfiltermin: u32, wmsgfiltermax: u32) -> super::super::Foundation::BOOL;
    }
    GetMessageW(::core::mem::transmute(lpmsg), hwnd.into(), wmsgfiltermin, wmsgfiltermax)
}

pub unsafe fn TranslateMessage(lpmsg: *const MSG) -> super::super::Foundation::BOOL {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn TranslateMessage(lpmsg: *const MSG) -> super::super::Foundation::BOOL;
    }
    TranslateMessage(::core::mem::transmute(lpmsg))
}

pub unsafe fn DispatchMessageW(lpmsg: *const MSG) -> super::super::Foundation::LRESULT {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn DispatchMessageW(lpmsg: *const MSG) -> super::super::Foundation::LRESULT;
    }
    DispatchMessageW(::core::mem::transmute(lpmsg))
}

pub unsafe fn PeekMessageW<'a, P0>(lpmsg: *mut MSG, hwnd: P0, wmsgfiltermin: u32, wmsgfiltermax: u32, wremovemsg: PEEK_MESSAGE_REMOVE_TYPE) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn PeekMessageW(lpmsg: *mut MSG, hwnd: super::super::Foundation::HWND, wmsgfiltermin: u32, wmsgfiltermax: u32, wremovemsg: PEEK_MESSAGE_REMOVE_TYPE) -> super::super::Foundation::BOOL;
    }
    PeekMessageW(::core::mem::transmute(lpmsg), hwnd.into(), wmsgfiltermin, wmsgfiltermax, wremovemsg)
}

pub unsafe fn SetTimer<'a, P0>(hwnd: P0, nidevent: usize, uelapse: u32, lptimerfunc: TIMERPROC) -> usize
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn SetTimer(hwnd: super::super::Foundation::HWND, nidevent: usize, uelapse: u32, lptimerfunc: *mut ::core::ffi::c_void) -> usize;
    }
    SetTimer(hwnd.into(), nidevent, uelapse, ::core::mem::transmute(lptimerfunc))
}

pub unsafe fn KillTimer<'a, P0>(hwnd: P0, uidevent: usize) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn KillTimer(hwnd: super::super::Foundation::HWND, uidevent: usize) -> super::super::Foundation::BOOL;
    }
    KillTimer(hwnd.into(), uidevent)
}

pub unsafe fn PostMessageW<'a, P0, P1, P2>(hwnd: P0, msg: u32, wparam: P1, lparam: P2) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
    P1: ::std::convert::Into<super::super::Foundation::WPARAM>,
    P2: ::std::convert::Into<super::super::Foundation::LPARAM>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn PostMessageW(hwnd: super::super::Foundation::HWND, msg: u32, wparam: super::super::Foundation::WPARAM, lparam: super::super::Foundation::LPARAM) -> super::super::Foundation::BOOL;
    }
    PostMessageW(hwnd.into(), msg, wparam.into(), lparam.into())
}

pub unsafe fn ShowCursor<'a, P0>(bshow: P0) -> i32
where
    P0: ::std::convert::Into<super::super::Foundation::BOOL>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn ShowCursor(bshow: super::super::Foundation::BOOL) -> i32;
    }
    ShowCursor(bshow.into())
}

pub unsafe fn SetCursor<'a, P0>(hcursor: P0) -> HCURSOR
where
    P0: ::std::convert::Into<HCURSOR>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn SetCursor(hcursor: HCURSOR) -> HCURSOR;
    }
    SetCursor(hcursor.into())
}

pub unsafe fn LoadCursorW<'a, P0, P1>(hinstance: P0, lpcursorname: P1) -> ::windows::core::Result<HCURSOR>
where
    P0: ::std::convert::Into<super::super::Foundation::HINSTANCE>,
    P1: ::std::convert::Into<::windows::core::PCWSTR>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn LoadCursorW(hinstance: super::super::Foundation::HINSTANCE, lpcursorname: ::windows::core::PCWSTR) -> HCURSOR;
    }
    let result__ = LoadCursorW(hinstance.into(), lpcursorname.into());
    (!result__.is_invalid()).then(|| result__).ok_or_else(::windows::core::Error::from_win32)
}

pub unsafe fn IsProcessDPIAware() -> super::super::Foundation::BOOL {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn IsProcessDPIAware() -> super::super::Foundation::BOOL;
    }
    IsProcessDPIAware()
}

pub const IDC_ARROW: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32512i32 as _);

pub const IDC_CROSS: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32515i32 as _);

pub const IDC_HAND: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32649i32 as _);

pub const IDC_SIZEALL: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32646i32 as _);

pub const IDC_IBEAM: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32513i32 as _);

pub const IDC_HELP: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32651i32 as _);

pub const IDC_NO: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32648i32 as _);

pub const IDC_SIZEWE: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32644i32 as _);

pub const IDC_SIZENS: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32645i32 as _);

pub const IDC_SIZENESW: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32643i32 as _);

pub const IDC_SIZENWSE: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32642i32 as _);

pub const WM_QUIT: u32 = 18u32;

pub const CS_HREDRAW: WNDCLASS_STYLES = WNDCLASS_STYLES(2u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct WNDCLASS_STYLES(pub u32);
impl ::core::marker::Copy for WNDCLASS_STYLES {}
impl ::core::clone::Clone for WNDCLASS_STYLES {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for WNDCLASS_STYLES {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for WNDCLASS_STYLES {
    type Abi = Self;
}
impl ::core::fmt::Debug for WNDCLASS_STYLES {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("WNDCLASS_STYLES").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for WNDCLASS_STYLES {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for WNDCLASS_STYLES {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for WNDCLASS_STYLES {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for WNDCLASS_STYLES {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for WNDCLASS_STYLES {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub const CS_VREDRAW: WNDCLASS_STYLES = WNDCLASS_STYLES(1u32);

pub const CS_OWNDC: WNDCLASS_STYLES = WNDCLASS_STYLES(32u32);

pub const IDI_WINLOGO: ::windows::core::PCWSTR = ::windows::core::PCWSTR(32517u32 as _);

pub const WM_USER: u32 = 1024u32;

pub unsafe fn CreateWindowExW<'a, P0, P1, P2, P3, P4>(dwexstyle: WINDOW_EX_STYLE, lpclassname: P0, lpwindowname: P1, dwstyle: WINDOW_STYLE, x: i32, y: i32, nwidth: i32, nheight: i32, hwndparent: P2, hmenu: P3, hinstance: P4, lpparam: ::core::option::Option<*const ::core::ffi::c_void>) -> super::super::Foundation::HWND
where
    P0: ::std::convert::Into<::windows::core::PCWSTR>,
    P1: ::std::convert::Into<::windows::core::PCWSTR>,
    P2: ::std::convert::Into<super::super::Foundation::HWND>,
    P3: ::std::convert::Into<HMENU>,
    P4: ::std::convert::Into<super::super::Foundation::HINSTANCE>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn CreateWindowExW(dwexstyle: WINDOW_EX_STYLE, lpclassname: ::windows::core::PCWSTR, lpwindowname: ::windows::core::PCWSTR, dwstyle: WINDOW_STYLE, x: i32, y: i32, nwidth: i32, nheight: i32, hwndparent: super::super::Foundation::HWND, hmenu: HMENU, hinstance: super::super::Foundation::HINSTANCE, lpparam: *const ::core::ffi::c_void) -> super::super::Foundation::HWND;
    }
    CreateWindowExW(dwexstyle, lpclassname.into(), lpwindowname.into(), dwstyle, x, y, nwidth, nheight, hwndparent.into(), hmenu.into(), hinstance.into(), ::core::mem::transmute(lpparam.unwrap_or(::std::ptr::null())))
}

pub unsafe fn SetWindowLongPtrW<'a, P0>(hwnd: P0, nindex: WINDOW_LONG_PTR_INDEX, dwnewlong: isize) -> isize
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn SetWindowLongPtrW(hwnd: super::super::Foundation::HWND, nindex: WINDOW_LONG_PTR_INDEX, dwnewlong: isize) -> isize;
    }
    SetWindowLongPtrW(hwnd.into(), nindex, dwnewlong)
}

pub unsafe fn GetWindowLongPtrW<'a, P0>(hwnd: P0, nindex: WINDOW_LONG_PTR_INDEX) -> isize
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetWindowLongPtrW(hwnd: super::super::Foundation::HWND, nindex: WINDOW_LONG_PTR_INDEX) -> isize;
    }
    GetWindowLongPtrW(hwnd.into(), nindex)
}

pub unsafe fn DefWindowProcW<'a, P0, P1, P2>(hwnd: P0, msg: u32, wparam: P1, lparam: P2) -> super::super::Foundation::LRESULT
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
    P1: ::std::convert::Into<super::super::Foundation::WPARAM>,
    P2: ::std::convert::Into<super::super::Foundation::LPARAM>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn DefWindowProcW(hwnd: super::super::Foundation::HWND, msg: u32, wparam: super::super::Foundation::WPARAM, lparam: super::super::Foundation::LPARAM) -> super::super::Foundation::LRESULT;
    }
    DefWindowProcW(hwnd.into(), msg, wparam.into(), lparam.into())
}

pub unsafe fn ShowWindow<'a, P0>(hwnd: P0, ncmdshow: SHOW_WINDOW_CMD) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn ShowWindow(hwnd: super::super::Foundation::HWND, ncmdshow: SHOW_WINDOW_CMD) -> super::super::Foundation::BOOL;
    }
    ShowWindow(hwnd.into(), ncmdshow)
}

pub unsafe fn GetWindowRect<'a, P0>(hwnd: P0, lprect: *mut super::super::Foundation::RECT) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetWindowRect(hwnd: super::super::Foundation::HWND, lprect: *mut super::super::Foundation::RECT) -> super::super::Foundation::BOOL;
    }
    GetWindowRect(hwnd.into(), ::core::mem::transmute(lprect))
}

pub unsafe fn DestroyWindow<'a, P0>(hwnd: P0) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn DestroyWindow(hwnd: super::super::Foundation::HWND) -> super::super::Foundation::BOOL;
    }
    DestroyWindow(hwnd.into())
}

pub unsafe fn SetWindowPos<'a, P0, P1>(hwnd: P0, hwndinsertafter: P1, x: i32, y: i32, cx: i32, cy: i32, uflags: SET_WINDOW_POS_FLAGS) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
    P1: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn SetWindowPos(hwnd: super::super::Foundation::HWND, hwndinsertafter: super::super::Foundation::HWND, x: i32, y: i32, cx: i32, cy: i32, uflags: SET_WINDOW_POS_FLAGS) -> super::super::Foundation::BOOL;
    }
    SetWindowPos(hwnd.into(), hwndinsertafter.into(), x, y, cx, cy, uflags)
}

pub unsafe fn GetWindowPlacement<'a, P0>(hwnd: P0, lpwndpl: *mut WINDOWPLACEMENT) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetWindowPlacement(hwnd: super::super::Foundation::HWND, lpwndpl: *mut WINDOWPLACEMENT) -> super::super::Foundation::BOOL;
    }
    GetWindowPlacement(hwnd.into(), ::core::mem::transmute(lpwndpl))
}

#[repr(C)]
pub struct WINDOWPLACEMENT {
    pub length: u32,
    pub flags: WINDOWPLACEMENT_FLAGS,
    pub showCmd: SHOW_WINDOW_CMD,
    pub ptMinPosition: super::super::Foundation::POINT,
    pub ptMaxPosition: super::super::Foundation::POINT,
    pub rcNormalPosition: super::super::Foundation::RECT,
}
impl ::core::marker::Copy for WINDOWPLACEMENT {}
impl ::core::cmp::Eq for WINDOWPLACEMENT {}
impl ::core::cmp::PartialEq for WINDOWPLACEMENT {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<WINDOWPLACEMENT>()) == 0 }
    }
}
impl ::core::clone::Clone for WINDOWPLACEMENT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for WINDOWPLACEMENT {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for WINDOWPLACEMENT {
    type Abi = Self;
}
impl ::core::fmt::Debug for WINDOWPLACEMENT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("WINDOWPLACEMENT").field("length", &self.length).field("flags", &self.flags).field("showCmd", &self.showCmd).field("ptMinPosition", &self.ptMinPosition).field("ptMaxPosition", &self.ptMaxPosition).field("rcNormalPosition", &self.rcNormalPosition).finish()
    }
}

pub unsafe fn GetClientRect<'a, P0>(hwnd: P0, lprect: *mut super::super::Foundation::RECT) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetClientRect(hwnd: super::super::Foundation::HWND, lprect: *mut super::super::Foundation::RECT) -> super::super::Foundation::BOOL;
    }
    GetClientRect(hwnd.into(), ::core::mem::transmute(lprect))
}

pub unsafe fn MoveWindow<'a, P0, P1>(hwnd: P0, x: i32, y: i32, nwidth: i32, nheight: i32, brepaint: P1) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
    P1: ::std::convert::Into<super::super::Foundation::BOOL>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn MoveWindow(hwnd: super::super::Foundation::HWND, x: i32, y: i32, nwidth: i32, nheight: i32, brepaint: super::super::Foundation::BOOL) -> super::super::Foundation::BOOL;
    }
    MoveWindow(hwnd.into(), x, y, nwidth, nheight, brepaint.into())
}

pub const GWL_EXSTYLE: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(-20i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct WINDOW_LONG_PTR_INDEX(pub i32);
impl ::core::marker::Copy for WINDOW_LONG_PTR_INDEX {}
impl ::core::clone::Clone for WINDOW_LONG_PTR_INDEX {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for WINDOW_LONG_PTR_INDEX {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for WINDOW_LONG_PTR_INDEX {
    type Abi = Self;
}
impl ::core::fmt::Debug for WINDOW_LONG_PTR_INDEX {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("WINDOW_LONG_PTR_INDEX").field(&self.0).finish()
    }
}

pub const HWND_TOPMOST: super::super::Foundation::HWND = super::super::Foundation::HWND(-1i32 as _);

pub const HWND_NOTOPMOST: super::super::Foundation::HWND = super::super::Foundation::HWND(-2i32 as _);

pub const WS_SIZEBOX: WINDOW_STYLE = WINDOW_STYLE(262144u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct WINDOW_STYLE(pub u32);
impl ::core::marker::Copy for WINDOW_STYLE {}
impl ::core::clone::Clone for WINDOW_STYLE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for WINDOW_STYLE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for WINDOW_STYLE {
    type Abi = Self;
}
impl ::core::fmt::Debug for WINDOW_STYLE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("WINDOW_STYLE").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for WINDOW_STYLE {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for WINDOW_STYLE {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for WINDOW_STYLE {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for WINDOW_STYLE {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for WINDOW_STYLE {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub const WS_MAXIMIZEBOX: WINDOW_STYLE = WINDOW_STYLE(65536u32);

pub const WS_MINIMIZEBOX: WINDOW_STYLE = WINDOW_STYLE(131072u32);

pub const WS_POPUP: WINDOW_STYLE = WINDOW_STYLE(2147483648u32);

pub const WS_CLIPSIBLINGS: WINDOW_STYLE = WINDOW_STYLE(67108864u32);

pub const WS_CLIPCHILDREN: WINDOW_STYLE = WINDOW_STYLE(33554432u32);

pub const WS_SYSMENU: WINDOW_STYLE = WINDOW_STYLE(524288u32);

pub const WS_EX_WINDOWEDGE: WINDOW_EX_STYLE = WINDOW_EX_STYLE(256u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct WINDOW_EX_STYLE(pub u32);
impl ::core::marker::Copy for WINDOW_EX_STYLE {}
impl ::core::clone::Clone for WINDOW_EX_STYLE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for WINDOW_EX_STYLE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for WINDOW_EX_STYLE {
    type Abi = Self;
}
impl ::core::fmt::Debug for WINDOW_EX_STYLE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("WINDOW_EX_STYLE").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for WINDOW_EX_STYLE {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for WINDOW_EX_STYLE {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for WINDOW_EX_STYLE {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for WINDOW_EX_STYLE {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for WINDOW_EX_STYLE {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub const WS_EX_APPWINDOW: WINDOW_EX_STYLE = WINDOW_EX_STYLE(262144u32);

pub const WS_EX_ACCEPTFILES: WINDOW_EX_STYLE = WINDOW_EX_STYLE(16u32);

pub const WS_EX_TOPMOST: WINDOW_EX_STYLE = WINDOW_EX_STYLE(8u32);

pub const CW_USEDEFAULT: i32 = -2147483648i32;

pub const GWLP_USERDATA: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(-21i32);

pub const SW_SHOW: SHOW_WINDOW_CMD = SHOW_WINDOW_CMD(5u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct SHOW_WINDOW_CMD(pub u32);
impl ::core::marker::Copy for SHOW_WINDOW_CMD {}
impl ::core::clone::Clone for SHOW_WINDOW_CMD {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for SHOW_WINDOW_CMD {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for SHOW_WINDOW_CMD {
    type Abi = Self;
}
impl ::core::fmt::Debug for SHOW_WINDOW_CMD {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("SHOW_WINDOW_CMD").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for SHOW_WINDOW_CMD {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for SHOW_WINDOW_CMD {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for SHOW_WINDOW_CMD {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for SHOW_WINDOW_CMD {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for SHOW_WINDOW_CMD {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub const SW_RESTORE: SHOW_WINDOW_CMD = SHOW_WINDOW_CMD(9u32);

pub const SW_MAXIMIZE: SHOW_WINDOW_CMD = SHOW_WINDOW_CMD(3u32);

pub const SW_MINIMIZE: SHOW_WINDOW_CMD = SHOW_WINDOW_CMD(6u32);

pub const SWP_NOMOVE: SET_WINDOW_POS_FLAGS = SET_WINDOW_POS_FLAGS(2u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct SET_WINDOW_POS_FLAGS(pub u32);
impl ::core::marker::Copy for SET_WINDOW_POS_FLAGS {}
impl ::core::clone::Clone for SET_WINDOW_POS_FLAGS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for SET_WINDOW_POS_FLAGS {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for SET_WINDOW_POS_FLAGS {
    type Abi = Self;
}
impl ::core::fmt::Debug for SET_WINDOW_POS_FLAGS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("SET_WINDOW_POS_FLAGS").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for SET_WINDOW_POS_FLAGS {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for SET_WINDOW_POS_FLAGS {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for SET_WINDOW_POS_FLAGS {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for SET_WINDOW_POS_FLAGS {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for SET_WINDOW_POS_FLAGS {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub const SWP_NOSIZE: SET_WINDOW_POS_FLAGS = SET_WINDOW_POS_FLAGS(1u32);

pub const WM_ACTIVATE: u32 = 6u32;

pub const WM_NCCALCSIZE: u32 = 131u32;

pub const WM_NCHITTEST: u32 = 132u32;

pub const WA_ACTIVE: u32 = 1u32;

pub const WM_ERASEBKGND: u32 = 20u32;

pub const WM_MOUSEMOVE: u32 = 512u32;

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

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct WINDOWPLACEMENT_FLAGS(pub u32);
impl ::core::marker::Copy for WINDOWPLACEMENT_FLAGS {}
impl ::core::clone::Clone for WINDOWPLACEMENT_FLAGS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for WINDOWPLACEMENT_FLAGS {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for WINDOWPLACEMENT_FLAGS {
    type Abi = Self;
}
impl ::core::fmt::Debug for WINDOWPLACEMENT_FLAGS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("WINDOWPLACEMENT_FLAGS").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for WINDOWPLACEMENT_FLAGS {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for WINDOWPLACEMENT_FLAGS {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for WINDOWPLACEMENT_FLAGS {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for WINDOWPLACEMENT_FLAGS {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for WINDOWPLACEMENT_FLAGS {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub type TIMERPROC = ::core::option::Option<unsafe extern "system" fn(param0: super::super::Foundation::HWND, param1: u32, param2: usize, param3: u32)>;

#[repr(C)]
pub struct MSG {
    pub hwnd: super::super::Foundation::HWND,
    pub message: u32,
    pub wParam: super::super::Foundation::WPARAM,
    pub lParam: super::super::Foundation::LPARAM,
    pub time: u32,
    pub pt: super::super::Foundation::POINT,
}
impl ::core::marker::Copy for MSG {}
impl ::core::cmp::Eq for MSG {}
impl ::core::cmp::PartialEq for MSG {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<MSG>()) == 0 }
    }
}
impl ::core::clone::Clone for MSG {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for MSG {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for MSG {
    type Abi = Self;
}
impl ::core::fmt::Debug for MSG {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("MSG").field("hwnd", &self.hwnd).field("message", &self.message).field("wParam", &self.wParam).field("lParam", &self.lParam).field("time", &self.time).field("pt", &self.pt).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct HMENU(pub isize);
impl HMENU {
    pub fn is_invalid(&self) -> bool {
        self.0 == -1 || self.0 == 0
    }
}
impl ::core::marker::Copy for HMENU {}
impl ::core::clone::Clone for HMENU {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for HMENU {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for HMENU {
    type Abi = Self;
}
impl ::core::fmt::Debug for HMENU {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("HMENU").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<HMENU>> for HMENU {
    fn from(optional: ::core::option::Option<HMENU>) -> HMENU {
        optional.unwrap_or_default()
    }
}

pub type WNDPROC = ::core::option::Option<unsafe extern "system" fn(param0: super::super::Foundation::HWND, param1: u32, param2: super::super::Foundation::WPARAM, param3: super::super::Foundation::LPARAM) -> super::super::Foundation::LRESULT>;

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct HCURSOR(pub isize);
impl HCURSOR {
    pub fn is_invalid(&self) -> bool {
        self.0 == -1 || self.0 == 0
    }
}
impl ::core::marker::Copy for HCURSOR {}
impl ::core::clone::Clone for HCURSOR {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for HCURSOR {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for HCURSOR {
    type Abi = Self;
}
impl ::core::fmt::Debug for HCURSOR {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("HCURSOR").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<HCURSOR>> for HCURSOR {
    fn from(optional: ::core::option::Option<HCURSOR>) -> HCURSOR {
        optional.unwrap_or_default()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct HICON(pub isize);
impl HICON {
    pub fn is_invalid(&self) -> bool {
        self.0 == -1 || self.0 == 0
    }
}
impl ::core::marker::Copy for HICON {}
impl ::core::clone::Clone for HICON {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for HICON {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for HICON {
    type Abi = Self;
}
impl ::core::fmt::Debug for HICON {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("HICON").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<HICON>> for HICON {
    fn from(optional: ::core::option::Option<HICON>) -> HICON {
        optional.unwrap_or_default()
    }
}

}
pub mod HiDpi{
#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct PROCESS_DPI_AWARENESS(pub i32);
impl ::core::marker::Copy for PROCESS_DPI_AWARENESS {}
impl ::core::clone::Clone for PROCESS_DPI_AWARENESS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for PROCESS_DPI_AWARENESS {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for PROCESS_DPI_AWARENESS {
    type Abi = Self;
}
impl ::core::fmt::Debug for PROCESS_DPI_AWARENESS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("PROCESS_DPI_AWARENESS").field(&self.0).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct DPI_AWARENESS_CONTEXT(pub isize);
impl DPI_AWARENESS_CONTEXT {
    pub fn is_invalid(&self) -> bool {
        self.0 == -1 || self.0 == 0
    }
}
impl ::core::marker::Copy for DPI_AWARENESS_CONTEXT {}
impl ::core::clone::Clone for DPI_AWARENESS_CONTEXT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DPI_AWARENESS_CONTEXT {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DPI_AWARENESS_CONTEXT {
    type Abi = Self;
}
impl ::core::fmt::Debug for DPI_AWARENESS_CONTEXT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("DPI_AWARENESS_CONTEXT").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<DPI_AWARENESS_CONTEXT>> for DPI_AWARENESS_CONTEXT {
    fn from(optional: ::core::option::Option<DPI_AWARENESS_CONTEXT>) -> DPI_AWARENESS_CONTEXT {
        optional.unwrap_or_default()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct MONITOR_DPI_TYPE(pub i32);
impl ::core::marker::Copy for MONITOR_DPI_TYPE {}
impl ::core::clone::Clone for MONITOR_DPI_TYPE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for MONITOR_DPI_TYPE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for MONITOR_DPI_TYPE {
    type Abi = Self;
}
impl ::core::fmt::Debug for MONITOR_DPI_TYPE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("MONITOR_DPI_TYPE").field(&self.0).finish()
    }
}

pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: DPI_AWARENESS_CONTEXT = DPI_AWARENESS_CONTEXT(-4i32 as _);

pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE: DPI_AWARENESS_CONTEXT = DPI_AWARENESS_CONTEXT(-3i32 as _);

pub const PROCESS_PER_MONITOR_DPI_AWARE: PROCESS_DPI_AWARENESS = PROCESS_DPI_AWARENESS(2i32);

pub const MDT_EFFECTIVE_DPI: MONITOR_DPI_TYPE = MONITOR_DPI_TYPE(0i32);

}
pub mod Controls{
#[repr(C)]
pub struct MARGINS {
    pub cxLeftWidth: i32,
    pub cxRightWidth: i32,
    pub cyTopHeight: i32,
    pub cyBottomHeight: i32,
}
impl ::core::marker::Copy for MARGINS {}
impl ::core::cmp::Eq for MARGINS {}
impl ::core::cmp::PartialEq for MARGINS {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<MARGINS>()) == 0 }
    }
}
impl ::core::clone::Clone for MARGINS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for MARGINS {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for MARGINS {
    type Abi = Self;
}
impl ::core::fmt::Debug for MARGINS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("MARGINS").field("cxLeftWidth", &self.cxLeftWidth).field("cxRightWidth", &self.cxRightWidth).field("cyTopHeight", &self.cyTopHeight).field("cyBottomHeight", &self.cyBottomHeight).finish()
    }
}

pub const WM_MOUSELEAVE: u32 = 675u32;

}
pub mod Input{
pub mod KeyboardAndMouse{
#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct VIRTUAL_KEY(pub u16);
impl ::core::marker::Copy for VIRTUAL_KEY {}
impl ::core::clone::Clone for VIRTUAL_KEY {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for VIRTUAL_KEY {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for VIRTUAL_KEY {
    type Abi = Self;
}
impl ::core::fmt::Debug for VIRTUAL_KEY {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("VIRTUAL_KEY").field(&self.0).finish()
    }
}

pub unsafe fn ReleaseCapture() -> super::super::super::Foundation::BOOL {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn ReleaseCapture() -> super::super::super::Foundation::BOOL;
    }
    ReleaseCapture()
}

pub unsafe fn SetCapture<'a, P0>(hwnd: P0) -> super::super::super::Foundation::HWND
where
    P0: ::std::convert::Into<super::super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn SetCapture(hwnd: super::super::super::Foundation::HWND) -> super::super::super::Foundation::HWND;
    }
    SetCapture(hwnd.into())
}

pub unsafe fn TrackMouseEvent(lpeventtrack: *mut TRACKMOUSEEVENT) -> super::super::super::Foundation::BOOL {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn TrackMouseEvent(lpeventtrack: *mut TRACKMOUSEEVENT) -> super::super::super::Foundation::BOOL;
    }
    TrackMouseEvent(::core::mem::transmute(lpeventtrack))
}

pub unsafe fn GetKeyState(nvirtkey: i32) -> i16 {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetKeyState(nvirtkey: i32) -> i16;
    }
    GetKeyState(nvirtkey)
}

#[repr(C)]
pub struct TRACKMOUSEEVENT {
    pub cbSize: u32,
    pub dwFlags: TRACKMOUSEEVENT_FLAGS,
    pub hwndTrack: super::super::super::Foundation::HWND,
    pub dwHoverTime: u32,
}
impl ::core::marker::Copy for TRACKMOUSEEVENT {}
impl ::core::cmp::Eq for TRACKMOUSEEVENT {}
impl ::core::cmp::PartialEq for TRACKMOUSEEVENT {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<TRACKMOUSEEVENT>()) == 0 }
    }
}
impl ::core::clone::Clone for TRACKMOUSEEVENT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for TRACKMOUSEEVENT {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for TRACKMOUSEEVENT {
    type Abi = Self;
}
impl ::core::fmt::Debug for TRACKMOUSEEVENT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("TRACKMOUSEEVENT").field("cbSize", &self.cbSize).field("dwFlags", &self.dwFlags).field("hwndTrack", &self.hwndTrack).field("dwHoverTime", &self.dwHoverTime).finish()
    }
}

pub const TME_LEAVE: TRACKMOUSEEVENT_FLAGS = TRACKMOUSEEVENT_FLAGS(2u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct TRACKMOUSEEVENT_FLAGS(pub u32);
impl ::core::marker::Copy for TRACKMOUSEEVENT_FLAGS {}
impl ::core::clone::Clone for TRACKMOUSEEVENT_FLAGS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for TRACKMOUSEEVENT_FLAGS {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for TRACKMOUSEEVENT_FLAGS {
    type Abi = Self;
}
impl ::core::fmt::Debug for TRACKMOUSEEVENT_FLAGS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("TRACKMOUSEEVENT_FLAGS").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for TRACKMOUSEEVENT_FLAGS {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for TRACKMOUSEEVENT_FLAGS {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for TRACKMOUSEEVENT_FLAGS {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for TRACKMOUSEEVENT_FLAGS {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for TRACKMOUSEEVENT_FLAGS {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(17u16);

pub const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(16u16);

pub const VK_MENU: VIRTUAL_KEY = VIRTUAL_KEY(18u16);

pub const VK_LWIN: VIRTUAL_KEY = VIRTUAL_KEY(91u16);

pub const VK_RWIN: VIRTUAL_KEY = VIRTUAL_KEY(92u16);

pub const VK_ESCAPE: VIRTUAL_KEY = VIRTUAL_KEY(27u16);

pub const VK_OEM_3: VIRTUAL_KEY = VIRTUAL_KEY(192u16);

pub const VK_0: VIRTUAL_KEY = VIRTUAL_KEY(48u16);

pub const VK_1: VIRTUAL_KEY = VIRTUAL_KEY(49u16);

pub const VK_2: VIRTUAL_KEY = VIRTUAL_KEY(50u16);

pub const VK_3: VIRTUAL_KEY = VIRTUAL_KEY(51u16);

pub const VK_4: VIRTUAL_KEY = VIRTUAL_KEY(52u16);

pub const VK_5: VIRTUAL_KEY = VIRTUAL_KEY(53u16);

pub const VK_6: VIRTUAL_KEY = VIRTUAL_KEY(54u16);

pub const VK_7: VIRTUAL_KEY = VIRTUAL_KEY(55u16);

pub const VK_8: VIRTUAL_KEY = VIRTUAL_KEY(56u16);

pub const VK_9: VIRTUAL_KEY = VIRTUAL_KEY(57u16);

pub const VK_OEM_MINUS: VIRTUAL_KEY = VIRTUAL_KEY(189u16);

pub const VK_OEM_PLUS: VIRTUAL_KEY = VIRTUAL_KEY(187u16);

pub const VK_BACK: VIRTUAL_KEY = VIRTUAL_KEY(8u16);

pub const VK_TAB: VIRTUAL_KEY = VIRTUAL_KEY(9u16);

pub const VK_Q: VIRTUAL_KEY = VIRTUAL_KEY(81u16);

pub const VK_W: VIRTUAL_KEY = VIRTUAL_KEY(87u16);

pub const VK_E: VIRTUAL_KEY = VIRTUAL_KEY(69u16);

pub const VK_R: VIRTUAL_KEY = VIRTUAL_KEY(82u16);

pub const VK_T: VIRTUAL_KEY = VIRTUAL_KEY(84u16);

pub const VK_Y: VIRTUAL_KEY = VIRTUAL_KEY(89u16);

pub const VK_U: VIRTUAL_KEY = VIRTUAL_KEY(85u16);

pub const VK_I: VIRTUAL_KEY = VIRTUAL_KEY(73u16);

pub const VK_O: VIRTUAL_KEY = VIRTUAL_KEY(79u16);

pub const VK_P: VIRTUAL_KEY = VIRTUAL_KEY(80u16);

pub const VK_OEM_4: VIRTUAL_KEY = VIRTUAL_KEY(219u16);

pub const VK_OEM_6: VIRTUAL_KEY = VIRTUAL_KEY(221u16);

pub const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(13u16);

pub const VK_A: VIRTUAL_KEY = VIRTUAL_KEY(65u16);

pub const VK_S: VIRTUAL_KEY = VIRTUAL_KEY(83u16);

pub const VK_D: VIRTUAL_KEY = VIRTUAL_KEY(68u16);

pub const VK_F: VIRTUAL_KEY = VIRTUAL_KEY(70u16);

pub const VK_G: VIRTUAL_KEY = VIRTUAL_KEY(71u16);

pub const VK_H: VIRTUAL_KEY = VIRTUAL_KEY(72u16);

pub const VK_J: VIRTUAL_KEY = VIRTUAL_KEY(74u16);

pub const VK_K: VIRTUAL_KEY = VIRTUAL_KEY(75u16);

pub const VK_L: VIRTUAL_KEY = VIRTUAL_KEY(76u16);

pub const VK_OEM_1: VIRTUAL_KEY = VIRTUAL_KEY(186u16);

pub const VK_OEM_7: VIRTUAL_KEY = VIRTUAL_KEY(222u16);

pub const VK_OEM_5: VIRTUAL_KEY = VIRTUAL_KEY(220u16);

pub const VK_Z: VIRTUAL_KEY = VIRTUAL_KEY(90u16);

pub const VK_X: VIRTUAL_KEY = VIRTUAL_KEY(88u16);

pub const VK_C: VIRTUAL_KEY = VIRTUAL_KEY(67u16);

pub const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(86u16);

pub const VK_B: VIRTUAL_KEY = VIRTUAL_KEY(66u16);

pub const VK_N: VIRTUAL_KEY = VIRTUAL_KEY(78u16);

pub const VK_M: VIRTUAL_KEY = VIRTUAL_KEY(77u16);

pub const VK_OEM_COMMA: VIRTUAL_KEY = VIRTUAL_KEY(188u16);

pub const VK_OEM_PERIOD: VIRTUAL_KEY = VIRTUAL_KEY(190u16);

pub const VK_OEM_2: VIRTUAL_KEY = VIRTUAL_KEY(191u16);

pub const VK_LCONTROL: VIRTUAL_KEY = VIRTUAL_KEY(162u16);

pub const VK_RCONTROL: VIRTUAL_KEY = VIRTUAL_KEY(163u16);

pub const VK_LMENU: VIRTUAL_KEY = VIRTUAL_KEY(164u16);

pub const VK_RMENU: VIRTUAL_KEY = VIRTUAL_KEY(165u16);

pub const VK_LSHIFT: VIRTUAL_KEY = VIRTUAL_KEY(160u16);

pub const VK_RSHIFT: VIRTUAL_KEY = VIRTUAL_KEY(161u16);

pub const VK_SPACE: VIRTUAL_KEY = VIRTUAL_KEY(32u16);

pub const VK_CAPITAL: VIRTUAL_KEY = VIRTUAL_KEY(20u16);

pub const VK_F1: VIRTUAL_KEY = VIRTUAL_KEY(112u16);

pub const VK_F2: VIRTUAL_KEY = VIRTUAL_KEY(113u16);

pub const VK_F3: VIRTUAL_KEY = VIRTUAL_KEY(114u16);

pub const VK_F4: VIRTUAL_KEY = VIRTUAL_KEY(115u16);

pub const VK_F5: VIRTUAL_KEY = VIRTUAL_KEY(116u16);

pub const VK_F6: VIRTUAL_KEY = VIRTUAL_KEY(117u16);

pub const VK_F7: VIRTUAL_KEY = VIRTUAL_KEY(118u16);

pub const VK_F8: VIRTUAL_KEY = VIRTUAL_KEY(119u16);

pub const VK_F9: VIRTUAL_KEY = VIRTUAL_KEY(120u16);

pub const VK_F10: VIRTUAL_KEY = VIRTUAL_KEY(121u16);

pub const VK_F11: VIRTUAL_KEY = VIRTUAL_KEY(122u16);

pub const VK_F12: VIRTUAL_KEY = VIRTUAL_KEY(123u16);

pub const VK_SNAPSHOT: VIRTUAL_KEY = VIRTUAL_KEY(44u16);

pub const VK_SCROLL: VIRTUAL_KEY = VIRTUAL_KEY(145u16);

pub const VK_PAUSE: VIRTUAL_KEY = VIRTUAL_KEY(19u16);

pub const VK_INSERT: VIRTUAL_KEY = VIRTUAL_KEY(45u16);

pub const VK_DELETE: VIRTUAL_KEY = VIRTUAL_KEY(46u16);

pub const VK_HOME: VIRTUAL_KEY = VIRTUAL_KEY(36u16);

pub const VK_END: VIRTUAL_KEY = VIRTUAL_KEY(35u16);

pub const VK_PRIOR: VIRTUAL_KEY = VIRTUAL_KEY(33u16);

pub const VK_NEXT: VIRTUAL_KEY = VIRTUAL_KEY(34u16);

pub const VK_NUMPAD0: VIRTUAL_KEY = VIRTUAL_KEY(96u16);

pub const VK_NUMPAD1: VIRTUAL_KEY = VIRTUAL_KEY(97u16);

pub const VK_NUMPAD2: VIRTUAL_KEY = VIRTUAL_KEY(98u16);

pub const VK_NUMPAD3: VIRTUAL_KEY = VIRTUAL_KEY(99u16);

pub const VK_NUMPAD4: VIRTUAL_KEY = VIRTUAL_KEY(100u16);

pub const VK_NUMPAD5: VIRTUAL_KEY = VIRTUAL_KEY(101u16);

pub const VK_NUMPAD6: VIRTUAL_KEY = VIRTUAL_KEY(102u16);

pub const VK_NUMPAD7: VIRTUAL_KEY = VIRTUAL_KEY(103u16);

pub const VK_NUMPAD8: VIRTUAL_KEY = VIRTUAL_KEY(104u16);

pub const VK_NUMPAD9: VIRTUAL_KEY = VIRTUAL_KEY(105u16);

pub const VK_SUBTRACT: VIRTUAL_KEY = VIRTUAL_KEY(109u16);

pub const VK_ADD: VIRTUAL_KEY = VIRTUAL_KEY(107u16);

pub const VK_DECIMAL: VIRTUAL_KEY = VIRTUAL_KEY(110u16);

pub const VK_MULTIPLY: VIRTUAL_KEY = VIRTUAL_KEY(106u16);

pub const VK_DIVIDE: VIRTUAL_KEY = VIRTUAL_KEY(111u16);

pub const VK_NUMLOCK: VIRTUAL_KEY = VIRTUAL_KEY(144u16);

pub const VK_UP: VIRTUAL_KEY = VIRTUAL_KEY(38u16);

pub const VK_DOWN: VIRTUAL_KEY = VIRTUAL_KEY(40u16);

pub const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(37u16);

pub const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(39u16);

}
}
}
pub mod Graphics{
pub mod Gdi{
#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct HMONITOR(pub isize);
impl HMONITOR {
    pub fn is_invalid(&self) -> bool {
        self.0 == -1 || self.0 == 0
    }
}
impl ::core::marker::Copy for HMONITOR {}
impl ::core::clone::Clone for HMONITOR {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for HMONITOR {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for HMONITOR {
    type Abi = Self;
}
impl ::core::fmt::Debug for HMONITOR {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("HMONITOR").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<HMONITOR>> for HMONITOR {
    fn from(optional: ::core::option::Option<HMONITOR>) -> HMONITOR {
        optional.unwrap_or_default()
    }
}

pub unsafe fn GetDC<'a, P0>(hwnd: P0) -> HDC
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetDC(hwnd: super::super::Foundation::HWND) -> HDC;
    }
    GetDC(hwnd.into())
}

pub unsafe fn MonitorFromWindow<'a, P0>(hwnd: P0, dwflags: MONITOR_FROM_FLAGS) -> HMONITOR
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn MonitorFromWindow(hwnd: super::super::Foundation::HWND, dwflags: MONITOR_FROM_FLAGS) -> HMONITOR;
    }
    MonitorFromWindow(hwnd.into(), dwflags)
}

pub unsafe fn GetDeviceCaps<'a, P0>(hdc: P0, index: GET_DEVICE_CAPS_INDEX) -> i32
where
    P0: ::std::convert::Into<HDC>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetDeviceCaps(hdc: HDC, index: GET_DEVICE_CAPS_INDEX) -> i32;
    }
    GetDeviceCaps(hdc.into(), index)
}

pub const MONITOR_DEFAULTTONEAREST: MONITOR_FROM_FLAGS = MONITOR_FROM_FLAGS(2u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct MONITOR_FROM_FLAGS(pub u32);
impl ::core::marker::Copy for MONITOR_FROM_FLAGS {}
impl ::core::clone::Clone for MONITOR_FROM_FLAGS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for MONITOR_FROM_FLAGS {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for MONITOR_FROM_FLAGS {
    type Abi = Self;
}
impl ::core::fmt::Debug for MONITOR_FROM_FLAGS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("MONITOR_FROM_FLAGS").field(&self.0).finish()
    }
}

pub const LOGPIXELSX: GET_DEVICE_CAPS_INDEX = GET_DEVICE_CAPS_INDEX(88u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct GET_DEVICE_CAPS_INDEX(pub u32);
impl ::core::marker::Copy for GET_DEVICE_CAPS_INDEX {}
impl ::core::clone::Clone for GET_DEVICE_CAPS_INDEX {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for GET_DEVICE_CAPS_INDEX {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for GET_DEVICE_CAPS_INDEX {
    type Abi = Self;
}
impl ::core::fmt::Debug for GET_DEVICE_CAPS_INDEX {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("GET_DEVICE_CAPS_INDEX").field(&self.0).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct HDC(pub isize);
impl HDC {
    pub fn is_invalid(&self) -> bool {
        self.0 == -1 || self.0 == 0
    }
}
impl ::core::marker::Copy for HDC {}
impl ::core::clone::Clone for HDC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for HDC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for HDC {
    type Abi = Self;
}
impl ::core::fmt::Debug for HDC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("HDC").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<HDC>> for HDC {
    fn from(optional: ::core::option::Option<HDC>) -> HDC {
        optional.unwrap_or_default()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct HBRUSH(pub isize);
impl HBRUSH {
    pub fn is_invalid(&self) -> bool {
        self.0 == -1 || self.0 == 0
    }
}
impl ::core::marker::Copy for HBRUSH {}
impl ::core::clone::Clone for HBRUSH {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for HBRUSH {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for HBRUSH {
    type Abi = Self;
}
impl ::core::fmt::Debug for HBRUSH {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("HBRUSH").field(&self.0).finish()
    }
}
impl ::core::convert::From<::core::option::Option<HBRUSH>> for HBRUSH {
    fn from(optional: ::core::option::Option<HBRUSH>) -> HBRUSH {
        optional.unwrap_or_default()
    }
}

}
pub mod Dwm{
pub unsafe fn DwmExtendFrameIntoClientArea<'a, P0>(hwnd: P0, pmarinset: *const super::super::UI::Controls::MARGINS) -> ::windows::core::Result<()>
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn DwmExtendFrameIntoClientArea(hwnd: super::super::Foundation::HWND, pmarinset: *const super::super::UI::Controls::MARGINS) -> ::windows::core::HRESULT;
    }
    DwmExtendFrameIntoClientArea(hwnd.into(), ::core::mem::transmute(pmarinset)).ok()
}

}
pub mod Direct3D{
#[repr(transparent)]
pub struct ID3DInclude(::std::ptr::NonNull<::std::ffi::c_void>);
impl ID3DInclude {
    pub unsafe fn Open<'a, P0>(&self, includetype: D3D_INCLUDE_TYPE, pfilename: P0, pparentdata: *const ::core::ffi::c_void, ppdata: *mut *mut ::core::ffi::c_void, pbytes: *mut u32) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::PCSTR>,
    {
        (::windows::core::Vtable::vtable(self).Open)(::windows::core::Vtable::as_raw(self), includetype, pfilename.into(), ::core::mem::transmute(pparentdata), ::core::mem::transmute(ppdata), ::core::mem::transmute(pbytes)).ok()
    }
    pub unsafe fn Close(&self, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).Close)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdata)).ok()
    }
}
impl ::core::cmp::Eq for ID3DInclude {}
impl ::core::cmp::PartialEq for ID3DInclude {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3DInclude {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3DInclude {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3DInclude").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3DInclude {}
unsafe impl ::core::marker::Sync for ID3DInclude {}
unsafe impl ::windows::core::Vtable for ID3DInclude {
    type Vtable = ID3DInclude_Vtbl;
}

#[repr(C)]
pub struct ID3DInclude_Vtbl {
    pub Open: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, includetype: D3D_INCLUDE_TYPE, pfilename: ::windows::core::PCSTR, pparentdata: *const ::core::ffi::c_void, ppdata: *mut *mut ::core::ffi::c_void, pbytes: *mut u32) -> ::windows::core::HRESULT,
    pub Close: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdata: *const ::core::ffi::c_void) -> ::windows::core::HRESULT,
}

pub trait ID3DInclude_Impl: Sized {
    fn Open(&self, includetype: D3D_INCLUDE_TYPE, pfilename: &::windows::core::PCSTR, pparentdata: *const ::core::ffi::c_void, ppdata: *mut *mut ::core::ffi::c_void, pbytes: *mut u32) -> ::windows::core::Result<()>;
    fn Close(&self, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()>;
}

impl ID3DInclude_Vtbl {
    pub const fn new<Impl: ID3DInclude_Impl>() -> ID3DInclude_Vtbl {
        unsafe extern "system" fn Open<Impl: ID3DInclude_Impl>(this: *mut ::core::ffi::c_void, includetype: D3D_INCLUDE_TYPE, pfilename: ::windows::core::PCSTR, pparentdata: *const ::core::ffi::c_void, ppdata: *mut *mut ::core::ffi::c_void, pbytes: *mut u32) -> ::windows::core::HRESULT {
            let this = (this as *mut *mut ::core::ffi::c_void) as *const ::windows::core::ScopedHeap;
            let this = &*((*this).this as *const Impl);
            this.Open(::core::mem::transmute_copy(&includetype), ::core::mem::transmute(&pfilename), ::core::mem::transmute_copy(&pparentdata), ::core::mem::transmute_copy(&ppdata), ::core::mem::transmute_copy(&pbytes)).into()
        }
        unsafe extern "system" fn Close<Impl: ID3DInclude_Impl>(this: *mut ::core::ffi::c_void, pdata: *const ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *mut *mut ::core::ffi::c_void) as *const ::windows::core::ScopedHeap;
            let this = &*((*this).this as *const Impl);
            this.Close(::core::mem::transmute_copy(&pdata)).into()
        }
        Self { Open: Open::<Impl>, Close: Close::<Impl> }
    }
}

#[repr(C)]
pub struct D3D_SHADER_MACRO {
    pub Name: ::windows::core::PCSTR,
    pub Definition: ::windows::core::PCSTR,
}
impl ::core::marker::Copy for D3D_SHADER_MACRO {}
impl ::core::cmp::Eq for D3D_SHADER_MACRO {}
impl ::core::cmp::PartialEq for D3D_SHADER_MACRO {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D_SHADER_MACRO>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D_SHADER_MACRO {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D_SHADER_MACRO {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D_SHADER_MACRO {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D_SHADER_MACRO {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D_SHADER_MACRO").field("Name", &self.Name).field("Definition", &self.Definition).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D_SRV_DIMENSION(pub i32);
impl ::core::marker::Copy for D3D_SRV_DIMENSION {}
impl ::core::clone::Clone for D3D_SRV_DIMENSION {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D_SRV_DIMENSION {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D_SRV_DIMENSION {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D_SRV_DIMENSION {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D_SRV_DIMENSION").field(&self.0).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D_INCLUDE_TYPE(pub i32);
impl ::core::marker::Copy for D3D_INCLUDE_TYPE {}
impl ::core::clone::Clone for D3D_INCLUDE_TYPE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D_INCLUDE_TYPE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D_INCLUDE_TYPE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D_INCLUDE_TYPE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D_INCLUDE_TYPE").field(&self.0).finish()
    }
}

pub mod Fxc{
pub unsafe fn D3DCompile<'a, P0, P1, P2, P3>(psrcdata: *const ::core::ffi::c_void, srcdatasize: usize, psourcename: P0, pdefines: ::core::option::Option<*const super::D3D_SHADER_MACRO>, pinclude: P1, pentrypoint: P2, ptarget: P3, flags1: u32, flags2: u32, ppcode: *mut ::core::option::Option<super::ID3DBlob>, pperrormsgs: ::core::option::Option<*mut ::core::option::Option<super::ID3DBlob>>) -> ::windows::core::Result<()>
where
    P0: ::std::convert::Into<::windows::core::PCSTR>,
    P1: ::std::convert::Into<::windows::core::InParam<'a, super::ID3DInclude>>,
    P2: ::std::convert::Into<::windows::core::PCSTR>,
    P3: ::std::convert::Into<::windows::core::PCSTR>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn D3DCompile(psrcdata: *const ::core::ffi::c_void, srcdatasize: usize, psourcename: ::windows::core::PCSTR, pdefines: *const super::D3D_SHADER_MACRO, pinclude: *mut ::core::ffi::c_void, pentrypoint: ::windows::core::PCSTR, ptarget: ::windows::core::PCSTR, flags1: u32, flags2: u32, ppcode: *mut *mut ::core::ffi::c_void, pperrormsgs: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT;
    }
    D3DCompile(::core::mem::transmute(psrcdata), srcdatasize, psourcename.into(), ::core::mem::transmute(pdefines.unwrap_or(::std::ptr::null())), pinclude.into().abi(), pentrypoint.into(), ptarget.into(), flags1, flags2, ::core::mem::transmute(ppcode), ::core::mem::transmute(pperrormsgs.unwrap_or(::std::ptr::null_mut()))).ok()
}

}
#[repr(transparent)]
pub struct ID3DBlob(::windows::core::IUnknown);
impl ID3DBlob {
    pub unsafe fn GetBufferPointer(&self) -> *mut ::core::ffi::c_void {
        (::windows::core::Vtable::vtable(self).GetBufferPointer)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetBufferSize(&self) -> usize {
        (::windows::core::Vtable::vtable(self).GetBufferSize)(::windows::core::Vtable::as_raw(self))
    }
}
impl ::core::cmp::Eq for ID3DBlob {}
impl ::core::cmp::PartialEq for ID3DBlob {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3DBlob {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3DBlob {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3DBlob").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3DBlob {}
unsafe impl ::core::marker::Sync for ID3DBlob {}
unsafe impl ::windows::core::Vtable for ID3DBlob {
    type Vtable = ID3DBlob_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3DBlob {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x8ba5fb08_5195_40e2_ac58_0d989c3a0102);
}

#[repr(C)]
pub struct ID3DBlob_Vtbl {
    pub base__: ::windows::core::IUnknown_Vtbl,
    pub GetBufferPointer: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> *mut ::core::ffi::c_void,
    pub GetBufferSize: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> usize,
}

pub trait ID3DBlob_Impl: Sized {
    fn GetBufferPointer(&self) -> *mut ::core::ffi::c_void;
    fn GetBufferSize(&self) -> usize;
}

impl ID3DBlob_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3DBlob_Impl, const OFFSET: isize>() -> ID3DBlob_Vtbl {
        unsafe extern "system" fn GetBufferPointer<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3DBlob_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> *mut ::core::ffi::c_void {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetBufferPointer()
        }
        unsafe extern "system" fn GetBufferSize<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3DBlob_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> usize {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetBufferSize()
        }
        Self {
            base__: ::windows::core::IUnknown_Vtbl::new::<Identity, OFFSET>(),
            GetBufferPointer: GetBufferPointer::<Identity, Impl, OFFSET>,
            GetBufferSize: GetBufferSize::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3DBlob as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3DBlob, ::windows::core::IUnknown);

pub const D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST: D3D_PRIMITIVE_TOPOLOGY = D3D_PRIMITIVE_TOPOLOGY(4i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D_PRIMITIVE_TOPOLOGY(pub i32);
impl ::core::marker::Copy for D3D_PRIMITIVE_TOPOLOGY {}
impl ::core::clone::Clone for D3D_PRIMITIVE_TOPOLOGY {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D_PRIMITIVE_TOPOLOGY {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D_PRIMITIVE_TOPOLOGY {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D_PRIMITIVE_TOPOLOGY {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D_PRIMITIVE_TOPOLOGY").field(&self.0).finish()
    }
}

pub const D3D_DRIVER_TYPE_UNKNOWN: D3D_DRIVER_TYPE = D3D_DRIVER_TYPE(0i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D_DRIVER_TYPE(pub i32);
impl ::core::marker::Copy for D3D_DRIVER_TYPE {}
impl ::core::clone::Clone for D3D_DRIVER_TYPE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D_DRIVER_TYPE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D_DRIVER_TYPE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D_DRIVER_TYPE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D_DRIVER_TYPE").field(&self.0).finish()
    }
}

pub const D3D_FEATURE_LEVEL_11_0: D3D_FEATURE_LEVEL = D3D_FEATURE_LEVEL(45056i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D_FEATURE_LEVEL(pub i32);
impl ::core::marker::Copy for D3D_FEATURE_LEVEL {}
impl ::core::clone::Clone for D3D_FEATURE_LEVEL {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D_FEATURE_LEVEL {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D_FEATURE_LEVEL {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D_FEATURE_LEVEL {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D_FEATURE_LEVEL").field(&self.0).finish()
    }
}

}
pub mod Direct3D11{
#[repr(transparent)]
pub struct ID3D11Asynchronous(::windows::core::IUnknown);
impl ID3D11Asynchronous {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetDataSize(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).GetDataSize)(::windows::core::Vtable::as_raw(self))
    }
}
impl ::core::cmp::Eq for ID3D11Asynchronous {}
impl ::core::cmp::PartialEq for ID3D11Asynchronous {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Asynchronous {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Asynchronous {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Asynchronous").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Asynchronous {}
unsafe impl ::core::marker::Sync for ID3D11Asynchronous {}
unsafe impl ::windows::core::Vtable for ID3D11Asynchronous {
    type Vtable = ID3D11Asynchronous_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Asynchronous {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x4b35d0cd_1e15_4258_9c98_1b1333f6dd3b);
}

#[repr(C)]
pub struct ID3D11Asynchronous_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    pub GetDataSize: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> u32,
}

pub trait ID3D11Asynchronous_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetDataSize(&self) -> u32;
}

impl ID3D11Asynchronous_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Asynchronous_Impl, const OFFSET: isize>() -> ID3D11Asynchronous_Vtbl {
        unsafe extern "system" fn GetDataSize<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Asynchronous_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> u32 {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDataSize()
        }
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(), GetDataSize: GetDataSize::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Asynchronous as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Asynchronous, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11ClassInstance(::windows::core::IUnknown);
impl ID3D11ClassInstance {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetClassLinkage(&self, pplinkage: *mut ::core::option::Option<ID3D11ClassLinkage>) {
        (::windows::core::Vtable::vtable(self).GetClassLinkage)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pplinkage))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_CLASS_INSTANCE_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
    pub unsafe fn GetInstanceName(&self, pinstancename: ::windows::core::PSTR, pbufferlength: *mut usize) {
        (::windows::core::Vtable::vtable(self).GetInstanceName)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pinstancename), ::core::mem::transmute(pbufferlength))
    }
    pub unsafe fn GetTypeName(&self, ptypename: ::windows::core::PSTR, pbufferlength: *mut usize) {
        (::windows::core::Vtable::vtable(self).GetTypeName)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ptypename), ::core::mem::transmute(pbufferlength))
    }
}
impl ::core::cmp::Eq for ID3D11ClassInstance {}
impl ::core::cmp::PartialEq for ID3D11ClassInstance {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11ClassInstance {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11ClassInstance {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11ClassInstance").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11ClassInstance {}
unsafe impl ::core::marker::Sync for ID3D11ClassInstance {}
unsafe impl ::windows::core::Vtable for ID3D11ClassInstance {
    type Vtable = ID3D11ClassInstance_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11ClassInstance {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xa6cd7faa_b0b7_4a2f_9436_8662a65797cb);
}

#[repr(C)]
pub struct ID3D11ClassInstance_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    pub GetClassLinkage: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pplinkage: *mut *mut ::core::ffi::c_void),
    #[cfg(feature = "Win32_Foundation")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_CLASS_INSTANCE_DESC),
    #[cfg(not(feature = "Win32_Foundation"))]
    GetDesc: usize,
    pub GetInstanceName: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pinstancename: ::windows::core::PSTR, pbufferlength: *mut usize),
    pub GetTypeName: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ptypename: ::windows::core::PSTR, pbufferlength: *mut usize),
}

pub trait ID3D11ClassInstance_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetClassLinkage(&self, pplinkage: *mut ::core::option::Option<ID3D11ClassLinkage>);
    fn GetDesc(&self, pdesc: *mut D3D11_CLASS_INSTANCE_DESC);
    fn GetInstanceName(&self, pinstancename: ::windows::core::PSTR, pbufferlength: *mut usize);
    fn GetTypeName(&self, ptypename: ::windows::core::PSTR, pbufferlength: *mut usize);
}

impl ID3D11ClassInstance_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ClassInstance_Impl, const OFFSET: isize>() -> ID3D11ClassInstance_Vtbl {
        unsafe extern "system" fn GetClassLinkage<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ClassInstance_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pplinkage: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetClassLinkage(::core::mem::transmute_copy(&pplinkage))
        }
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ClassInstance_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_CLASS_INSTANCE_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        unsafe extern "system" fn GetInstanceName<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ClassInstance_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pinstancename: ::windows::core::PSTR, pbufferlength: *mut usize) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetInstanceName(::core::mem::transmute_copy(&pinstancename), ::core::mem::transmute_copy(&pbufferlength))
        }
        unsafe extern "system" fn GetTypeName<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ClassInstance_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ptypename: ::windows::core::PSTR, pbufferlength: *mut usize) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetTypeName(::core::mem::transmute_copy(&ptypename), ::core::mem::transmute_copy(&pbufferlength))
        }
        Self {
            base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(),
            GetClassLinkage: GetClassLinkage::<Identity, Impl, OFFSET>,
            GetDesc: GetDesc::<Identity, Impl, OFFSET>,
            GetInstanceName: GetInstanceName::<Identity, Impl, OFFSET>,
            GetTypeName: GetTypeName::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11ClassInstance as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11ClassInstance, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11Texture3D(::windows::core::IUnknown);
impl ID3D11Texture3D {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetType(&self, presourcedimension: *mut D3D11_RESOURCE_DIMENSION) {
        (::windows::core::Vtable::vtable(self).base__.GetType)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(presourcedimension))
    }
    pub unsafe fn SetEvictionPriority(&self, evictionpriority: u32) {
        (::windows::core::Vtable::vtable(self).base__.SetEvictionPriority)(::windows::core::Vtable::as_raw(self), evictionpriority)
    }
    pub unsafe fn GetEvictionPriority(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).base__.GetEvictionPriority)(::windows::core::Vtable::as_raw(self))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_TEXTURE3D_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11Texture3D {}
impl ::core::cmp::PartialEq for ID3D11Texture3D {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Texture3D {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Texture3D {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Texture3D").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Texture3D {}
unsafe impl ::core::marker::Sync for ID3D11Texture3D {}
unsafe impl ::windows::core::Vtable for ID3D11Texture3D {
    type Vtable = ID3D11Texture3D_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Texture3D {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x037e866e_f56d_4357_a8af_9dabbe6e250e);
}

#[repr(C)]
pub struct ID3D11Texture3D_Vtbl {
    pub base__: ID3D11Resource_Vtbl,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_TEXTURE3D_DESC),
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetDesc: usize,
}

pub trait ID3D11Texture3D_Impl: Sized + ID3D11Resource_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_TEXTURE3D_DESC);
}

impl ID3D11Texture3D_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Texture3D_Impl, const OFFSET: isize>() -> ID3D11Texture3D_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Texture3D_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_TEXTURE3D_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11Resource_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Texture3D as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11Resource as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Texture3D, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11Resource);

#[repr(transparent)]
pub struct ID3D11Texture1D(::windows::core::IUnknown);
impl ID3D11Texture1D {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetType(&self, presourcedimension: *mut D3D11_RESOURCE_DIMENSION) {
        (::windows::core::Vtable::vtable(self).base__.GetType)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(presourcedimension))
    }
    pub unsafe fn SetEvictionPriority(&self, evictionpriority: u32) {
        (::windows::core::Vtable::vtable(self).base__.SetEvictionPriority)(::windows::core::Vtable::as_raw(self), evictionpriority)
    }
    pub unsafe fn GetEvictionPriority(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).base__.GetEvictionPriority)(::windows::core::Vtable::as_raw(self))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_TEXTURE1D_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11Texture1D {}
impl ::core::cmp::PartialEq for ID3D11Texture1D {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Texture1D {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Texture1D {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Texture1D").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Texture1D {}
unsafe impl ::core::marker::Sync for ID3D11Texture1D {}
unsafe impl ::windows::core::Vtable for ID3D11Texture1D {
    type Vtable = ID3D11Texture1D_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Texture1D {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xf8fb5c27_c6b3_4f75_a4c8_439af2ef564c);
}

#[repr(C)]
pub struct ID3D11Texture1D_Vtbl {
    pub base__: ID3D11Resource_Vtbl,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_TEXTURE1D_DESC),
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetDesc: usize,
}

pub trait ID3D11Texture1D_Impl: Sized + ID3D11Resource_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_TEXTURE1D_DESC);
}

impl ID3D11Texture1D_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Texture1D_Impl, const OFFSET: isize>() -> ID3D11Texture1D_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Texture1D_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_TEXTURE1D_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11Resource_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Texture1D as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11Resource as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Texture1D, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11Resource);

#[repr(transparent)]
pub struct ID3D11Resource(::windows::core::IUnknown);
impl ID3D11Resource {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetType(&self, presourcedimension: *mut D3D11_RESOURCE_DIMENSION) {
        (::windows::core::Vtable::vtable(self).GetType)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(presourcedimension))
    }
    pub unsafe fn SetEvictionPriority(&self, evictionpriority: u32) {
        (::windows::core::Vtable::vtable(self).SetEvictionPriority)(::windows::core::Vtable::as_raw(self), evictionpriority)
    }
    pub unsafe fn GetEvictionPriority(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).GetEvictionPriority)(::windows::core::Vtable::as_raw(self))
    }
}
impl ::core::cmp::Eq for ID3D11Resource {}
impl ::core::cmp::PartialEq for ID3D11Resource {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Resource {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Resource {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Resource").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Resource {}
unsafe impl ::core::marker::Sync for ID3D11Resource {}
unsafe impl ::windows::core::Vtable for ID3D11Resource {
    type Vtable = ID3D11Resource_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Resource {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xdc8e63f3_d12b_4952_b47b_5e45026a862d);
}

#[repr(C)]
pub struct ID3D11Resource_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    pub GetType: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, presourcedimension: *mut D3D11_RESOURCE_DIMENSION),
    pub SetEvictionPriority: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, evictionpriority: u32),
    pub GetEvictionPriority: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> u32,
}

pub trait ID3D11Resource_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetType(&self, presourcedimension: *mut D3D11_RESOURCE_DIMENSION);
    fn SetEvictionPriority(&self, evictionpriority: u32);
    fn GetEvictionPriority(&self) -> u32;
}

impl ID3D11Resource_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Resource_Impl, const OFFSET: isize>() -> ID3D11Resource_Vtbl {
        unsafe extern "system" fn GetType<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Resource_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, presourcedimension: *mut D3D11_RESOURCE_DIMENSION) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetType(::core::mem::transmute_copy(&presourcedimension))
        }
        unsafe extern "system" fn SetEvictionPriority<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Resource_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, evictionpriority: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetEvictionPriority(::core::mem::transmute_copy(&evictionpriority))
        }
        unsafe extern "system" fn GetEvictionPriority<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Resource_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> u32 {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetEvictionPriority()
        }
        Self {
            base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(),
            GetType: GetType::<Identity, Impl, OFFSET>,
            SetEvictionPriority: SetEvictionPriority::<Identity, Impl, OFFSET>,
            GetEvictionPriority: GetEvictionPriority::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Resource as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Resource, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11UnorderedAccessView(::windows::core::IUnknown);
impl ID3D11UnorderedAccessView {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetResource(&self, ppresource: *mut ::core::option::Option<ID3D11Resource>) {
        (::windows::core::Vtable::vtable(self).base__.GetResource)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppresource))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_UNORDERED_ACCESS_VIEW_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11UnorderedAccessView {}
impl ::core::cmp::PartialEq for ID3D11UnorderedAccessView {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11UnorderedAccessView {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11UnorderedAccessView {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11UnorderedAccessView").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11UnorderedAccessView {}
unsafe impl ::core::marker::Sync for ID3D11UnorderedAccessView {}
unsafe impl ::windows::core::Vtable for ID3D11UnorderedAccessView {
    type Vtable = ID3D11UnorderedAccessView_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11UnorderedAccessView {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x28acf509_7f5c_48f6_8611_f316010a6380);
}

#[repr(C)]
pub struct ID3D11UnorderedAccessView_Vtbl {
    pub base__: ID3D11View_Vtbl,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_UNORDERED_ACCESS_VIEW_DESC),
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetDesc: usize,
}

pub trait ID3D11UnorderedAccessView_Impl: Sized + ID3D11View_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_UNORDERED_ACCESS_VIEW_DESC);
}

impl ID3D11UnorderedAccessView_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11UnorderedAccessView_Impl, const OFFSET: isize>() -> ID3D11UnorderedAccessView_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11UnorderedAccessView_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_UNORDERED_ACCESS_VIEW_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11View_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11UnorderedAccessView as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11View as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11UnorderedAccessView, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11View);

#[repr(transparent)]
pub struct ID3D11ClassLinkage(::windows::core::IUnknown);
impl ID3D11ClassLinkage {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetClassInstance<'a, P0>(&self, pclassinstancename: P0, instanceindex: u32) -> ::windows::core::Result<ID3D11ClassInstance>
    where
        P0: ::std::convert::Into<::windows::core::PCSTR>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetClassInstance)(::windows::core::Vtable::as_raw(self), pclassinstancename.into(), instanceindex, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11ClassInstance>(result__)
    }
    pub unsafe fn CreateClassInstance<'a, P0>(&self, pclasstypename: P0, constantbufferoffset: u32, constantvectoroffset: u32, textureoffset: u32, sampleroffset: u32) -> ::windows::core::Result<ID3D11ClassInstance>
    where
        P0: ::std::convert::Into<::windows::core::PCSTR>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateClassInstance)(::windows::core::Vtable::as_raw(self), pclasstypename.into(), constantbufferoffset, constantvectoroffset, textureoffset, sampleroffset, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11ClassInstance>(result__)
    }
}
impl ::core::cmp::Eq for ID3D11ClassLinkage {}
impl ::core::cmp::PartialEq for ID3D11ClassLinkage {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11ClassLinkage {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11ClassLinkage {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11ClassLinkage").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11ClassLinkage {}
unsafe impl ::core::marker::Sync for ID3D11ClassLinkage {}
unsafe impl ::windows::core::Vtable for ID3D11ClassLinkage {
    type Vtable = ID3D11ClassLinkage_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11ClassLinkage {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xddf57cba_9543_46e4_a12b_f207a0fe7fed);
}

#[repr(C)]
pub struct ID3D11ClassLinkage_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    pub GetClassInstance: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pclassinstancename: ::windows::core::PCSTR, instanceindex: u32, ppinstance: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateClassInstance: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pclasstypename: ::windows::core::PCSTR, constantbufferoffset: u32, constantvectoroffset: u32, textureoffset: u32, sampleroffset: u32, ppinstance: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
}

pub trait ID3D11ClassLinkage_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetClassInstance(&self, pclassinstancename: &::windows::core::PCSTR, instanceindex: u32) -> ::windows::core::Result<ID3D11ClassInstance>;
    fn CreateClassInstance(&self, pclasstypename: &::windows::core::PCSTR, constantbufferoffset: u32, constantvectoroffset: u32, textureoffset: u32, sampleroffset: u32) -> ::windows::core::Result<ID3D11ClassInstance>;
}

impl ID3D11ClassLinkage_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ClassLinkage_Impl, const OFFSET: isize>() -> ID3D11ClassLinkage_Vtbl {
        unsafe extern "system" fn GetClassInstance<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ClassLinkage_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pclassinstancename: ::windows::core::PCSTR, instanceindex: u32, ppinstance: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetClassInstance(::core::mem::transmute(&pclassinstancename), ::core::mem::transmute_copy(&instanceindex)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppinstance, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateClassInstance<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ClassLinkage_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pclasstypename: ::windows::core::PCSTR, constantbufferoffset: u32, constantvectoroffset: u32, textureoffset: u32, sampleroffset: u32, ppinstance: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateClassInstance(::core::mem::transmute(&pclasstypename), ::core::mem::transmute_copy(&constantbufferoffset), ::core::mem::transmute_copy(&constantvectoroffset), ::core::mem::transmute_copy(&textureoffset), ::core::mem::transmute_copy(&sampleroffset)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppinstance, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        Self {
            base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(),
            GetClassInstance: GetClassInstance::<Identity, Impl, OFFSET>,
            CreateClassInstance: CreateClassInstance::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11ClassLinkage as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11ClassLinkage, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11GeometryShader(::windows::core::IUnknown);
impl ID3D11GeometryShader {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
}
impl ::core::cmp::Eq for ID3D11GeometryShader {}
impl ::core::cmp::PartialEq for ID3D11GeometryShader {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11GeometryShader {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11GeometryShader {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11GeometryShader").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11GeometryShader {}
unsafe impl ::core::marker::Sync for ID3D11GeometryShader {}
unsafe impl ::windows::core::Vtable for ID3D11GeometryShader {
    type Vtable = ID3D11GeometryShader_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11GeometryShader {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x38325b96_effb_4022_ba02_2e795b70275c);
}

#[repr(C)]
pub struct ID3D11GeometryShader_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
}

pub trait ID3D11GeometryShader_Impl: Sized + ID3D11DeviceChild_Impl {}

impl ID3D11GeometryShader_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11GeometryShader_Impl, const OFFSET: isize>() -> ID3D11GeometryShader_Vtbl {
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>() }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11GeometryShader as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11GeometryShader, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11HullShader(::windows::core::IUnknown);
impl ID3D11HullShader {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
}
impl ::core::cmp::Eq for ID3D11HullShader {}
impl ::core::cmp::PartialEq for ID3D11HullShader {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11HullShader {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11HullShader {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11HullShader").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11HullShader {}
unsafe impl ::core::marker::Sync for ID3D11HullShader {}
unsafe impl ::windows::core::Vtable for ID3D11HullShader {
    type Vtable = ID3D11HullShader_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11HullShader {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x8e5c6061_628a_4c8e_8264_bbe45cb3d5dd);
}

#[repr(C)]
pub struct ID3D11HullShader_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
}

pub trait ID3D11HullShader_Impl: Sized + ID3D11DeviceChild_Impl {}

impl ID3D11HullShader_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11HullShader_Impl, const OFFSET: isize>() -> ID3D11HullShader_Vtbl {
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>() }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11HullShader as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11HullShader, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11DomainShader(::windows::core::IUnknown);
impl ID3D11DomainShader {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
}
impl ::core::cmp::Eq for ID3D11DomainShader {}
impl ::core::cmp::PartialEq for ID3D11DomainShader {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11DomainShader {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11DomainShader {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11DomainShader").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11DomainShader {}
unsafe impl ::core::marker::Sync for ID3D11DomainShader {}
unsafe impl ::windows::core::Vtable for ID3D11DomainShader {
    type Vtable = ID3D11DomainShader_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11DomainShader {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xf582c508_0f36_490c_9977_31eece268cfa);
}

#[repr(C)]
pub struct ID3D11DomainShader_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
}

pub trait ID3D11DomainShader_Impl: Sized + ID3D11DeviceChild_Impl {}

impl ID3D11DomainShader_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DomainShader_Impl, const OFFSET: isize>() -> ID3D11DomainShader_Vtbl {
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>() }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11DomainShader as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11DomainShader, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11ComputeShader(::windows::core::IUnknown);
impl ID3D11ComputeShader {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
}
impl ::core::cmp::Eq for ID3D11ComputeShader {}
impl ::core::cmp::PartialEq for ID3D11ComputeShader {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11ComputeShader {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11ComputeShader {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11ComputeShader").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11ComputeShader {}
unsafe impl ::core::marker::Sync for ID3D11ComputeShader {}
unsafe impl ::windows::core::Vtable for ID3D11ComputeShader {
    type Vtable = ID3D11ComputeShader_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11ComputeShader {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x4f5b196e_c2bd_495e_bd01_1fded38e4969);
}

#[repr(C)]
pub struct ID3D11ComputeShader_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
}

pub trait ID3D11ComputeShader_Impl: Sized + ID3D11DeviceChild_Impl {}

impl ID3D11ComputeShader_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ComputeShader_Impl, const OFFSET: isize>() -> ID3D11ComputeShader_Vtbl {
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>() }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11ComputeShader as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11ComputeShader, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11SamplerState(::windows::core::IUnknown);
impl ID3D11SamplerState {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_SAMPLER_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11SamplerState {}
impl ::core::cmp::PartialEq for ID3D11SamplerState {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11SamplerState {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11SamplerState {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11SamplerState").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11SamplerState {}
unsafe impl ::core::marker::Sync for ID3D11SamplerState {}
unsafe impl ::windows::core::Vtable for ID3D11SamplerState {
    type Vtable = ID3D11SamplerState_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11SamplerState {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xda6fea51_564c_4487_9810_f0d0f9b4e3a5);
}

#[repr(C)]
pub struct ID3D11SamplerState_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_SAMPLER_DESC),
}

pub trait ID3D11SamplerState_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_SAMPLER_DESC);
}

impl ID3D11SamplerState_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11SamplerState_Impl, const OFFSET: isize>() -> ID3D11SamplerState_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11SamplerState_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_SAMPLER_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11SamplerState as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11SamplerState, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11DeviceChild(::windows::core::IUnknown);
impl ID3D11DeviceChild {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
}
impl ::core::cmp::Eq for ID3D11DeviceChild {}
impl ::core::cmp::PartialEq for ID3D11DeviceChild {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11DeviceChild {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11DeviceChild {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11DeviceChild").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11DeviceChild {}
unsafe impl ::core::marker::Sync for ID3D11DeviceChild {}
unsafe impl ::windows::core::Vtable for ID3D11DeviceChild {
    type Vtable = ID3D11DeviceChild_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11DeviceChild {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x1841e5c8_16b0_489b_bcc8_44cfb0d5deae);
}

#[repr(C)]
pub struct ID3D11DeviceChild_Vtbl {
    pub base__: ::windows::core::IUnknown_Vtbl,
    pub GetDevice: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppdevice: *mut *mut ::core::ffi::c_void),
    pub GetPrivateData: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub SetPrivateData: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub SetPrivateDataInterface: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
}

pub trait ID3D11DeviceChild_Impl: Sized {
    fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>);
    fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()>;
    fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()>;
    fn SetPrivateDataInterface(&self, guid: *const ::windows::core::GUID, pdata: &::core::option::Option<::windows::core::IUnknown>) -> ::windows::core::Result<()>;
}

impl ID3D11DeviceChild_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceChild_Impl, const OFFSET: isize>() -> ID3D11DeviceChild_Vtbl {
        unsafe extern "system" fn GetDevice<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceChild_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppdevice: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDevice(::core::mem::transmute_copy(&ppdevice))
        }
        unsafe extern "system" fn GetPrivateData<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceChild_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetPrivateData(::core::mem::transmute_copy(&guid), ::core::mem::transmute_copy(&pdatasize), ::core::mem::transmute_copy(&pdata)).into()
        }
        unsafe extern "system" fn SetPrivateData<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceChild_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetPrivateData(::core::mem::transmute_copy(&guid), ::core::mem::transmute_copy(&datasize), ::core::mem::transmute_copy(&pdata)).into()
        }
        unsafe extern "system" fn SetPrivateDataInterface<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceChild_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetPrivateDataInterface(::core::mem::transmute_copy(&guid), ::core::mem::transmute(&pdata)).into()
        }
        Self {
            base__: ::windows::core::IUnknown_Vtbl::new::<Identity, OFFSET>(),
            GetDevice: GetDevice::<Identity, Impl, OFFSET>,
            GetPrivateData: GetPrivateData::<Identity, Impl, OFFSET>,
            SetPrivateData: SetPrivateData::<Identity, Impl, OFFSET>,
            SetPrivateDataInterface: SetPrivateDataInterface::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11DeviceChild, ::windows::core::IUnknown);

#[repr(transparent)]
pub struct ID3D11View(::windows::core::IUnknown);
impl ID3D11View {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetResource(&self, ppresource: *mut ::core::option::Option<ID3D11Resource>) {
        (::windows::core::Vtable::vtable(self).GetResource)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppresource))
    }
}
impl ::core::cmp::Eq for ID3D11View {}
impl ::core::cmp::PartialEq for ID3D11View {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11View {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11View {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11View").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11View {}
unsafe impl ::core::marker::Sync for ID3D11View {}
unsafe impl ::windows::core::Vtable for ID3D11View {
    type Vtable = ID3D11View_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11View {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x839d1216_bb2e_412b_b7f4_a9dbebe08ed1);
}

#[repr(C)]
pub struct ID3D11View_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    pub GetResource: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppresource: *mut *mut ::core::ffi::c_void),
}

pub trait ID3D11View_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetResource(&self, ppresource: *mut ::core::option::Option<ID3D11Resource>);
}

impl ID3D11View_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11View_Impl, const OFFSET: isize>() -> ID3D11View_Vtbl {
        unsafe extern "system" fn GetResource<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11View_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppresource: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetResource(::core::mem::transmute_copy(&ppresource))
        }
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(), GetResource: GetResource::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11View as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11View, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11Query(::windows::core::IUnknown);
impl ID3D11Query {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetDataSize(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).base__.GetDataSize)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_QUERY_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11Query {}
impl ::core::cmp::PartialEq for ID3D11Query {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Query {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Query {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Query").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Query {}
unsafe impl ::core::marker::Sync for ID3D11Query {}
unsafe impl ::windows::core::Vtable for ID3D11Query {
    type Vtable = ID3D11Query_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Query {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xd6c00747_87b7_425e_b84d_44d108560afd);
}

#[repr(C)]
pub struct ID3D11Query_Vtbl {
    pub base__: ID3D11Asynchronous_Vtbl,
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_QUERY_DESC),
}

pub trait ID3D11Query_Impl: Sized + ID3D11Asynchronous_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_QUERY_DESC);
}

impl ID3D11Query_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Query_Impl, const OFFSET: isize>() -> ID3D11Query_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Query_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_QUERY_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11Asynchronous_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Query as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11Asynchronous as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Query, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11Asynchronous);

#[repr(transparent)]
pub struct ID3D11Predicate(::windows::core::IUnknown);
impl ID3D11Predicate {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetDataSize(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDataSize)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_QUERY_DESC) {
        (::windows::core::Vtable::vtable(self).base__.GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11Predicate {}
impl ::core::cmp::PartialEq for ID3D11Predicate {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Predicate {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Predicate {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Predicate").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Predicate {}
unsafe impl ::core::marker::Sync for ID3D11Predicate {}
unsafe impl ::windows::core::Vtable for ID3D11Predicate {
    type Vtable = ID3D11Predicate_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Predicate {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x9eb576dd_9f77_4d86_81aa_8bab5fe490e2);
}

#[repr(C)]
pub struct ID3D11Predicate_Vtbl {
    pub base__: ID3D11Query_Vtbl,
}

pub trait ID3D11Predicate_Impl: Sized + ID3D11Query_Impl {}

impl ID3D11Predicate_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Predicate_Impl, const OFFSET: isize>() -> ID3D11Predicate_Vtbl {
        Self { base__: ID3D11Query_Vtbl::new::<Identity, Impl, OFFSET>() }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Predicate as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11Asynchronous as ::windows::core::Interface>::IID || iid == &<ID3D11Query as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Predicate, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11Asynchronous, ID3D11Query);

#[repr(transparent)]
pub struct ID3D11Counter(::windows::core::IUnknown);
impl ID3D11Counter {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetDataSize(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).base__.GetDataSize)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_COUNTER_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11Counter {}
impl ::core::cmp::PartialEq for ID3D11Counter {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Counter {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Counter {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Counter").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Counter {}
unsafe impl ::core::marker::Sync for ID3D11Counter {}
unsafe impl ::windows::core::Vtable for ID3D11Counter {
    type Vtable = ID3D11Counter_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Counter {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x6e8c49fb_a371_4770_b440_29086022b741);
}

#[repr(C)]
pub struct ID3D11Counter_Vtbl {
    pub base__: ID3D11Asynchronous_Vtbl,
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_COUNTER_DESC),
}

pub trait ID3D11Counter_Impl: Sized + ID3D11Asynchronous_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_COUNTER_DESC);
}

impl ID3D11Counter_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Counter_Impl, const OFFSET: isize>() -> ID3D11Counter_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Counter_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_COUNTER_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11Asynchronous_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Counter as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11Asynchronous as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Counter, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11Asynchronous);

#[repr(transparent)]
pub struct ID3D11CommandList(::windows::core::IUnknown);
impl ID3D11CommandList {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetContextFlags(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).GetContextFlags)(::windows::core::Vtable::as_raw(self))
    }
}
impl ::core::cmp::Eq for ID3D11CommandList {}
impl ::core::cmp::PartialEq for ID3D11CommandList {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11CommandList {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11CommandList {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11CommandList").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11CommandList {}
unsafe impl ::core::marker::Sync for ID3D11CommandList {}
unsafe impl ::windows::core::Vtable for ID3D11CommandList {
    type Vtable = ID3D11CommandList_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11CommandList {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xa24bc4d1_769e_43f7_8013_98ff566c18e2);
}

#[repr(C)]
pub struct ID3D11CommandList_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    pub GetContextFlags: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> u32,
}

pub trait ID3D11CommandList_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetContextFlags(&self) -> u32;
}

impl ID3D11CommandList_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11CommandList_Impl, const OFFSET: isize>() -> ID3D11CommandList_Vtbl {
        unsafe extern "system" fn GetContextFlags<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11CommandList_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> u32 {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetContextFlags()
        }
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(), GetContextFlags: GetContextFlags::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11CommandList as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11CommandList, ::windows::core::IUnknown, ID3D11DeviceChild);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_DEVICE_CONTEXT_TYPE(pub i32);
impl ::core::marker::Copy for D3D11_DEVICE_CONTEXT_TYPE {}
impl ::core::clone::Clone for D3D11_DEVICE_CONTEXT_TYPE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_DEVICE_CONTEXT_TYPE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_DEVICE_CONTEXT_TYPE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_DEVICE_CONTEXT_TYPE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_DEVICE_CONTEXT_TYPE").field(&self.0).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_FEATURE(pub i32);
impl ::core::marker::Copy for D3D11_FEATURE {}
impl ::core::clone::Clone for D3D11_FEATURE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_FEATURE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_FEATURE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_FEATURE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_FEATURE").field(&self.0).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_COUNTER(pub i32);
impl ::core::marker::Copy for D3D11_COUNTER {}
impl ::core::clone::Clone for D3D11_COUNTER {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_COUNTER {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_COUNTER {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_COUNTER {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_COUNTER").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct D3D11_COUNTER_INFO {
    pub LastDeviceDependentCounter: D3D11_COUNTER,
    pub NumSimultaneousCounters: u32,
    pub NumDetectableParallelUnits: u8,
}
impl ::core::marker::Copy for D3D11_COUNTER_INFO {}
impl ::core::cmp::Eq for D3D11_COUNTER_INFO {}
impl ::core::cmp::PartialEq for D3D11_COUNTER_INFO {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_COUNTER_INFO>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_COUNTER_INFO {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_COUNTER_INFO {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_COUNTER_INFO {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_COUNTER_INFO {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_COUNTER_INFO").field("LastDeviceDependentCounter", &self.LastDeviceDependentCounter).field("NumSimultaneousCounters", &self.NumSimultaneousCounters).field("NumDetectableParallelUnits", &self.NumDetectableParallelUnits).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_COUNTER_TYPE(pub i32);
impl ::core::marker::Copy for D3D11_COUNTER_TYPE {}
impl ::core::clone::Clone for D3D11_COUNTER_TYPE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_COUNTER_TYPE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_COUNTER_TYPE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_COUNTER_TYPE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_COUNTER_TYPE").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct D3D11_COUNTER_DESC {
    pub Counter: D3D11_COUNTER,
    pub MiscFlags: u32,
}
impl ::core::marker::Copy for D3D11_COUNTER_DESC {}
impl ::core::cmp::Eq for D3D11_COUNTER_DESC {}
impl ::core::cmp::PartialEq for D3D11_COUNTER_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_COUNTER_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_COUNTER_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_COUNTER_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_COUNTER_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_COUNTER_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_COUNTER_DESC").field("Counter", &self.Counter).field("MiscFlags", &self.MiscFlags).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_RTV_DIMENSION(pub i32);
impl ::core::marker::Copy for D3D11_RTV_DIMENSION {}
impl ::core::clone::Clone for D3D11_RTV_DIMENSION {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_RTV_DIMENSION {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_RTV_DIMENSION {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_RTV_DIMENSION {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_RTV_DIMENSION").field(&self.0).finish()
    }
}

#[repr(C)]#[derive(Clone, Copy)]
pub union D3D11_BUFFER_RTV_0 {
    pub FirstElement: u32,
    pub ElementOffset: u32,
}

#[repr(C)]#[derive(Clone, Copy)]
pub union D3D11_BUFFER_RTV_1 {
    pub NumElements: u32,
    pub ElementWidth: u32,
}

#[repr(C)]
pub struct D3D11_BUFFER_RTV {
    pub Anonymous1: D3D11_BUFFER_RTV_0,
    pub Anonymous2: D3D11_BUFFER_RTV_1,
}
impl ::core::marker::Copy for D3D11_BUFFER_RTV {}
impl ::core::cmp::Eq for D3D11_BUFFER_RTV {}
impl ::core::cmp::PartialEq for D3D11_BUFFER_RTV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_BUFFER_RTV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_BUFFER_RTV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BUFFER_RTV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_BUFFER_RTV {
    type Abi = Self;
}

#[repr(C)]
pub struct D3D11_TEX1D_RTV {
    pub MipSlice: u32,
}
impl ::core::marker::Copy for D3D11_TEX1D_RTV {}
impl ::core::cmp::Eq for D3D11_TEX1D_RTV {}
impl ::core::cmp::PartialEq for D3D11_TEX1D_RTV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX1D_RTV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX1D_RTV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX1D_RTV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX1D_RTV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX1D_RTV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX1D_RTV").field("MipSlice", &self.MipSlice).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX1D_ARRAY_RTV {
    pub MipSlice: u32,
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX1D_ARRAY_RTV {}
impl ::core::cmp::Eq for D3D11_TEX1D_ARRAY_RTV {}
impl ::core::cmp::PartialEq for D3D11_TEX1D_ARRAY_RTV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX1D_ARRAY_RTV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX1D_ARRAY_RTV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX1D_ARRAY_RTV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX1D_ARRAY_RTV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX1D_ARRAY_RTV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX1D_ARRAY_RTV").field("MipSlice", &self.MipSlice).field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2D_RTV {
    pub MipSlice: u32,
}
impl ::core::marker::Copy for D3D11_TEX2D_RTV {}
impl ::core::cmp::Eq for D3D11_TEX2D_RTV {}
impl ::core::cmp::PartialEq for D3D11_TEX2D_RTV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2D_RTV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2D_RTV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2D_RTV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2D_RTV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2D_RTV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2D_RTV").field("MipSlice", &self.MipSlice).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2D_ARRAY_RTV {
    pub MipSlice: u32,
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX2D_ARRAY_RTV {}
impl ::core::cmp::Eq for D3D11_TEX2D_ARRAY_RTV {}
impl ::core::cmp::PartialEq for D3D11_TEX2D_ARRAY_RTV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2D_ARRAY_RTV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2D_ARRAY_RTV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2D_ARRAY_RTV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2D_ARRAY_RTV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2D_ARRAY_RTV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2D_ARRAY_RTV").field("MipSlice", &self.MipSlice).field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2DMS_RTV {
    pub UnusedField_NothingToDefine: u32,
}
impl ::core::marker::Copy for D3D11_TEX2DMS_RTV {}
impl ::core::cmp::Eq for D3D11_TEX2DMS_RTV {}
impl ::core::cmp::PartialEq for D3D11_TEX2DMS_RTV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2DMS_RTV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2DMS_RTV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2DMS_RTV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2DMS_RTV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2DMS_RTV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2DMS_RTV").field("UnusedField_NothingToDefine", &self.UnusedField_NothingToDefine).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2DMS_ARRAY_RTV {
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX2DMS_ARRAY_RTV {}
impl ::core::cmp::Eq for D3D11_TEX2DMS_ARRAY_RTV {}
impl ::core::cmp::PartialEq for D3D11_TEX2DMS_ARRAY_RTV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2DMS_ARRAY_RTV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2DMS_ARRAY_RTV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2DMS_ARRAY_RTV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2DMS_ARRAY_RTV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2DMS_ARRAY_RTV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2DMS_ARRAY_RTV").field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX3D_RTV {
    pub MipSlice: u32,
    pub FirstWSlice: u32,
    pub WSize: u32,
}
impl ::core::marker::Copy for D3D11_TEX3D_RTV {}
impl ::core::cmp::Eq for D3D11_TEX3D_RTV {}
impl ::core::cmp::PartialEq for D3D11_TEX3D_RTV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX3D_RTV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX3D_RTV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX3D_RTV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX3D_RTV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX3D_RTV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX3D_RTV").field("MipSlice", &self.MipSlice).field("FirstWSlice", &self.FirstWSlice).field("WSize", &self.WSize).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_MAP(pub i32);
impl ::core::marker::Copy for D3D11_MAP {}
impl ::core::clone::Clone for D3D11_MAP {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_MAP {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_MAP {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_MAP {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_MAP").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct D3D11_MAPPED_SUBRESOURCE {
    pub pData: *mut ::core::ffi::c_void,
    pub RowPitch: u32,
    pub DepthPitch: u32,
}
impl ::core::marker::Copy for D3D11_MAPPED_SUBRESOURCE {}
impl ::core::cmp::Eq for D3D11_MAPPED_SUBRESOURCE {}
impl ::core::cmp::PartialEq for D3D11_MAPPED_SUBRESOURCE {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_MAPPED_SUBRESOURCE>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_MAPPED_SUBRESOURCE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_MAPPED_SUBRESOURCE {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_MAPPED_SUBRESOURCE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_MAPPED_SUBRESOURCE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_MAPPED_SUBRESOURCE").field("pData", &self.pData).field("RowPitch", &self.RowPitch).field("DepthPitch", &self.DepthPitch).finish()
    }
}

#[repr(C)]
pub struct D3D11_BOX {
    pub left: u32,
    pub top: u32,
    pub front: u32,
    pub right: u32,
    pub bottom: u32,
    pub back: u32,
}
impl ::core::marker::Copy for D3D11_BOX {}
impl ::core::cmp::Eq for D3D11_BOX {}
impl ::core::cmp::PartialEq for D3D11_BOX {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_BOX>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_BOX {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BOX {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_BOX {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_BOX {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_BOX").field("left", &self.left).field("top", &self.top).field("front", &self.front).field("right", &self.right).field("bottom", &self.bottom).field("back", &self.back).finish()
    }
}

#[repr(C)]#[derive(Clone, Copy)]
pub union D3D11_RENDER_TARGET_VIEW_DESC_0 {
    pub Buffer: D3D11_BUFFER_RTV,
    pub Texture1D: D3D11_TEX1D_RTV,
    pub Texture1DArray: D3D11_TEX1D_ARRAY_RTV,
    pub Texture2D: D3D11_TEX2D_RTV,
    pub Texture2DArray: D3D11_TEX2D_ARRAY_RTV,
    pub Texture2DMS: D3D11_TEX2DMS_RTV,
    pub Texture2DMSArray: D3D11_TEX2DMS_ARRAY_RTV,
    pub Texture3D: D3D11_TEX3D_RTV,
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_QUERY(pub i32);
impl ::core::marker::Copy for D3D11_QUERY {}
impl ::core::clone::Clone for D3D11_QUERY {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_QUERY {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_QUERY {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_QUERY {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_QUERY").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct D3D11_CLASS_INSTANCE_DESC {
    pub InstanceId: u32,
    pub InstanceIndex: u32,
    pub TypeId: u32,
    pub ConstantBuffer: u32,
    pub BaseConstantBufferOffset: u32,
    pub BaseTexture: u32,
    pub BaseSampler: u32,
    pub Created: super::super::Foundation::BOOL,
}
impl ::core::marker::Copy for D3D11_CLASS_INSTANCE_DESC {}
impl ::core::cmp::Eq for D3D11_CLASS_INSTANCE_DESC {}
impl ::core::cmp::PartialEq for D3D11_CLASS_INSTANCE_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_CLASS_INSTANCE_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_CLASS_INSTANCE_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_CLASS_INSTANCE_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_CLASS_INSTANCE_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_CLASS_INSTANCE_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_CLASS_INSTANCE_DESC").field("InstanceId", &self.InstanceId).field("InstanceIndex", &self.InstanceIndex).field("TypeId", &self.TypeId).field("ConstantBuffer", &self.ConstantBuffer).field("BaseConstantBufferOffset", &self.BaseConstantBufferOffset).field("BaseTexture", &self.BaseTexture).field("BaseSampler", &self.BaseSampler).field("Created", &self.Created).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_UAV_DIMENSION(pub i32);
impl ::core::marker::Copy for D3D11_UAV_DIMENSION {}
impl ::core::clone::Clone for D3D11_UAV_DIMENSION {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_UAV_DIMENSION {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_UAV_DIMENSION {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_UAV_DIMENSION {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_UAV_DIMENSION").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct D3D11_BUFFER_UAV {
    pub FirstElement: u32,
    pub NumElements: u32,
    pub Flags: u32,
}
impl ::core::marker::Copy for D3D11_BUFFER_UAV {}
impl ::core::cmp::Eq for D3D11_BUFFER_UAV {}
impl ::core::cmp::PartialEq for D3D11_BUFFER_UAV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_BUFFER_UAV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_BUFFER_UAV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BUFFER_UAV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_BUFFER_UAV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_BUFFER_UAV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_BUFFER_UAV").field("FirstElement", &self.FirstElement).field("NumElements", &self.NumElements).field("Flags", &self.Flags).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX1D_UAV {
    pub MipSlice: u32,
}
impl ::core::marker::Copy for D3D11_TEX1D_UAV {}
impl ::core::cmp::Eq for D3D11_TEX1D_UAV {}
impl ::core::cmp::PartialEq for D3D11_TEX1D_UAV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX1D_UAV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX1D_UAV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX1D_UAV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX1D_UAV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX1D_UAV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX1D_UAV").field("MipSlice", &self.MipSlice).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX1D_ARRAY_UAV {
    pub MipSlice: u32,
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX1D_ARRAY_UAV {}
impl ::core::cmp::Eq for D3D11_TEX1D_ARRAY_UAV {}
impl ::core::cmp::PartialEq for D3D11_TEX1D_ARRAY_UAV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX1D_ARRAY_UAV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX1D_ARRAY_UAV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX1D_ARRAY_UAV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX1D_ARRAY_UAV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX1D_ARRAY_UAV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX1D_ARRAY_UAV").field("MipSlice", &self.MipSlice).field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2D_UAV {
    pub MipSlice: u32,
}
impl ::core::marker::Copy for D3D11_TEX2D_UAV {}
impl ::core::cmp::Eq for D3D11_TEX2D_UAV {}
impl ::core::cmp::PartialEq for D3D11_TEX2D_UAV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2D_UAV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2D_UAV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2D_UAV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2D_UAV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2D_UAV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2D_UAV").field("MipSlice", &self.MipSlice).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2D_ARRAY_UAV {
    pub MipSlice: u32,
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX2D_ARRAY_UAV {}
impl ::core::cmp::Eq for D3D11_TEX2D_ARRAY_UAV {}
impl ::core::cmp::PartialEq for D3D11_TEX2D_ARRAY_UAV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2D_ARRAY_UAV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2D_ARRAY_UAV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2D_ARRAY_UAV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2D_ARRAY_UAV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2D_ARRAY_UAV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2D_ARRAY_UAV").field("MipSlice", &self.MipSlice).field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX3D_UAV {
    pub MipSlice: u32,
    pub FirstWSlice: u32,
    pub WSize: u32,
}
impl ::core::marker::Copy for D3D11_TEX3D_UAV {}
impl ::core::cmp::Eq for D3D11_TEX3D_UAV {}
impl ::core::cmp::PartialEq for D3D11_TEX3D_UAV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX3D_UAV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX3D_UAV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX3D_UAV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX3D_UAV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX3D_UAV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX3D_UAV").field("MipSlice", &self.MipSlice).field("FirstWSlice", &self.FirstWSlice).field("WSize", &self.WSize).finish()
    }
}

#[repr(C)]
pub struct D3D11_BUFFER_SRV {
    pub Anonymous1: D3D11_BUFFER_SRV_0,
    pub Anonymous2: D3D11_BUFFER_SRV_1,
}
impl ::core::marker::Copy for D3D11_BUFFER_SRV {}
impl ::core::cmp::Eq for D3D11_BUFFER_SRV {}
impl ::core::cmp::PartialEq for D3D11_BUFFER_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_BUFFER_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_BUFFER_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BUFFER_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_BUFFER_SRV {
    type Abi = Self;
}

#[repr(C)]
pub struct D3D11_TEX1D_SRV {
    pub MostDetailedMip: u32,
    pub MipLevels: u32,
}
impl ::core::marker::Copy for D3D11_TEX1D_SRV {}
impl ::core::cmp::Eq for D3D11_TEX1D_SRV {}
impl ::core::cmp::PartialEq for D3D11_TEX1D_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX1D_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX1D_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX1D_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX1D_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX1D_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX1D_SRV").field("MostDetailedMip", &self.MostDetailedMip).field("MipLevels", &self.MipLevels).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX1D_ARRAY_SRV {
    pub MostDetailedMip: u32,
    pub MipLevels: u32,
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX1D_ARRAY_SRV {}
impl ::core::cmp::Eq for D3D11_TEX1D_ARRAY_SRV {}
impl ::core::cmp::PartialEq for D3D11_TEX1D_ARRAY_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX1D_ARRAY_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX1D_ARRAY_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX1D_ARRAY_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX1D_ARRAY_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX1D_ARRAY_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX1D_ARRAY_SRV").field("MostDetailedMip", &self.MostDetailedMip).field("MipLevels", &self.MipLevels).field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2D_SRV {
    pub MostDetailedMip: u32,
    pub MipLevels: u32,
}
impl ::core::marker::Copy for D3D11_TEX2D_SRV {}
impl ::core::cmp::Eq for D3D11_TEX2D_SRV {}
impl ::core::cmp::PartialEq for D3D11_TEX2D_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2D_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2D_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2D_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2D_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2D_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2D_SRV").field("MostDetailedMip", &self.MostDetailedMip).field("MipLevels", &self.MipLevels).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2D_ARRAY_SRV {
    pub MostDetailedMip: u32,
    pub MipLevels: u32,
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX2D_ARRAY_SRV {}
impl ::core::cmp::Eq for D3D11_TEX2D_ARRAY_SRV {}
impl ::core::cmp::PartialEq for D3D11_TEX2D_ARRAY_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2D_ARRAY_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2D_ARRAY_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2D_ARRAY_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2D_ARRAY_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2D_ARRAY_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2D_ARRAY_SRV").field("MostDetailedMip", &self.MostDetailedMip).field("MipLevels", &self.MipLevels).field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2DMS_SRV {
    pub UnusedField_NothingToDefine: u32,
}
impl ::core::marker::Copy for D3D11_TEX2DMS_SRV {}
impl ::core::cmp::Eq for D3D11_TEX2DMS_SRV {}
impl ::core::cmp::PartialEq for D3D11_TEX2DMS_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2DMS_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2DMS_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2DMS_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2DMS_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2DMS_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2DMS_SRV").field("UnusedField_NothingToDefine", &self.UnusedField_NothingToDefine).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2DMS_ARRAY_SRV {
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX2DMS_ARRAY_SRV {}
impl ::core::cmp::Eq for D3D11_TEX2DMS_ARRAY_SRV {}
impl ::core::cmp::PartialEq for D3D11_TEX2DMS_ARRAY_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2DMS_ARRAY_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2DMS_ARRAY_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2DMS_ARRAY_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2DMS_ARRAY_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2DMS_ARRAY_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2DMS_ARRAY_SRV").field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX3D_SRV {
    pub MostDetailedMip: u32,
    pub MipLevels: u32,
}
impl ::core::marker::Copy for D3D11_TEX3D_SRV {}
impl ::core::cmp::Eq for D3D11_TEX3D_SRV {}
impl ::core::cmp::PartialEq for D3D11_TEX3D_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX3D_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX3D_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX3D_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX3D_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX3D_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX3D_SRV").field("MostDetailedMip", &self.MostDetailedMip).field("MipLevels", &self.MipLevels).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEXCUBE_SRV {
    pub MostDetailedMip: u32,
    pub MipLevels: u32,
}
impl ::core::marker::Copy for D3D11_TEXCUBE_SRV {}
impl ::core::cmp::Eq for D3D11_TEXCUBE_SRV {}
impl ::core::cmp::PartialEq for D3D11_TEXCUBE_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEXCUBE_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEXCUBE_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEXCUBE_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEXCUBE_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEXCUBE_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEXCUBE_SRV").field("MostDetailedMip", &self.MostDetailedMip).field("MipLevels", &self.MipLevels).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEXCUBE_ARRAY_SRV {
    pub MostDetailedMip: u32,
    pub MipLevels: u32,
    pub First2DArrayFace: u32,
    pub NumCubes: u32,
}
impl ::core::marker::Copy for D3D11_TEXCUBE_ARRAY_SRV {}
impl ::core::cmp::Eq for D3D11_TEXCUBE_ARRAY_SRV {}
impl ::core::cmp::PartialEq for D3D11_TEXCUBE_ARRAY_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEXCUBE_ARRAY_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEXCUBE_ARRAY_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEXCUBE_ARRAY_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEXCUBE_ARRAY_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEXCUBE_ARRAY_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEXCUBE_ARRAY_SRV").field("MostDetailedMip", &self.MostDetailedMip).field("MipLevels", &self.MipLevels).field("First2DArrayFace", &self.First2DArrayFace).field("NumCubes", &self.NumCubes).finish()
    }
}

#[repr(C)]
pub struct D3D11_BUFFEREX_SRV {
    pub FirstElement: u32,
    pub NumElements: u32,
    pub Flags: u32,
}
impl ::core::marker::Copy for D3D11_BUFFEREX_SRV {}
impl ::core::cmp::Eq for D3D11_BUFFEREX_SRV {}
impl ::core::cmp::PartialEq for D3D11_BUFFEREX_SRV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_BUFFEREX_SRV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_BUFFEREX_SRV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BUFFEREX_SRV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_BUFFEREX_SRV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_BUFFEREX_SRV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_BUFFEREX_SRV").field("FirstElement", &self.FirstElement).field("NumElements", &self.NumElements).field("Flags", &self.Flags).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX1D_DSV {
    pub MipSlice: u32,
}
impl ::core::marker::Copy for D3D11_TEX1D_DSV {}
impl ::core::cmp::Eq for D3D11_TEX1D_DSV {}
impl ::core::cmp::PartialEq for D3D11_TEX1D_DSV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX1D_DSV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX1D_DSV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX1D_DSV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX1D_DSV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX1D_DSV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX1D_DSV").field("MipSlice", &self.MipSlice).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2D_DSV {
    pub MipSlice: u32,
}
impl ::core::marker::Copy for D3D11_TEX2D_DSV {}
impl ::core::cmp::Eq for D3D11_TEX2D_DSV {}
impl ::core::cmp::PartialEq for D3D11_TEX2D_DSV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2D_DSV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2D_DSV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2D_DSV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2D_DSV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2D_DSV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2D_DSV").field("MipSlice", &self.MipSlice).finish()
    }
}

#[repr(C)]#[derive(Clone, Copy)]
pub union D3D11_BUFFER_SRV_0 {
    pub FirstElement: u32,
    pub ElementOffset: u32,
}

#[repr(C)]#[derive(Clone, Copy)]
pub union D3D11_BUFFER_SRV_1 {
    pub NumElements: u32,
    pub ElementWidth: u32,
}

#[repr(C)]
pub struct D3D11_TEX1D_ARRAY_DSV {
    pub MipSlice: u32,
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX1D_ARRAY_DSV {}
impl ::core::cmp::Eq for D3D11_TEX1D_ARRAY_DSV {}
impl ::core::cmp::PartialEq for D3D11_TEX1D_ARRAY_DSV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX1D_ARRAY_DSV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX1D_ARRAY_DSV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX1D_ARRAY_DSV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX1D_ARRAY_DSV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX1D_ARRAY_DSV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX1D_ARRAY_DSV").field("MipSlice", &self.MipSlice).field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2D_ARRAY_DSV {
    pub MipSlice: u32,
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX2D_ARRAY_DSV {}
impl ::core::cmp::Eq for D3D11_TEX2D_ARRAY_DSV {}
impl ::core::cmp::PartialEq for D3D11_TEX2D_ARRAY_DSV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2D_ARRAY_DSV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2D_ARRAY_DSV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2D_ARRAY_DSV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2D_ARRAY_DSV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2D_ARRAY_DSV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2D_ARRAY_DSV").field("MipSlice", &self.MipSlice).field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2DMS_DSV {
    pub UnusedField_NothingToDefine: u32,
}
impl ::core::marker::Copy for D3D11_TEX2DMS_DSV {}
impl ::core::cmp::Eq for D3D11_TEX2DMS_DSV {}
impl ::core::cmp::PartialEq for D3D11_TEX2DMS_DSV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2DMS_DSV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2DMS_DSV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2DMS_DSV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2DMS_DSV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2DMS_DSV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2DMS_DSV").field("UnusedField_NothingToDefine", &self.UnusedField_NothingToDefine).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEX2DMS_ARRAY_DSV {
    pub FirstArraySlice: u32,
    pub ArraySize: u32,
}
impl ::core::marker::Copy for D3D11_TEX2DMS_ARRAY_DSV {}
impl ::core::cmp::Eq for D3D11_TEX2DMS_ARRAY_DSV {}
impl ::core::cmp::PartialEq for D3D11_TEX2DMS_ARRAY_DSV {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEX2DMS_ARRAY_DSV>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEX2DMS_ARRAY_DSV {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEX2DMS_ARRAY_DSV {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEX2DMS_ARRAY_DSV {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEX2DMS_ARRAY_DSV {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEX2DMS_ARRAY_DSV").field("FirstArraySlice", &self.FirstArraySlice).field("ArraySize", &self.ArraySize).finish()
    }
}

#[repr(C)]
pub struct D3D11_RENDER_TARGET_VIEW_DESC {
    pub Format: super::Dxgi::Common::DXGI_FORMAT,
    pub ViewDimension: D3D11_RTV_DIMENSION,
    pub Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0,
}
impl ::core::marker::Copy for D3D11_RENDER_TARGET_VIEW_DESC {}
impl ::core::cmp::Eq for D3D11_RENDER_TARGET_VIEW_DESC {}
impl ::core::cmp::PartialEq for D3D11_RENDER_TARGET_VIEW_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_RENDER_TARGET_VIEW_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_RENDER_TARGET_VIEW_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_RENDER_TARGET_VIEW_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_RENDER_TARGET_VIEW_DESC {
    type Abi = Self;
}

#[repr(C)]
pub struct D3D11_QUERY_DESC {
    pub Query: D3D11_QUERY,
    pub MiscFlags: u32,
}
impl ::core::marker::Copy for D3D11_QUERY_DESC {}
impl ::core::cmp::Eq for D3D11_QUERY_DESC {}
impl ::core::cmp::PartialEq for D3D11_QUERY_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_QUERY_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_QUERY_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_QUERY_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_QUERY_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_QUERY_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_QUERY_DESC").field("Query", &self.Query).field("MiscFlags", &self.MiscFlags).finish()
    }
}

#[repr(C)]
pub struct D3D11_UNORDERED_ACCESS_VIEW_DESC {
    pub Format: super::Dxgi::Common::DXGI_FORMAT,
    pub ViewDimension: D3D11_UAV_DIMENSION,
    pub Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0,
}
impl ::core::marker::Copy for D3D11_UNORDERED_ACCESS_VIEW_DESC {}
impl ::core::cmp::Eq for D3D11_UNORDERED_ACCESS_VIEW_DESC {}
impl ::core::cmp::PartialEq for D3D11_UNORDERED_ACCESS_VIEW_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_UNORDERED_ACCESS_VIEW_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_UNORDERED_ACCESS_VIEW_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_UNORDERED_ACCESS_VIEW_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_UNORDERED_ACCESS_VIEW_DESC {
    type Abi = Self;
}

#[repr(C)]#[derive(Clone, Copy)]
pub union D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
    pub Buffer: D3D11_BUFFER_UAV,
    pub Texture1D: D3D11_TEX1D_UAV,
    pub Texture1DArray: D3D11_TEX1D_ARRAY_UAV,
    pub Texture2D: D3D11_TEX2D_UAV,
    pub Texture2DArray: D3D11_TEX2D_ARRAY_UAV,
    pub Texture3D: D3D11_TEX3D_UAV,
}

#[repr(C)]#[derive(Clone, Copy)]
pub union D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
    pub Buffer: D3D11_BUFFER_SRV,
    pub Texture1D: D3D11_TEX1D_SRV,
    pub Texture1DArray: D3D11_TEX1D_ARRAY_SRV,
    pub Texture2D: D3D11_TEX2D_SRV,
    pub Texture2DArray: D3D11_TEX2D_ARRAY_SRV,
    pub Texture2DMS: D3D11_TEX2DMS_SRV,
    pub Texture2DMSArray: D3D11_TEX2DMS_ARRAY_SRV,
    pub Texture3D: D3D11_TEX3D_SRV,
    pub TextureCube: D3D11_TEXCUBE_SRV,
    pub TextureCubeArray: D3D11_TEXCUBE_ARRAY_SRV,
    pub BufferEx: D3D11_BUFFEREX_SRV,
}

#[repr(C)]#[derive(Clone, Copy)]
pub union D3D11_DEPTH_STENCIL_VIEW_DESC_0 {
    pub Texture1D: D3D11_TEX1D_DSV,
    pub Texture1DArray: D3D11_TEX1D_ARRAY_DSV,
    pub Texture2D: D3D11_TEX2D_DSV,
    pub Texture2DArray: D3D11_TEX2D_ARRAY_DSV,
    pub Texture2DMS: D3D11_TEX2DMS_DSV,
    pub Texture2DMSArray: D3D11_TEX2DMS_ARRAY_DSV,
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_FILTER(pub i32);
impl ::core::marker::Copy for D3D11_FILTER {}
impl ::core::clone::Clone for D3D11_FILTER {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_FILTER {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_FILTER {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_FILTER {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_FILTER").field(&self.0).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_TEXTURE_ADDRESS_MODE(pub i32);
impl ::core::marker::Copy for D3D11_TEXTURE_ADDRESS_MODE {}
impl ::core::clone::Clone for D3D11_TEXTURE_ADDRESS_MODE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEXTURE_ADDRESS_MODE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEXTURE_ADDRESS_MODE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEXTURE_ADDRESS_MODE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_TEXTURE_ADDRESS_MODE").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEXTURE1D_DESC {
    pub Width: u32,
    pub MipLevels: u32,
    pub ArraySize: u32,
    pub Format: super::Dxgi::Common::DXGI_FORMAT,
    pub Usage: D3D11_USAGE,
    pub BindFlags: D3D11_BIND_FLAG,
    pub CPUAccessFlags: D3D11_CPU_ACCESS_FLAG,
    pub MiscFlags: D3D11_RESOURCE_MISC_FLAG,
}
impl ::core::marker::Copy for D3D11_TEXTURE1D_DESC {}
impl ::core::cmp::Eq for D3D11_TEXTURE1D_DESC {}
impl ::core::cmp::PartialEq for D3D11_TEXTURE1D_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEXTURE1D_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEXTURE1D_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEXTURE1D_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEXTURE1D_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEXTURE1D_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEXTURE1D_DESC").field("Width", &self.Width).field("MipLevels", &self.MipLevels).field("ArraySize", &self.ArraySize).field("Format", &self.Format).field("Usage", &self.Usage).field("BindFlags", &self.BindFlags).field("CPUAccessFlags", &self.CPUAccessFlags).field("MiscFlags", &self.MiscFlags).finish()
    }
}

#[repr(C)]
pub struct D3D11_TEXTURE3D_DESC {
    pub Width: u32,
    pub Height: u32,
    pub Depth: u32,
    pub MipLevels: u32,
    pub Format: super::Dxgi::Common::DXGI_FORMAT,
    pub Usage: D3D11_USAGE,
    pub BindFlags: D3D11_BIND_FLAG,
    pub CPUAccessFlags: D3D11_CPU_ACCESS_FLAG,
    pub MiscFlags: D3D11_RESOURCE_MISC_FLAG,
}
impl ::core::marker::Copy for D3D11_TEXTURE3D_DESC {}
impl ::core::cmp::Eq for D3D11_TEXTURE3D_DESC {}
impl ::core::cmp::PartialEq for D3D11_TEXTURE3D_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEXTURE3D_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEXTURE3D_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEXTURE3D_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEXTURE3D_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEXTURE3D_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEXTURE3D_DESC").field("Width", &self.Width).field("Height", &self.Height).field("Depth", &self.Depth).field("MipLevels", &self.MipLevels).field("Format", &self.Format).field("Usage", &self.Usage).field("BindFlags", &self.BindFlags).field("CPUAccessFlags", &self.CPUAccessFlags).field("MiscFlags", &self.MiscFlags).finish()
    }
}

#[repr(C)]
pub struct D3D11_SHADER_RESOURCE_VIEW_DESC {
    pub Format: super::Dxgi::Common::DXGI_FORMAT,
    pub ViewDimension: super::Direct3D::D3D_SRV_DIMENSION,
    pub Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0,
}
impl ::core::marker::Copy for D3D11_SHADER_RESOURCE_VIEW_DESC {}
impl ::core::cmp::Eq for D3D11_SHADER_RESOURCE_VIEW_DESC {}
impl ::core::cmp::PartialEq for D3D11_SHADER_RESOURCE_VIEW_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_SHADER_RESOURCE_VIEW_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_SHADER_RESOURCE_VIEW_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_SHADER_RESOURCE_VIEW_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_SHADER_RESOURCE_VIEW_DESC {
    type Abi = Self;
}

#[repr(C)]
pub struct D3D11_SO_DECLARATION_ENTRY {
    pub Stream: u32,
    pub SemanticName: ::windows::core::PCSTR,
    pub SemanticIndex: u32,
    pub StartComponent: u8,
    pub ComponentCount: u8,
    pub OutputSlot: u8,
}
impl ::core::marker::Copy for D3D11_SO_DECLARATION_ENTRY {}
impl ::core::cmp::Eq for D3D11_SO_DECLARATION_ENTRY {}
impl ::core::cmp::PartialEq for D3D11_SO_DECLARATION_ENTRY {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_SO_DECLARATION_ENTRY>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_SO_DECLARATION_ENTRY {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_SO_DECLARATION_ENTRY {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_SO_DECLARATION_ENTRY {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_SO_DECLARATION_ENTRY {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_SO_DECLARATION_ENTRY").field("Stream", &self.Stream).field("SemanticName", &self.SemanticName).field("SemanticIndex", &self.SemanticIndex).field("StartComponent", &self.StartComponent).field("ComponentCount", &self.ComponentCount).field("OutputSlot", &self.OutputSlot).finish()
    }
}

#[repr(C)]
pub struct D3D11_SAMPLER_DESC {
    pub Filter: D3D11_FILTER,
    pub AddressU: D3D11_TEXTURE_ADDRESS_MODE,
    pub AddressV: D3D11_TEXTURE_ADDRESS_MODE,
    pub AddressW: D3D11_TEXTURE_ADDRESS_MODE,
    pub MipLODBias: f32,
    pub MaxAnisotropy: u32,
    pub ComparisonFunc: D3D11_COMPARISON_FUNC,
    pub BorderColor: [f32; 4],
    pub MinLOD: f32,
    pub MaxLOD: f32,
}
impl ::core::marker::Copy for D3D11_SAMPLER_DESC {}
impl ::core::cmp::Eq for D3D11_SAMPLER_DESC {}
impl ::core::cmp::PartialEq for D3D11_SAMPLER_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_SAMPLER_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_SAMPLER_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_SAMPLER_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_SAMPLER_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_SAMPLER_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_SAMPLER_DESC").field("Filter", &self.Filter).field("AddressU", &self.AddressU).field("AddressV", &self.AddressV).field("AddressW", &self.AddressW).field("MipLODBias", &self.MipLODBias).field("MaxAnisotropy", &self.MaxAnisotropy).field("ComparisonFunc", &self.ComparisonFunc).field("BorderColor", &self.BorderColor).field("MinLOD", &self.MinLOD).field("MaxLOD", &self.MaxLOD).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_RESOURCE_DIMENSION(pub i32);
impl ::core::marker::Copy for D3D11_RESOURCE_DIMENSION {}
impl ::core::clone::Clone for D3D11_RESOURCE_DIMENSION {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_RESOURCE_DIMENSION {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_RESOURCE_DIMENSION {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_RESOURCE_DIMENSION {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_RESOURCE_DIMENSION").field(&self.0).finish()
    }
}

pub const D3D11_BIND_INDEX_BUFFER: D3D11_BIND_FLAG = D3D11_BIND_FLAG(2u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_BIND_FLAG(pub u32);
impl ::core::marker::Copy for D3D11_BIND_FLAG {}
impl ::core::clone::Clone for D3D11_BIND_FLAG {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BIND_FLAG {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_BIND_FLAG {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_BIND_FLAG {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_BIND_FLAG").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for D3D11_BIND_FLAG {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for D3D11_BIND_FLAG {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for D3D11_BIND_FLAG {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for D3D11_BIND_FLAG {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for D3D11_BIND_FLAG {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub const D3D11_BIND_VERTEX_BUFFER: D3D11_BIND_FLAG = D3D11_BIND_FLAG(1u32);

#[repr(C)]
pub struct D3D11_VIEWPORT {
    pub TopLeftX: f32,
    pub TopLeftY: f32,
    pub Width: f32,
    pub Height: f32,
    pub MinDepth: f32,
    pub MaxDepth: f32,
}
impl ::core::marker::Copy for D3D11_VIEWPORT {}
impl ::core::cmp::Eq for D3D11_VIEWPORT {}
impl ::core::cmp::PartialEq for D3D11_VIEWPORT {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_VIEWPORT>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_VIEWPORT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_VIEWPORT {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_VIEWPORT {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_VIEWPORT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_VIEWPORT").field("TopLeftX", &self.TopLeftX).field("TopLeftY", &self.TopLeftY).field("Width", &self.Width).field("Height", &self.Height).field("MinDepth", &self.MinDepth).field("MaxDepth", &self.MaxDepth).finish()
    }
}

#[repr(C)]
pub struct D3D11_BUFFER_DESC {
    pub ByteWidth: u32,
    pub Usage: D3D11_USAGE,
    pub BindFlags: D3D11_BIND_FLAG,
    pub CPUAccessFlags: D3D11_CPU_ACCESS_FLAG,
    pub MiscFlags: D3D11_RESOURCE_MISC_FLAG,
    pub StructureByteStride: u32,
}
impl ::core::marker::Copy for D3D11_BUFFER_DESC {}
impl ::core::cmp::Eq for D3D11_BUFFER_DESC {}
impl ::core::cmp::PartialEq for D3D11_BUFFER_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_BUFFER_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_BUFFER_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BUFFER_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_BUFFER_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_BUFFER_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_BUFFER_DESC").field("ByteWidth", &self.ByteWidth).field("Usage", &self.Usage).field("BindFlags", &self.BindFlags).field("CPUAccessFlags", &self.CPUAccessFlags).field("MiscFlags", &self.MiscFlags).field("StructureByteStride", &self.StructureByteStride).finish()
    }
}

pub const D3D11_USAGE_DEFAULT: D3D11_USAGE = D3D11_USAGE(0i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_USAGE(pub i32);
impl ::core::marker::Copy for D3D11_USAGE {}
impl ::core::clone::Clone for D3D11_USAGE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_USAGE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_USAGE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_USAGE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_USAGE").field(&self.0).finish()
    }
}

pub const D3D11_BIND_CONSTANT_BUFFER: D3D11_BIND_FLAG = D3D11_BIND_FLAG(4u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_CPU_ACCESS_FLAG(pub u32);
impl ::core::marker::Copy for D3D11_CPU_ACCESS_FLAG {}
impl ::core::clone::Clone for D3D11_CPU_ACCESS_FLAG {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_CPU_ACCESS_FLAG {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_CPU_ACCESS_FLAG {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_CPU_ACCESS_FLAG {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_CPU_ACCESS_FLAG").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for D3D11_CPU_ACCESS_FLAG {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for D3D11_CPU_ACCESS_FLAG {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for D3D11_CPU_ACCESS_FLAG {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for D3D11_CPU_ACCESS_FLAG {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for D3D11_CPU_ACCESS_FLAG {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_RESOURCE_MISC_FLAG(pub u32);
impl ::core::marker::Copy for D3D11_RESOURCE_MISC_FLAG {}
impl ::core::clone::Clone for D3D11_RESOURCE_MISC_FLAG {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_RESOURCE_MISC_FLAG {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_RESOURCE_MISC_FLAG {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_RESOURCE_MISC_FLAG {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_RESOURCE_MISC_FLAG").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for D3D11_RESOURCE_MISC_FLAG {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for D3D11_RESOURCE_MISC_FLAG {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for D3D11_RESOURCE_MISC_FLAG {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for D3D11_RESOURCE_MISC_FLAG {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for D3D11_RESOURCE_MISC_FLAG {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

#[repr(C)]
pub struct D3D11_SUBRESOURCE_DATA {
    pub pSysMem: *const ::core::ffi::c_void,
    pub SysMemPitch: u32,
    pub SysMemSlicePitch: u32,
}
impl ::core::marker::Copy for D3D11_SUBRESOURCE_DATA {}
impl ::core::cmp::Eq for D3D11_SUBRESOURCE_DATA {}
impl ::core::cmp::PartialEq for D3D11_SUBRESOURCE_DATA {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_SUBRESOURCE_DATA>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_SUBRESOURCE_DATA {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_SUBRESOURCE_DATA {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_SUBRESOURCE_DATA {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_SUBRESOURCE_DATA {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_SUBRESOURCE_DATA").field("pSysMem", &self.pSysMem).field("SysMemPitch", &self.SysMemPitch).field("SysMemSlicePitch", &self.SysMemSlicePitch).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_CREATE_DEVICE_FLAG(pub u32);
impl ::core::marker::Copy for D3D11_CREATE_DEVICE_FLAG {}
impl ::core::clone::Clone for D3D11_CREATE_DEVICE_FLAG {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_CREATE_DEVICE_FLAG {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_CREATE_DEVICE_FLAG {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_CREATE_DEVICE_FLAG {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_CREATE_DEVICE_FLAG").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for D3D11_CREATE_DEVICE_FLAG {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for D3D11_CREATE_DEVICE_FLAG {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for D3D11_CREATE_DEVICE_FLAG {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for D3D11_CREATE_DEVICE_FLAG {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for D3D11_CREATE_DEVICE_FLAG {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

pub const D3D11_SDK_VERSION: u32 = 7u32;

pub const D3D11_BIND_SHADER_RESOURCE: D3D11_BIND_FLAG = D3D11_BIND_FLAG(8u32);

#[repr(C)]
pub struct D3D11_TEXTURE2D_DESC {
    pub Width: u32,
    pub Height: u32,
    pub MipLevels: u32,
    pub ArraySize: u32,
    pub Format: super::Dxgi::Common::DXGI_FORMAT,
    pub SampleDesc: super::Dxgi::Common::DXGI_SAMPLE_DESC,
    pub Usage: D3D11_USAGE,
    pub BindFlags: D3D11_BIND_FLAG,
    pub CPUAccessFlags: D3D11_CPU_ACCESS_FLAG,
    pub MiscFlags: D3D11_RESOURCE_MISC_FLAG,
}
impl ::core::marker::Copy for D3D11_TEXTURE2D_DESC {}
impl ::core::cmp::Eq for D3D11_TEXTURE2D_DESC {}
impl ::core::cmp::PartialEq for D3D11_TEXTURE2D_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_TEXTURE2D_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_TEXTURE2D_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_TEXTURE2D_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_TEXTURE2D_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_TEXTURE2D_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_TEXTURE2D_DESC").field("Width", &self.Width).field("Height", &self.Height).field("MipLevels", &self.MipLevels).field("ArraySize", &self.ArraySize).field("Format", &self.Format).field("SampleDesc", &self.SampleDesc).field("Usage", &self.Usage).field("BindFlags", &self.BindFlags).field("CPUAccessFlags", &self.CPUAccessFlags).field("MiscFlags", &self.MiscFlags).finish()
    }
}

pub const D3D11_BIND_RENDER_TARGET: D3D11_BIND_FLAG = D3D11_BIND_FLAG(32u32);

pub const D3D11_BIND_DEPTH_STENCIL: D3D11_BIND_FLAG = D3D11_BIND_FLAG(64u32);

#[repr(C)]
pub struct D3D11_DEPTH_STENCIL_DESC {
    pub DepthEnable: super::super::Foundation::BOOL,
    pub DepthWriteMask: D3D11_DEPTH_WRITE_MASK,
    pub DepthFunc: D3D11_COMPARISON_FUNC,
    pub StencilEnable: super::super::Foundation::BOOL,
    pub StencilReadMask: u8,
    pub StencilWriteMask: u8,
    pub FrontFace: D3D11_DEPTH_STENCILOP_DESC,
    pub BackFace: D3D11_DEPTH_STENCILOP_DESC,
}
impl ::core::marker::Copy for D3D11_DEPTH_STENCIL_DESC {}
impl ::core::cmp::Eq for D3D11_DEPTH_STENCIL_DESC {}
impl ::core::cmp::PartialEq for D3D11_DEPTH_STENCIL_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_DEPTH_STENCIL_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_DEPTH_STENCIL_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_DEPTH_STENCIL_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_DEPTH_STENCIL_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_DEPTH_STENCIL_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_DEPTH_STENCIL_DESC").field("DepthEnable", &self.DepthEnable).field("DepthWriteMask", &self.DepthWriteMask).field("DepthFunc", &self.DepthFunc).field("StencilEnable", &self.StencilEnable).field("StencilReadMask", &self.StencilReadMask).field("StencilWriteMask", &self.StencilWriteMask).field("FrontFace", &self.FrontFace).field("BackFace", &self.BackFace).finish()
    }
}

pub const D3D11_DEPTH_WRITE_MASK_ALL: D3D11_DEPTH_WRITE_MASK = D3D11_DEPTH_WRITE_MASK(1i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_DEPTH_WRITE_MASK(pub i32);
impl ::core::marker::Copy for D3D11_DEPTH_WRITE_MASK {}
impl ::core::clone::Clone for D3D11_DEPTH_WRITE_MASK {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_DEPTH_WRITE_MASK {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_DEPTH_WRITE_MASK {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_DEPTH_WRITE_MASK {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_DEPTH_WRITE_MASK").field(&self.0).finish()
    }
}

pub const D3D11_COMPARISON_LESS_EQUAL: D3D11_COMPARISON_FUNC = D3D11_COMPARISON_FUNC(4i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_COMPARISON_FUNC(pub i32);
impl ::core::marker::Copy for D3D11_COMPARISON_FUNC {}
impl ::core::clone::Clone for D3D11_COMPARISON_FUNC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_COMPARISON_FUNC {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_COMPARISON_FUNC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_COMPARISON_FUNC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_COMPARISON_FUNC").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct D3D11_DEPTH_STENCILOP_DESC {
    pub StencilFailOp: D3D11_STENCIL_OP,
    pub StencilDepthFailOp: D3D11_STENCIL_OP,
    pub StencilPassOp: D3D11_STENCIL_OP,
    pub StencilFunc: D3D11_COMPARISON_FUNC,
}
impl ::core::marker::Copy for D3D11_DEPTH_STENCILOP_DESC {}
impl ::core::cmp::Eq for D3D11_DEPTH_STENCILOP_DESC {}
impl ::core::cmp::PartialEq for D3D11_DEPTH_STENCILOP_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_DEPTH_STENCILOP_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_DEPTH_STENCILOP_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_DEPTH_STENCILOP_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_DEPTH_STENCILOP_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_DEPTH_STENCILOP_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_DEPTH_STENCILOP_DESC").field("StencilFailOp", &self.StencilFailOp).field("StencilDepthFailOp", &self.StencilDepthFailOp).field("StencilPassOp", &self.StencilPassOp).field("StencilFunc", &self.StencilFunc).finish()
    }
}

pub const D3D11_STENCIL_OP_REPLACE: D3D11_STENCIL_OP = D3D11_STENCIL_OP(3i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_STENCIL_OP(pub i32);
impl ::core::marker::Copy for D3D11_STENCIL_OP {}
impl ::core::clone::Clone for D3D11_STENCIL_OP {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_STENCIL_OP {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_STENCIL_OP {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_STENCIL_OP {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_STENCIL_OP").field(&self.0).finish()
    }
}

pub const D3D11_COMPARISON_ALWAYS: D3D11_COMPARISON_FUNC = D3D11_COMPARISON_FUNC(8i32);

#[repr(C)]
pub struct D3D11_DEPTH_STENCIL_VIEW_DESC {
    pub Format: super::Dxgi::Common::DXGI_FORMAT,
    pub ViewDimension: D3D11_DSV_DIMENSION,
    pub Flags: u32,
    pub Anonymous: D3D11_DEPTH_STENCIL_VIEW_DESC_0,
}
impl ::core::marker::Copy for D3D11_DEPTH_STENCIL_VIEW_DESC {}
impl ::core::cmp::Eq for D3D11_DEPTH_STENCIL_VIEW_DESC {}
impl ::core::cmp::PartialEq for D3D11_DEPTH_STENCIL_VIEW_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_DEPTH_STENCIL_VIEW_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_DEPTH_STENCIL_VIEW_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_DEPTH_STENCIL_VIEW_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_DEPTH_STENCIL_VIEW_DESC {
    type Abi = Self;
}

pub const D3D11_DSV_DIMENSION_TEXTURE2D: D3D11_DSV_DIMENSION = D3D11_DSV_DIMENSION(3i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_DSV_DIMENSION(pub i32);
impl ::core::marker::Copy for D3D11_DSV_DIMENSION {}
impl ::core::clone::Clone for D3D11_DSV_DIMENSION {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_DSV_DIMENSION {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_DSV_DIMENSION {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_DSV_DIMENSION {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_DSV_DIMENSION").field(&self.0).finish()
    }
}

pub const D3D11_CLEAR_DEPTH: D3D11_CLEAR_FLAG = D3D11_CLEAR_FLAG(1i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_CLEAR_FLAG(pub i32);
impl ::core::marker::Copy for D3D11_CLEAR_FLAG {}
impl ::core::clone::Clone for D3D11_CLEAR_FLAG {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_CLEAR_FLAG {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_CLEAR_FLAG {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_CLEAR_FLAG {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_CLEAR_FLAG").field(&self.0).finish()
    }
}

pub const D3D11_CLEAR_STENCIL: D3D11_CLEAR_FLAG = D3D11_CLEAR_FLAG(2i32);

#[repr(C)]
pub struct D3D11_BLEND_DESC {
    pub AlphaToCoverageEnable: super::super::Foundation::BOOL,
    pub IndependentBlendEnable: super::super::Foundation::BOOL,
    pub RenderTarget: [D3D11_RENDER_TARGET_BLEND_DESC; 8],
}
impl ::core::marker::Copy for D3D11_BLEND_DESC {}
impl ::core::cmp::Eq for D3D11_BLEND_DESC {}
impl ::core::cmp::PartialEq for D3D11_BLEND_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_BLEND_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_BLEND_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BLEND_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_BLEND_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_BLEND_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_BLEND_DESC").field("AlphaToCoverageEnable", &self.AlphaToCoverageEnable).field("IndependentBlendEnable", &self.IndependentBlendEnable).field("RenderTarget", &self.RenderTarget).finish()
    }
}

#[repr(C)]
pub struct D3D11_RENDER_TARGET_BLEND_DESC {
    pub BlendEnable: super::super::Foundation::BOOL,
    pub SrcBlend: D3D11_BLEND,
    pub DestBlend: D3D11_BLEND,
    pub BlendOp: D3D11_BLEND_OP,
    pub SrcBlendAlpha: D3D11_BLEND,
    pub DestBlendAlpha: D3D11_BLEND,
    pub BlendOpAlpha: D3D11_BLEND_OP,
    pub RenderTargetWriteMask: u8,
}
impl ::core::marker::Copy for D3D11_RENDER_TARGET_BLEND_DESC {}
impl ::core::cmp::Eq for D3D11_RENDER_TARGET_BLEND_DESC {}
impl ::core::cmp::PartialEq for D3D11_RENDER_TARGET_BLEND_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_RENDER_TARGET_BLEND_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_RENDER_TARGET_BLEND_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_RENDER_TARGET_BLEND_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_RENDER_TARGET_BLEND_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_RENDER_TARGET_BLEND_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_RENDER_TARGET_BLEND_DESC").field("BlendEnable", &self.BlendEnable).field("SrcBlend", &self.SrcBlend).field("DestBlend", &self.DestBlend).field("BlendOp", &self.BlendOp).field("SrcBlendAlpha", &self.SrcBlendAlpha).field("DestBlendAlpha", &self.DestBlendAlpha).field("BlendOpAlpha", &self.BlendOpAlpha).field("RenderTargetWriteMask", &self.RenderTargetWriteMask).finish()
    }
}

pub const D3D11_BLEND_ONE: D3D11_BLEND = D3D11_BLEND(2i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_BLEND(pub i32);
impl ::core::marker::Copy for D3D11_BLEND {}
impl ::core::clone::Clone for D3D11_BLEND {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BLEND {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_BLEND {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_BLEND {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_BLEND").field(&self.0).finish()
    }
}

pub const D3D11_BLEND_INV_SRC_ALPHA: D3D11_BLEND = D3D11_BLEND(6i32);

pub const D3D11_BLEND_OP_ADD: D3D11_BLEND_OP = D3D11_BLEND_OP(1i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_BLEND_OP(pub i32);
impl ::core::marker::Copy for D3D11_BLEND_OP {}
impl ::core::clone::Clone for D3D11_BLEND_OP {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_BLEND_OP {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_BLEND_OP {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_BLEND_OP {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_BLEND_OP").field(&self.0).finish()
    }
}

pub const D3D11_COLOR_WRITE_ENABLE_ALL: D3D11_COLOR_WRITE_ENABLE = D3D11_COLOR_WRITE_ENABLE(15i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_COLOR_WRITE_ENABLE(pub i32);
impl ::core::marker::Copy for D3D11_COLOR_WRITE_ENABLE {}
impl ::core::clone::Clone for D3D11_COLOR_WRITE_ENABLE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_COLOR_WRITE_ENABLE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_COLOR_WRITE_ENABLE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_COLOR_WRITE_ENABLE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_COLOR_WRITE_ENABLE").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct D3D11_RASTERIZER_DESC {
    pub FillMode: D3D11_FILL_MODE,
    pub CullMode: D3D11_CULL_MODE,
    pub FrontCounterClockwise: super::super::Foundation::BOOL,
    pub DepthBias: i32,
    pub DepthBiasClamp: f32,
    pub SlopeScaledDepthBias: f32,
    pub DepthClipEnable: super::super::Foundation::BOOL,
    pub ScissorEnable: super::super::Foundation::BOOL,
    pub MultisampleEnable: super::super::Foundation::BOOL,
    pub AntialiasedLineEnable: super::super::Foundation::BOOL,
}
impl ::core::marker::Copy for D3D11_RASTERIZER_DESC {}
impl ::core::cmp::Eq for D3D11_RASTERIZER_DESC {}
impl ::core::cmp::PartialEq for D3D11_RASTERIZER_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_RASTERIZER_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_RASTERIZER_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_RASTERIZER_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_RASTERIZER_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_RASTERIZER_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_RASTERIZER_DESC")
            .field("FillMode", &self.FillMode)
            .field("CullMode", &self.CullMode)
            .field("FrontCounterClockwise", &self.FrontCounterClockwise)
            .field("DepthBias", &self.DepthBias)
            .field("DepthBiasClamp", &self.DepthBiasClamp)
            .field("SlopeScaledDepthBias", &self.SlopeScaledDepthBias)
            .field("DepthClipEnable", &self.DepthClipEnable)
            .field("ScissorEnable", &self.ScissorEnable)
            .field("MultisampleEnable", &self.MultisampleEnable)
            .field("AntialiasedLineEnable", &self.AntialiasedLineEnable)
            .finish()
    }
}

pub const D3D11_CULL_NONE: D3D11_CULL_MODE = D3D11_CULL_MODE(1i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_CULL_MODE(pub i32);
impl ::core::marker::Copy for D3D11_CULL_MODE {}
impl ::core::clone::Clone for D3D11_CULL_MODE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_CULL_MODE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_CULL_MODE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_CULL_MODE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_CULL_MODE").field(&self.0).finish()
    }
}

pub const D3D11_FILL_SOLID: D3D11_FILL_MODE = D3D11_FILL_MODE(3i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_FILL_MODE(pub i32);
impl ::core::marker::Copy for D3D11_FILL_MODE {}
impl ::core::clone::Clone for D3D11_FILL_MODE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_FILL_MODE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_FILL_MODE {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_FILL_MODE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_FILL_MODE").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct D3D11_INPUT_ELEMENT_DESC {
    pub SemanticName: ::windows::core::PCSTR,
    pub SemanticIndex: u32,
    pub Format: super::Dxgi::Common::DXGI_FORMAT,
    pub InputSlot: u32,
    pub AlignedByteOffset: u32,
    pub InputSlotClass: D3D11_INPUT_CLASSIFICATION,
    pub InstanceDataStepRate: u32,
}
impl ::core::marker::Copy for D3D11_INPUT_ELEMENT_DESC {}
impl ::core::cmp::Eq for D3D11_INPUT_ELEMENT_DESC {}
impl ::core::cmp::PartialEq for D3D11_INPUT_ELEMENT_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<D3D11_INPUT_ELEMENT_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for D3D11_INPUT_ELEMENT_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_INPUT_ELEMENT_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for D3D11_INPUT_ELEMENT_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_INPUT_ELEMENT_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("D3D11_INPUT_ELEMENT_DESC").field("SemanticName", &self.SemanticName).field("SemanticIndex", &self.SemanticIndex).field("Format", &self.Format).field("InputSlot", &self.InputSlot).field("AlignedByteOffset", &self.AlignedByteOffset).field("InputSlotClass", &self.InputSlotClass).field("InstanceDataStepRate", &self.InstanceDataStepRate).finish()
    }
}

pub const D3D11_INPUT_PER_VERTEX_DATA: D3D11_INPUT_CLASSIFICATION = D3D11_INPUT_CLASSIFICATION(0i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct D3D11_INPUT_CLASSIFICATION(pub i32);
impl ::core::marker::Copy for D3D11_INPUT_CLASSIFICATION {}
impl ::core::clone::Clone for D3D11_INPUT_CLASSIFICATION {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for D3D11_INPUT_CLASSIFICATION {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for D3D11_INPUT_CLASSIFICATION {
    type Abi = Self;
}
impl ::core::fmt::Debug for D3D11_INPUT_CLASSIFICATION {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("D3D11_INPUT_CLASSIFICATION").field(&self.0).finish()
    }
}

pub const D3D11_INPUT_PER_INSTANCE_DATA: D3D11_INPUT_CLASSIFICATION = D3D11_INPUT_CLASSIFICATION(1i32);

#[repr(transparent)]
pub struct ID3D11Device(::windows::core::IUnknown);
impl ID3D11Device {
    pub unsafe fn CreateBuffer(&self, pdesc: *const D3D11_BUFFER_DESC, pinitialdata: ::core::option::Option<*const D3D11_SUBRESOURCE_DATA>) -> ::windows::core::Result<ID3D11Buffer> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateBuffer)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc), ::core::mem::transmute(pinitialdata.unwrap_or(::std::ptr::null())), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11Buffer>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn CreateTexture1D(&self, pdesc: *const D3D11_TEXTURE1D_DESC, pinitialdata: ::core::option::Option<*const D3D11_SUBRESOURCE_DATA>) -> ::windows::core::Result<ID3D11Texture1D> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateTexture1D)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc), ::core::mem::transmute(pinitialdata.unwrap_or(::std::ptr::null())), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11Texture1D>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn CreateTexture2D(&self, pdesc: *const D3D11_TEXTURE2D_DESC, pinitialdata: ::core::option::Option<*const D3D11_SUBRESOURCE_DATA>) -> ::windows::core::Result<ID3D11Texture2D> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateTexture2D)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc), ::core::mem::transmute(pinitialdata.unwrap_or(::std::ptr::null())), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11Texture2D>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn CreateTexture3D(&self, pdesc: *const D3D11_TEXTURE3D_DESC, pinitialdata: ::core::option::Option<*const D3D11_SUBRESOURCE_DATA>) -> ::windows::core::Result<ID3D11Texture3D> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateTexture3D)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc), ::core::mem::transmute(pinitialdata.unwrap_or(::std::ptr::null())), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11Texture3D>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Direct3D\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Graphics_Direct3D", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn CreateShaderResourceView<'a, P0>(&self, presource: P0, pdesc: ::core::option::Option<*const D3D11_SHADER_RESOURCE_VIEW_DESC>) -> ::windows::core::Result<ID3D11ShaderResourceView>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateShaderResourceView)(::windows::core::Vtable::as_raw(self), presource.into().abi(), ::core::mem::transmute(pdesc.unwrap_or(::std::ptr::null())), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11ShaderResourceView>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn CreateUnorderedAccessView<'a, P0>(&self, presource: P0, pdesc: ::core::option::Option<*const D3D11_UNORDERED_ACCESS_VIEW_DESC>) -> ::windows::core::Result<ID3D11UnorderedAccessView>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateUnorderedAccessView)(::windows::core::Vtable::as_raw(self), presource.into().abi(), ::core::mem::transmute(pdesc.unwrap_or(::std::ptr::null())), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11UnorderedAccessView>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn CreateRenderTargetView<'a, P0>(&self, presource: P0, pdesc: ::core::option::Option<*const D3D11_RENDER_TARGET_VIEW_DESC>) -> ::windows::core::Result<ID3D11RenderTargetView>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateRenderTargetView)(::windows::core::Vtable::as_raw(self), presource.into().abi(), ::core::mem::transmute(pdesc.unwrap_or(::std::ptr::null())), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11RenderTargetView>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn CreateDepthStencilView<'a, P0>(&self, presource: P0, pdesc: ::core::option::Option<*const D3D11_DEPTH_STENCIL_VIEW_DESC>) -> ::windows::core::Result<ID3D11DepthStencilView>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateDepthStencilView)(::windows::core::Vtable::as_raw(self), presource.into().abi(), ::core::mem::transmute(pdesc.unwrap_or(::std::ptr::null())), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11DepthStencilView>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn CreateInputLayout(&self, pinputelementdescs: &[D3D11_INPUT_ELEMENT_DESC], pshaderbytecodewithinputsignature: &[u8]) -> ::windows::core::Result<ID3D11InputLayout> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateInputLayout)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pinputelementdescs.as_ptr()), pinputelementdescs.len() as _, ::core::mem::transmute(pshaderbytecodewithinputsignature.as_ptr()), pshaderbytecodewithinputsignature.len() as _, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11InputLayout>(result__)
    }
    pub unsafe fn CreateVertexShader<'a, P0>(&self, pshaderbytecode: &[u8], pclasslinkage: P0) -> ::windows::core::Result<ID3D11VertexShader>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11ClassLinkage>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateVertexShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pshaderbytecode.as_ptr()), pshaderbytecode.len() as _, pclasslinkage.into().abi(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11VertexShader>(result__)
    }
    pub unsafe fn CreateGeometryShader<'a, P0>(&self, pshaderbytecode: &[u8], pclasslinkage: P0) -> ::windows::core::Result<ID3D11GeometryShader>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11ClassLinkage>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateGeometryShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pshaderbytecode.as_ptr()), pshaderbytecode.len() as _, pclasslinkage.into().abi(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11GeometryShader>(result__)
    }
    pub unsafe fn CreateGeometryShaderWithStreamOutput<'a, P0>(&self, pshaderbytecode: &[u8], psodeclaration: ::core::option::Option<&[D3D11_SO_DECLARATION_ENTRY]>, pbufferstrides: ::core::option::Option<&[u32]>, rasterizedstream: u32, pclasslinkage: P0) -> ::windows::core::Result<ID3D11GeometryShader>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11ClassLinkage>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateGeometryShaderWithStreamOutput)(
            ::windows::core::Vtable::as_raw(self),
            ::core::mem::transmute(pshaderbytecode.as_ptr()),
            pshaderbytecode.len() as _,
            ::core::mem::transmute(psodeclaration.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())),
            psodeclaration.as_deref().map_or(0, |slice| slice.len() as _),
            ::core::mem::transmute(pbufferstrides.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())),
            pbufferstrides.as_deref().map_or(0, |slice| slice.len() as _),
            rasterizedstream,
            pclasslinkage.into().abi(),
            ::core::mem::transmute(result__.as_mut_ptr()),
        )
        .from_abi::<ID3D11GeometryShader>(result__)
    }
    pub unsafe fn CreatePixelShader<'a, P0>(&self, pshaderbytecode: &[u8], pclasslinkage: P0) -> ::windows::core::Result<ID3D11PixelShader>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11ClassLinkage>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreatePixelShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pshaderbytecode.as_ptr()), pshaderbytecode.len() as _, pclasslinkage.into().abi(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11PixelShader>(result__)
    }
    pub unsafe fn CreateHullShader<'a, P0>(&self, pshaderbytecode: &[u8], pclasslinkage: P0) -> ::windows::core::Result<ID3D11HullShader>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11ClassLinkage>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateHullShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pshaderbytecode.as_ptr()), pshaderbytecode.len() as _, pclasslinkage.into().abi(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11HullShader>(result__)
    }
    pub unsafe fn CreateDomainShader<'a, P0>(&self, pshaderbytecode: &[u8], pclasslinkage: P0) -> ::windows::core::Result<ID3D11DomainShader>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11ClassLinkage>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateDomainShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pshaderbytecode.as_ptr()), pshaderbytecode.len() as _, pclasslinkage.into().abi(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11DomainShader>(result__)
    }
    pub unsafe fn CreateComputeShader<'a, P0>(&self, pshaderbytecode: &[u8], pclasslinkage: P0) -> ::windows::core::Result<ID3D11ComputeShader>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11ClassLinkage>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateComputeShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pshaderbytecode.as_ptr()), pshaderbytecode.len() as _, pclasslinkage.into().abi(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11ComputeShader>(result__)
    }
    pub unsafe fn CreateClassLinkage(&self) -> ::windows::core::Result<ID3D11ClassLinkage> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateClassLinkage)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11ClassLinkage>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn CreateBlendState(&self, pblendstatedesc: *const D3D11_BLEND_DESC) -> ::windows::core::Result<ID3D11BlendState> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateBlendState)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pblendstatedesc), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11BlendState>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn CreateDepthStencilState(&self, pdepthstencildesc: *const D3D11_DEPTH_STENCIL_DESC) -> ::windows::core::Result<ID3D11DepthStencilState> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateDepthStencilState)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdepthstencildesc), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11DepthStencilState>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn CreateRasterizerState(&self, prasterizerdesc: *const D3D11_RASTERIZER_DESC) -> ::windows::core::Result<ID3D11RasterizerState> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateRasterizerState)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(prasterizerdesc), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11RasterizerState>(result__)
    }
    pub unsafe fn CreateSamplerState(&self, psamplerdesc: *const D3D11_SAMPLER_DESC) -> ::windows::core::Result<ID3D11SamplerState> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateSamplerState)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(psamplerdesc), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11SamplerState>(result__)
    }
    pub unsafe fn CreateQuery(&self, pquerydesc: *const D3D11_QUERY_DESC) -> ::windows::core::Result<ID3D11Query> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateQuery)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pquerydesc), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11Query>(result__)
    }
    pub unsafe fn CreatePredicate(&self, ppredicatedesc: *const D3D11_QUERY_DESC) -> ::windows::core::Result<ID3D11Predicate> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreatePredicate)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppredicatedesc), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11Predicate>(result__)
    }
    pub unsafe fn CreateCounter(&self, pcounterdesc: *const D3D11_COUNTER_DESC) -> ::windows::core::Result<ID3D11Counter> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateCounter)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pcounterdesc), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11Counter>(result__)
    }
    pub unsafe fn CreateDeferredContext(&self, contextflags: u32) -> ::windows::core::Result<ID3D11DeviceContext> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateDeferredContext)(::windows::core::Vtable::as_raw(self), contextflags, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11DeviceContext>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn OpenSharedResource<'a, P0, T>(&self, hresource: P0, result__: *mut ::core::option::Option<T>) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<super::super::Foundation::HANDLE>,
        T: ::windows::core::Interface,
    {
        (::windows::core::Vtable::vtable(self).OpenSharedResource)(::windows::core::Vtable::as_raw(self), hresource.into(), &<T as ::windows::core::Interface>::IID, result__ as *mut _ as *mut _).ok()
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn CheckFormatSupport(&self, format: super::Dxgi::Common::DXGI_FORMAT) -> ::windows::core::Result<u32> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CheckFormatSupport)(::windows::core::Vtable::as_raw(self), format, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<u32>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn CheckMultisampleQualityLevels(&self, format: super::Dxgi::Common::DXGI_FORMAT, samplecount: u32) -> ::windows::core::Result<u32> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CheckMultisampleQualityLevels)(::windows::core::Vtable::as_raw(self), format, samplecount, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<u32>(result__)
    }
    pub unsafe fn CheckCounterInfo(&self, pcounterinfo: *mut D3D11_COUNTER_INFO) {
        (::windows::core::Vtable::vtable(self).CheckCounterInfo)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pcounterinfo))
    }
    pub unsafe fn CheckCounter(&self, pdesc: *const D3D11_COUNTER_DESC, ptype: *mut D3D11_COUNTER_TYPE, pactivecounters: *mut u32, szname: ::windows::core::PSTR, pnamelength: ::core::option::Option<*mut u32>, szunits: ::windows::core::PSTR, punitslength: ::core::option::Option<*mut u32>, szdescription: ::windows::core::PSTR, pdescriptionlength: ::core::option::Option<*mut u32>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).CheckCounter)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc), ::core::mem::transmute(ptype), ::core::mem::transmute(pactivecounters), ::core::mem::transmute(szname), ::core::mem::transmute(pnamelength.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(szunits), ::core::mem::transmute(punitslength.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(szdescription), ::core::mem::transmute(pdescriptionlength.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn CheckFeatureSupport(&self, feature: D3D11_FEATURE, pfeaturesupportdata: *mut ::core::ffi::c_void, featuresupportdatasize: u32) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).CheckFeatureSupport)(::windows::core::Vtable::as_raw(self), feature, ::core::mem::transmute(pfeaturesupportdata), featuresupportdatasize).ok()
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    #[doc = "*Required features: `\"Win32_Graphics_Direct3D\"`*"]
    #[cfg(feature = "Win32_Graphics_Direct3D")]
    pub unsafe fn GetFeatureLevel(&self) -> super::Direct3D::D3D_FEATURE_LEVEL {
        (::windows::core::Vtable::vtable(self).GetFeatureLevel)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetCreationFlags(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).GetCreationFlags)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetDeviceRemovedReason(&self) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).GetDeviceRemovedReason)(::windows::core::Vtable::as_raw(self)).ok()
    }
    pub unsafe fn GetImmediateContext(&self, ppimmediatecontext: *mut ::core::option::Option<ID3D11DeviceContext>) {
        (::windows::core::Vtable::vtable(self).GetImmediateContext)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppimmediatecontext))
    }
    pub unsafe fn SetExceptionMode(&self, raiseflags: u32) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).SetExceptionMode)(::windows::core::Vtable::as_raw(self), raiseflags).ok()
    }
    pub unsafe fn GetExceptionMode(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).GetExceptionMode)(::windows::core::Vtable::as_raw(self))
    }
}
impl ::core::cmp::Eq for ID3D11Device {}
impl ::core::cmp::PartialEq for ID3D11Device {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Device {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Device {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Device").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Device {}
unsafe impl ::core::marker::Sync for ID3D11Device {}
unsafe impl ::windows::core::Vtable for ID3D11Device {
    type Vtable = ID3D11Device_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Device {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xdb6f6ddb_ac77_4e88_8253_819df9bbf140);
}

#[repr(C)]
pub struct ID3D11Device_Vtbl {
    pub base__: ::windows::core::IUnknown_Vtbl,
    pub CreateBuffer: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_BUFFER_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA, ppbuffer: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub CreateTexture1D: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_TEXTURE1D_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA, pptexture1d: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    CreateTexture1D: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub CreateTexture2D: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_TEXTURE2D_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA, pptexture2d: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    CreateTexture2D: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub CreateTexture3D: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_TEXTURE3D_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA, pptexture3d: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    CreateTexture3D: usize,
    #[cfg(all(feature = "Win32_Graphics_Direct3D", feature = "Win32_Graphics_Dxgi_Common"))]
    pub CreateShaderResourceView: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, pdesc: *const D3D11_SHADER_RESOURCE_VIEW_DESC, ppsrview: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Graphics_Direct3D", feature = "Win32_Graphics_Dxgi_Common")))]
    CreateShaderResourceView: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub CreateUnorderedAccessView: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, pdesc: *const D3D11_UNORDERED_ACCESS_VIEW_DESC, ppuaview: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    CreateUnorderedAccessView: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub CreateRenderTargetView: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, pdesc: *const D3D11_RENDER_TARGET_VIEW_DESC, pprtview: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    CreateRenderTargetView: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub CreateDepthStencilView: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, pdesc: *const D3D11_DEPTH_STENCIL_VIEW_DESC, ppdepthstencilview: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    CreateDepthStencilView: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub CreateInputLayout: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pinputelementdescs: *const D3D11_INPUT_ELEMENT_DESC, numelements: u32, pshaderbytecodewithinputsignature: *const ::core::ffi::c_void, bytecodelength: usize, ppinputlayout: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    CreateInputLayout: usize,
    pub CreateVertexShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, ppvertexshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateGeometryShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, ppgeometryshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateGeometryShaderWithStreamOutput: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, psodeclaration: *const D3D11_SO_DECLARATION_ENTRY, numentries: u32, pbufferstrides: *const u32, numstrides: u32, rasterizedstream: u32, pclasslinkage: *mut ::core::ffi::c_void, ppgeometryshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreatePixelShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, pppixelshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateHullShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, pphullshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateDomainShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, ppdomainshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateComputeShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, ppcomputeshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateClassLinkage: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pplinkage: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Foundation")]
    pub CreateBlendState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pblendstatedesc: *const D3D11_BLEND_DESC, ppblendstate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    CreateBlendState: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub CreateDepthStencilState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdepthstencildesc: *const D3D11_DEPTH_STENCIL_DESC, ppdepthstencilstate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    CreateDepthStencilState: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub CreateRasterizerState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, prasterizerdesc: *const D3D11_RASTERIZER_DESC, pprasterizerstate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    CreateRasterizerState: usize,
    pub CreateSamplerState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, psamplerdesc: *const D3D11_SAMPLER_DESC, ppsamplerstate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateQuery: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pquerydesc: *const D3D11_QUERY_DESC, ppquery: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreatePredicate: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppredicatedesc: *const D3D11_QUERY_DESC, pppredicate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateCounter: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pcounterdesc: *const D3D11_COUNTER_DESC, ppcounter: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub CreateDeferredContext: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, contextflags: u32, ppdeferredcontext: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Foundation")]
    pub OpenSharedResource: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, hresource: super::super::Foundation::HANDLE, returnedinterface: *const ::windows::core::GUID, ppresource: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    OpenSharedResource: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub CheckFormatSupport: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, format: super::Dxgi::Common::DXGI_FORMAT, pformatsupport: *mut u32) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    CheckFormatSupport: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub CheckMultisampleQualityLevels: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, format: super::Dxgi::Common::DXGI_FORMAT, samplecount: u32, pnumqualitylevels: *mut u32) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    CheckMultisampleQualityLevels: usize,
    pub CheckCounterInfo: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pcounterinfo: *mut D3D11_COUNTER_INFO),
    pub CheckCounter: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_COUNTER_DESC, ptype: *mut D3D11_COUNTER_TYPE, pactivecounters: *mut u32, szname: ::windows::core::PSTR, pnamelength: *mut u32, szunits: ::windows::core::PSTR, punitslength: *mut u32, szdescription: ::windows::core::PSTR, pdescriptionlength: *mut u32) -> ::windows::core::HRESULT,
    pub CheckFeatureSupport: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, feature: D3D11_FEATURE, pfeaturesupportdata: *mut ::core::ffi::c_void, featuresupportdatasize: u32) -> ::windows::core::HRESULT,
    pub GetPrivateData: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub SetPrivateData: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub SetPrivateDataInterface: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Graphics_Direct3D")]
    pub GetFeatureLevel: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> super::Direct3D::D3D_FEATURE_LEVEL,
    #[cfg(not(feature = "Win32_Graphics_Direct3D"))]
    GetFeatureLevel: usize,
    pub GetCreationFlags: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> u32,
    pub GetDeviceRemovedReason: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub GetImmediateContext: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppimmediatecontext: *mut *mut ::core::ffi::c_void),
    pub SetExceptionMode: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, raiseflags: u32) -> ::windows::core::HRESULT,
    pub GetExceptionMode: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> u32,
}

pub trait ID3D11Device_Impl: Sized {
    fn CreateBuffer(&self, pdesc: *const D3D11_BUFFER_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA) -> ::windows::core::Result<ID3D11Buffer>;
    fn CreateTexture1D(&self, pdesc: *const D3D11_TEXTURE1D_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA) -> ::windows::core::Result<ID3D11Texture1D>;
    fn CreateTexture2D(&self, pdesc: *const D3D11_TEXTURE2D_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA) -> ::windows::core::Result<ID3D11Texture2D>;
    fn CreateTexture3D(&self, pdesc: *const D3D11_TEXTURE3D_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA) -> ::windows::core::Result<ID3D11Texture3D>;
    fn CreateShaderResourceView(&self, presource: &::core::option::Option<ID3D11Resource>, pdesc: *const D3D11_SHADER_RESOURCE_VIEW_DESC) -> ::windows::core::Result<ID3D11ShaderResourceView>;
    fn CreateUnorderedAccessView(&self, presource: &::core::option::Option<ID3D11Resource>, pdesc: *const D3D11_UNORDERED_ACCESS_VIEW_DESC) -> ::windows::core::Result<ID3D11UnorderedAccessView>;
    fn CreateRenderTargetView(&self, presource: &::core::option::Option<ID3D11Resource>, pdesc: *const D3D11_RENDER_TARGET_VIEW_DESC) -> ::windows::core::Result<ID3D11RenderTargetView>;
    fn CreateDepthStencilView(&self, presource: &::core::option::Option<ID3D11Resource>, pdesc: *const D3D11_DEPTH_STENCIL_VIEW_DESC) -> ::windows::core::Result<ID3D11DepthStencilView>;
    fn CreateInputLayout(&self, pinputelementdescs: *const D3D11_INPUT_ELEMENT_DESC, numelements: u32, pshaderbytecodewithinputsignature: *const ::core::ffi::c_void, bytecodelength: usize) -> ::windows::core::Result<ID3D11InputLayout>;
    fn CreateVertexShader(&self, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: &::core::option::Option<ID3D11ClassLinkage>) -> ::windows::core::Result<ID3D11VertexShader>;
    fn CreateGeometryShader(&self, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: &::core::option::Option<ID3D11ClassLinkage>) -> ::windows::core::Result<ID3D11GeometryShader>;
    fn CreateGeometryShaderWithStreamOutput(&self, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, psodeclaration: *const D3D11_SO_DECLARATION_ENTRY, numentries: u32, pbufferstrides: *const u32, numstrides: u32, rasterizedstream: u32, pclasslinkage: &::core::option::Option<ID3D11ClassLinkage>) -> ::windows::core::Result<ID3D11GeometryShader>;
    fn CreatePixelShader(&self, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: &::core::option::Option<ID3D11ClassLinkage>) -> ::windows::core::Result<ID3D11PixelShader>;
    fn CreateHullShader(&self, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: &::core::option::Option<ID3D11ClassLinkage>) -> ::windows::core::Result<ID3D11HullShader>;
    fn CreateDomainShader(&self, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: &::core::option::Option<ID3D11ClassLinkage>) -> ::windows::core::Result<ID3D11DomainShader>;
    fn CreateComputeShader(&self, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: &::core::option::Option<ID3D11ClassLinkage>) -> ::windows::core::Result<ID3D11ComputeShader>;
    fn CreateClassLinkage(&self) -> ::windows::core::Result<ID3D11ClassLinkage>;
    fn CreateBlendState(&self, pblendstatedesc: *const D3D11_BLEND_DESC) -> ::windows::core::Result<ID3D11BlendState>;
    fn CreateDepthStencilState(&self, pdepthstencildesc: *const D3D11_DEPTH_STENCIL_DESC) -> ::windows::core::Result<ID3D11DepthStencilState>;
    fn CreateRasterizerState(&self, prasterizerdesc: *const D3D11_RASTERIZER_DESC) -> ::windows::core::Result<ID3D11RasterizerState>;
    fn CreateSamplerState(&self, psamplerdesc: *const D3D11_SAMPLER_DESC) -> ::windows::core::Result<ID3D11SamplerState>;
    fn CreateQuery(&self, pquerydesc: *const D3D11_QUERY_DESC) -> ::windows::core::Result<ID3D11Query>;
    fn CreatePredicate(&self, ppredicatedesc: *const D3D11_QUERY_DESC) -> ::windows::core::Result<ID3D11Predicate>;
    fn CreateCounter(&self, pcounterdesc: *const D3D11_COUNTER_DESC) -> ::windows::core::Result<ID3D11Counter>;
    fn CreateDeferredContext(&self, contextflags: u32) -> ::windows::core::Result<ID3D11DeviceContext>;
    fn OpenSharedResource(&self, hresource: super::super::Foundation::HANDLE, returnedinterface: *const ::windows::core::GUID, ppresource: *mut *mut ::core::ffi::c_void) -> ::windows::core::Result<()>;
    fn CheckFormatSupport(&self, format: super::Dxgi::Common::DXGI_FORMAT) -> ::windows::core::Result<u32>;
    fn CheckMultisampleQualityLevels(&self, format: super::Dxgi::Common::DXGI_FORMAT, samplecount: u32) -> ::windows::core::Result<u32>;
    fn CheckCounterInfo(&self, pcounterinfo: *mut D3D11_COUNTER_INFO);
    fn CheckCounter(&self, pdesc: *const D3D11_COUNTER_DESC, ptype: *mut D3D11_COUNTER_TYPE, pactivecounters: *mut u32, szname: ::windows::core::PSTR, pnamelength: *mut u32, szunits: ::windows::core::PSTR, punitslength: *mut u32, szdescription: ::windows::core::PSTR, pdescriptionlength: *mut u32) -> ::windows::core::Result<()>;
    fn CheckFeatureSupport(&self, feature: D3D11_FEATURE, pfeaturesupportdata: *mut ::core::ffi::c_void, featuresupportdatasize: u32) -> ::windows::core::Result<()>;
    fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()>;
    fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()>;
    fn SetPrivateDataInterface(&self, guid: *const ::windows::core::GUID, pdata: &::core::option::Option<::windows::core::IUnknown>) -> ::windows::core::Result<()>;
    fn GetFeatureLevel(&self) -> super::Direct3D::D3D_FEATURE_LEVEL;
    fn GetCreationFlags(&self) -> u32;
    fn GetDeviceRemovedReason(&self) -> ::windows::core::Result<()>;
    fn GetImmediateContext(&self, ppimmediatecontext: *mut ::core::option::Option<ID3D11DeviceContext>);
    fn SetExceptionMode(&self, raiseflags: u32) -> ::windows::core::Result<()>;
    fn GetExceptionMode(&self) -> u32;
}

impl ID3D11Device_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>() -> ID3D11Device_Vtbl {
        unsafe extern "system" fn CreateBuffer<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_BUFFER_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA, ppbuffer: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateBuffer(::core::mem::transmute_copy(&pdesc), ::core::mem::transmute_copy(&pinitialdata)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppbuffer, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateTexture1D<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_TEXTURE1D_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA, pptexture1d: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateTexture1D(::core::mem::transmute_copy(&pdesc), ::core::mem::transmute_copy(&pinitialdata)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pptexture1d, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateTexture2D<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_TEXTURE2D_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA, pptexture2d: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateTexture2D(::core::mem::transmute_copy(&pdesc), ::core::mem::transmute_copy(&pinitialdata)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pptexture2d, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateTexture3D<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_TEXTURE3D_DESC, pinitialdata: *const D3D11_SUBRESOURCE_DATA, pptexture3d: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateTexture3D(::core::mem::transmute_copy(&pdesc), ::core::mem::transmute_copy(&pinitialdata)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pptexture3d, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateShaderResourceView<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, pdesc: *const D3D11_SHADER_RESOURCE_VIEW_DESC, ppsrview: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateShaderResourceView(::core::mem::transmute(&presource), ::core::mem::transmute_copy(&pdesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppsrview, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateUnorderedAccessView<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, pdesc: *const D3D11_UNORDERED_ACCESS_VIEW_DESC, ppuaview: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateUnorderedAccessView(::core::mem::transmute(&presource), ::core::mem::transmute_copy(&pdesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppuaview, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateRenderTargetView<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, pdesc: *const D3D11_RENDER_TARGET_VIEW_DESC, pprtview: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateRenderTargetView(::core::mem::transmute(&presource), ::core::mem::transmute_copy(&pdesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pprtview, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateDepthStencilView<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, pdesc: *const D3D11_DEPTH_STENCIL_VIEW_DESC, ppdepthstencilview: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateDepthStencilView(::core::mem::transmute(&presource), ::core::mem::transmute_copy(&pdesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppdepthstencilview, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateInputLayout<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pinputelementdescs: *const D3D11_INPUT_ELEMENT_DESC, numelements: u32, pshaderbytecodewithinputsignature: *const ::core::ffi::c_void, bytecodelength: usize, ppinputlayout: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateInputLayout(::core::mem::transmute_copy(&pinputelementdescs), ::core::mem::transmute_copy(&numelements), ::core::mem::transmute_copy(&pshaderbytecodewithinputsignature), ::core::mem::transmute_copy(&bytecodelength)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppinputlayout, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateVertexShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, ppvertexshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateVertexShader(::core::mem::transmute_copy(&pshaderbytecode), ::core::mem::transmute_copy(&bytecodelength), ::core::mem::transmute(&pclasslinkage)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppvertexshader, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateGeometryShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, ppgeometryshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateGeometryShader(::core::mem::transmute_copy(&pshaderbytecode), ::core::mem::transmute_copy(&bytecodelength), ::core::mem::transmute(&pclasslinkage)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppgeometryshader, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateGeometryShaderWithStreamOutput<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, psodeclaration: *const D3D11_SO_DECLARATION_ENTRY, numentries: u32, pbufferstrides: *const u32, numstrides: u32, rasterizedstream: u32, pclasslinkage: *mut ::core::ffi::c_void, ppgeometryshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateGeometryShaderWithStreamOutput(::core::mem::transmute_copy(&pshaderbytecode), ::core::mem::transmute_copy(&bytecodelength), ::core::mem::transmute_copy(&psodeclaration), ::core::mem::transmute_copy(&numentries), ::core::mem::transmute_copy(&pbufferstrides), ::core::mem::transmute_copy(&numstrides), ::core::mem::transmute_copy(&rasterizedstream), ::core::mem::transmute(&pclasslinkage)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppgeometryshader, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreatePixelShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, pppixelshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreatePixelShader(::core::mem::transmute_copy(&pshaderbytecode), ::core::mem::transmute_copy(&bytecodelength), ::core::mem::transmute(&pclasslinkage)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pppixelshader, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateHullShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, pphullshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateHullShader(::core::mem::transmute_copy(&pshaderbytecode), ::core::mem::transmute_copy(&bytecodelength), ::core::mem::transmute(&pclasslinkage)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pphullshader, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateDomainShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, ppdomainshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateDomainShader(::core::mem::transmute_copy(&pshaderbytecode), ::core::mem::transmute_copy(&bytecodelength), ::core::mem::transmute(&pclasslinkage)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppdomainshader, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateComputeShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pshaderbytecode: *const ::core::ffi::c_void, bytecodelength: usize, pclasslinkage: *mut ::core::ffi::c_void, ppcomputeshader: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateComputeShader(::core::mem::transmute_copy(&pshaderbytecode), ::core::mem::transmute_copy(&bytecodelength), ::core::mem::transmute(&pclasslinkage)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppcomputeshader, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateClassLinkage<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pplinkage: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateClassLinkage() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pplinkage, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateBlendState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pblendstatedesc: *const D3D11_BLEND_DESC, ppblendstate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateBlendState(::core::mem::transmute_copy(&pblendstatedesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppblendstate, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateDepthStencilState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdepthstencildesc: *const D3D11_DEPTH_STENCIL_DESC, ppdepthstencilstate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateDepthStencilState(::core::mem::transmute_copy(&pdepthstencildesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppdepthstencilstate, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateRasterizerState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, prasterizerdesc: *const D3D11_RASTERIZER_DESC, pprasterizerstate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateRasterizerState(::core::mem::transmute_copy(&prasterizerdesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pprasterizerstate, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateSamplerState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, psamplerdesc: *const D3D11_SAMPLER_DESC, ppsamplerstate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateSamplerState(::core::mem::transmute_copy(&psamplerdesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppsamplerstate, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateQuery<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pquerydesc: *const D3D11_QUERY_DESC, ppquery: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateQuery(::core::mem::transmute_copy(&pquerydesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppquery, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreatePredicate<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppredicatedesc: *const D3D11_QUERY_DESC, pppredicate: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreatePredicate(::core::mem::transmute_copy(&ppredicatedesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pppredicate, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateCounter<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pcounterdesc: *const D3D11_COUNTER_DESC, ppcounter: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateCounter(::core::mem::transmute_copy(&pcounterdesc)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppcounter, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateDeferredContext<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, contextflags: u32, ppdeferredcontext: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateDeferredContext(::core::mem::transmute_copy(&contextflags)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppdeferredcontext, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn OpenSharedResource<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, hresource: super::super::Foundation::HANDLE, returnedinterface: *const ::windows::core::GUID, ppresource: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.OpenSharedResource(::core::mem::transmute_copy(&hresource), ::core::mem::transmute_copy(&returnedinterface), ::core::mem::transmute_copy(&ppresource)).into()
        }
        unsafe extern "system" fn CheckFormatSupport<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, format: super::Dxgi::Common::DXGI_FORMAT, pformatsupport: *mut u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CheckFormatSupport(::core::mem::transmute_copy(&format)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pformatsupport, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CheckMultisampleQualityLevels<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, format: super::Dxgi::Common::DXGI_FORMAT, samplecount: u32, pnumqualitylevels: *mut u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CheckMultisampleQualityLevels(::core::mem::transmute_copy(&format), ::core::mem::transmute_copy(&samplecount)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pnumqualitylevels, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CheckCounterInfo<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pcounterinfo: *mut D3D11_COUNTER_INFO) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CheckCounterInfo(::core::mem::transmute_copy(&pcounterinfo))
        }
        unsafe extern "system" fn CheckCounter<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *const D3D11_COUNTER_DESC, ptype: *mut D3D11_COUNTER_TYPE, pactivecounters: *mut u32, szname: ::windows::core::PSTR, pnamelength: *mut u32, szunits: ::windows::core::PSTR, punitslength: *mut u32, szdescription: ::windows::core::PSTR, pdescriptionlength: *mut u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CheckCounter(::core::mem::transmute_copy(&pdesc), ::core::mem::transmute_copy(&ptype), ::core::mem::transmute_copy(&pactivecounters), ::core::mem::transmute_copy(&szname), ::core::mem::transmute_copy(&pnamelength), ::core::mem::transmute_copy(&szunits), ::core::mem::transmute_copy(&punitslength), ::core::mem::transmute_copy(&szdescription), ::core::mem::transmute_copy(&pdescriptionlength)).into()
        }
        unsafe extern "system" fn CheckFeatureSupport<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, feature: D3D11_FEATURE, pfeaturesupportdata: *mut ::core::ffi::c_void, featuresupportdatasize: u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CheckFeatureSupport(::core::mem::transmute_copy(&feature), ::core::mem::transmute_copy(&pfeaturesupportdata), ::core::mem::transmute_copy(&featuresupportdatasize)).into()
        }
        unsafe extern "system" fn GetPrivateData<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetPrivateData(::core::mem::transmute_copy(&guid), ::core::mem::transmute_copy(&pdatasize), ::core::mem::transmute_copy(&pdata)).into()
        }
        unsafe extern "system" fn SetPrivateData<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetPrivateData(::core::mem::transmute_copy(&guid), ::core::mem::transmute_copy(&datasize), ::core::mem::transmute_copy(&pdata)).into()
        }
        unsafe extern "system" fn SetPrivateDataInterface<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, guid: *const ::windows::core::GUID, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetPrivateDataInterface(::core::mem::transmute_copy(&guid), ::core::mem::transmute(&pdata)).into()
        }
        unsafe extern "system" fn GetFeatureLevel<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> super::Direct3D::D3D_FEATURE_LEVEL {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetFeatureLevel()
        }
        unsafe extern "system" fn GetCreationFlags<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> u32 {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetCreationFlags()
        }
        unsafe extern "system" fn GetDeviceRemovedReason<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDeviceRemovedReason().into()
        }
        unsafe extern "system" fn GetImmediateContext<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppimmediatecontext: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetImmediateContext(::core::mem::transmute_copy(&ppimmediatecontext))
        }
        unsafe extern "system" fn SetExceptionMode<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, raiseflags: u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetExceptionMode(::core::mem::transmute_copy(&raiseflags)).into()
        }
        unsafe extern "system" fn GetExceptionMode<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Device_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> u32 {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetExceptionMode()
        }
        Self {
            base__: ::windows::core::IUnknown_Vtbl::new::<Identity, OFFSET>(),
            CreateBuffer: CreateBuffer::<Identity, Impl, OFFSET>,
            CreateTexture1D: CreateTexture1D::<Identity, Impl, OFFSET>,
            CreateTexture2D: CreateTexture2D::<Identity, Impl, OFFSET>,
            CreateTexture3D: CreateTexture3D::<Identity, Impl, OFFSET>,
            CreateShaderResourceView: CreateShaderResourceView::<Identity, Impl, OFFSET>,
            CreateUnorderedAccessView: CreateUnorderedAccessView::<Identity, Impl, OFFSET>,
            CreateRenderTargetView: CreateRenderTargetView::<Identity, Impl, OFFSET>,
            CreateDepthStencilView: CreateDepthStencilView::<Identity, Impl, OFFSET>,
            CreateInputLayout: CreateInputLayout::<Identity, Impl, OFFSET>,
            CreateVertexShader: CreateVertexShader::<Identity, Impl, OFFSET>,
            CreateGeometryShader: CreateGeometryShader::<Identity, Impl, OFFSET>,
            CreateGeometryShaderWithStreamOutput: CreateGeometryShaderWithStreamOutput::<Identity, Impl, OFFSET>,
            CreatePixelShader: CreatePixelShader::<Identity, Impl, OFFSET>,
            CreateHullShader: CreateHullShader::<Identity, Impl, OFFSET>,
            CreateDomainShader: CreateDomainShader::<Identity, Impl, OFFSET>,
            CreateComputeShader: CreateComputeShader::<Identity, Impl, OFFSET>,
            CreateClassLinkage: CreateClassLinkage::<Identity, Impl, OFFSET>,
            CreateBlendState: CreateBlendState::<Identity, Impl, OFFSET>,
            CreateDepthStencilState: CreateDepthStencilState::<Identity, Impl, OFFSET>,
            CreateRasterizerState: CreateRasterizerState::<Identity, Impl, OFFSET>,
            CreateSamplerState: CreateSamplerState::<Identity, Impl, OFFSET>,
            CreateQuery: CreateQuery::<Identity, Impl, OFFSET>,
            CreatePredicate: CreatePredicate::<Identity, Impl, OFFSET>,
            CreateCounter: CreateCounter::<Identity, Impl, OFFSET>,
            CreateDeferredContext: CreateDeferredContext::<Identity, Impl, OFFSET>,
            OpenSharedResource: OpenSharedResource::<Identity, Impl, OFFSET>,
            CheckFormatSupport: CheckFormatSupport::<Identity, Impl, OFFSET>,
            CheckMultisampleQualityLevels: CheckMultisampleQualityLevels::<Identity, Impl, OFFSET>,
            CheckCounterInfo: CheckCounterInfo::<Identity, Impl, OFFSET>,
            CheckCounter: CheckCounter::<Identity, Impl, OFFSET>,
            CheckFeatureSupport: CheckFeatureSupport::<Identity, Impl, OFFSET>,
            GetPrivateData: GetPrivateData::<Identity, Impl, OFFSET>,
            SetPrivateData: SetPrivateData::<Identity, Impl, OFFSET>,
            SetPrivateDataInterface: SetPrivateDataInterface::<Identity, Impl, OFFSET>,
            GetFeatureLevel: GetFeatureLevel::<Identity, Impl, OFFSET>,
            GetCreationFlags: GetCreationFlags::<Identity, Impl, OFFSET>,
            GetDeviceRemovedReason: GetDeviceRemovedReason::<Identity, Impl, OFFSET>,
            GetImmediateContext: GetImmediateContext::<Identity, Impl, OFFSET>,
            SetExceptionMode: SetExceptionMode::<Identity, Impl, OFFSET>,
            GetExceptionMode: GetExceptionMode::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Device as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Device, ::windows::core::IUnknown);

#[repr(transparent)]
pub struct ID3D11DeviceContext(::windows::core::IUnknown);
impl ID3D11DeviceContext {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn VSSetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&[::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).VSSetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn PSSetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&[::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).PSSetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn PSSetShader<'a, P0>(&self, ppixelshader: P0, ppclassinstances: ::core::option::Option<&[::core::option::Option<ID3D11ClassInstance>]>)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11PixelShader>>,
    {
        (::windows::core::Vtable::vtable(self).PSSetShader)(::windows::core::Vtable::as_raw(self), ppixelshader.into().abi(), ::core::mem::transmute(ppclassinstances.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), ppclassinstances.as_deref().map_or(0, |slice| slice.len() as _))
    }
    pub unsafe fn PSSetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&[::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).PSSetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn VSSetShader<'a, P0>(&self, pvertexshader: P0, ppclassinstances: ::core::option::Option<&[::core::option::Option<ID3D11ClassInstance>]>)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11VertexShader>>,
    {
        (::windows::core::Vtable::vtable(self).VSSetShader)(::windows::core::Vtable::as_raw(self), pvertexshader.into().abi(), ::core::mem::transmute(ppclassinstances.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), ppclassinstances.as_deref().map_or(0, |slice| slice.len() as _))
    }
    pub unsafe fn DrawIndexed(&self, indexcount: u32, startindexlocation: u32, basevertexlocation: i32) {
        (::windows::core::Vtable::vtable(self).DrawIndexed)(::windows::core::Vtable::as_raw(self), indexcount, startindexlocation, basevertexlocation)
    }
    pub unsafe fn Draw(&self, vertexcount: u32, startvertexlocation: u32) {
        (::windows::core::Vtable::vtable(self).Draw)(::windows::core::Vtable::as_raw(self), vertexcount, startvertexlocation)
    }
    pub unsafe fn Map<'a, P0>(&self, presource: P0, subresource: u32, maptype: D3D11_MAP, mapflags: u32) -> ::windows::core::Result<D3D11_MAPPED_SUBRESOURCE>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).Map)(::windows::core::Vtable::as_raw(self), presource.into().abi(), subresource, maptype, mapflags, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<D3D11_MAPPED_SUBRESOURCE>(result__)
    }
    pub unsafe fn Unmap<'a, P0>(&self, presource: P0, subresource: u32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        (::windows::core::Vtable::vtable(self).Unmap)(::windows::core::Vtable::as_raw(self), presource.into().abi(), subresource)
    }
    pub unsafe fn PSSetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&[::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).PSSetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn IASetInputLayout<'a, P0>(&self, pinputlayout: P0)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11InputLayout>>,
    {
        (::windows::core::Vtable::vtable(self).IASetInputLayout)(::windows::core::Vtable::as_raw(self), pinputlayout.into().abi())
    }
    pub unsafe fn IASetVertexBuffers(&self, startslot: u32, numbuffers: u32, ppvertexbuffers: ::core::option::Option<*const ::core::option::Option<ID3D11Buffer>>, pstrides: ::core::option::Option<*const u32>, poffsets: ::core::option::Option<*const u32>) {
        (::windows::core::Vtable::vtable(self).IASetVertexBuffers)(::windows::core::Vtable::as_raw(self), startslot, numbuffers, ::core::mem::transmute(ppvertexbuffers.unwrap_or(::std::ptr::null())), ::core::mem::transmute(pstrides.unwrap_or(::std::ptr::null())), ::core::mem::transmute(poffsets.unwrap_or(::std::ptr::null())))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn IASetIndexBuffer<'a, P0>(&self, pindexbuffer: P0, format: super::Dxgi::Common::DXGI_FORMAT, offset: u32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Buffer>>,
    {
        (::windows::core::Vtable::vtable(self).IASetIndexBuffer)(::windows::core::Vtable::as_raw(self), pindexbuffer.into().abi(), format, offset)
    }
    pub unsafe fn DrawIndexedInstanced(&self, indexcountperinstance: u32, instancecount: u32, startindexlocation: u32, basevertexlocation: i32, startinstancelocation: u32) {
        (::windows::core::Vtable::vtable(self).DrawIndexedInstanced)(::windows::core::Vtable::as_raw(self), indexcountperinstance, instancecount, startindexlocation, basevertexlocation, startinstancelocation)
    }
    pub unsafe fn DrawInstanced(&self, vertexcountperinstance: u32, instancecount: u32, startvertexlocation: u32, startinstancelocation: u32) {
        (::windows::core::Vtable::vtable(self).DrawInstanced)(::windows::core::Vtable::as_raw(self), vertexcountperinstance, instancecount, startvertexlocation, startinstancelocation)
    }
    pub unsafe fn GSSetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&[::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).GSSetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn GSSetShader<'a, P0>(&self, pshader: P0, ppclassinstances: ::core::option::Option<&[::core::option::Option<ID3D11ClassInstance>]>)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11GeometryShader>>,
    {
        (::windows::core::Vtable::vtable(self).GSSetShader)(::windows::core::Vtable::as_raw(self), pshader.into().abi(), ::core::mem::transmute(ppclassinstances.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), ppclassinstances.as_deref().map_or(0, |slice| slice.len() as _))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Direct3D\"`*"]
    #[cfg(feature = "Win32_Graphics_Direct3D")]
    pub unsafe fn IASetPrimitiveTopology(&self, topology: super::Direct3D::D3D_PRIMITIVE_TOPOLOGY) {
        (::windows::core::Vtable::vtable(self).IASetPrimitiveTopology)(::windows::core::Vtable::as_raw(self), topology)
    }
    pub unsafe fn VSSetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&[::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).VSSetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn VSSetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&[::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).VSSetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn Begin<'a, P0>(&self, pasync: P0)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Asynchronous>>,
    {
        (::windows::core::Vtable::vtable(self).Begin)(::windows::core::Vtable::as_raw(self), pasync.into().abi())
    }
    pub unsafe fn End<'a, P0>(&self, pasync: P0)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Asynchronous>>,
    {
        (::windows::core::Vtable::vtable(self).End)(::windows::core::Vtable::as_raw(self), pasync.into().abi())
    }
    pub unsafe fn GetData<'a, P0>(&self, pasync: P0, pdata: ::core::option::Option<*mut ::core::ffi::c_void>, datasize: u32, getdataflags: u32) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Asynchronous>>,
    {
        (::windows::core::Vtable::vtable(self).GetData)(::windows::core::Vtable::as_raw(self), pasync.into().abi(), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut())), datasize, getdataflags).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn SetPredication<'a, P0, P1>(&self, ppredicate: P0, predicatevalue: P1)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Predicate>>,
        P1: ::std::convert::Into<super::super::Foundation::BOOL>,
    {
        (::windows::core::Vtable::vtable(self).SetPredication)(::windows::core::Vtable::as_raw(self), ppredicate.into().abi(), predicatevalue.into())
    }
    pub unsafe fn GSSetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&[::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).GSSetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn GSSetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&[::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).GSSetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn OMSetRenderTargets<'a, P0>(&self, pprendertargetviews: ::core::option::Option<&[::core::option::Option<ID3D11RenderTargetView>]>, pdepthstencilview: P0)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11DepthStencilView>>,
    {
        (::windows::core::Vtable::vtable(self).OMSetRenderTargets)(::windows::core::Vtable::as_raw(self), pprendertargetviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(pprendertargetviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), pdepthstencilview.into().abi())
    }
    pub unsafe fn OMSetRenderTargetsAndUnorderedAccessViews<'a, P0>(&self, pprendertargetviews: ::core::option::Option<&[::core::option::Option<ID3D11RenderTargetView>]>, pdepthstencilview: P0, uavstartslot: u32, numuavs: u32, ppunorderedaccessviews: ::core::option::Option<*const ::core::option::Option<ID3D11UnorderedAccessView>>, puavinitialcounts: ::core::option::Option<*const u32>)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11DepthStencilView>>,
    {
        (::windows::core::Vtable::vtable(self).OMSetRenderTargetsAndUnorderedAccessViews)(::windows::core::Vtable::as_raw(self), pprendertargetviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(pprendertargetviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), pdepthstencilview.into().abi(), uavstartslot, numuavs, ::core::mem::transmute(ppunorderedaccessviews.unwrap_or(::std::ptr::null())), ::core::mem::transmute(puavinitialcounts.unwrap_or(::std::ptr::null())))
    }
    pub unsafe fn OMSetBlendState<'a, P0>(&self, pblendstate: P0, blendfactor: ::core::option::Option<*const f32>, samplemask: u32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11BlendState>>,
    {
        (::windows::core::Vtable::vtable(self).OMSetBlendState)(::windows::core::Vtable::as_raw(self), pblendstate.into().abi(), ::core::mem::transmute(blendfactor.unwrap_or(::std::ptr::null())), samplemask)
    }
    pub unsafe fn OMSetDepthStencilState<'a, P0>(&self, pdepthstencilstate: P0, stencilref: u32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11DepthStencilState>>,
    {
        (::windows::core::Vtable::vtable(self).OMSetDepthStencilState)(::windows::core::Vtable::as_raw(self), pdepthstencilstate.into().abi(), stencilref)
    }
    pub unsafe fn SOSetTargets(&self, numbuffers: u32, ppsotargets: ::core::option::Option<*const ::core::option::Option<ID3D11Buffer>>, poffsets: ::core::option::Option<*const u32>) {
        (::windows::core::Vtable::vtable(self).SOSetTargets)(::windows::core::Vtable::as_raw(self), numbuffers, ::core::mem::transmute(ppsotargets.unwrap_or(::std::ptr::null())), ::core::mem::transmute(poffsets.unwrap_or(::std::ptr::null())))
    }
    pub unsafe fn DrawAuto(&self) {
        (::windows::core::Vtable::vtable(self).DrawAuto)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn DrawIndexedInstancedIndirect<'a, P0>(&self, pbufferforargs: P0, alignedbyteoffsetforargs: u32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Buffer>>,
    {
        (::windows::core::Vtable::vtable(self).DrawIndexedInstancedIndirect)(::windows::core::Vtable::as_raw(self), pbufferforargs.into().abi(), alignedbyteoffsetforargs)
    }
    pub unsafe fn DrawInstancedIndirect<'a, P0>(&self, pbufferforargs: P0, alignedbyteoffsetforargs: u32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Buffer>>,
    {
        (::windows::core::Vtable::vtable(self).DrawInstancedIndirect)(::windows::core::Vtable::as_raw(self), pbufferforargs.into().abi(), alignedbyteoffsetforargs)
    }
    pub unsafe fn Dispatch(&self, threadgroupcountx: u32, threadgroupcounty: u32, threadgroupcountz: u32) {
        (::windows::core::Vtable::vtable(self).Dispatch)(::windows::core::Vtable::as_raw(self), threadgroupcountx, threadgroupcounty, threadgroupcountz)
    }
    pub unsafe fn DispatchIndirect<'a, P0>(&self, pbufferforargs: P0, alignedbyteoffsetforargs: u32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Buffer>>,
    {
        (::windows::core::Vtable::vtable(self).DispatchIndirect)(::windows::core::Vtable::as_raw(self), pbufferforargs.into().abi(), alignedbyteoffsetforargs)
    }
    pub unsafe fn RSSetState<'a, P0>(&self, prasterizerstate: P0)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11RasterizerState>>,
    {
        (::windows::core::Vtable::vtable(self).RSSetState)(::windows::core::Vtable::as_raw(self), prasterizerstate.into().abi())
    }
    pub unsafe fn RSSetViewports(&self, pviewports: ::core::option::Option<&[D3D11_VIEWPORT]>) {
        (::windows::core::Vtable::vtable(self).RSSetViewports)(::windows::core::Vtable::as_raw(self), pviewports.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(pviewports.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn RSSetScissorRects(&self, prects: ::core::option::Option<&[super::super::Foundation::RECT]>) {
        (::windows::core::Vtable::vtable(self).RSSetScissorRects)(::windows::core::Vtable::as_raw(self), prects.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(prects.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn CopySubresourceRegion<'a, P0, P1>(&self, pdstresource: P0, dstsubresource: u32, dstx: u32, dsty: u32, dstz: u32, psrcresource: P1, srcsubresource: u32, psrcbox: ::core::option::Option<*const D3D11_BOX>)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
        P1: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        (::windows::core::Vtable::vtable(self).CopySubresourceRegion)(::windows::core::Vtable::as_raw(self), pdstresource.into().abi(), dstsubresource, dstx, dsty, dstz, psrcresource.into().abi(), srcsubresource, ::core::mem::transmute(psrcbox.unwrap_or(::std::ptr::null())))
    }
    pub unsafe fn CopyResource<'a, P0, P1>(&self, pdstresource: P0, psrcresource: P1)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
        P1: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        (::windows::core::Vtable::vtable(self).CopyResource)(::windows::core::Vtable::as_raw(self), pdstresource.into().abi(), psrcresource.into().abi())
    }
    pub unsafe fn UpdateSubresource<'a, P0>(&self, pdstresource: P0, dstsubresource: u32, pdstbox: ::core::option::Option<*const D3D11_BOX>, psrcdata: *const ::core::ffi::c_void, srcrowpitch: u32, srcdepthpitch: u32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        (::windows::core::Vtable::vtable(self).UpdateSubresource)(::windows::core::Vtable::as_raw(self), pdstresource.into().abi(), dstsubresource, ::core::mem::transmute(pdstbox.unwrap_or(::std::ptr::null())), ::core::mem::transmute(psrcdata), srcrowpitch, srcdepthpitch)
    }
    pub unsafe fn CopyStructureCount<'a, P0, P1>(&self, pdstbuffer: P0, dstalignedbyteoffset: u32, psrcview: P1)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Buffer>>,
        P1: ::std::convert::Into<::windows::core::InParam<'a, ID3D11UnorderedAccessView>>,
    {
        (::windows::core::Vtable::vtable(self).CopyStructureCount)(::windows::core::Vtable::as_raw(self), pdstbuffer.into().abi(), dstalignedbyteoffset, psrcview.into().abi())
    }
    pub unsafe fn ClearRenderTargetView<'a, P0>(&self, prendertargetview: P0, colorrgba: *const f32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11RenderTargetView>>,
    {
        (::windows::core::Vtable::vtable(self).ClearRenderTargetView)(::windows::core::Vtable::as_raw(self), prendertargetview.into().abi(), ::core::mem::transmute(colorrgba))
    }
    pub unsafe fn ClearUnorderedAccessViewUint<'a, P0>(&self, punorderedaccessview: P0, values: *const u32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11UnorderedAccessView>>,
    {
        (::windows::core::Vtable::vtable(self).ClearUnorderedAccessViewUint)(::windows::core::Vtable::as_raw(self), punorderedaccessview.into().abi(), ::core::mem::transmute(values))
    }
    pub unsafe fn ClearUnorderedAccessViewFloat<'a, P0>(&self, punorderedaccessview: P0, values: *const f32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11UnorderedAccessView>>,
    {
        (::windows::core::Vtable::vtable(self).ClearUnorderedAccessViewFloat)(::windows::core::Vtable::as_raw(self), punorderedaccessview.into().abi(), ::core::mem::transmute(values))
    }
    pub unsafe fn ClearDepthStencilView<'a, P0>(&self, pdepthstencilview: P0, clearflags: u32, depth: f32, stencil: u8)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11DepthStencilView>>,
    {
        (::windows::core::Vtable::vtable(self).ClearDepthStencilView)(::windows::core::Vtable::as_raw(self), pdepthstencilview.into().abi(), clearflags, depth, stencil)
    }
    pub unsafe fn GenerateMips<'a, P0>(&self, pshaderresourceview: P0)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11ShaderResourceView>>,
    {
        (::windows::core::Vtable::vtable(self).GenerateMips)(::windows::core::Vtable::as_raw(self), pshaderresourceview.into().abi())
    }
    pub unsafe fn SetResourceMinLOD<'a, P0>(&self, presource: P0, minlod: f32)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        (::windows::core::Vtable::vtable(self).SetResourceMinLOD)(::windows::core::Vtable::as_raw(self), presource.into().abi(), minlod)
    }
    pub unsafe fn GetResourceMinLOD<'a, P0>(&self, presource: P0) -> f32
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        (::windows::core::Vtable::vtable(self).GetResourceMinLOD)(::windows::core::Vtable::as_raw(self), presource.into().abi())
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn ResolveSubresource<'a, P0, P1>(&self, pdstresource: P0, dstsubresource: u32, psrcresource: P1, srcsubresource: u32, format: super::Dxgi::Common::DXGI_FORMAT)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
        P1: ::std::convert::Into<::windows::core::InParam<'a, ID3D11Resource>>,
    {
        (::windows::core::Vtable::vtable(self).ResolveSubresource)(::windows::core::Vtable::as_raw(self), pdstresource.into().abi(), dstsubresource, psrcresource.into().abi(), srcsubresource, format)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn ExecuteCommandList<'a, P0, P1>(&self, pcommandlist: P0, restorecontextstate: P1)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11CommandList>>,
        P1: ::std::convert::Into<super::super::Foundation::BOOL>,
    {
        (::windows::core::Vtable::vtable(self).ExecuteCommandList)(::windows::core::Vtable::as_raw(self), pcommandlist.into().abi(), restorecontextstate.into())
    }
    pub unsafe fn HSSetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&[::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).HSSetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn HSSetShader<'a, P0>(&self, phullshader: P0, ppclassinstances: ::core::option::Option<&[::core::option::Option<ID3D11ClassInstance>]>)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11HullShader>>,
    {
        (::windows::core::Vtable::vtable(self).HSSetShader)(::windows::core::Vtable::as_raw(self), phullshader.into().abi(), ::core::mem::transmute(ppclassinstances.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), ppclassinstances.as_deref().map_or(0, |slice| slice.len() as _))
    }
    pub unsafe fn HSSetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&[::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).HSSetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn HSSetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&[::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).HSSetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn DSSetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&[::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).DSSetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn DSSetShader<'a, P0>(&self, pdomainshader: P0, ppclassinstances: ::core::option::Option<&[::core::option::Option<ID3D11ClassInstance>]>)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11DomainShader>>,
    {
        (::windows::core::Vtable::vtable(self).DSSetShader)(::windows::core::Vtable::as_raw(self), pdomainshader.into().abi(), ::core::mem::transmute(ppclassinstances.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), ppclassinstances.as_deref().map_or(0, |slice| slice.len() as _))
    }
    pub unsafe fn DSSetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&[::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).DSSetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn DSSetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&[::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).DSSetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn CSSetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&[::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).CSSetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn CSSetUnorderedAccessViews(&self, startslot: u32, numuavs: u32, ppunorderedaccessviews: ::core::option::Option<*const ::core::option::Option<ID3D11UnorderedAccessView>>, puavinitialcounts: ::core::option::Option<*const u32>) {
        (::windows::core::Vtable::vtable(self).CSSetUnorderedAccessViews)(::windows::core::Vtable::as_raw(self), startslot, numuavs, ::core::mem::transmute(ppunorderedaccessviews.unwrap_or(::std::ptr::null())), ::core::mem::transmute(puavinitialcounts.unwrap_or(::std::ptr::null())))
    }
    pub unsafe fn CSSetShader<'a, P0>(&self, pcomputeshader: P0, ppclassinstances: ::core::option::Option<&[::core::option::Option<ID3D11ClassInstance>]>)
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ID3D11ComputeShader>>,
    {
        (::windows::core::Vtable::vtable(self).CSSetShader)(::windows::core::Vtable::as_raw(self), pcomputeshader.into().abi(), ::core::mem::transmute(ppclassinstances.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), ppclassinstances.as_deref().map_or(0, |slice| slice.len() as _))
    }
    pub unsafe fn CSSetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&[::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).CSSetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn CSSetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&[::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).CSSetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn VSGetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&mut [::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).VSGetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn PSGetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&mut [::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).PSGetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn PSGetShader(&self, pppixelshader: ::core::option::Option<*mut ::core::option::Option<ID3D11PixelShader>>, ppclassinstances: ::core::option::Option<*mut ::core::option::Option<ID3D11ClassInstance>>, pnumclassinstances: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).PSGetShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pppixelshader.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(ppclassinstances.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pnumclassinstances.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn PSGetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&mut [::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).PSGetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn VSGetShader(&self, ppvertexshader: ::core::option::Option<*mut ::core::option::Option<ID3D11VertexShader>>, ppclassinstances: ::core::option::Option<*mut ::core::option::Option<ID3D11ClassInstance>>, pnumclassinstances: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).VSGetShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppvertexshader.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(ppclassinstances.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pnumclassinstances.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn PSGetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&mut [::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).PSGetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn IAGetInputLayout(&self, ppinputlayout: ::core::option::Option<*mut ::core::option::Option<ID3D11InputLayout>>) {
        (::windows::core::Vtable::vtable(self).IAGetInputLayout)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppinputlayout.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn IAGetVertexBuffers(&self, startslot: u32, numbuffers: u32, ppvertexbuffers: ::core::option::Option<*mut ::core::option::Option<ID3D11Buffer>>, pstrides: ::core::option::Option<*mut u32>, poffsets: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).IAGetVertexBuffers)(::windows::core::Vtable::as_raw(self), startslot, numbuffers, ::core::mem::transmute(ppvertexbuffers.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pstrides.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(poffsets.unwrap_or(::std::ptr::null_mut())))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn IAGetIndexBuffer(&self, pindexbuffer: ::core::option::Option<*mut ::core::option::Option<ID3D11Buffer>>, format: ::core::option::Option<*mut super::Dxgi::Common::DXGI_FORMAT>, offset: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).IAGetIndexBuffer)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pindexbuffer.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(format.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(offset.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn GSGetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&mut [::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).GSGetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn GSGetShader(&self, ppgeometryshader: ::core::option::Option<*mut ::core::option::Option<ID3D11GeometryShader>>, ppclassinstances: ::core::option::Option<*mut ::core::option::Option<ID3D11ClassInstance>>, pnumclassinstances: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).GSGetShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppgeometryshader.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(ppclassinstances.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pnumclassinstances.unwrap_or(::std::ptr::null_mut())))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Direct3D\"`*"]
    #[cfg(feature = "Win32_Graphics_Direct3D")]
    pub unsafe fn IAGetPrimitiveTopology(&self, ptopology: *mut super::Direct3D::D3D_PRIMITIVE_TOPOLOGY) {
        (::windows::core::Vtable::vtable(self).IAGetPrimitiveTopology)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ptopology))
    }
    pub unsafe fn VSGetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&mut [::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).VSGetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn VSGetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&mut [::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).VSGetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetPredication(&self, pppredicate: ::core::option::Option<*mut ::core::option::Option<ID3D11Predicate>>, ppredicatevalue: ::core::option::Option<*mut super::super::Foundation::BOOL>) {
        (::windows::core::Vtable::vtable(self).GetPredication)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pppredicate.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(ppredicatevalue.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn GSGetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&mut [::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).GSGetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn GSGetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&mut [::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).GSGetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn OMGetRenderTargets(&self, pprendertargetviews: ::core::option::Option<&mut [::core::option::Option<ID3D11RenderTargetView>]>, ppdepthstencilview: ::core::option::Option<*mut ::core::option::Option<ID3D11DepthStencilView>>) {
        (::windows::core::Vtable::vtable(self).OMGetRenderTargets)(::windows::core::Vtable::as_raw(self), pprendertargetviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(pprendertargetviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), ::core::mem::transmute(ppdepthstencilview.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn OMGetRenderTargetsAndUnorderedAccessViews(&self, pprendertargetviews: ::core::option::Option<&mut [::core::option::Option<ID3D11RenderTargetView>]>, ppdepthstencilview: ::core::option::Option<*mut ::core::option::Option<ID3D11DepthStencilView>>, uavstartslot: u32, ppunorderedaccessviews: ::core::option::Option<&mut [::core::option::Option<ID3D11UnorderedAccessView>]>) {
        (::windows::core::Vtable::vtable(self).OMGetRenderTargetsAndUnorderedAccessViews)(
            ::windows::core::Vtable::as_raw(self),
            pprendertargetviews.as_deref().map_or(0, |slice| slice.len() as _),
            ::core::mem::transmute(pprendertargetviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())),
            ::core::mem::transmute(ppdepthstencilview.unwrap_or(::std::ptr::null_mut())),
            uavstartslot,
            ppunorderedaccessviews.as_deref().map_or(0, |slice| slice.len() as _),
            ::core::mem::transmute(ppunorderedaccessviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())),
        )
    }
    pub unsafe fn OMGetBlendState(&self, ppblendstate: ::core::option::Option<*mut ::core::option::Option<ID3D11BlendState>>, blendfactor: ::core::option::Option<*mut f32>, psamplemask: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).OMGetBlendState)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppblendstate.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(blendfactor.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(psamplemask.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn OMGetDepthStencilState(&self, ppdepthstencilstate: ::core::option::Option<*mut ::core::option::Option<ID3D11DepthStencilState>>, pstencilref: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).OMGetDepthStencilState)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdepthstencilstate.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pstencilref.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn SOGetTargets(&self, ppsotargets: ::core::option::Option<&mut [::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).SOGetTargets)(::windows::core::Vtable::as_raw(self), ppsotargets.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsotargets.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn RSGetState(&self, pprasterizerstate: ::core::option::Option<*mut ::core::option::Option<ID3D11RasterizerState>>) {
        (::windows::core::Vtable::vtable(self).RSGetState)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pprasterizerstate.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn RSGetViewports(&self, pnumviewports: *mut u32, pviewports: ::core::option::Option<*mut D3D11_VIEWPORT>) {
        (::windows::core::Vtable::vtable(self).RSGetViewports)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pnumviewports), ::core::mem::transmute(pviewports.unwrap_or(::std::ptr::null_mut())))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn RSGetScissorRects(&self, pnumrects: *mut u32, prects: ::core::option::Option<*mut super::super::Foundation::RECT>) {
        (::windows::core::Vtable::vtable(self).RSGetScissorRects)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pnumrects), ::core::mem::transmute(prects.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn HSGetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&mut [::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).HSGetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn HSGetShader(&self, pphullshader: ::core::option::Option<*mut ::core::option::Option<ID3D11HullShader>>, ppclassinstances: ::core::option::Option<*mut ::core::option::Option<ID3D11ClassInstance>>, pnumclassinstances: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).HSGetShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pphullshader.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(ppclassinstances.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pnumclassinstances.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn HSGetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&mut [::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).HSGetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn HSGetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&mut [::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).HSGetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn DSGetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&mut [::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).DSGetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn DSGetShader(&self, ppdomainshader: ::core::option::Option<*mut ::core::option::Option<ID3D11DomainShader>>, ppclassinstances: ::core::option::Option<*mut ::core::option::Option<ID3D11ClassInstance>>, pnumclassinstances: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).DSGetShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdomainshader.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(ppclassinstances.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pnumclassinstances.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn DSGetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&mut [::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).DSGetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn DSGetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&mut [::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).DSGetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn CSGetShaderResources(&self, startslot: u32, ppshaderresourceviews: ::core::option::Option<&mut [::core::option::Option<ID3D11ShaderResourceView>]>) {
        (::windows::core::Vtable::vtable(self).CSGetShaderResources)(::windows::core::Vtable::as_raw(self), startslot, ppshaderresourceviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppshaderresourceviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn CSGetUnorderedAccessViews(&self, startslot: u32, ppunorderedaccessviews: ::core::option::Option<&mut [::core::option::Option<ID3D11UnorderedAccessView>]>) {
        (::windows::core::Vtable::vtable(self).CSGetUnorderedAccessViews)(::windows::core::Vtable::as_raw(self), startslot, ppunorderedaccessviews.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppunorderedaccessviews.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn CSGetShader(&self, ppcomputeshader: ::core::option::Option<*mut ::core::option::Option<ID3D11ComputeShader>>, ppclassinstances: ::core::option::Option<*mut ::core::option::Option<ID3D11ClassInstance>>, pnumclassinstances: ::core::option::Option<*mut u32>) {
        (::windows::core::Vtable::vtable(self).CSGetShader)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppcomputeshader.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(ppclassinstances.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pnumclassinstances.unwrap_or(::std::ptr::null_mut())))
    }
    pub unsafe fn CSGetSamplers(&self, startslot: u32, ppsamplers: ::core::option::Option<&mut [::core::option::Option<ID3D11SamplerState>]>) {
        (::windows::core::Vtable::vtable(self).CSGetSamplers)(::windows::core::Vtable::as_raw(self), startslot, ppsamplers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppsamplers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn CSGetConstantBuffers(&self, startslot: u32, ppconstantbuffers: ::core::option::Option<&mut [::core::option::Option<ID3D11Buffer>]>) {
        (::windows::core::Vtable::vtable(self).CSGetConstantBuffers)(::windows::core::Vtable::as_raw(self), startslot, ppconstantbuffers.as_deref().map_or(0, |slice| slice.len() as _), ::core::mem::transmute(ppconstantbuffers.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())))
    }
    pub unsafe fn ClearState(&self) {
        (::windows::core::Vtable::vtable(self).ClearState)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn Flush(&self) {
        (::windows::core::Vtable::vtable(self).Flush)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetType(&self) -> D3D11_DEVICE_CONTEXT_TYPE {
        (::windows::core::Vtable::vtable(self).GetType)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetContextFlags(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).GetContextFlags)(::windows::core::Vtable::as_raw(self))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn FinishCommandList<'a, P0>(&self, restoredeferredcontextstate: P0) -> ::windows::core::Result<ID3D11CommandList>
    where
        P0: ::std::convert::Into<super::super::Foundation::BOOL>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).FinishCommandList)(::windows::core::Vtable::as_raw(self), restoredeferredcontextstate.into(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<ID3D11CommandList>(result__)
    }
}
impl ::core::cmp::Eq for ID3D11DeviceContext {}
impl ::core::cmp::PartialEq for ID3D11DeviceContext {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11DeviceContext {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11DeviceContext {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11DeviceContext").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11DeviceContext {}
unsafe impl ::core::marker::Sync for ID3D11DeviceContext {}
unsafe impl ::windows::core::Vtable for ID3D11DeviceContext {
    type Vtable = ID3D11DeviceContext_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11DeviceContext {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xc0bfa96c_e089_44fb_8eaf_26f8796190da);
}

#[repr(C)]
pub struct ID3D11DeviceContext_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    pub VSSetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void),
    pub PSSetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void),
    pub PSSetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppixelshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32),
    pub PSSetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void),
    pub VSSetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pvertexshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32),
    pub DrawIndexed: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, indexcount: u32, startindexlocation: u32, basevertexlocation: i32),
    pub Draw: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, vertexcount: u32, startvertexlocation: u32),
    pub Map: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, subresource: u32, maptype: D3D11_MAP, mapflags: u32, pmappedresource: *mut D3D11_MAPPED_SUBRESOURCE) -> ::windows::core::HRESULT,
    pub Unmap: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, subresource: u32),
    pub PSSetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void),
    pub IASetInputLayout: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pinputlayout: *mut ::core::ffi::c_void),
    pub IASetVertexBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppvertexbuffers: *const *mut ::core::ffi::c_void, pstrides: *const u32, poffsets: *const u32),
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub IASetIndexBuffer: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pindexbuffer: *mut ::core::ffi::c_void, format: super::Dxgi::Common::DXGI_FORMAT, offset: u32),
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    IASetIndexBuffer: usize,
    pub DrawIndexedInstanced: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, indexcountperinstance: u32, instancecount: u32, startindexlocation: u32, basevertexlocation: i32, startinstancelocation: u32),
    pub DrawInstanced: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, vertexcountperinstance: u32, instancecount: u32, startvertexlocation: u32, startinstancelocation: u32),
    pub GSSetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void),
    pub GSSetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32),
    #[cfg(feature = "Win32_Graphics_Direct3D")]
    pub IASetPrimitiveTopology: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, topology: super::Direct3D::D3D_PRIMITIVE_TOPOLOGY),
    #[cfg(not(feature = "Win32_Graphics_Direct3D"))]
    IASetPrimitiveTopology: usize,
    pub VSSetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void),
    pub VSSetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void),
    pub Begin: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pasync: *mut ::core::ffi::c_void),
    pub End: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pasync: *mut ::core::ffi::c_void),
    pub GetData: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pasync: *mut ::core::ffi::c_void, pdata: *mut ::core::ffi::c_void, datasize: u32, getdataflags: u32) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Foundation")]
    pub SetPredication: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppredicate: *mut ::core::ffi::c_void, predicatevalue: super::super::Foundation::BOOL),
    #[cfg(not(feature = "Win32_Foundation"))]
    SetPredication: usize,
    pub GSSetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void),
    pub GSSetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void),
    pub OMSetRenderTargets: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, numviews: u32, pprendertargetviews: *const *mut ::core::ffi::c_void, pdepthstencilview: *mut ::core::ffi::c_void),
    pub OMSetRenderTargetsAndUnorderedAccessViews: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, numrtvs: u32, pprendertargetviews: *const *mut ::core::ffi::c_void, pdepthstencilview: *mut ::core::ffi::c_void, uavstartslot: u32, numuavs: u32, ppunorderedaccessviews: *const *mut ::core::ffi::c_void, puavinitialcounts: *const u32),
    pub OMSetBlendState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pblendstate: *mut ::core::ffi::c_void, blendfactor: *const f32, samplemask: u32),
    pub OMSetDepthStencilState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdepthstencilstate: *mut ::core::ffi::c_void, stencilref: u32),
    pub SOSetTargets: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, numbuffers: u32, ppsotargets: *const *mut ::core::ffi::c_void, poffsets: *const u32),
    pub DrawAuto: unsafe extern "system" fn(this: *mut ::core::ffi::c_void),
    pub DrawIndexedInstancedIndirect: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pbufferforargs: *mut ::core::ffi::c_void, alignedbyteoffsetforargs: u32),
    pub DrawInstancedIndirect: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pbufferforargs: *mut ::core::ffi::c_void, alignedbyteoffsetforargs: u32),
    pub Dispatch: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, threadgroupcountx: u32, threadgroupcounty: u32, threadgroupcountz: u32),
    pub DispatchIndirect: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pbufferforargs: *mut ::core::ffi::c_void, alignedbyteoffsetforargs: u32),
    pub RSSetState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, prasterizerstate: *mut ::core::ffi::c_void),
    pub RSSetViewports: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, numviewports: u32, pviewports: *const D3D11_VIEWPORT),
    #[cfg(feature = "Win32_Foundation")]
    pub RSSetScissorRects: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, numrects: u32, prects: *const super::super::Foundation::RECT),
    #[cfg(not(feature = "Win32_Foundation"))]
    RSSetScissorRects: usize,
    pub CopySubresourceRegion: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdstresource: *mut ::core::ffi::c_void, dstsubresource: u32, dstx: u32, dsty: u32, dstz: u32, psrcresource: *mut ::core::ffi::c_void, srcsubresource: u32, psrcbox: *const D3D11_BOX),
    pub CopyResource: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdstresource: *mut ::core::ffi::c_void, psrcresource: *mut ::core::ffi::c_void),
    pub UpdateSubresource: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdstresource: *mut ::core::ffi::c_void, dstsubresource: u32, pdstbox: *const D3D11_BOX, psrcdata: *const ::core::ffi::c_void, srcrowpitch: u32, srcdepthpitch: u32),
    pub CopyStructureCount: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdstbuffer: *mut ::core::ffi::c_void, dstalignedbyteoffset: u32, psrcview: *mut ::core::ffi::c_void),
    pub ClearRenderTargetView: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, prendertargetview: *mut ::core::ffi::c_void, colorrgba: *const f32),
    pub ClearUnorderedAccessViewUint: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, punorderedaccessview: *mut ::core::ffi::c_void, values: *const u32),
    pub ClearUnorderedAccessViewFloat: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, punorderedaccessview: *mut ::core::ffi::c_void, values: *const f32),
    pub ClearDepthStencilView: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdepthstencilview: *mut ::core::ffi::c_void, clearflags: u32, depth: f32, stencil: u8),
    pub GenerateMips: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pshaderresourceview: *mut ::core::ffi::c_void),
    pub SetResourceMinLOD: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, minlod: f32),
    pub GetResourceMinLOD: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void) -> f32,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub ResolveSubresource: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdstresource: *mut ::core::ffi::c_void, dstsubresource: u32, psrcresource: *mut ::core::ffi::c_void, srcsubresource: u32, format: super::Dxgi::Common::DXGI_FORMAT),
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    ResolveSubresource: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub ExecuteCommandList: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pcommandlist: *mut ::core::ffi::c_void, restorecontextstate: super::super::Foundation::BOOL),
    #[cfg(not(feature = "Win32_Foundation"))]
    ExecuteCommandList: usize,
    pub HSSetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void),
    pub HSSetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, phullshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32),
    pub HSSetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void),
    pub HSSetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void),
    pub DSSetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void),
    pub DSSetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdomainshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32),
    pub DSSetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void),
    pub DSSetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void),
    pub CSSetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void),
    pub CSSetUnorderedAccessViews: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numuavs: u32, ppunorderedaccessviews: *const *mut ::core::ffi::c_void, puavinitialcounts: *const u32),
    pub CSSetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pcomputeshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32),
    pub CSSetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void),
    pub CSSetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void),
    pub VSGetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void),
    pub PSGetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void),
    pub PSGetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pppixelshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32),
    pub PSGetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void),
    pub VSGetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppvertexshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32),
    pub PSGetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void),
    pub IAGetInputLayout: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppinputlayout: *mut *mut ::core::ffi::c_void),
    pub IAGetVertexBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppvertexbuffers: *mut *mut ::core::ffi::c_void, pstrides: *mut u32, poffsets: *mut u32),
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub IAGetIndexBuffer: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pindexbuffer: *mut *mut ::core::ffi::c_void, format: *mut super::Dxgi::Common::DXGI_FORMAT, offset: *mut u32),
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    IAGetIndexBuffer: usize,
    pub GSGetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void),
    pub GSGetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppgeometryshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32),
    #[cfg(feature = "Win32_Graphics_Direct3D")]
    pub IAGetPrimitiveTopology: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ptopology: *mut super::Direct3D::D3D_PRIMITIVE_TOPOLOGY),
    #[cfg(not(feature = "Win32_Graphics_Direct3D"))]
    IAGetPrimitiveTopology: usize,
    pub VSGetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void),
    pub VSGetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void),
    #[cfg(feature = "Win32_Foundation")]
    pub GetPredication: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pppredicate: *mut *mut ::core::ffi::c_void, ppredicatevalue: *mut super::super::Foundation::BOOL),
    #[cfg(not(feature = "Win32_Foundation"))]
    GetPredication: usize,
    pub GSGetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void),
    pub GSGetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void),
    pub OMGetRenderTargets: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, numviews: u32, pprendertargetviews: *mut *mut ::core::ffi::c_void, ppdepthstencilview: *mut *mut ::core::ffi::c_void),
    pub OMGetRenderTargetsAndUnorderedAccessViews: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, numrtvs: u32, pprendertargetviews: *mut *mut ::core::ffi::c_void, ppdepthstencilview: *mut *mut ::core::ffi::c_void, uavstartslot: u32, numuavs: u32, ppunorderedaccessviews: *mut *mut ::core::ffi::c_void),
    pub OMGetBlendState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppblendstate: *mut *mut ::core::ffi::c_void, blendfactor: *mut f32, psamplemask: *mut u32),
    pub OMGetDepthStencilState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppdepthstencilstate: *mut *mut ::core::ffi::c_void, pstencilref: *mut u32),
    pub SOGetTargets: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, numbuffers: u32, ppsotargets: *mut *mut ::core::ffi::c_void),
    pub RSGetState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pprasterizerstate: *mut *mut ::core::ffi::c_void),
    pub RSGetViewports: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pnumviewports: *mut u32, pviewports: *mut D3D11_VIEWPORT),
    #[cfg(feature = "Win32_Foundation")]
    pub RSGetScissorRects: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pnumrects: *mut u32, prects: *mut super::super::Foundation::RECT),
    #[cfg(not(feature = "Win32_Foundation"))]
    RSGetScissorRects: usize,
    pub HSGetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void),
    pub HSGetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pphullshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32),
    pub HSGetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void),
    pub HSGetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void),
    pub DSGetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void),
    pub DSGetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppdomainshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32),
    pub DSGetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void),
    pub DSGetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void),
    pub CSGetShaderResources: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void),
    pub CSGetUnorderedAccessViews: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numuavs: u32, ppunorderedaccessviews: *mut *mut ::core::ffi::c_void),
    pub CSGetShader: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppcomputeshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32),
    pub CSGetSamplers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void),
    pub CSGetConstantBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void),
    pub ClearState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void),
    pub Flush: unsafe extern "system" fn(this: *mut ::core::ffi::c_void),
    pub GetType: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> D3D11_DEVICE_CONTEXT_TYPE,
    pub GetContextFlags: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> u32,
    #[cfg(feature = "Win32_Foundation")]
    pub FinishCommandList: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, restoredeferredcontextstate: super::super::Foundation::BOOL, ppcommandlist: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    FinishCommandList: usize,
}

pub trait ID3D11DeviceContext_Impl: Sized + ID3D11DeviceChild_Impl {
    fn VSSetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *const ::core::option::Option<ID3D11Buffer>);
    fn PSSetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *const ::core::option::Option<ID3D11ShaderResourceView>);
    fn PSSetShader(&self, ppixelshader: &::core::option::Option<ID3D11PixelShader>, ppclassinstances: *const ::core::option::Option<ID3D11ClassInstance>, numclassinstances: u32);
    fn PSSetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *const ::core::option::Option<ID3D11SamplerState>);
    fn VSSetShader(&self, pvertexshader: &::core::option::Option<ID3D11VertexShader>, ppclassinstances: *const ::core::option::Option<ID3D11ClassInstance>, numclassinstances: u32);
    fn DrawIndexed(&self, indexcount: u32, startindexlocation: u32, basevertexlocation: i32);
    fn Draw(&self, vertexcount: u32, startvertexlocation: u32);
    fn Map(&self, presource: &::core::option::Option<ID3D11Resource>, subresource: u32, maptype: D3D11_MAP, mapflags: u32) -> ::windows::core::Result<D3D11_MAPPED_SUBRESOURCE>;
    fn Unmap(&self, presource: &::core::option::Option<ID3D11Resource>, subresource: u32);
    fn PSSetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *const ::core::option::Option<ID3D11Buffer>);
    fn IASetInputLayout(&self, pinputlayout: &::core::option::Option<ID3D11InputLayout>);
    fn IASetVertexBuffers(&self, startslot: u32, numbuffers: u32, ppvertexbuffers: *const ::core::option::Option<ID3D11Buffer>, pstrides: *const u32, poffsets: *const u32);
    fn IASetIndexBuffer(&self, pindexbuffer: &::core::option::Option<ID3D11Buffer>, format: super::Dxgi::Common::DXGI_FORMAT, offset: u32);
    fn DrawIndexedInstanced(&self, indexcountperinstance: u32, instancecount: u32, startindexlocation: u32, basevertexlocation: i32, startinstancelocation: u32);
    fn DrawInstanced(&self, vertexcountperinstance: u32, instancecount: u32, startvertexlocation: u32, startinstancelocation: u32);
    fn GSSetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *const ::core::option::Option<ID3D11Buffer>);
    fn GSSetShader(&self, pshader: &::core::option::Option<ID3D11GeometryShader>, ppclassinstances: *const ::core::option::Option<ID3D11ClassInstance>, numclassinstances: u32);
    fn IASetPrimitiveTopology(&self, topology: super::Direct3D::D3D_PRIMITIVE_TOPOLOGY);
    fn VSSetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *const ::core::option::Option<ID3D11ShaderResourceView>);
    fn VSSetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *const ::core::option::Option<ID3D11SamplerState>);
    fn Begin(&self, pasync: &::core::option::Option<ID3D11Asynchronous>);
    fn End(&self, pasync: &::core::option::Option<ID3D11Asynchronous>);
    fn GetData(&self, pasync: &::core::option::Option<ID3D11Asynchronous>, pdata: *mut ::core::ffi::c_void, datasize: u32, getdataflags: u32) -> ::windows::core::Result<()>;
    fn SetPredication(&self, ppredicate: &::core::option::Option<ID3D11Predicate>, predicatevalue: super::super::Foundation::BOOL);
    fn GSSetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *const ::core::option::Option<ID3D11ShaderResourceView>);
    fn GSSetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *const ::core::option::Option<ID3D11SamplerState>);
    fn OMSetRenderTargets(&self, numviews: u32, pprendertargetviews: *const ::core::option::Option<ID3D11RenderTargetView>, pdepthstencilview: &::core::option::Option<ID3D11DepthStencilView>);
    fn OMSetRenderTargetsAndUnorderedAccessViews(&self, numrtvs: u32, pprendertargetviews: *const ::core::option::Option<ID3D11RenderTargetView>, pdepthstencilview: &::core::option::Option<ID3D11DepthStencilView>, uavstartslot: u32, numuavs: u32, ppunorderedaccessviews: *const ::core::option::Option<ID3D11UnorderedAccessView>, puavinitialcounts: *const u32);
    fn OMSetBlendState(&self, pblendstate: &::core::option::Option<ID3D11BlendState>, blendfactor: *const f32, samplemask: u32);
    fn OMSetDepthStencilState(&self, pdepthstencilstate: &::core::option::Option<ID3D11DepthStencilState>, stencilref: u32);
    fn SOSetTargets(&self, numbuffers: u32, ppsotargets: *const ::core::option::Option<ID3D11Buffer>, poffsets: *const u32);
    fn DrawAuto(&self);
    fn DrawIndexedInstancedIndirect(&self, pbufferforargs: &::core::option::Option<ID3D11Buffer>, alignedbyteoffsetforargs: u32);
    fn DrawInstancedIndirect(&self, pbufferforargs: &::core::option::Option<ID3D11Buffer>, alignedbyteoffsetforargs: u32);
    fn Dispatch(&self, threadgroupcountx: u32, threadgroupcounty: u32, threadgroupcountz: u32);
    fn DispatchIndirect(&self, pbufferforargs: &::core::option::Option<ID3D11Buffer>, alignedbyteoffsetforargs: u32);
    fn RSSetState(&self, prasterizerstate: &::core::option::Option<ID3D11RasterizerState>);
    fn RSSetViewports(&self, numviewports: u32, pviewports: *const D3D11_VIEWPORT);
    fn RSSetScissorRects(&self, numrects: u32, prects: *const super::super::Foundation::RECT);
    fn CopySubresourceRegion(&self, pdstresource: &::core::option::Option<ID3D11Resource>, dstsubresource: u32, dstx: u32, dsty: u32, dstz: u32, psrcresource: &::core::option::Option<ID3D11Resource>, srcsubresource: u32, psrcbox: *const D3D11_BOX);
    fn CopyResource(&self, pdstresource: &::core::option::Option<ID3D11Resource>, psrcresource: &::core::option::Option<ID3D11Resource>);
    fn UpdateSubresource(&self, pdstresource: &::core::option::Option<ID3D11Resource>, dstsubresource: u32, pdstbox: *const D3D11_BOX, psrcdata: *const ::core::ffi::c_void, srcrowpitch: u32, srcdepthpitch: u32);
    fn CopyStructureCount(&self, pdstbuffer: &::core::option::Option<ID3D11Buffer>, dstalignedbyteoffset: u32, psrcview: &::core::option::Option<ID3D11UnorderedAccessView>);
    fn ClearRenderTargetView(&self, prendertargetview: &::core::option::Option<ID3D11RenderTargetView>, colorrgba: *const f32);
    fn ClearUnorderedAccessViewUint(&self, punorderedaccessview: &::core::option::Option<ID3D11UnorderedAccessView>, values: *const u32);
    fn ClearUnorderedAccessViewFloat(&self, punorderedaccessview: &::core::option::Option<ID3D11UnorderedAccessView>, values: *const f32);
    fn ClearDepthStencilView(&self, pdepthstencilview: &::core::option::Option<ID3D11DepthStencilView>, clearflags: u32, depth: f32, stencil: u8);
    fn GenerateMips(&self, pshaderresourceview: &::core::option::Option<ID3D11ShaderResourceView>);
    fn SetResourceMinLOD(&self, presource: &::core::option::Option<ID3D11Resource>, minlod: f32);
    fn GetResourceMinLOD(&self, presource: &::core::option::Option<ID3D11Resource>) -> f32;
    fn ResolveSubresource(&self, pdstresource: &::core::option::Option<ID3D11Resource>, dstsubresource: u32, psrcresource: &::core::option::Option<ID3D11Resource>, srcsubresource: u32, format: super::Dxgi::Common::DXGI_FORMAT);
    fn ExecuteCommandList(&self, pcommandlist: &::core::option::Option<ID3D11CommandList>, restorecontextstate: super::super::Foundation::BOOL);
    fn HSSetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *const ::core::option::Option<ID3D11ShaderResourceView>);
    fn HSSetShader(&self, phullshader: &::core::option::Option<ID3D11HullShader>, ppclassinstances: *const ::core::option::Option<ID3D11ClassInstance>, numclassinstances: u32);
    fn HSSetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *const ::core::option::Option<ID3D11SamplerState>);
    fn HSSetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *const ::core::option::Option<ID3D11Buffer>);
    fn DSSetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *const ::core::option::Option<ID3D11ShaderResourceView>);
    fn DSSetShader(&self, pdomainshader: &::core::option::Option<ID3D11DomainShader>, ppclassinstances: *const ::core::option::Option<ID3D11ClassInstance>, numclassinstances: u32);
    fn DSSetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *const ::core::option::Option<ID3D11SamplerState>);
    fn DSSetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *const ::core::option::Option<ID3D11Buffer>);
    fn CSSetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *const ::core::option::Option<ID3D11ShaderResourceView>);
    fn CSSetUnorderedAccessViews(&self, startslot: u32, numuavs: u32, ppunorderedaccessviews: *const ::core::option::Option<ID3D11UnorderedAccessView>, puavinitialcounts: *const u32);
    fn CSSetShader(&self, pcomputeshader: &::core::option::Option<ID3D11ComputeShader>, ppclassinstances: *const ::core::option::Option<ID3D11ClassInstance>, numclassinstances: u32);
    fn CSSetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *const ::core::option::Option<ID3D11SamplerState>);
    fn CSSetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *const ::core::option::Option<ID3D11Buffer>);
    fn VSGetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut ::core::option::Option<ID3D11Buffer>);
    fn PSGetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *mut ::core::option::Option<ID3D11ShaderResourceView>);
    fn PSGetShader(&self, pppixelshader: *mut ::core::option::Option<ID3D11PixelShader>, ppclassinstances: *mut ::core::option::Option<ID3D11ClassInstance>, pnumclassinstances: *mut u32);
    fn PSGetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *mut ::core::option::Option<ID3D11SamplerState>);
    fn VSGetShader(&self, ppvertexshader: *mut ::core::option::Option<ID3D11VertexShader>, ppclassinstances: *mut ::core::option::Option<ID3D11ClassInstance>, pnumclassinstances: *mut u32);
    fn PSGetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut ::core::option::Option<ID3D11Buffer>);
    fn IAGetInputLayout(&self, ppinputlayout: *mut ::core::option::Option<ID3D11InputLayout>);
    fn IAGetVertexBuffers(&self, startslot: u32, numbuffers: u32, ppvertexbuffers: *mut ::core::option::Option<ID3D11Buffer>, pstrides: *mut u32, poffsets: *mut u32);
    fn IAGetIndexBuffer(&self, pindexbuffer: *mut ::core::option::Option<ID3D11Buffer>, format: *mut super::Dxgi::Common::DXGI_FORMAT, offset: *mut u32);
    fn GSGetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut ::core::option::Option<ID3D11Buffer>);
    fn GSGetShader(&self, ppgeometryshader: *mut ::core::option::Option<ID3D11GeometryShader>, ppclassinstances: *mut ::core::option::Option<ID3D11ClassInstance>, pnumclassinstances: *mut u32);
    fn IAGetPrimitiveTopology(&self, ptopology: *mut super::Direct3D::D3D_PRIMITIVE_TOPOLOGY);
    fn VSGetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *mut ::core::option::Option<ID3D11ShaderResourceView>);
    fn VSGetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *mut ::core::option::Option<ID3D11SamplerState>);
    fn GetPredication(&self, pppredicate: *mut ::core::option::Option<ID3D11Predicate>, ppredicatevalue: *mut super::super::Foundation::BOOL);
    fn GSGetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *mut ::core::option::Option<ID3D11ShaderResourceView>);
    fn GSGetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *mut ::core::option::Option<ID3D11SamplerState>);
    fn OMGetRenderTargets(&self, numviews: u32, pprendertargetviews: *mut ::core::option::Option<ID3D11RenderTargetView>, ppdepthstencilview: *mut ::core::option::Option<ID3D11DepthStencilView>);
    fn OMGetRenderTargetsAndUnorderedAccessViews(&self, numrtvs: u32, pprendertargetviews: *mut ::core::option::Option<ID3D11RenderTargetView>, ppdepthstencilview: *mut ::core::option::Option<ID3D11DepthStencilView>, uavstartslot: u32, numuavs: u32, ppunorderedaccessviews: *mut ::core::option::Option<ID3D11UnorderedAccessView>);
    fn OMGetBlendState(&self, ppblendstate: *mut ::core::option::Option<ID3D11BlendState>, blendfactor: *mut f32, psamplemask: *mut u32);
    fn OMGetDepthStencilState(&self, ppdepthstencilstate: *mut ::core::option::Option<ID3D11DepthStencilState>, pstencilref: *mut u32);
    fn SOGetTargets(&self, numbuffers: u32, ppsotargets: *mut ::core::option::Option<ID3D11Buffer>);
    fn RSGetState(&self, pprasterizerstate: *mut ::core::option::Option<ID3D11RasterizerState>);
    fn RSGetViewports(&self, pnumviewports: *mut u32, pviewports: *mut D3D11_VIEWPORT);
    fn RSGetScissorRects(&self, pnumrects: *mut u32, prects: *mut super::super::Foundation::RECT);
    fn HSGetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *mut ::core::option::Option<ID3D11ShaderResourceView>);
    fn HSGetShader(&self, pphullshader: *mut ::core::option::Option<ID3D11HullShader>, ppclassinstances: *mut ::core::option::Option<ID3D11ClassInstance>, pnumclassinstances: *mut u32);
    fn HSGetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *mut ::core::option::Option<ID3D11SamplerState>);
    fn HSGetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut ::core::option::Option<ID3D11Buffer>);
    fn DSGetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *mut ::core::option::Option<ID3D11ShaderResourceView>);
    fn DSGetShader(&self, ppdomainshader: *mut ::core::option::Option<ID3D11DomainShader>, ppclassinstances: *mut ::core::option::Option<ID3D11ClassInstance>, pnumclassinstances: *mut u32);
    fn DSGetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *mut ::core::option::Option<ID3D11SamplerState>);
    fn DSGetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut ::core::option::Option<ID3D11Buffer>);
    fn CSGetShaderResources(&self, startslot: u32, numviews: u32, ppshaderresourceviews: *mut ::core::option::Option<ID3D11ShaderResourceView>);
    fn CSGetUnorderedAccessViews(&self, startslot: u32, numuavs: u32, ppunorderedaccessviews: *mut ::core::option::Option<ID3D11UnorderedAccessView>);
    fn CSGetShader(&self, ppcomputeshader: *mut ::core::option::Option<ID3D11ComputeShader>, ppclassinstances: *mut ::core::option::Option<ID3D11ClassInstance>, pnumclassinstances: *mut u32);
    fn CSGetSamplers(&self, startslot: u32, numsamplers: u32, ppsamplers: *mut ::core::option::Option<ID3D11SamplerState>);
    fn CSGetConstantBuffers(&self, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut ::core::option::Option<ID3D11Buffer>);
    fn ClearState(&self);
    fn Flush(&self);
    fn GetType(&self) -> D3D11_DEVICE_CONTEXT_TYPE;
    fn GetContextFlags(&self) -> u32;
    fn FinishCommandList(&self, restoredeferredcontextstate: super::super::Foundation::BOOL) -> ::windows::core::Result<ID3D11CommandList>;
}

impl ID3D11DeviceContext_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>() -> ID3D11DeviceContext_Vtbl {
        unsafe extern "system" fn VSSetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.VSSetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn PSSetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.PSSetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn PSSetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppixelshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.PSSetShader(::core::mem::transmute(&ppixelshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&numclassinstances))
        }
        unsafe extern "system" fn PSSetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.PSSetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn VSSetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pvertexshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.VSSetShader(::core::mem::transmute(&pvertexshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&numclassinstances))
        }
        unsafe extern "system" fn DrawIndexed<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, indexcount: u32, startindexlocation: u32, basevertexlocation: i32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DrawIndexed(::core::mem::transmute_copy(&indexcount), ::core::mem::transmute_copy(&startindexlocation), ::core::mem::transmute_copy(&basevertexlocation))
        }
        unsafe extern "system" fn Draw<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, vertexcount: u32, startvertexlocation: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Draw(::core::mem::transmute_copy(&vertexcount), ::core::mem::transmute_copy(&startvertexlocation))
        }
        unsafe extern "system" fn Map<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, subresource: u32, maptype: D3D11_MAP, mapflags: u32, pmappedresource: *mut D3D11_MAPPED_SUBRESOURCE) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.Map(::core::mem::transmute(&presource), ::core::mem::transmute_copy(&subresource), ::core::mem::transmute_copy(&maptype), ::core::mem::transmute_copy(&mapflags)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pmappedresource, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn Unmap<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, subresource: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Unmap(::core::mem::transmute(&presource), ::core::mem::transmute_copy(&subresource))
        }
        unsafe extern "system" fn PSSetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.PSSetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn IASetInputLayout<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pinputlayout: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IASetInputLayout(::core::mem::transmute(&pinputlayout))
        }
        unsafe extern "system" fn IASetVertexBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppvertexbuffers: *const *mut ::core::ffi::c_void, pstrides: *const u32, poffsets: *const u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IASetVertexBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppvertexbuffers), ::core::mem::transmute_copy(&pstrides), ::core::mem::transmute_copy(&poffsets))
        }
        unsafe extern "system" fn IASetIndexBuffer<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pindexbuffer: *mut ::core::ffi::c_void, format: super::Dxgi::Common::DXGI_FORMAT, offset: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IASetIndexBuffer(::core::mem::transmute(&pindexbuffer), ::core::mem::transmute_copy(&format), ::core::mem::transmute_copy(&offset))
        }
        unsafe extern "system" fn DrawIndexedInstanced<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, indexcountperinstance: u32, instancecount: u32, startindexlocation: u32, basevertexlocation: i32, startinstancelocation: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DrawIndexedInstanced(::core::mem::transmute_copy(&indexcountperinstance), ::core::mem::transmute_copy(&instancecount), ::core::mem::transmute_copy(&startindexlocation), ::core::mem::transmute_copy(&basevertexlocation), ::core::mem::transmute_copy(&startinstancelocation))
        }
        unsafe extern "system" fn DrawInstanced<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, vertexcountperinstance: u32, instancecount: u32, startvertexlocation: u32, startinstancelocation: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DrawInstanced(::core::mem::transmute_copy(&vertexcountperinstance), ::core::mem::transmute_copy(&instancecount), ::core::mem::transmute_copy(&startvertexlocation), ::core::mem::transmute_copy(&startinstancelocation))
        }
        unsafe extern "system" fn GSSetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GSSetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn GSSetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GSSetShader(::core::mem::transmute(&pshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&numclassinstances))
        }
        unsafe extern "system" fn IASetPrimitiveTopology<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, topology: super::Direct3D::D3D_PRIMITIVE_TOPOLOGY) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IASetPrimitiveTopology(::core::mem::transmute_copy(&topology))
        }
        unsafe extern "system" fn VSSetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.VSSetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn VSSetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.VSSetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn Begin<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pasync: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Begin(::core::mem::transmute(&pasync))
        }
        unsafe extern "system" fn End<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pasync: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.End(::core::mem::transmute(&pasync))
        }
        unsafe extern "system" fn GetData<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pasync: *mut ::core::ffi::c_void, pdata: *mut ::core::ffi::c_void, datasize: u32, getdataflags: u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetData(::core::mem::transmute(&pasync), ::core::mem::transmute_copy(&pdata), ::core::mem::transmute_copy(&datasize), ::core::mem::transmute_copy(&getdataflags)).into()
        }
        unsafe extern "system" fn SetPredication<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppredicate: *mut ::core::ffi::c_void, predicatevalue: super::super::Foundation::BOOL) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetPredication(::core::mem::transmute(&ppredicate), ::core::mem::transmute_copy(&predicatevalue))
        }
        unsafe extern "system" fn GSSetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GSSetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn GSSetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GSSetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn OMSetRenderTargets<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, numviews: u32, pprendertargetviews: *const *mut ::core::ffi::c_void, pdepthstencilview: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.OMSetRenderTargets(::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&pprendertargetviews), ::core::mem::transmute(&pdepthstencilview))
        }
        unsafe extern "system" fn OMSetRenderTargetsAndUnorderedAccessViews<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, numrtvs: u32, pprendertargetviews: *const *mut ::core::ffi::c_void, pdepthstencilview: *mut ::core::ffi::c_void, uavstartslot: u32, numuavs: u32, ppunorderedaccessviews: *const *mut ::core::ffi::c_void, puavinitialcounts: *const u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.OMSetRenderTargetsAndUnorderedAccessViews(::core::mem::transmute_copy(&numrtvs), ::core::mem::transmute_copy(&pprendertargetviews), ::core::mem::transmute(&pdepthstencilview), ::core::mem::transmute_copy(&uavstartslot), ::core::mem::transmute_copy(&numuavs), ::core::mem::transmute_copy(&ppunorderedaccessviews), ::core::mem::transmute_copy(&puavinitialcounts))
        }
        unsafe extern "system" fn OMSetBlendState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pblendstate: *mut ::core::ffi::c_void, blendfactor: *const f32, samplemask: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.OMSetBlendState(::core::mem::transmute(&pblendstate), ::core::mem::transmute_copy(&blendfactor), ::core::mem::transmute_copy(&samplemask))
        }
        unsafe extern "system" fn OMSetDepthStencilState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdepthstencilstate: *mut ::core::ffi::c_void, stencilref: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.OMSetDepthStencilState(::core::mem::transmute(&pdepthstencilstate), ::core::mem::transmute_copy(&stencilref))
        }
        unsafe extern "system" fn SOSetTargets<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, numbuffers: u32, ppsotargets: *const *mut ::core::ffi::c_void, poffsets: *const u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SOSetTargets(::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppsotargets), ::core::mem::transmute_copy(&poffsets))
        }
        unsafe extern "system" fn DrawAuto<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DrawAuto()
        }
        unsafe extern "system" fn DrawIndexedInstancedIndirect<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pbufferforargs: *mut ::core::ffi::c_void, alignedbyteoffsetforargs: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DrawIndexedInstancedIndirect(::core::mem::transmute(&pbufferforargs), ::core::mem::transmute_copy(&alignedbyteoffsetforargs))
        }
        unsafe extern "system" fn DrawInstancedIndirect<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pbufferforargs: *mut ::core::ffi::c_void, alignedbyteoffsetforargs: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DrawInstancedIndirect(::core::mem::transmute(&pbufferforargs), ::core::mem::transmute_copy(&alignedbyteoffsetforargs))
        }
        unsafe extern "system" fn Dispatch<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, threadgroupcountx: u32, threadgroupcounty: u32, threadgroupcountz: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Dispatch(::core::mem::transmute_copy(&threadgroupcountx), ::core::mem::transmute_copy(&threadgroupcounty), ::core::mem::transmute_copy(&threadgroupcountz))
        }
        unsafe extern "system" fn DispatchIndirect<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pbufferforargs: *mut ::core::ffi::c_void, alignedbyteoffsetforargs: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DispatchIndirect(::core::mem::transmute(&pbufferforargs), ::core::mem::transmute_copy(&alignedbyteoffsetforargs))
        }
        unsafe extern "system" fn RSSetState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, prasterizerstate: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.RSSetState(::core::mem::transmute(&prasterizerstate))
        }
        unsafe extern "system" fn RSSetViewports<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, numviewports: u32, pviewports: *const D3D11_VIEWPORT) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.RSSetViewports(::core::mem::transmute_copy(&numviewports), ::core::mem::transmute_copy(&pviewports))
        }
        unsafe extern "system" fn RSSetScissorRects<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, numrects: u32, prects: *const super::super::Foundation::RECT) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.RSSetScissorRects(::core::mem::transmute_copy(&numrects), ::core::mem::transmute_copy(&prects))
        }
        unsafe extern "system" fn CopySubresourceRegion<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdstresource: *mut ::core::ffi::c_void, dstsubresource: u32, dstx: u32, dsty: u32, dstz: u32, psrcresource: *mut ::core::ffi::c_void, srcsubresource: u32, psrcbox: *const D3D11_BOX) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CopySubresourceRegion(::core::mem::transmute(&pdstresource), ::core::mem::transmute_copy(&dstsubresource), ::core::mem::transmute_copy(&dstx), ::core::mem::transmute_copy(&dsty), ::core::mem::transmute_copy(&dstz), ::core::mem::transmute(&psrcresource), ::core::mem::transmute_copy(&srcsubresource), ::core::mem::transmute_copy(&psrcbox))
        }
        unsafe extern "system" fn CopyResource<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdstresource: *mut ::core::ffi::c_void, psrcresource: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CopyResource(::core::mem::transmute(&pdstresource), ::core::mem::transmute(&psrcresource))
        }
        unsafe extern "system" fn UpdateSubresource<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdstresource: *mut ::core::ffi::c_void, dstsubresource: u32, pdstbox: *const D3D11_BOX, psrcdata: *const ::core::ffi::c_void, srcrowpitch: u32, srcdepthpitch: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.UpdateSubresource(::core::mem::transmute(&pdstresource), ::core::mem::transmute_copy(&dstsubresource), ::core::mem::transmute_copy(&pdstbox), ::core::mem::transmute_copy(&psrcdata), ::core::mem::transmute_copy(&srcrowpitch), ::core::mem::transmute_copy(&srcdepthpitch))
        }
        unsafe extern "system" fn CopyStructureCount<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdstbuffer: *mut ::core::ffi::c_void, dstalignedbyteoffset: u32, psrcview: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CopyStructureCount(::core::mem::transmute(&pdstbuffer), ::core::mem::transmute_copy(&dstalignedbyteoffset), ::core::mem::transmute(&psrcview))
        }
        unsafe extern "system" fn ClearRenderTargetView<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, prendertargetview: *mut ::core::ffi::c_void, colorrgba: *const f32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ClearRenderTargetView(::core::mem::transmute(&prendertargetview), ::core::mem::transmute_copy(&colorrgba))
        }
        unsafe extern "system" fn ClearUnorderedAccessViewUint<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, punorderedaccessview: *mut ::core::ffi::c_void, values: *const u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ClearUnorderedAccessViewUint(::core::mem::transmute(&punorderedaccessview), ::core::mem::transmute_copy(&values))
        }
        unsafe extern "system" fn ClearUnorderedAccessViewFloat<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, punorderedaccessview: *mut ::core::ffi::c_void, values: *const f32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ClearUnorderedAccessViewFloat(::core::mem::transmute(&punorderedaccessview), ::core::mem::transmute_copy(&values))
        }
        unsafe extern "system" fn ClearDepthStencilView<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdepthstencilview: *mut ::core::ffi::c_void, clearflags: u32, depth: f32, stencil: u8) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ClearDepthStencilView(::core::mem::transmute(&pdepthstencilview), ::core::mem::transmute_copy(&clearflags), ::core::mem::transmute_copy(&depth), ::core::mem::transmute_copy(&stencil))
        }
        unsafe extern "system" fn GenerateMips<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pshaderresourceview: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GenerateMips(::core::mem::transmute(&pshaderresourceview))
        }
        unsafe extern "system" fn SetResourceMinLOD<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void, minlod: f32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetResourceMinLOD(::core::mem::transmute(&presource), ::core::mem::transmute_copy(&minlod))
        }
        unsafe extern "system" fn GetResourceMinLOD<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, presource: *mut ::core::ffi::c_void) -> f32 {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetResourceMinLOD(::core::mem::transmute(&presource))
        }
        unsafe extern "system" fn ResolveSubresource<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdstresource: *mut ::core::ffi::c_void, dstsubresource: u32, psrcresource: *mut ::core::ffi::c_void, srcsubresource: u32, format: super::Dxgi::Common::DXGI_FORMAT) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ResolveSubresource(::core::mem::transmute(&pdstresource), ::core::mem::transmute_copy(&dstsubresource), ::core::mem::transmute(&psrcresource), ::core::mem::transmute_copy(&srcsubresource), ::core::mem::transmute_copy(&format))
        }
        unsafe extern "system" fn ExecuteCommandList<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pcommandlist: *mut ::core::ffi::c_void, restorecontextstate: super::super::Foundation::BOOL) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ExecuteCommandList(::core::mem::transmute(&pcommandlist), ::core::mem::transmute_copy(&restorecontextstate))
        }
        unsafe extern "system" fn HSSetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.HSSetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn HSSetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, phullshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.HSSetShader(::core::mem::transmute(&phullshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&numclassinstances))
        }
        unsafe extern "system" fn HSSetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.HSSetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn HSSetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.HSSetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn DSSetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DSSetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn DSSetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdomainshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DSSetShader(::core::mem::transmute(&pdomainshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&numclassinstances))
        }
        unsafe extern "system" fn DSSetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DSSetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn DSSetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DSSetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn CSSetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSSetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn CSSetUnorderedAccessViews<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numuavs: u32, ppunorderedaccessviews: *const *mut ::core::ffi::c_void, puavinitialcounts: *const u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSSetUnorderedAccessViews(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numuavs), ::core::mem::transmute_copy(&ppunorderedaccessviews), ::core::mem::transmute_copy(&puavinitialcounts))
        }
        unsafe extern "system" fn CSSetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pcomputeshader: *mut ::core::ffi::c_void, ppclassinstances: *const *mut ::core::ffi::c_void, numclassinstances: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSSetShader(::core::mem::transmute(&pcomputeshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&numclassinstances))
        }
        unsafe extern "system" fn CSSetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSSetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn CSSetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *const *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSSetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn VSGetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.VSGetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn PSGetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.PSGetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn PSGetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pppixelshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.PSGetShader(::core::mem::transmute_copy(&pppixelshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&pnumclassinstances))
        }
        unsafe extern "system" fn PSGetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.PSGetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn VSGetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppvertexshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.VSGetShader(::core::mem::transmute_copy(&ppvertexshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&pnumclassinstances))
        }
        unsafe extern "system" fn PSGetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.PSGetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn IAGetInputLayout<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppinputlayout: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IAGetInputLayout(::core::mem::transmute_copy(&ppinputlayout))
        }
        unsafe extern "system" fn IAGetVertexBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppvertexbuffers: *mut *mut ::core::ffi::c_void, pstrides: *mut u32, poffsets: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IAGetVertexBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppvertexbuffers), ::core::mem::transmute_copy(&pstrides), ::core::mem::transmute_copy(&poffsets))
        }
        unsafe extern "system" fn IAGetIndexBuffer<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pindexbuffer: *mut *mut ::core::ffi::c_void, format: *mut super::Dxgi::Common::DXGI_FORMAT, offset: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IAGetIndexBuffer(::core::mem::transmute_copy(&pindexbuffer), ::core::mem::transmute_copy(&format), ::core::mem::transmute_copy(&offset))
        }
        unsafe extern "system" fn GSGetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GSGetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn GSGetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppgeometryshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GSGetShader(::core::mem::transmute_copy(&ppgeometryshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&pnumclassinstances))
        }
        unsafe extern "system" fn IAGetPrimitiveTopology<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ptopology: *mut super::Direct3D::D3D_PRIMITIVE_TOPOLOGY) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IAGetPrimitiveTopology(::core::mem::transmute_copy(&ptopology))
        }
        unsafe extern "system" fn VSGetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.VSGetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn VSGetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.VSGetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn GetPredication<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pppredicate: *mut *mut ::core::ffi::c_void, ppredicatevalue: *mut super::super::Foundation::BOOL) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetPredication(::core::mem::transmute_copy(&pppredicate), ::core::mem::transmute_copy(&ppredicatevalue))
        }
        unsafe extern "system" fn GSGetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GSGetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn GSGetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GSGetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn OMGetRenderTargets<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, numviews: u32, pprendertargetviews: *mut *mut ::core::ffi::c_void, ppdepthstencilview: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.OMGetRenderTargets(::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&pprendertargetviews), ::core::mem::transmute_copy(&ppdepthstencilview))
        }
        unsafe extern "system" fn OMGetRenderTargetsAndUnorderedAccessViews<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, numrtvs: u32, pprendertargetviews: *mut *mut ::core::ffi::c_void, ppdepthstencilview: *mut *mut ::core::ffi::c_void, uavstartslot: u32, numuavs: u32, ppunorderedaccessviews: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.OMGetRenderTargetsAndUnorderedAccessViews(::core::mem::transmute_copy(&numrtvs), ::core::mem::transmute_copy(&pprendertargetviews), ::core::mem::transmute_copy(&ppdepthstencilview), ::core::mem::transmute_copy(&uavstartslot), ::core::mem::transmute_copy(&numuavs), ::core::mem::transmute_copy(&ppunorderedaccessviews))
        }
        unsafe extern "system" fn OMGetBlendState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppblendstate: *mut *mut ::core::ffi::c_void, blendfactor: *mut f32, psamplemask: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.OMGetBlendState(::core::mem::transmute_copy(&ppblendstate), ::core::mem::transmute_copy(&blendfactor), ::core::mem::transmute_copy(&psamplemask))
        }
        unsafe extern "system" fn OMGetDepthStencilState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppdepthstencilstate: *mut *mut ::core::ffi::c_void, pstencilref: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.OMGetDepthStencilState(::core::mem::transmute_copy(&ppdepthstencilstate), ::core::mem::transmute_copy(&pstencilref))
        }
        unsafe extern "system" fn SOGetTargets<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, numbuffers: u32, ppsotargets: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SOGetTargets(::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppsotargets))
        }
        unsafe extern "system" fn RSGetState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pprasterizerstate: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.RSGetState(::core::mem::transmute_copy(&pprasterizerstate))
        }
        unsafe extern "system" fn RSGetViewports<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pnumviewports: *mut u32, pviewports: *mut D3D11_VIEWPORT) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.RSGetViewports(::core::mem::transmute_copy(&pnumviewports), ::core::mem::transmute_copy(&pviewports))
        }
        unsafe extern "system" fn RSGetScissorRects<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pnumrects: *mut u32, prects: *mut super::super::Foundation::RECT) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.RSGetScissorRects(::core::mem::transmute_copy(&pnumrects), ::core::mem::transmute_copy(&prects))
        }
        unsafe extern "system" fn HSGetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.HSGetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn HSGetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pphullshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.HSGetShader(::core::mem::transmute_copy(&pphullshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&pnumclassinstances))
        }
        unsafe extern "system" fn HSGetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.HSGetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn HSGetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.HSGetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn DSGetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DSGetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn DSGetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppdomainshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DSGetShader(::core::mem::transmute_copy(&ppdomainshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&pnumclassinstances))
        }
        unsafe extern "system" fn DSGetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DSGetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn DSGetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.DSGetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn CSGetShaderResources<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numviews: u32, ppshaderresourceviews: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSGetShaderResources(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numviews), ::core::mem::transmute_copy(&ppshaderresourceviews))
        }
        unsafe extern "system" fn CSGetUnorderedAccessViews<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numuavs: u32, ppunorderedaccessviews: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSGetUnorderedAccessViews(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numuavs), ::core::mem::transmute_copy(&ppunorderedaccessviews))
        }
        unsafe extern "system" fn CSGetShader<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppcomputeshader: *mut *mut ::core::ffi::c_void, ppclassinstances: *mut *mut ::core::ffi::c_void, pnumclassinstances: *mut u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSGetShader(::core::mem::transmute_copy(&ppcomputeshader), ::core::mem::transmute_copy(&ppclassinstances), ::core::mem::transmute_copy(&pnumclassinstances))
        }
        unsafe extern "system" fn CSGetSamplers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numsamplers: u32, ppsamplers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSGetSamplers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numsamplers), ::core::mem::transmute_copy(&ppsamplers))
        }
        unsafe extern "system" fn CSGetConstantBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, startslot: u32, numbuffers: u32, ppconstantbuffers: *mut *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CSGetConstantBuffers(::core::mem::transmute_copy(&startslot), ::core::mem::transmute_copy(&numbuffers), ::core::mem::transmute_copy(&ppconstantbuffers))
        }
        unsafe extern "system" fn ClearState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ClearState()
        }
        unsafe extern "system" fn Flush<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Flush()
        }
        unsafe extern "system" fn GetType<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> D3D11_DEVICE_CONTEXT_TYPE {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetType()
        }
        unsafe extern "system" fn GetContextFlags<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> u32 {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetContextFlags()
        }
        unsafe extern "system" fn FinishCommandList<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DeviceContext_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, restoredeferredcontextstate: super::super::Foundation::BOOL, ppcommandlist: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.FinishCommandList(::core::mem::transmute_copy(&restoredeferredcontextstate)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppcommandlist, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        Self {
            base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(),
            VSSetConstantBuffers: VSSetConstantBuffers::<Identity, Impl, OFFSET>,
            PSSetShaderResources: PSSetShaderResources::<Identity, Impl, OFFSET>,
            PSSetShader: PSSetShader::<Identity, Impl, OFFSET>,
            PSSetSamplers: PSSetSamplers::<Identity, Impl, OFFSET>,
            VSSetShader: VSSetShader::<Identity, Impl, OFFSET>,
            DrawIndexed: DrawIndexed::<Identity, Impl, OFFSET>,
            Draw: Draw::<Identity, Impl, OFFSET>,
            Map: Map::<Identity, Impl, OFFSET>,
            Unmap: Unmap::<Identity, Impl, OFFSET>,
            PSSetConstantBuffers: PSSetConstantBuffers::<Identity, Impl, OFFSET>,
            IASetInputLayout: IASetInputLayout::<Identity, Impl, OFFSET>,
            IASetVertexBuffers: IASetVertexBuffers::<Identity, Impl, OFFSET>,
            IASetIndexBuffer: IASetIndexBuffer::<Identity, Impl, OFFSET>,
            DrawIndexedInstanced: DrawIndexedInstanced::<Identity, Impl, OFFSET>,
            DrawInstanced: DrawInstanced::<Identity, Impl, OFFSET>,
            GSSetConstantBuffers: GSSetConstantBuffers::<Identity, Impl, OFFSET>,
            GSSetShader: GSSetShader::<Identity, Impl, OFFSET>,
            IASetPrimitiveTopology: IASetPrimitiveTopology::<Identity, Impl, OFFSET>,
            VSSetShaderResources: VSSetShaderResources::<Identity, Impl, OFFSET>,
            VSSetSamplers: VSSetSamplers::<Identity, Impl, OFFSET>,
            Begin: Begin::<Identity, Impl, OFFSET>,
            End: End::<Identity, Impl, OFFSET>,
            GetData: GetData::<Identity, Impl, OFFSET>,
            SetPredication: SetPredication::<Identity, Impl, OFFSET>,
            GSSetShaderResources: GSSetShaderResources::<Identity, Impl, OFFSET>,
            GSSetSamplers: GSSetSamplers::<Identity, Impl, OFFSET>,
            OMSetRenderTargets: OMSetRenderTargets::<Identity, Impl, OFFSET>,
            OMSetRenderTargetsAndUnorderedAccessViews: OMSetRenderTargetsAndUnorderedAccessViews::<Identity, Impl, OFFSET>,
            OMSetBlendState: OMSetBlendState::<Identity, Impl, OFFSET>,
            OMSetDepthStencilState: OMSetDepthStencilState::<Identity, Impl, OFFSET>,
            SOSetTargets: SOSetTargets::<Identity, Impl, OFFSET>,
            DrawAuto: DrawAuto::<Identity, Impl, OFFSET>,
            DrawIndexedInstancedIndirect: DrawIndexedInstancedIndirect::<Identity, Impl, OFFSET>,
            DrawInstancedIndirect: DrawInstancedIndirect::<Identity, Impl, OFFSET>,
            Dispatch: Dispatch::<Identity, Impl, OFFSET>,
            DispatchIndirect: DispatchIndirect::<Identity, Impl, OFFSET>,
            RSSetState: RSSetState::<Identity, Impl, OFFSET>,
            RSSetViewports: RSSetViewports::<Identity, Impl, OFFSET>,
            RSSetScissorRects: RSSetScissorRects::<Identity, Impl, OFFSET>,
            CopySubresourceRegion: CopySubresourceRegion::<Identity, Impl, OFFSET>,
            CopyResource: CopyResource::<Identity, Impl, OFFSET>,
            UpdateSubresource: UpdateSubresource::<Identity, Impl, OFFSET>,
            CopyStructureCount: CopyStructureCount::<Identity, Impl, OFFSET>,
            ClearRenderTargetView: ClearRenderTargetView::<Identity, Impl, OFFSET>,
            ClearUnorderedAccessViewUint: ClearUnorderedAccessViewUint::<Identity, Impl, OFFSET>,
            ClearUnorderedAccessViewFloat: ClearUnorderedAccessViewFloat::<Identity, Impl, OFFSET>,
            ClearDepthStencilView: ClearDepthStencilView::<Identity, Impl, OFFSET>,
            GenerateMips: GenerateMips::<Identity, Impl, OFFSET>,
            SetResourceMinLOD: SetResourceMinLOD::<Identity, Impl, OFFSET>,
            GetResourceMinLOD: GetResourceMinLOD::<Identity, Impl, OFFSET>,
            ResolveSubresource: ResolveSubresource::<Identity, Impl, OFFSET>,
            ExecuteCommandList: ExecuteCommandList::<Identity, Impl, OFFSET>,
            HSSetShaderResources: HSSetShaderResources::<Identity, Impl, OFFSET>,
            HSSetShader: HSSetShader::<Identity, Impl, OFFSET>,
            HSSetSamplers: HSSetSamplers::<Identity, Impl, OFFSET>,
            HSSetConstantBuffers: HSSetConstantBuffers::<Identity, Impl, OFFSET>,
            DSSetShaderResources: DSSetShaderResources::<Identity, Impl, OFFSET>,
            DSSetShader: DSSetShader::<Identity, Impl, OFFSET>,
            DSSetSamplers: DSSetSamplers::<Identity, Impl, OFFSET>,
            DSSetConstantBuffers: DSSetConstantBuffers::<Identity, Impl, OFFSET>,
            CSSetShaderResources: CSSetShaderResources::<Identity, Impl, OFFSET>,
            CSSetUnorderedAccessViews: CSSetUnorderedAccessViews::<Identity, Impl, OFFSET>,
            CSSetShader: CSSetShader::<Identity, Impl, OFFSET>,
            CSSetSamplers: CSSetSamplers::<Identity, Impl, OFFSET>,
            CSSetConstantBuffers: CSSetConstantBuffers::<Identity, Impl, OFFSET>,
            VSGetConstantBuffers: VSGetConstantBuffers::<Identity, Impl, OFFSET>,
            PSGetShaderResources: PSGetShaderResources::<Identity, Impl, OFFSET>,
            PSGetShader: PSGetShader::<Identity, Impl, OFFSET>,
            PSGetSamplers: PSGetSamplers::<Identity, Impl, OFFSET>,
            VSGetShader: VSGetShader::<Identity, Impl, OFFSET>,
            PSGetConstantBuffers: PSGetConstantBuffers::<Identity, Impl, OFFSET>,
            IAGetInputLayout: IAGetInputLayout::<Identity, Impl, OFFSET>,
            IAGetVertexBuffers: IAGetVertexBuffers::<Identity, Impl, OFFSET>,
            IAGetIndexBuffer: IAGetIndexBuffer::<Identity, Impl, OFFSET>,
            GSGetConstantBuffers: GSGetConstantBuffers::<Identity, Impl, OFFSET>,
            GSGetShader: GSGetShader::<Identity, Impl, OFFSET>,
            IAGetPrimitiveTopology: IAGetPrimitiveTopology::<Identity, Impl, OFFSET>,
            VSGetShaderResources: VSGetShaderResources::<Identity, Impl, OFFSET>,
            VSGetSamplers: VSGetSamplers::<Identity, Impl, OFFSET>,
            GetPredication: GetPredication::<Identity, Impl, OFFSET>,
            GSGetShaderResources: GSGetShaderResources::<Identity, Impl, OFFSET>,
            GSGetSamplers: GSGetSamplers::<Identity, Impl, OFFSET>,
            OMGetRenderTargets: OMGetRenderTargets::<Identity, Impl, OFFSET>,
            OMGetRenderTargetsAndUnorderedAccessViews: OMGetRenderTargetsAndUnorderedAccessViews::<Identity, Impl, OFFSET>,
            OMGetBlendState: OMGetBlendState::<Identity, Impl, OFFSET>,
            OMGetDepthStencilState: OMGetDepthStencilState::<Identity, Impl, OFFSET>,
            SOGetTargets: SOGetTargets::<Identity, Impl, OFFSET>,
            RSGetState: RSGetState::<Identity, Impl, OFFSET>,
            RSGetViewports: RSGetViewports::<Identity, Impl, OFFSET>,
            RSGetScissorRects: RSGetScissorRects::<Identity, Impl, OFFSET>,
            HSGetShaderResources: HSGetShaderResources::<Identity, Impl, OFFSET>,
            HSGetShader: HSGetShader::<Identity, Impl, OFFSET>,
            HSGetSamplers: HSGetSamplers::<Identity, Impl, OFFSET>,
            HSGetConstantBuffers: HSGetConstantBuffers::<Identity, Impl, OFFSET>,
            DSGetShaderResources: DSGetShaderResources::<Identity, Impl, OFFSET>,
            DSGetShader: DSGetShader::<Identity, Impl, OFFSET>,
            DSGetSamplers: DSGetSamplers::<Identity, Impl, OFFSET>,
            DSGetConstantBuffers: DSGetConstantBuffers::<Identity, Impl, OFFSET>,
            CSGetShaderResources: CSGetShaderResources::<Identity, Impl, OFFSET>,
            CSGetUnorderedAccessViews: CSGetUnorderedAccessViews::<Identity, Impl, OFFSET>,
            CSGetShader: CSGetShader::<Identity, Impl, OFFSET>,
            CSGetSamplers: CSGetSamplers::<Identity, Impl, OFFSET>,
            CSGetConstantBuffers: CSGetConstantBuffers::<Identity, Impl, OFFSET>,
            ClearState: ClearState::<Identity, Impl, OFFSET>,
            Flush: Flush::<Identity, Impl, OFFSET>,
            GetType: GetType::<Identity, Impl, OFFSET>,
            GetContextFlags: GetContextFlags::<Identity, Impl, OFFSET>,
            FinishCommandList: FinishCommandList::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11DeviceContext as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11DeviceContext, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11RenderTargetView(::windows::core::IUnknown);
impl ID3D11RenderTargetView {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetResource(&self, ppresource: *mut ::core::option::Option<ID3D11Resource>) {
        (::windows::core::Vtable::vtable(self).base__.GetResource)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppresource))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_RENDER_TARGET_VIEW_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11RenderTargetView {}
impl ::core::cmp::PartialEq for ID3D11RenderTargetView {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11RenderTargetView {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11RenderTargetView {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11RenderTargetView").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11RenderTargetView {}
unsafe impl ::core::marker::Sync for ID3D11RenderTargetView {}
unsafe impl ::windows::core::Vtable for ID3D11RenderTargetView {
    type Vtable = ID3D11RenderTargetView_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11RenderTargetView {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xdfdba067_0b8d_4865_875b_d7b4516cc164);
}

#[repr(C)]
pub struct ID3D11RenderTargetView_Vtbl {
    pub base__: ID3D11View_Vtbl,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_RENDER_TARGET_VIEW_DESC),
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetDesc: usize,
}

pub trait ID3D11RenderTargetView_Impl: Sized + ID3D11View_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_RENDER_TARGET_VIEW_DESC);
}

impl ID3D11RenderTargetView_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11RenderTargetView_Impl, const OFFSET: isize>() -> ID3D11RenderTargetView_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11RenderTargetView_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_RENDER_TARGET_VIEW_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11View_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11RenderTargetView as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11View as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11RenderTargetView, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11View);

#[repr(transparent)]
pub struct ID3D11Texture2D(::windows::core::IUnknown);
impl ID3D11Texture2D {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetType(&self, presourcedimension: *mut D3D11_RESOURCE_DIMENSION) {
        (::windows::core::Vtable::vtable(self).base__.GetType)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(presourcedimension))
    }
    pub unsafe fn SetEvictionPriority(&self, evictionpriority: u32) {
        (::windows::core::Vtable::vtable(self).base__.SetEvictionPriority)(::windows::core::Vtable::as_raw(self), evictionpriority)
    }
    pub unsafe fn GetEvictionPriority(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).base__.GetEvictionPriority)(::windows::core::Vtable::as_raw(self))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_TEXTURE2D_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11Texture2D {}
impl ::core::cmp::PartialEq for ID3D11Texture2D {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Texture2D {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Texture2D {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Texture2D").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Texture2D {}
unsafe impl ::core::marker::Sync for ID3D11Texture2D {}
unsafe impl ::windows::core::Vtable for ID3D11Texture2D {
    type Vtable = ID3D11Texture2D_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Texture2D {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x6f15aaf2_d208_4e89_9ab4_489535d34f9c);
}

#[repr(C)]
pub struct ID3D11Texture2D_Vtbl {
    pub base__: ID3D11Resource_Vtbl,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_TEXTURE2D_DESC),
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetDesc: usize,
}

pub trait ID3D11Texture2D_Impl: Sized + ID3D11Resource_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_TEXTURE2D_DESC);
}

impl ID3D11Texture2D_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Texture2D_Impl, const OFFSET: isize>() -> ID3D11Texture2D_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Texture2D_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_TEXTURE2D_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11Resource_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Texture2D as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11Resource as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Texture2D, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11Resource);

#[repr(transparent)]
pub struct ID3D11ShaderResourceView(::windows::core::IUnknown);
impl ID3D11ShaderResourceView {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetResource(&self, ppresource: *mut ::core::option::Option<ID3D11Resource>) {
        (::windows::core::Vtable::vtable(self).base__.GetResource)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppresource))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Direct3D\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Graphics_Direct3D", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_SHADER_RESOURCE_VIEW_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11ShaderResourceView {}
impl ::core::cmp::PartialEq for ID3D11ShaderResourceView {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11ShaderResourceView {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11ShaderResourceView {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11ShaderResourceView").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11ShaderResourceView {}
unsafe impl ::core::marker::Sync for ID3D11ShaderResourceView {}
unsafe impl ::windows::core::Vtable for ID3D11ShaderResourceView {
    type Vtable = ID3D11ShaderResourceView_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11ShaderResourceView {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xb0e06fe0_8192_4e1a_b1ca_36d7414710b2);
}

#[repr(C)]
pub struct ID3D11ShaderResourceView_Vtbl {
    pub base__: ID3D11View_Vtbl,
    #[cfg(all(feature = "Win32_Graphics_Direct3D", feature = "Win32_Graphics_Dxgi_Common"))]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_SHADER_RESOURCE_VIEW_DESC),
    #[cfg(not(all(feature = "Win32_Graphics_Direct3D", feature = "Win32_Graphics_Dxgi_Common")))]
    GetDesc: usize,
}

pub trait ID3D11ShaderResourceView_Impl: Sized + ID3D11View_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_SHADER_RESOURCE_VIEW_DESC);
}

impl ID3D11ShaderResourceView_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ShaderResourceView_Impl, const OFFSET: isize>() -> ID3D11ShaderResourceView_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11ShaderResourceView_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_SHADER_RESOURCE_VIEW_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11View_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11ShaderResourceView as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11View as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11ShaderResourceView, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11View);

#[repr(transparent)]
pub struct ID3D11DepthStencilView(::windows::core::IUnknown);
impl ID3D11DepthStencilView {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetResource(&self, ppresource: *mut ::core::option::Option<ID3D11Resource>) {
        (::windows::core::Vtable::vtable(self).base__.GetResource)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppresource))
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_DEPTH_STENCIL_VIEW_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11DepthStencilView {}
impl ::core::cmp::PartialEq for ID3D11DepthStencilView {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11DepthStencilView {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11DepthStencilView {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11DepthStencilView").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11DepthStencilView {}
unsafe impl ::core::marker::Sync for ID3D11DepthStencilView {}
unsafe impl ::windows::core::Vtable for ID3D11DepthStencilView {
    type Vtable = ID3D11DepthStencilView_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11DepthStencilView {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x9fdac92a_1876_48c3_afad_25b94f84a9b6);
}

#[repr(C)]
pub struct ID3D11DepthStencilView_Vtbl {
    pub base__: ID3D11View_Vtbl,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_DEPTH_STENCIL_VIEW_DESC),
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetDesc: usize,
}

pub trait ID3D11DepthStencilView_Impl: Sized + ID3D11View_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_DEPTH_STENCIL_VIEW_DESC);
}

impl ID3D11DepthStencilView_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DepthStencilView_Impl, const OFFSET: isize>() -> ID3D11DepthStencilView_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DepthStencilView_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_DEPTH_STENCIL_VIEW_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11View_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11DepthStencilView as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11View as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11DepthStencilView, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11View);

#[repr(transparent)]
pub struct ID3D11BlendState(::windows::core::IUnknown);
impl ID3D11BlendState {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_BLEND_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11BlendState {}
impl ::core::cmp::PartialEq for ID3D11BlendState {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11BlendState {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11BlendState {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11BlendState").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11BlendState {}
unsafe impl ::core::marker::Sync for ID3D11BlendState {}
unsafe impl ::windows::core::Vtable for ID3D11BlendState {
    type Vtable = ID3D11BlendState_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11BlendState {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x75b68faa_347d_4159_8f45_a0640f01cd9a);
}

#[repr(C)]
pub struct ID3D11BlendState_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    #[cfg(feature = "Win32_Foundation")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_BLEND_DESC),
    #[cfg(not(feature = "Win32_Foundation"))]
    GetDesc: usize,
}

pub trait ID3D11BlendState_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_BLEND_DESC);
}

impl ID3D11BlendState_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11BlendState_Impl, const OFFSET: isize>() -> ID3D11BlendState_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11BlendState_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_BLEND_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11BlendState as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11BlendState, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11RasterizerState(::windows::core::IUnknown);
impl ID3D11RasterizerState {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_RASTERIZER_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11RasterizerState {}
impl ::core::cmp::PartialEq for ID3D11RasterizerState {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11RasterizerState {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11RasterizerState {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11RasterizerState").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11RasterizerState {}
unsafe impl ::core::marker::Sync for ID3D11RasterizerState {}
unsafe impl ::windows::core::Vtable for ID3D11RasterizerState {
    type Vtable = ID3D11RasterizerState_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11RasterizerState {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x9bb4ab81_ab1a_4d8f_b506_fc04200b6ee7);
}

#[repr(C)]
pub struct ID3D11RasterizerState_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    #[cfg(feature = "Win32_Foundation")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_RASTERIZER_DESC),
    #[cfg(not(feature = "Win32_Foundation"))]
    GetDesc: usize,
}

pub trait ID3D11RasterizerState_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_RASTERIZER_DESC);
}

impl ID3D11RasterizerState_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11RasterizerState_Impl, const OFFSET: isize>() -> ID3D11RasterizerState_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11RasterizerState_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_RASTERIZER_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11RasterizerState as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11RasterizerState, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11DepthStencilState(::windows::core::IUnknown);
impl ID3D11DepthStencilState {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_DEPTH_STENCIL_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11DepthStencilState {}
impl ::core::cmp::PartialEq for ID3D11DepthStencilState {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11DepthStencilState {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11DepthStencilState {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11DepthStencilState").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11DepthStencilState {}
unsafe impl ::core::marker::Sync for ID3D11DepthStencilState {}
unsafe impl ::windows::core::Vtable for ID3D11DepthStencilState {
    type Vtable = ID3D11DepthStencilState_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11DepthStencilState {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x03823efb_8d8f_4e1c_9aa2_f64bb2cbfdf1);
}

#[repr(C)]
pub struct ID3D11DepthStencilState_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
    #[cfg(feature = "Win32_Foundation")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_DEPTH_STENCIL_DESC),
    #[cfg(not(feature = "Win32_Foundation"))]
    GetDesc: usize,
}

pub trait ID3D11DepthStencilState_Impl: Sized + ID3D11DeviceChild_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_DEPTH_STENCIL_DESC);
}

impl ID3D11DepthStencilState_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DepthStencilState_Impl, const OFFSET: isize>() -> ID3D11DepthStencilState_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11DepthStencilState_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_DEPTH_STENCIL_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11DepthStencilState as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11DepthStencilState, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11PixelShader(::windows::core::IUnknown);
impl ID3D11PixelShader {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
}
impl ::core::cmp::Eq for ID3D11PixelShader {}
impl ::core::cmp::PartialEq for ID3D11PixelShader {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11PixelShader {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11PixelShader {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11PixelShader").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11PixelShader {}
unsafe impl ::core::marker::Sync for ID3D11PixelShader {}
unsafe impl ::windows::core::Vtable for ID3D11PixelShader {
    type Vtable = ID3D11PixelShader_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11PixelShader {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xea82e40d_51dc_4f33_93d4_db7c9125ae8c);
}

#[repr(C)]
pub struct ID3D11PixelShader_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
}

pub trait ID3D11PixelShader_Impl: Sized + ID3D11DeviceChild_Impl {}

impl ID3D11PixelShader_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11PixelShader_Impl, const OFFSET: isize>() -> ID3D11PixelShader_Vtbl {
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>() }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11PixelShader as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11PixelShader, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11VertexShader(::windows::core::IUnknown);
impl ID3D11VertexShader {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
}
impl ::core::cmp::Eq for ID3D11VertexShader {}
impl ::core::cmp::PartialEq for ID3D11VertexShader {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11VertexShader {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11VertexShader {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11VertexShader").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11VertexShader {}
unsafe impl ::core::marker::Sync for ID3D11VertexShader {}
unsafe impl ::windows::core::Vtable for ID3D11VertexShader {
    type Vtable = ID3D11VertexShader_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11VertexShader {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x3b301d64_d678_4289_8897_22f8928b72f3);
}

#[repr(C)]
pub struct ID3D11VertexShader_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
}

pub trait ID3D11VertexShader_Impl: Sized + ID3D11DeviceChild_Impl {}

impl ID3D11VertexShader_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11VertexShader_Impl, const OFFSET: isize>() -> ID3D11VertexShader_Vtbl {
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>() }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11VertexShader as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11VertexShader, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11InputLayout(::windows::core::IUnknown);
impl ID3D11InputLayout {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
}
impl ::core::cmp::Eq for ID3D11InputLayout {}
impl ::core::cmp::PartialEq for ID3D11InputLayout {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11InputLayout {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11InputLayout {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11InputLayout").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11InputLayout {}
unsafe impl ::core::marker::Sync for ID3D11InputLayout {}
unsafe impl ::windows::core::Vtable for ID3D11InputLayout {
    type Vtable = ID3D11InputLayout_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11InputLayout {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xe4819ddc_4cf0_4025_bd26_5de82a3e07b7);
}

#[repr(C)]
pub struct ID3D11InputLayout_Vtbl {
    pub base__: ID3D11DeviceChild_Vtbl,
}

pub trait ID3D11InputLayout_Impl: Sized + ID3D11DeviceChild_Impl {}

impl ID3D11InputLayout_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11InputLayout_Impl, const OFFSET: isize>() -> ID3D11InputLayout_Vtbl {
        Self { base__: ID3D11DeviceChild_Vtbl::new::<Identity, Impl, OFFSET>() }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11InputLayout as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11InputLayout, ::windows::core::IUnknown, ID3D11DeviceChild);

#[repr(transparent)]
pub struct ID3D11Buffer(::windows::core::IUnknown);
impl ID3D11Buffer {
    pub unsafe fn GetDevice(&self, ppdevice: *mut ::core::option::Option<ID3D11Device>) {
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(ppdevice))
    }
    pub unsafe fn GetPrivateData(&self, guid: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: ::core::option::Option<*mut ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    pub unsafe fn SetPrivateData(&self, guid: *const ::windows::core::GUID, datasize: u32, pdata: ::core::option::Option<*const ::core::ffi::c_void>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), datasize, ::core::mem::transmute(pdata.unwrap_or(::std::ptr::null()))).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, guid: *const ::windows::core::GUID, pdata: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(guid), pdata.into().abi()).ok()
    }
    pub unsafe fn GetType(&self, presourcedimension: *mut D3D11_RESOURCE_DIMENSION) {
        (::windows::core::Vtable::vtable(self).base__.GetType)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(presourcedimension))
    }
    pub unsafe fn SetEvictionPriority(&self, evictionpriority: u32) {
        (::windows::core::Vtable::vtable(self).base__.SetEvictionPriority)(::windows::core::Vtable::as_raw(self), evictionpriority)
    }
    pub unsafe fn GetEvictionPriority(&self) -> u32 {
        (::windows::core::Vtable::vtable(self).base__.GetEvictionPriority)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetDesc(&self, pdesc: *mut D3D11_BUFFER_DESC) {
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pdesc))
    }
}
impl ::core::cmp::Eq for ID3D11Buffer {}
impl ::core::cmp::PartialEq for ID3D11Buffer {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for ID3D11Buffer {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for ID3D11Buffer {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("ID3D11Buffer").field(&self.0).finish()
    }
}
unsafe impl ::core::marker::Send for ID3D11Buffer {}
unsafe impl ::core::marker::Sync for ID3D11Buffer {}
unsafe impl ::windows::core::Vtable for ID3D11Buffer {
    type Vtable = ID3D11Buffer_Vtbl;
}
unsafe impl ::windows::core::Interface for ID3D11Buffer {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x48570b85_d1ee_4fcd_a250_eb350722b037);
}

#[repr(C)]
pub struct ID3D11Buffer_Vtbl {
    pub base__: ID3D11Resource_Vtbl,
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_BUFFER_DESC),
}

pub trait ID3D11Buffer_Impl: Sized + ID3D11Resource_Impl {
    fn GetDesc(&self, pdesc: *mut D3D11_BUFFER_DESC);
}

impl ID3D11Buffer_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Buffer_Impl, const OFFSET: isize>() -> ID3D11Buffer_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: ID3D11Buffer_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut D3D11_BUFFER_DESC) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDesc(::core::mem::transmute_copy(&pdesc))
        }
        Self { base__: ID3D11Resource_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc: GetDesc::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<ID3D11Buffer as ::windows::core::Interface>::IID || iid == &<ID3D11DeviceChild as ::windows::core::Interface>::IID || iid == &<ID3D11Resource as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(ID3D11Buffer, ::windows::core::IUnknown, ID3D11DeviceChild, ID3D11Resource);

pub unsafe fn D3D11CreateDevice<'a, P0, P1>(padapter: P0, drivertype: super::Direct3D::D3D_DRIVER_TYPE, software: P1, flags: D3D11_CREATE_DEVICE_FLAG, pfeaturelevels: ::core::option::Option<&[super::Direct3D::D3D_FEATURE_LEVEL]>, sdkversion: u32, ppdevice: ::core::option::Option<*mut ::core::option::Option<ID3D11Device>>, pfeaturelevel: ::core::option::Option<*mut super::Direct3D::D3D_FEATURE_LEVEL>, ppimmediatecontext: ::core::option::Option<*mut ::core::option::Option<ID3D11DeviceContext>>) -> ::windows::core::Result<()>
where
    P0: ::std::convert::Into<::windows::core::InParam<'a, super::Dxgi::IDXGIAdapter>>,
    P1: ::std::convert::Into<super::super::Foundation::HINSTANCE>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn D3D11CreateDevice(padapter: *mut ::core::ffi::c_void, drivertype: super::Direct3D::D3D_DRIVER_TYPE, software: super::super::Foundation::HINSTANCE, flags: D3D11_CREATE_DEVICE_FLAG, pfeaturelevels: *const super::Direct3D::D3D_FEATURE_LEVEL, featurelevels: u32, sdkversion: u32, ppdevice: *mut *mut ::core::ffi::c_void, pfeaturelevel: *mut super::Direct3D::D3D_FEATURE_LEVEL, ppimmediatecontext: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT;
    }
    D3D11CreateDevice(padapter.into().abi(), drivertype, software.into(), flags, ::core::mem::transmute(pfeaturelevels.as_deref().map_or(::core::ptr::null(), |slice| slice.as_ptr())), pfeaturelevels.as_deref().map_or(0, |slice| slice.len() as _), sdkversion, ::core::mem::transmute(ppdevice.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pfeaturelevel.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(ppimmediatecontext.unwrap_or(::std::ptr::null_mut()))).ok()
}

}
pub mod Dxgi{
pub mod Common{
#[repr(C)]
pub struct DXGI_MODE_DESC {
    pub Width: u32,
    pub Height: u32,
    pub RefreshRate: DXGI_RATIONAL,
    pub Format: DXGI_FORMAT,
    pub ScanlineOrdering: DXGI_MODE_SCANLINE_ORDER,
    pub Scaling: DXGI_MODE_SCALING,
}
impl ::core::marker::Copy for DXGI_MODE_DESC {}
impl ::core::cmp::Eq for DXGI_MODE_DESC {}
impl ::core::cmp::PartialEq for DXGI_MODE_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_MODE_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_MODE_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_MODE_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_MODE_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_MODE_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_MODE_DESC").field("Width", &self.Width).field("Height", &self.Height).field("RefreshRate", &self.RefreshRate).field("Format", &self.Format).field("ScanlineOrdering", &self.ScanlineOrdering).field("Scaling", &self.Scaling).finish()
    }
}

#[repr(C)]
pub struct DXGI_RATIONAL {
    pub Numerator: u32,
    pub Denominator: u32,
}
impl ::core::marker::Copy for DXGI_RATIONAL {}
impl ::core::cmp::Eq for DXGI_RATIONAL {}
impl ::core::cmp::PartialEq for DXGI_RATIONAL {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_RATIONAL>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_RATIONAL {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_RATIONAL {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_RATIONAL {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_RATIONAL {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_RATIONAL").field("Numerator", &self.Numerator).field("Denominator", &self.Denominator).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct DXGI_MODE_SCANLINE_ORDER(pub i32);
impl ::core::marker::Copy for DXGI_MODE_SCANLINE_ORDER {}
impl ::core::clone::Clone for DXGI_MODE_SCANLINE_ORDER {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_MODE_SCANLINE_ORDER {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for DXGI_MODE_SCANLINE_ORDER {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_MODE_SCANLINE_ORDER {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("DXGI_MODE_SCANLINE_ORDER").field(&self.0).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct DXGI_MODE_SCALING(pub i32);
impl ::core::marker::Copy for DXGI_MODE_SCALING {}
impl ::core::clone::Clone for DXGI_MODE_SCALING {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_MODE_SCALING {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for DXGI_MODE_SCALING {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_MODE_SCALING {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("DXGI_MODE_SCALING").field(&self.0).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct DXGI_MODE_ROTATION(pub i32);
impl ::core::marker::Copy for DXGI_MODE_ROTATION {}
impl ::core::clone::Clone for DXGI_MODE_ROTATION {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_MODE_ROTATION {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for DXGI_MODE_ROTATION {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_MODE_ROTATION {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("DXGI_MODE_ROTATION").field(&self.0).finish()
    }
}

#[repr(C)]
pub struct DXGI_GAMMA_CONTROL_CAPABILITIES {
    pub ScaleAndOffsetSupported: super::super::super::Foundation::BOOL,
    pub MaxConvertedValue: f32,
    pub MinConvertedValue: f32,
    pub NumGammaControlPoints: u32,
    pub ControlPointPositions: [f32; 1025],
}
impl ::core::marker::Copy for DXGI_GAMMA_CONTROL_CAPABILITIES {}
impl ::core::cmp::Eq for DXGI_GAMMA_CONTROL_CAPABILITIES {}
impl ::core::cmp::PartialEq for DXGI_GAMMA_CONTROL_CAPABILITIES {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_GAMMA_CONTROL_CAPABILITIES>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_GAMMA_CONTROL_CAPABILITIES {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_GAMMA_CONTROL_CAPABILITIES {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_GAMMA_CONTROL_CAPABILITIES {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_GAMMA_CONTROL_CAPABILITIES {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_GAMMA_CONTROL_CAPABILITIES").field("ScaleAndOffsetSupported", &self.ScaleAndOffsetSupported).field("MaxConvertedValue", &self.MaxConvertedValue).field("MinConvertedValue", &self.MinConvertedValue).field("NumGammaControlPoints", &self.NumGammaControlPoints).field("ControlPointPositions", &self.ControlPointPositions).finish()
    }
}

#[repr(C)]
pub struct DXGI_GAMMA_CONTROL {
    pub Scale: DXGI_RGB,
    pub Offset: DXGI_RGB,
    pub GammaCurve: [DXGI_RGB; 1025],
}
impl ::core::marker::Copy for DXGI_GAMMA_CONTROL {}
impl ::core::cmp::Eq for DXGI_GAMMA_CONTROL {}
impl ::core::cmp::PartialEq for DXGI_GAMMA_CONTROL {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_GAMMA_CONTROL>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_GAMMA_CONTROL {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_GAMMA_CONTROL {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_GAMMA_CONTROL {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_GAMMA_CONTROL {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_GAMMA_CONTROL").field("Scale", &self.Scale).field("Offset", &self.Offset).field("GammaCurve", &self.GammaCurve).finish()
    }
}

#[repr(C)]
pub struct DXGI_RGB {
    pub Red: f32,
    pub Green: f32,
    pub Blue: f32,
}
impl ::core::marker::Copy for DXGI_RGB {}
impl ::core::cmp::Eq for DXGI_RGB {}
impl ::core::cmp::PartialEq for DXGI_RGB {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_RGB>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_RGB {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_RGB {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_RGB {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_RGB {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_RGB").field("Red", &self.Red).field("Green", &self.Green).field("Blue", &self.Blue).finish()
    }
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct DXGI_FORMAT(pub u32);
impl ::core::marker::Copy for DXGI_FORMAT {}
impl ::core::clone::Clone for DXGI_FORMAT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_FORMAT {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for DXGI_FORMAT {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_FORMAT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("DXGI_FORMAT").field(&self.0).finish()
    }
}

pub const DXGI_ALPHA_MODE_IGNORE: DXGI_ALPHA_MODE = DXGI_ALPHA_MODE(3u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct DXGI_ALPHA_MODE(pub u32);
impl ::core::marker::Copy for DXGI_ALPHA_MODE {}
impl ::core::clone::Clone for DXGI_ALPHA_MODE {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_ALPHA_MODE {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for DXGI_ALPHA_MODE {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_ALPHA_MODE {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("DXGI_ALPHA_MODE").field(&self.0).finish()
    }
}

pub const DXGI_FORMAT_R8G8B8A8_UNORM: DXGI_FORMAT = DXGI_FORMAT(28u32);

pub const DXGI_FORMAT_B8G8R8A8_UNORM: DXGI_FORMAT = DXGI_FORMAT(87u32);

#[repr(C)]
pub struct DXGI_SAMPLE_DESC {
    pub Count: u32,
    pub Quality: u32,
}
impl ::core::marker::Copy for DXGI_SAMPLE_DESC {}
impl ::core::cmp::Eq for DXGI_SAMPLE_DESC {}
impl ::core::cmp::PartialEq for DXGI_SAMPLE_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_SAMPLE_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_SAMPLE_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_SAMPLE_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_SAMPLE_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_SAMPLE_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_SAMPLE_DESC").field("Count", &self.Count).field("Quality", &self.Quality).finish()
    }
}

pub const DXGI_FORMAT_R32G32B32A32_FLOAT: DXGI_FORMAT = DXGI_FORMAT(2u32);

pub const DXGI_FORMAT_D32_FLOAT_S8X24_UINT: DXGI_FORMAT = DXGI_FORMAT(20u32);

pub const DXGI_FORMAT_R32_UINT: DXGI_FORMAT = DXGI_FORMAT(42u32);

pub const DXGI_FORMAT_R32_FLOAT: DXGI_FORMAT = DXGI_FORMAT(41u32);

pub const DXGI_FORMAT_R32G32_FLOAT: DXGI_FORMAT = DXGI_FORMAT(16u32);

pub const DXGI_FORMAT_R32G32B32_FLOAT: DXGI_FORMAT = DXGI_FORMAT(6u32);

}
#[repr(C)]
pub struct DXGI_RGBA {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}
impl ::core::marker::Copy for DXGI_RGBA {}
impl ::core::cmp::Eq for DXGI_RGBA {}
impl ::core::cmp::PartialEq for DXGI_RGBA {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_RGBA>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_RGBA {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_RGBA {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_RGBA {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_RGBA {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_RGBA").field("r", &self.r).field("g", &self.g).field("b", &self.b).field("a", &self.a).finish()
    }
}

#[repr(C)]
pub struct DXGI_PRESENT_PARAMETERS {
    pub DirtyRectsCount: u32,
    pub pDirtyRects: *mut super::super::Foundation::RECT,
    pub pScrollRect: *mut super::super::Foundation::RECT,
    pub pScrollOffset: *mut super::super::Foundation::POINT,
}
impl ::core::marker::Copy for DXGI_PRESENT_PARAMETERS {}
impl ::core::cmp::Eq for DXGI_PRESENT_PARAMETERS {}
impl ::core::cmp::PartialEq for DXGI_PRESENT_PARAMETERS {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_PRESENT_PARAMETERS>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_PRESENT_PARAMETERS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_PRESENT_PARAMETERS {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_PRESENT_PARAMETERS {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_PRESENT_PARAMETERS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_PRESENT_PARAMETERS").field("DirtyRectsCount", &self.DirtyRectsCount).field("pDirtyRects", &self.pDirtyRects).field("pScrollRect", &self.pScrollRect).field("pScrollOffset", &self.pScrollOffset).finish()
    }
}

#[repr(C)]
pub struct DXGI_OUTPUT_DESC {
    pub DeviceName: [u16; 32],
    pub DesktopCoordinates: super::super::Foundation::RECT,
    pub AttachedToDesktop: super::super::Foundation::BOOL,
    pub Rotation: Common::DXGI_MODE_ROTATION,
    pub Monitor: super::Gdi::HMONITOR,
}
impl ::core::marker::Copy for DXGI_OUTPUT_DESC {}
impl ::core::cmp::Eq for DXGI_OUTPUT_DESC {}
impl ::core::cmp::PartialEq for DXGI_OUTPUT_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_OUTPUT_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_OUTPUT_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_OUTPUT_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_OUTPUT_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_OUTPUT_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_OUTPUT_DESC").field("DeviceName", &self.DeviceName).field("DesktopCoordinates", &self.DesktopCoordinates).field("AttachedToDesktop", &self.AttachedToDesktop).field("Rotation", &self.Rotation).field("Monitor", &self.Monitor).finish()
    }
}

#[repr(C)]
pub struct DXGI_ADAPTER_DESC {
    pub Description: [u16; 128],
    pub VendorId: u32,
    pub DeviceId: u32,
    pub SubSysId: u32,
    pub Revision: u32,
    pub DedicatedVideoMemory: usize,
    pub DedicatedSystemMemory: usize,
    pub SharedSystemMemory: usize,
    pub AdapterLuid: super::super::Foundation::LUID,
}
impl ::core::marker::Copy for DXGI_ADAPTER_DESC {}
impl ::core::cmp::Eq for DXGI_ADAPTER_DESC {}
impl ::core::cmp::PartialEq for DXGI_ADAPTER_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_ADAPTER_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_ADAPTER_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_ADAPTER_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_ADAPTER_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_ADAPTER_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_ADAPTER_DESC").field("Description", &self.Description).field("VendorId", &self.VendorId).field("DeviceId", &self.DeviceId).field("SubSysId", &self.SubSysId).field("Revision", &self.Revision).field("DedicatedVideoMemory", &self.DedicatedVideoMemory).field("DedicatedSystemMemory", &self.DedicatedSystemMemory).field("SharedSystemMemory", &self.SharedSystemMemory).field("AdapterLuid", &self.AdapterLuid).finish()
    }
}

#[repr(C)]
pub struct DXGI_SURFACE_DESC {
    pub Width: u32,
    pub Height: u32,
    pub Format: Common::DXGI_FORMAT,
    pub SampleDesc: Common::DXGI_SAMPLE_DESC,
}
impl ::core::marker::Copy for DXGI_SURFACE_DESC {}
impl ::core::cmp::Eq for DXGI_SURFACE_DESC {}
impl ::core::cmp::PartialEq for DXGI_SURFACE_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_SURFACE_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_SURFACE_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_SURFACE_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_SURFACE_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_SURFACE_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_SURFACE_DESC").field("Width", &self.Width).field("Height", &self.Height).field("Format", &self.Format).field("SampleDesc", &self.SampleDesc).finish()
    }
}

#[repr(C)]
pub struct DXGI_MAPPED_RECT {
    pub Pitch: i32,
    pub pBits: *mut u8,
}
impl ::core::marker::Copy for DXGI_MAPPED_RECT {}
impl ::core::cmp::Eq for DXGI_MAPPED_RECT {}
impl ::core::cmp::PartialEq for DXGI_MAPPED_RECT {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_MAPPED_RECT>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_MAPPED_RECT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_MAPPED_RECT {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_MAPPED_RECT {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_MAPPED_RECT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_MAPPED_RECT").field("Pitch", &self.Pitch).field("pBits", &self.pBits).finish()
    }
}

#[repr(C)]
pub struct DXGI_ADAPTER_DESC1 {
    pub Description: [u16; 128],
    pub VendorId: u32,
    pub DeviceId: u32,
    pub SubSysId: u32,
    pub Revision: u32,
    pub DedicatedVideoMemory: usize,
    pub DedicatedSystemMemory: usize,
    pub SharedSystemMemory: usize,
    pub AdapterLuid: super::super::Foundation::LUID,
    pub Flags: u32,
}
impl ::core::marker::Copy for DXGI_ADAPTER_DESC1 {}
impl ::core::cmp::Eq for DXGI_ADAPTER_DESC1 {}
impl ::core::cmp::PartialEq for DXGI_ADAPTER_DESC1 {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_ADAPTER_DESC1>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_ADAPTER_DESC1 {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_ADAPTER_DESC1 {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_ADAPTER_DESC1 {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_ADAPTER_DESC1 {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_ADAPTER_DESC1").field("Description", &self.Description).field("VendorId", &self.VendorId).field("DeviceId", &self.DeviceId).field("SubSysId", &self.SubSysId).field("Revision", &self.Revision).field("DedicatedVideoMemory", &self.DedicatedVideoMemory).field("DedicatedSystemMemory", &self.DedicatedSystemMemory).field("SharedSystemMemory", &self.SharedSystemMemory).field("AdapterLuid", &self.AdapterLuid).field("Flags", &self.Flags).finish()
    }
}

#[repr(C)]
pub struct DXGI_SWAP_CHAIN_DESC {
    pub BufferDesc: Common::DXGI_MODE_DESC,
    pub SampleDesc: Common::DXGI_SAMPLE_DESC,
    pub BufferUsage: u32,
    pub BufferCount: u32,
    pub OutputWindow: super::super::Foundation::HWND,
    pub Windowed: super::super::Foundation::BOOL,
    pub SwapEffect: DXGI_SWAP_EFFECT,
    pub Flags: u32,
}
impl ::core::marker::Copy for DXGI_SWAP_CHAIN_DESC {}
impl ::core::cmp::Eq for DXGI_SWAP_CHAIN_DESC {}
impl ::core::cmp::PartialEq for DXGI_SWAP_CHAIN_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_SWAP_CHAIN_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_SWAP_CHAIN_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_SWAP_CHAIN_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_SWAP_CHAIN_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_SWAP_CHAIN_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_SWAP_CHAIN_DESC").field("BufferDesc", &self.BufferDesc).field("SampleDesc", &self.SampleDesc).field("BufferUsage", &self.BufferUsage).field("BufferCount", &self.BufferCount).field("OutputWindow", &self.OutputWindow).field("Windowed", &self.Windowed).field("SwapEffect", &self.SwapEffect).field("Flags", &self.Flags).finish()
    }
}

#[repr(C)]
pub struct DXGI_FRAME_STATISTICS {
    pub PresentCount: u32,
    pub PresentRefreshCount: u32,
    pub SyncRefreshCount: u32,
    pub SyncQPCTime: i64,
    pub SyncGPUTime: i64,
}
impl ::core::marker::Copy for DXGI_FRAME_STATISTICS {}
impl ::core::cmp::Eq for DXGI_FRAME_STATISTICS {}
impl ::core::cmp::PartialEq for DXGI_FRAME_STATISTICS {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_FRAME_STATISTICS>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_FRAME_STATISTICS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_FRAME_STATISTICS {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_FRAME_STATISTICS {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_FRAME_STATISTICS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_FRAME_STATISTICS").field("PresentCount", &self.PresentCount).field("PresentRefreshCount", &self.PresentRefreshCount).field("SyncRefreshCount", &self.SyncRefreshCount).field("SyncQPCTime", &self.SyncQPCTime).field("SyncGPUTime", &self.SyncGPUTime).finish()
    }
}

#[repr(C)]
pub struct DXGI_SWAP_CHAIN_FULLSCREEN_DESC {
    pub RefreshRate: Common::DXGI_RATIONAL,
    pub ScanlineOrdering: Common::DXGI_MODE_SCANLINE_ORDER,
    pub Scaling: Common::DXGI_MODE_SCALING,
    pub Windowed: super::super::Foundation::BOOL,
}
impl ::core::marker::Copy for DXGI_SWAP_CHAIN_FULLSCREEN_DESC {}
impl ::core::cmp::Eq for DXGI_SWAP_CHAIN_FULLSCREEN_DESC {}
impl ::core::cmp::PartialEq for DXGI_SWAP_CHAIN_FULLSCREEN_DESC {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_SWAP_CHAIN_FULLSCREEN_DESC>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_SWAP_CHAIN_FULLSCREEN_DESC {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_SWAP_CHAIN_FULLSCREEN_DESC {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_SWAP_CHAIN_FULLSCREEN_DESC {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_SWAP_CHAIN_FULLSCREEN_DESC {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_SWAP_CHAIN_FULLSCREEN_DESC").field("RefreshRate", &self.RefreshRate).field("ScanlineOrdering", &self.ScanlineOrdering).field("Scaling", &self.Scaling).field("Windowed", &self.Windowed).finish()
    }
}

#[repr(transparent)]
pub struct IDXGIAdapter1(::windows::core::IUnknown);
impl IDXGIAdapter1 {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn EnumOutputs(&self, output: u32) -> ::windows::core::Result<IDXGIOutput> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.EnumOutputs)(::windows::core::Vtable::as_raw(self), output, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIOutput>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetDesc(&self) -> ::windows::core::Result<DXGI_ADAPTER_DESC> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_ADAPTER_DESC>(result__)
    }
    pub unsafe fn CheckInterfaceSupport(&self, interfacename: *const ::windows::core::GUID) -> ::windows::core::Result<i64> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.CheckInterfaceSupport)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(interfacename), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<i64>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetDesc1(&self) -> ::windows::core::Result<DXGI_ADAPTER_DESC1> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetDesc1)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_ADAPTER_DESC1>(result__)
    }
}
impl ::core::cmp::Eq for IDXGIAdapter1 {}
impl ::core::cmp::PartialEq for IDXGIAdapter1 {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGIAdapter1 {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGIAdapter1 {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGIAdapter1").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGIAdapter1 {
    type Vtable = IDXGIAdapter1_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGIAdapter1 {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x29038f61_3839_4626_91fd_086879011a05);
}

#[repr(C)]
pub struct IDXGIAdapter1_Vtbl {
    pub base__: IDXGIAdapter_Vtbl,
    #[cfg(feature = "Win32_Foundation")]
    pub GetDesc1: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_ADAPTER_DESC1) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    GetDesc1: usize,
}

pub trait IDXGIAdapter1_Impl: Sized + IDXGIAdapter_Impl {
    fn GetDesc1(&self) -> ::windows::core::Result<DXGI_ADAPTER_DESC1>;
}

impl IDXGIAdapter1_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIAdapter1_Impl, const OFFSET: isize>() -> IDXGIAdapter1_Vtbl {
        unsafe extern "system" fn GetDesc1<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIAdapter1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_ADAPTER_DESC1) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetDesc1() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdesc, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        Self { base__: IDXGIAdapter_Vtbl::new::<Identity, Impl, OFFSET>(), GetDesc1: GetDesc1::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGIAdapter1 as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID || iid == &<IDXGIAdapter as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGIAdapter1, ::windows::core::IUnknown, IDXGIObject, IDXGIAdapter);

#[repr(transparent)]
pub struct IDXGISurface(::windows::core::IUnknown);
impl IDXGISurface {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn GetDevice<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetDesc(&self) -> ::windows::core::Result<DXGI_SURFACE_DESC> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_SURFACE_DESC>(result__)
    }
    pub unsafe fn Map(&self, plockedrect: *mut DXGI_MAPPED_RECT, mapflags: u32) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).Map)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(plockedrect), mapflags).ok()
    }
    pub unsafe fn Unmap(&self) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).Unmap)(::windows::core::Vtable::as_raw(self)).ok()
    }
}
impl ::core::cmp::Eq for IDXGISurface {}
impl ::core::cmp::PartialEq for IDXGISurface {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGISurface {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGISurface {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGISurface").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGISurface {
    type Vtable = IDXGISurface_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGISurface {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xcafcb56c_6ac3_4889_bf47_9e23bbd260ec);
}

#[repr(C)]
pub struct IDXGISurface_Vtbl {
    pub base__: IDXGIDeviceSubObject_Vtbl,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_SURFACE_DESC) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetDesc: usize,
    pub Map: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, plockedrect: *mut DXGI_MAPPED_RECT, mapflags: u32) -> ::windows::core::HRESULT,
    pub Unmap: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
}

pub trait IDXGISurface_Impl: Sized + IDXGIDeviceSubObject_Impl {
    fn GetDesc(&self) -> ::windows::core::Result<DXGI_SURFACE_DESC>;
    fn Map(&self, plockedrect: *mut DXGI_MAPPED_RECT, mapflags: u32) -> ::windows::core::Result<()>;
    fn Unmap(&self) -> ::windows::core::Result<()>;
}

impl IDXGISurface_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISurface_Impl, const OFFSET: isize>() -> IDXGISurface_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISurface_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_SURFACE_DESC) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetDesc() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdesc, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn Map<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISurface_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, plockedrect: *mut DXGI_MAPPED_RECT, mapflags: u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Map(::core::mem::transmute_copy(&plockedrect), ::core::mem::transmute_copy(&mapflags)).into()
        }
        unsafe extern "system" fn Unmap<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISurface_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Unmap().into()
        }
        Self {
            base__: IDXGIDeviceSubObject_Vtbl::new::<Identity, Impl, OFFSET>(),
            GetDesc: GetDesc::<Identity, Impl, OFFSET>,
            Map: Map::<Identity, Impl, OFFSET>,
            Unmap: Unmap::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGISurface as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID || iid == &<IDXGIDeviceSubObject as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGISurface, ::windows::core::IUnknown, IDXGIObject, IDXGIDeviceSubObject);

#[repr(transparent)]
pub struct IDXGIOutput(::windows::core::IUnknown);
impl IDXGIOutput {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`, `\"Win32_Graphics_Gdi\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common", feature = "Win32_Graphics_Gdi"))]
    pub unsafe fn GetDesc(&self) -> ::windows::core::Result<DXGI_OUTPUT_DESC> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_OUTPUT_DESC>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetDisplayModeList(&self, enumformat: Common::DXGI_FORMAT, flags: u32, pnummodes: *mut u32, pdesc: ::core::option::Option<*mut Common::DXGI_MODE_DESC>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).GetDisplayModeList)(::windows::core::Vtable::as_raw(self), enumformat, flags, ::core::mem::transmute(pnummodes), ::core::mem::transmute(pdesc.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn FindClosestMatchingMode<'a, P0>(&self, pmodetomatch: *const Common::DXGI_MODE_DESC, pclosestmatch: *mut Common::DXGI_MODE_DESC, pconcerneddevice: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).FindClosestMatchingMode)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pmodetomatch), ::core::mem::transmute(pclosestmatch), pconcerneddevice.into().abi()).ok()
    }
    pub unsafe fn WaitForVBlank(&self) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).WaitForVBlank)(::windows::core::Vtable::as_raw(self)).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn TakeOwnership<'a, P0, P1>(&self, pdevice: P0, exclusive: P1) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
        P1: ::std::convert::Into<super::super::Foundation::BOOL>,
    {
        (::windows::core::Vtable::vtable(self).TakeOwnership)(::windows::core::Vtable::as_raw(self), pdevice.into().abi(), exclusive.into()).ok()
    }
    pub unsafe fn ReleaseOwnership(&self) {
        (::windows::core::Vtable::vtable(self).ReleaseOwnership)(::windows::core::Vtable::as_raw(self))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn GetGammaControlCapabilities(&self) -> ::windows::core::Result<Common::DXGI_GAMMA_CONTROL_CAPABILITIES> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetGammaControlCapabilities)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<Common::DXGI_GAMMA_CONTROL_CAPABILITIES>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn SetGammaControl(&self, parray: *const Common::DXGI_GAMMA_CONTROL) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).SetGammaControl)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(parray)).ok()
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetGammaControl(&self) -> ::windows::core::Result<Common::DXGI_GAMMA_CONTROL> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetGammaControl)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<Common::DXGI_GAMMA_CONTROL>(result__)
    }
    pub unsafe fn SetDisplaySurface<'a, P0>(&self, pscanoutsurface: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, IDXGISurface>>,
    {
        (::windows::core::Vtable::vtable(self).SetDisplaySurface)(::windows::core::Vtable::as_raw(self), pscanoutsurface.into().abi()).ok()
    }
    pub unsafe fn GetDisplaySurfaceData<'a, P0>(&self, pdestination: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, IDXGISurface>>,
    {
        (::windows::core::Vtable::vtable(self).GetDisplaySurfaceData)(::windows::core::Vtable::as_raw(self), pdestination.into().abi()).ok()
    }
    pub unsafe fn GetFrameStatistics(&self) -> ::windows::core::Result<DXGI_FRAME_STATISTICS> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetFrameStatistics)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_FRAME_STATISTICS>(result__)
    }
}
impl ::core::cmp::Eq for IDXGIOutput {}
impl ::core::cmp::PartialEq for IDXGIOutput {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGIOutput {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGIOutput {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGIOutput").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGIOutput {
    type Vtable = IDXGIOutput_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGIOutput {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xae02eedb_c735_4690_8d52_5a8dc20213aa);
}

#[repr(C)]
pub struct IDXGIOutput_Vtbl {
    pub base__: IDXGIObject_Vtbl,
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common", feature = "Win32_Graphics_Gdi"))]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_OUTPUT_DESC) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common", feature = "Win32_Graphics_Gdi")))]
    GetDesc: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetDisplayModeList: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, enumformat: Common::DXGI_FORMAT, flags: u32, pnummodes: *mut u32, pdesc: *mut Common::DXGI_MODE_DESC) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetDisplayModeList: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub FindClosestMatchingMode: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pmodetomatch: *const Common::DXGI_MODE_DESC, pclosestmatch: *mut Common::DXGI_MODE_DESC, pconcerneddevice: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    FindClosestMatchingMode: usize,
    pub WaitForVBlank: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Foundation")]
    pub TakeOwnership: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, exclusive: super::super::Foundation::BOOL) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    TakeOwnership: usize,
    pub ReleaseOwnership: unsafe extern "system" fn(this: *mut ::core::ffi::c_void),
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub GetGammaControlCapabilities: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pgammacaps: *mut Common::DXGI_GAMMA_CONTROL_CAPABILITIES) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common")))]
    GetGammaControlCapabilities: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub SetGammaControl: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, parray: *const Common::DXGI_GAMMA_CONTROL) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    SetGammaControl: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetGammaControl: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, parray: *mut Common::DXGI_GAMMA_CONTROL) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetGammaControl: usize,
    pub SetDisplaySurface: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pscanoutsurface: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub GetDisplaySurfaceData: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdestination: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub GetFrameStatistics: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pstats: *mut DXGI_FRAME_STATISTICS) -> ::windows::core::HRESULT,
}

pub trait IDXGIOutput_Impl: Sized + IDXGIObject_Impl {
    fn GetDesc(&self) -> ::windows::core::Result<DXGI_OUTPUT_DESC>;
    fn GetDisplayModeList(&self, enumformat: Common::DXGI_FORMAT, flags: u32, pnummodes: *mut u32, pdesc: *mut Common::DXGI_MODE_DESC) -> ::windows::core::Result<()>;
    fn FindClosestMatchingMode(&self, pmodetomatch: *const Common::DXGI_MODE_DESC, pclosestmatch: *mut Common::DXGI_MODE_DESC, pconcerneddevice: &::core::option::Option<::windows::core::IUnknown>) -> ::windows::core::Result<()>;
    fn WaitForVBlank(&self) -> ::windows::core::Result<()>;
    fn TakeOwnership(&self, pdevice: &::core::option::Option<::windows::core::IUnknown>, exclusive: super::super::Foundation::BOOL) -> ::windows::core::Result<()>;
    fn ReleaseOwnership(&self);
    fn GetGammaControlCapabilities(&self) -> ::windows::core::Result<Common::DXGI_GAMMA_CONTROL_CAPABILITIES>;
    fn SetGammaControl(&self, parray: *const Common::DXGI_GAMMA_CONTROL) -> ::windows::core::Result<()>;
    fn GetGammaControl(&self) -> ::windows::core::Result<Common::DXGI_GAMMA_CONTROL>;
    fn SetDisplaySurface(&self, pscanoutsurface: &::core::option::Option<IDXGISurface>) -> ::windows::core::Result<()>;
    fn GetDisplaySurfaceData(&self, pdestination: &::core::option::Option<IDXGISurface>) -> ::windows::core::Result<()>;
    fn GetFrameStatistics(&self) -> ::windows::core::Result<DXGI_FRAME_STATISTICS>;
}

impl IDXGIOutput_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>() -> IDXGIOutput_Vtbl {
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_OUTPUT_DESC) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetDesc() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdesc, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn GetDisplayModeList<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, enumformat: Common::DXGI_FORMAT, flags: u32, pnummodes: *mut u32, pdesc: *mut Common::DXGI_MODE_DESC) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDisplayModeList(::core::mem::transmute_copy(&enumformat), ::core::mem::transmute_copy(&flags), ::core::mem::transmute_copy(&pnummodes), ::core::mem::transmute_copy(&pdesc)).into()
        }
        unsafe extern "system" fn FindClosestMatchingMode<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pmodetomatch: *const Common::DXGI_MODE_DESC, pclosestmatch: *mut Common::DXGI_MODE_DESC, pconcerneddevice: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.FindClosestMatchingMode(::core::mem::transmute_copy(&pmodetomatch), ::core::mem::transmute_copy(&pclosestmatch), ::core::mem::transmute(&pconcerneddevice)).into()
        }
        unsafe extern "system" fn WaitForVBlank<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.WaitForVBlank().into()
        }
        unsafe extern "system" fn TakeOwnership<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, exclusive: super::super::Foundation::BOOL) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.TakeOwnership(::core::mem::transmute(&pdevice), ::core::mem::transmute_copy(&exclusive)).into()
        }
        unsafe extern "system" fn ReleaseOwnership<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ReleaseOwnership()
        }
        unsafe extern "system" fn GetGammaControlCapabilities<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pgammacaps: *mut Common::DXGI_GAMMA_CONTROL_CAPABILITIES) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetGammaControlCapabilities() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pgammacaps, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn SetGammaControl<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, parray: *const Common::DXGI_GAMMA_CONTROL) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetGammaControl(::core::mem::transmute_copy(&parray)).into()
        }
        unsafe extern "system" fn GetGammaControl<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, parray: *mut Common::DXGI_GAMMA_CONTROL) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetGammaControl() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(parray, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn SetDisplaySurface<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pscanoutsurface: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetDisplaySurface(::core::mem::transmute(&pscanoutsurface)).into()
        }
        unsafe extern "system" fn GetDisplaySurfaceData<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdestination: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDisplaySurfaceData(::core::mem::transmute(&pdestination)).into()
        }
        unsafe extern "system" fn GetFrameStatistics<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIOutput_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pstats: *mut DXGI_FRAME_STATISTICS) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetFrameStatistics() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pstats, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        Self {
            base__: IDXGIObject_Vtbl::new::<Identity, Impl, OFFSET>(),
            GetDesc: GetDesc::<Identity, Impl, OFFSET>,
            GetDisplayModeList: GetDisplayModeList::<Identity, Impl, OFFSET>,
            FindClosestMatchingMode: FindClosestMatchingMode::<Identity, Impl, OFFSET>,
            WaitForVBlank: WaitForVBlank::<Identity, Impl, OFFSET>,
            TakeOwnership: TakeOwnership::<Identity, Impl, OFFSET>,
            ReleaseOwnership: ReleaseOwnership::<Identity, Impl, OFFSET>,
            GetGammaControlCapabilities: GetGammaControlCapabilities::<Identity, Impl, OFFSET>,
            SetGammaControl: SetGammaControl::<Identity, Impl, OFFSET>,
            GetGammaControl: GetGammaControl::<Identity, Impl, OFFSET>,
            SetDisplaySurface: SetDisplaySurface::<Identity, Impl, OFFSET>,
            GetDisplaySurfaceData: GetDisplaySurfaceData::<Identity, Impl, OFFSET>,
            GetFrameStatistics: GetFrameStatistics::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGIOutput as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGIOutput, ::windows::core::IUnknown, IDXGIObject);

#[repr(transparent)]
pub struct IDXGIAdapter(::windows::core::IUnknown);
impl IDXGIAdapter {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn EnumOutputs(&self, output: u32) -> ::windows::core::Result<IDXGIOutput> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).EnumOutputs)(::windows::core::Vtable::as_raw(self), output, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIOutput>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetDesc(&self) -> ::windows::core::Result<DXGI_ADAPTER_DESC> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_ADAPTER_DESC>(result__)
    }
    pub unsafe fn CheckInterfaceSupport(&self, interfacename: *const ::windows::core::GUID) -> ::windows::core::Result<i64> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CheckInterfaceSupport)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(interfacename), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<i64>(result__)
    }
}
impl ::core::cmp::Eq for IDXGIAdapter {}
impl ::core::cmp::PartialEq for IDXGIAdapter {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGIAdapter {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGIAdapter {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGIAdapter").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGIAdapter {
    type Vtable = IDXGIAdapter_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGIAdapter {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x2411e7e1_12ac_4ccf_bd14_9798e8534dc0);
}

#[repr(C)]
pub struct IDXGIAdapter_Vtbl {
    pub base__: IDXGIObject_Vtbl,
    pub EnumOutputs: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, output: u32, ppoutput: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Foundation")]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_ADAPTER_DESC) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    GetDesc: usize,
    pub CheckInterfaceSupport: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, interfacename: *const ::windows::core::GUID, pumdversion: *mut i64) -> ::windows::core::HRESULT,
}

pub trait IDXGIAdapter_Impl: Sized + IDXGIObject_Impl {
    fn EnumOutputs(&self, output: u32) -> ::windows::core::Result<IDXGIOutput>;
    fn GetDesc(&self) -> ::windows::core::Result<DXGI_ADAPTER_DESC>;
    fn CheckInterfaceSupport(&self, interfacename: *const ::windows::core::GUID) -> ::windows::core::Result<i64>;
}

impl IDXGIAdapter_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIAdapter_Impl, const OFFSET: isize>() -> IDXGIAdapter_Vtbl {
        unsafe extern "system" fn EnumOutputs<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIAdapter_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, output: u32, ppoutput: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.EnumOutputs(::core::mem::transmute_copy(&output)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppoutput, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIAdapter_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_ADAPTER_DESC) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetDesc() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdesc, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CheckInterfaceSupport<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIAdapter_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, interfacename: *const ::windows::core::GUID, pumdversion: *mut i64) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CheckInterfaceSupport(::core::mem::transmute_copy(&interfacename)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pumdversion, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        Self {
            base__: IDXGIObject_Vtbl::new::<Identity, Impl, OFFSET>(),
            EnumOutputs: EnumOutputs::<Identity, Impl, OFFSET>,
            GetDesc: GetDesc::<Identity, Impl, OFFSET>,
            CheckInterfaceSupport: CheckInterfaceSupport::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGIAdapter as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGIAdapter, ::windows::core::IUnknown, IDXGIObject);

#[repr(transparent)]
pub struct IDXGIObject(::windows::core::IUnknown);
impl IDXGIObject {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
}
impl ::core::cmp::Eq for IDXGIObject {}
impl ::core::cmp::PartialEq for IDXGIObject {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGIObject {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGIObject {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGIObject").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGIObject {
    type Vtable = IDXGIObject_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGIObject {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0xaec22fb8_76f3_4639_9be0_28eb43a67a2e);
}

#[repr(C)]
pub struct IDXGIObject_Vtbl {
    pub base__: ::windows::core::IUnknown_Vtbl,
    pub SetPrivateData: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub SetPrivateDataInterface: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, name: *const ::windows::core::GUID, punknown: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub GetPrivateData: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub GetParent: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, riid: *const ::windows::core::GUID, ppparent: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
}

pub trait IDXGIObject_Impl: Sized {
    fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()>;
    fn SetPrivateDataInterface(&self, name: *const ::windows::core::GUID, punknown: &::core::option::Option<::windows::core::IUnknown>) -> ::windows::core::Result<()>;
    fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()>;
    fn GetParent(&self, riid: *const ::windows::core::GUID, ppparent: *mut *mut ::core::ffi::c_void) -> ::windows::core::Result<()>;
}

impl IDXGIObject_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIObject_Impl, const OFFSET: isize>() -> IDXGIObject_Vtbl {
        unsafe extern "system" fn SetPrivateData<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIObject_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetPrivateData(::core::mem::transmute_copy(&name), ::core::mem::transmute_copy(&datasize), ::core::mem::transmute_copy(&pdata)).into()
        }
        unsafe extern "system" fn SetPrivateDataInterface<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIObject_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, name: *const ::windows::core::GUID, punknown: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetPrivateDataInterface(::core::mem::transmute_copy(&name), ::core::mem::transmute(&punknown)).into()
        }
        unsafe extern "system" fn GetPrivateData<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIObject_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetPrivateData(::core::mem::transmute_copy(&name), ::core::mem::transmute_copy(&pdatasize), ::core::mem::transmute_copy(&pdata)).into()
        }
        unsafe extern "system" fn GetParent<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIObject_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, riid: *const ::windows::core::GUID, ppparent: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetParent(::core::mem::transmute_copy(&riid), ::core::mem::transmute_copy(&ppparent)).into()
        }
        Self {
            base__: ::windows::core::IUnknown_Vtbl::new::<Identity, OFFSET>(),
            SetPrivateData: SetPrivateData::<Identity, Impl, OFFSET>,
            SetPrivateDataInterface: SetPrivateDataInterface::<Identity, Impl, OFFSET>,
            GetPrivateData: GetPrivateData::<Identity, Impl, OFFSET>,
            GetParent: GetParent::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGIObject as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGIObject, ::windows::core::IUnknown);

#[repr(transparent)]
pub struct IDXGIFactory(::windows::core::IUnknown);
impl IDXGIFactory {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn EnumAdapters(&self, adapter: u32) -> ::windows::core::Result<IDXGIAdapter> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).EnumAdapters)(::windows::core::Vtable::as_raw(self), adapter, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIAdapter>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn MakeWindowAssociation<'a, P0>(&self, windowhandle: P0, flags: u32) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<super::super::Foundation::HWND>,
    {
        (::windows::core::Vtable::vtable(self).MakeWindowAssociation)(::windows::core::Vtable::as_raw(self), windowhandle.into(), flags).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetWindowAssociation(&self) -> ::windows::core::Result<super::super::Foundation::HWND> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetWindowAssociation)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<super::super::Foundation::HWND>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn CreateSwapChain<'a, P0>(&self, pdevice: P0, pdesc: *const DXGI_SWAP_CHAIN_DESC, ppswapchain: *mut ::core::option::Option<IDXGISwapChain>) -> ::windows::core::HRESULT
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).CreateSwapChain)(::windows::core::Vtable::as_raw(self), pdevice.into().abi(), ::core::mem::transmute(pdesc), ::core::mem::transmute(ppswapchain))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn CreateSoftwareAdapter<'a, P0>(&self, module: P0) -> ::windows::core::Result<IDXGIAdapter>
    where
        P0: ::std::convert::Into<super::super::Foundation::HINSTANCE>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateSoftwareAdapter)(::windows::core::Vtable::as_raw(self), module.into(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIAdapter>(result__)
    }
}
impl ::core::cmp::Eq for IDXGIFactory {}
impl ::core::cmp::PartialEq for IDXGIFactory {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGIFactory {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGIFactory {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGIFactory").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGIFactory {
    type Vtable = IDXGIFactory_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGIFactory {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x7b7166ec_21c7_44ae_b21a_c9ae321ae369);
}

#[repr(C)]
pub struct IDXGIFactory_Vtbl {
    pub base__: IDXGIObject_Vtbl,
    pub EnumAdapters: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, adapter: u32, ppadapter: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Foundation")]
    pub MakeWindowAssociation: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, windowhandle: super::super::Foundation::HWND, flags: u32) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    MakeWindowAssociation: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub GetWindowAssociation: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pwindowhandle: *mut super::super::Foundation::HWND) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    GetWindowAssociation: usize,
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub CreateSwapChain: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, pdesc: *const DXGI_SWAP_CHAIN_DESC, ppswapchain: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common")))]
    CreateSwapChain: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub CreateSoftwareAdapter: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, module: super::super::Foundation::HINSTANCE, ppadapter: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    CreateSoftwareAdapter: usize,
}

pub trait IDXGIFactory_Impl: Sized + IDXGIObject_Impl {
    fn EnumAdapters(&self, adapter: u32) -> ::windows::core::Result<IDXGIAdapter>;
    fn MakeWindowAssociation(&self, windowhandle: super::super::Foundation::HWND, flags: u32) -> ::windows::core::Result<()>;
    fn GetWindowAssociation(&self) -> ::windows::core::Result<super::super::Foundation::HWND>;
    fn CreateSwapChain(&self, pdevice: &::core::option::Option<::windows::core::IUnknown>, pdesc: *const DXGI_SWAP_CHAIN_DESC, ppswapchain: *mut ::core::option::Option<IDXGISwapChain>) -> ::windows::core::HRESULT;
    fn CreateSoftwareAdapter(&self, module: super::super::Foundation::HINSTANCE) -> ::windows::core::Result<IDXGIAdapter>;
}

impl IDXGIFactory_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory_Impl, const OFFSET: isize>() -> IDXGIFactory_Vtbl {
        unsafe extern "system" fn EnumAdapters<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, adapter: u32, ppadapter: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.EnumAdapters(::core::mem::transmute_copy(&adapter)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppadapter, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn MakeWindowAssociation<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, windowhandle: super::super::Foundation::HWND, flags: u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.MakeWindowAssociation(::core::mem::transmute_copy(&windowhandle), ::core::mem::transmute_copy(&flags)).into()
        }
        unsafe extern "system" fn GetWindowAssociation<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pwindowhandle: *mut super::super::Foundation::HWND) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetWindowAssociation() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pwindowhandle, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateSwapChain<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, pdesc: *const DXGI_SWAP_CHAIN_DESC, ppswapchain: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.CreateSwapChain(::core::mem::transmute(&pdevice), ::core::mem::transmute_copy(&pdesc), ::core::mem::transmute_copy(&ppswapchain))
        }
        unsafe extern "system" fn CreateSoftwareAdapter<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, module: super::super::Foundation::HINSTANCE, ppadapter: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateSoftwareAdapter(::core::mem::transmute_copy(&module)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppadapter, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        Self {
            base__: IDXGIObject_Vtbl::new::<Identity, Impl, OFFSET>(),
            EnumAdapters: EnumAdapters::<Identity, Impl, OFFSET>,
            MakeWindowAssociation: MakeWindowAssociation::<Identity, Impl, OFFSET>,
            GetWindowAssociation: GetWindowAssociation::<Identity, Impl, OFFSET>,
            CreateSwapChain: CreateSwapChain::<Identity, Impl, OFFSET>,
            CreateSoftwareAdapter: CreateSoftwareAdapter::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGIFactory as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGIFactory, ::windows::core::IUnknown, IDXGIObject);

#[repr(transparent)]
pub struct IDXGIDeviceSubObject(::windows::core::IUnknown);
impl IDXGIDeviceSubObject {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn GetDevice<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).GetDevice)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
}
impl ::core::cmp::Eq for IDXGIDeviceSubObject {}
impl ::core::cmp::PartialEq for IDXGIDeviceSubObject {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGIDeviceSubObject {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGIDeviceSubObject {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGIDeviceSubObject").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGIDeviceSubObject {
    type Vtable = IDXGIDeviceSubObject_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGIDeviceSubObject {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x3d3e0379_f9de_4d58_bb6c_18d62992f1a6);
}

#[repr(C)]
pub struct IDXGIDeviceSubObject_Vtbl {
    pub base__: IDXGIObject_Vtbl,
    pub GetDevice: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, riid: *const ::windows::core::GUID, ppdevice: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
}

pub trait IDXGIDeviceSubObject_Impl: Sized + IDXGIObject_Impl {
    fn GetDevice(&self, riid: *const ::windows::core::GUID, ppdevice: *mut *mut ::core::ffi::c_void) -> ::windows::core::Result<()>;
}

impl IDXGIDeviceSubObject_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIDeviceSubObject_Impl, const OFFSET: isize>() -> IDXGIDeviceSubObject_Vtbl {
        unsafe extern "system" fn GetDevice<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIDeviceSubObject_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, riid: *const ::windows::core::GUID, ppdevice: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetDevice(::core::mem::transmute_copy(&riid), ::core::mem::transmute_copy(&ppdevice)).into()
        }
        Self { base__: IDXGIObject_Vtbl::new::<Identity, Impl, OFFSET>(), GetDevice: GetDevice::<Identity, Impl, OFFSET> }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGIDeviceSubObject as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGIDeviceSubObject, ::windows::core::IUnknown, IDXGIObject);

#[repr(transparent)]
pub struct IDXGIFactory1(::windows::core::IUnknown);
impl IDXGIFactory1 {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn EnumAdapters(&self, adapter: u32) -> ::windows::core::Result<IDXGIAdapter> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.EnumAdapters)(::windows::core::Vtable::as_raw(self), adapter, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIAdapter>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn MakeWindowAssociation<'a, P0>(&self, windowhandle: P0, flags: u32) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<super::super::Foundation::HWND>,
    {
        (::windows::core::Vtable::vtable(self).base__.MakeWindowAssociation)(::windows::core::Vtable::as_raw(self), windowhandle.into(), flags).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetWindowAssociation(&self) -> ::windows::core::Result<super::super::Foundation::HWND> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.GetWindowAssociation)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<super::super::Foundation::HWND>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn CreateSwapChain<'a, P0>(&self, pdevice: P0, pdesc: *const DXGI_SWAP_CHAIN_DESC, ppswapchain: *mut ::core::option::Option<IDXGISwapChain>) -> ::windows::core::HRESULT
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.CreateSwapChain)(::windows::core::Vtable::as_raw(self), pdevice.into().abi(), ::core::mem::transmute(pdesc), ::core::mem::transmute(ppswapchain))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn CreateSoftwareAdapter<'a, P0>(&self, module: P0) -> ::windows::core::Result<IDXGIAdapter>
    where
        P0: ::std::convert::Into<super::super::Foundation::HINSTANCE>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.CreateSoftwareAdapter)(::windows::core::Vtable::as_raw(self), module.into(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIAdapter>(result__)
    }
    pub unsafe fn EnumAdapters1(&self, adapter: u32) -> ::windows::core::Result<IDXGIAdapter1> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).EnumAdapters1)(::windows::core::Vtable::as_raw(self), adapter, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIAdapter1>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn IsCurrent(&self) -> super::super::Foundation::BOOL {
        (::windows::core::Vtable::vtable(self).IsCurrent)(::windows::core::Vtable::as_raw(self))
    }
}
impl ::core::cmp::Eq for IDXGIFactory1 {}
impl ::core::cmp::PartialEq for IDXGIFactory1 {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGIFactory1 {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGIFactory1 {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGIFactory1").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGIFactory1 {
    type Vtable = IDXGIFactory1_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGIFactory1 {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x770aae78_f26f_4dba_a829_253c83d1b387);
}

#[repr(C)]
pub struct IDXGIFactory1_Vtbl {
    pub base__: IDXGIFactory_Vtbl,
    pub EnumAdapters1: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, adapter: u32, ppadapter: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Foundation")]
    pub IsCurrent: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> super::super::Foundation::BOOL,
    #[cfg(not(feature = "Win32_Foundation"))]
    IsCurrent: usize,
}

pub trait IDXGIFactory1_Impl: Sized + IDXGIFactory_Impl {
    fn EnumAdapters1(&self, adapter: u32) -> ::windows::core::Result<IDXGIAdapter1>;
    fn IsCurrent(&self) -> super::super::Foundation::BOOL;
}

impl IDXGIFactory1_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory1_Impl, const OFFSET: isize>() -> IDXGIFactory1_Vtbl {
        unsafe extern "system" fn EnumAdapters1<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, adapter: u32, ppadapter: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.EnumAdapters1(::core::mem::transmute_copy(&adapter)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppadapter, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn IsCurrent<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> super::super::Foundation::BOOL {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IsCurrent()
        }
        Self {
            base__: IDXGIFactory_Vtbl::new::<Identity, Impl, OFFSET>(),
            EnumAdapters1: EnumAdapters1::<Identity, Impl, OFFSET>,
            IsCurrent: IsCurrent::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGIFactory1 as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID || iid == &<IDXGIFactory as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGIFactory1, ::windows::core::IUnknown, IDXGIObject, IDXGIFactory);

#[repr(transparent)]
pub struct IDXGISwapChain(::windows::core::IUnknown);
impl IDXGISwapChain {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn GetDevice<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.GetDevice)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn Present(&self, syncinterval: u32, flags: u32) -> ::windows::core::HRESULT {
        (::windows::core::Vtable::vtable(self).Present)(::windows::core::Vtable::as_raw(self), syncinterval, flags)
    }
    pub unsafe fn GetBuffer<T>(&self, buffer: u32) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).GetBuffer)(::windows::core::Vtable::as_raw(self), buffer, &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn SetFullscreenState<'a, P0, P1>(&self, fullscreen: P0, ptarget: P1) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<super::super::Foundation::BOOL>,
        P1: ::std::convert::Into<::windows::core::InParam<'a, IDXGIOutput>>,
    {
        (::windows::core::Vtable::vtable(self).SetFullscreenState)(::windows::core::Vtable::as_raw(self), fullscreen.into(), ptarget.into().abi()).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetFullscreenState(&self, pfullscreen: ::core::option::Option<*mut super::super::Foundation::BOOL>, pptarget: ::core::option::Option<*mut ::core::option::Option<IDXGIOutput>>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).GetFullscreenState)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pfullscreen.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pptarget.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn GetDesc(&self) -> ::windows::core::Result<DXGI_SWAP_CHAIN_DESC> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_SWAP_CHAIN_DESC>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn ResizeBuffers(&self, buffercount: u32, width: u32, height: u32, newformat: Common::DXGI_FORMAT, swapchainflags: u32) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).ResizeBuffers)(::windows::core::Vtable::as_raw(self), buffercount, width, height, newformat, swapchainflags).ok()
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn ResizeTarget(&self, pnewtargetparameters: *const Common::DXGI_MODE_DESC) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).ResizeTarget)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pnewtargetparameters)).ok()
    }
    pub unsafe fn GetContainingOutput(&self) -> ::windows::core::Result<IDXGIOutput> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetContainingOutput)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIOutput>(result__)
    }
    pub unsafe fn GetFrameStatistics(&self) -> ::windows::core::Result<DXGI_FRAME_STATISTICS> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetFrameStatistics)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_FRAME_STATISTICS>(result__)
    }
    pub unsafe fn GetLastPresentCount(&self) -> ::windows::core::Result<u32> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetLastPresentCount)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<u32>(result__)
    }
}
impl ::core::cmp::Eq for IDXGISwapChain {}
impl ::core::cmp::PartialEq for IDXGISwapChain {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGISwapChain {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGISwapChain {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGISwapChain").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGISwapChain {
    type Vtable = IDXGISwapChain_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGISwapChain {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x310d36a0_d2e7_4c0a_aa04_6a9d23b8886a);
}

#[repr(C)]
pub struct IDXGISwapChain_Vtbl {
    pub base__: IDXGIDeviceSubObject_Vtbl,
    pub Present: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, syncinterval: u32, flags: u32) -> ::windows::core::HRESULT,
    pub GetBuffer: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, buffer: u32, riid: *const ::windows::core::GUID, ppsurface: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Foundation")]
    pub SetFullscreenState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, fullscreen: super::super::Foundation::BOOL, ptarget: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    SetFullscreenState: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub GetFullscreenState: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pfullscreen: *mut super::super::Foundation::BOOL, pptarget: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    GetFullscreenState: usize,
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub GetDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_SWAP_CHAIN_DESC) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common")))]
    GetDesc: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub ResizeBuffers: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, buffercount: u32, width: u32, height: u32, newformat: Common::DXGI_FORMAT, swapchainflags: u32) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    ResizeBuffers: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub ResizeTarget: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pnewtargetparameters: *const Common::DXGI_MODE_DESC) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    ResizeTarget: usize,
    pub GetContainingOutput: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, ppoutput: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub GetFrameStatistics: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pstats: *mut DXGI_FRAME_STATISTICS) -> ::windows::core::HRESULT,
    pub GetLastPresentCount: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, plastpresentcount: *mut u32) -> ::windows::core::HRESULT,
}

pub trait IDXGISwapChain_Impl: Sized + IDXGIDeviceSubObject_Impl {
    fn Present(&self, syncinterval: u32, flags: u32) -> ::windows::core::HRESULT;
    fn GetBuffer(&self, buffer: u32, riid: *const ::windows::core::GUID, ppsurface: *mut *mut ::core::ffi::c_void) -> ::windows::core::Result<()>;
    fn SetFullscreenState(&self, fullscreen: super::super::Foundation::BOOL, ptarget: &::core::option::Option<IDXGIOutput>) -> ::windows::core::Result<()>;
    fn GetFullscreenState(&self, pfullscreen: *mut super::super::Foundation::BOOL, pptarget: *mut ::core::option::Option<IDXGIOutput>) -> ::windows::core::Result<()>;
    fn GetDesc(&self) -> ::windows::core::Result<DXGI_SWAP_CHAIN_DESC>;
    fn ResizeBuffers(&self, buffercount: u32, width: u32, height: u32, newformat: Common::DXGI_FORMAT, swapchainflags: u32) -> ::windows::core::Result<()>;
    fn ResizeTarget(&self, pnewtargetparameters: *const Common::DXGI_MODE_DESC) -> ::windows::core::Result<()>;
    fn GetContainingOutput(&self) -> ::windows::core::Result<IDXGIOutput>;
    fn GetFrameStatistics(&self) -> ::windows::core::Result<DXGI_FRAME_STATISTICS>;
    fn GetLastPresentCount(&self) -> ::windows::core::Result<u32>;
}

impl IDXGISwapChain_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>() -> IDXGISwapChain_Vtbl {
        unsafe extern "system" fn Present<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, syncinterval: u32, flags: u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Present(::core::mem::transmute_copy(&syncinterval), ::core::mem::transmute_copy(&flags))
        }
        unsafe extern "system" fn GetBuffer<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, buffer: u32, riid: *const ::windows::core::GUID, ppsurface: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetBuffer(::core::mem::transmute_copy(&buffer), ::core::mem::transmute_copy(&riid), ::core::mem::transmute_copy(&ppsurface)).into()
        }
        unsafe extern "system" fn SetFullscreenState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, fullscreen: super::super::Foundation::BOOL, ptarget: *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetFullscreenState(::core::mem::transmute_copy(&fullscreen), ::core::mem::transmute(&ptarget)).into()
        }
        unsafe extern "system" fn GetFullscreenState<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pfullscreen: *mut super::super::Foundation::BOOL, pptarget: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetFullscreenState(::core::mem::transmute_copy(&pfullscreen), ::core::mem::transmute_copy(&pptarget)).into()
        }
        unsafe extern "system" fn GetDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_SWAP_CHAIN_DESC) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetDesc() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdesc, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn ResizeBuffers<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, buffercount: u32, width: u32, height: u32, newformat: Common::DXGI_FORMAT, swapchainflags: u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ResizeBuffers(::core::mem::transmute_copy(&buffercount), ::core::mem::transmute_copy(&width), ::core::mem::transmute_copy(&height), ::core::mem::transmute_copy(&newformat), ::core::mem::transmute_copy(&swapchainflags)).into()
        }
        unsafe extern "system" fn ResizeTarget<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pnewtargetparameters: *const Common::DXGI_MODE_DESC) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.ResizeTarget(::core::mem::transmute_copy(&pnewtargetparameters)).into()
        }
        unsafe extern "system" fn GetContainingOutput<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, ppoutput: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetContainingOutput() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppoutput, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn GetFrameStatistics<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pstats: *mut DXGI_FRAME_STATISTICS) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetFrameStatistics() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pstats, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn GetLastPresentCount<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, plastpresentcount: *mut u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetLastPresentCount() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(plastpresentcount, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        Self {
            base__: IDXGIDeviceSubObject_Vtbl::new::<Identity, Impl, OFFSET>(),
            Present: Present::<Identity, Impl, OFFSET>,
            GetBuffer: GetBuffer::<Identity, Impl, OFFSET>,
            SetFullscreenState: SetFullscreenState::<Identity, Impl, OFFSET>,
            GetFullscreenState: GetFullscreenState::<Identity, Impl, OFFSET>,
            GetDesc: GetDesc::<Identity, Impl, OFFSET>,
            ResizeBuffers: ResizeBuffers::<Identity, Impl, OFFSET>,
            ResizeTarget: ResizeTarget::<Identity, Impl, OFFSET>,
            GetContainingOutput: GetContainingOutput::<Identity, Impl, OFFSET>,
            GetFrameStatistics: GetFrameStatistics::<Identity, Impl, OFFSET>,
            GetLastPresentCount: GetLastPresentCount::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGISwapChain as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID || iid == &<IDXGIDeviceSubObject as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGISwapChain, ::windows::core::IUnknown, IDXGIObject, IDXGIDeviceSubObject);

#[repr(transparent)]
pub struct IDXGIFactory2(::windows::core::IUnknown);
impl IDXGIFactory2 {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.base__.base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn EnumAdapters(&self, adapter: u32) -> ::windows::core::Result<IDXGIAdapter> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.base__.EnumAdapters)(::windows::core::Vtable::as_raw(self), adapter, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIAdapter>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn MakeWindowAssociation<'a, P0>(&self, windowhandle: P0, flags: u32) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<super::super::Foundation::HWND>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.MakeWindowAssociation)(::windows::core::Vtable::as_raw(self), windowhandle.into(), flags).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetWindowAssociation(&self) -> ::windows::core::Result<super::super::Foundation::HWND> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.base__.GetWindowAssociation)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<super::super::Foundation::HWND>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn CreateSwapChain<'a, P0>(&self, pdevice: P0, pdesc: *const DXGI_SWAP_CHAIN_DESC, ppswapchain: *mut ::core::option::Option<IDXGISwapChain>) -> ::windows::core::HRESULT
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.CreateSwapChain)(::windows::core::Vtable::as_raw(self), pdevice.into().abi(), ::core::mem::transmute(pdesc), ::core::mem::transmute(ppswapchain))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn CreateSoftwareAdapter<'a, P0>(&self, module: P0) -> ::windows::core::Result<IDXGIAdapter>
    where
        P0: ::std::convert::Into<super::super::Foundation::HINSTANCE>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.base__.CreateSoftwareAdapter)(::windows::core::Vtable::as_raw(self), module.into(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIAdapter>(result__)
    }
    pub unsafe fn EnumAdapters1(&self, adapter: u32) -> ::windows::core::Result<IDXGIAdapter1> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.EnumAdapters1)(::windows::core::Vtable::as_raw(self), adapter, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIAdapter1>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn IsCurrent(&self) -> super::super::Foundation::BOOL {
        (::windows::core::Vtable::vtable(self).base__.IsCurrent)(::windows::core::Vtable::as_raw(self))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn IsWindowedStereoEnabled(&self) -> super::super::Foundation::BOOL {
        (::windows::core::Vtable::vtable(self).IsWindowedStereoEnabled)(::windows::core::Vtable::as_raw(self))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn CreateSwapChainForHwnd<'a, P0, P1, P2>(&self, pdevice: P0, hwnd: P1, pdesc: *const DXGI_SWAP_CHAIN_DESC1, pfullscreendesc: ::core::option::Option<*const DXGI_SWAP_CHAIN_FULLSCREEN_DESC>, prestricttooutput: P2) -> ::windows::core::Result<IDXGISwapChain1>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
        P1: ::std::convert::Into<super::super::Foundation::HWND>,
        P2: ::std::convert::Into<::windows::core::InParam<'a, IDXGIOutput>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateSwapChainForHwnd)(::windows::core::Vtable::as_raw(self), pdevice.into().abi(), hwnd.into(), ::core::mem::transmute(pdesc), ::core::mem::transmute(pfullscreendesc.unwrap_or(::std::ptr::null())), prestricttooutput.into().abi(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGISwapChain1>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn CreateSwapChainForCoreWindow<'a, P0, P1, P2>(&self, pdevice: P0, pwindow: P1, pdesc: *const DXGI_SWAP_CHAIN_DESC1, prestricttooutput: P2) -> ::windows::core::Result<IDXGISwapChain1>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
        P1: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
        P2: ::std::convert::Into<::windows::core::InParam<'a, IDXGIOutput>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateSwapChainForCoreWindow)(::windows::core::Vtable::as_raw(self), pdevice.into().abi(), pwindow.into().abi(), ::core::mem::transmute(pdesc), prestricttooutput.into().abi(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGISwapChain1>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetSharedResourceAdapterLuid<'a, P0>(&self, hresource: P0) -> ::windows::core::Result<super::super::Foundation::LUID>
    where
        P0: ::std::convert::Into<super::super::Foundation::HANDLE>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetSharedResourceAdapterLuid)(::windows::core::Vtable::as_raw(self), hresource.into(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<super::super::Foundation::LUID>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn RegisterStereoStatusWindow<'a, P0>(&self, windowhandle: P0, wmsg: u32) -> ::windows::core::Result<u32>
    where
        P0: ::std::convert::Into<super::super::Foundation::HWND>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).RegisterStereoStatusWindow)(::windows::core::Vtable::as_raw(self), windowhandle.into(), wmsg, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<u32>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn RegisterStereoStatusEvent<'a, P0>(&self, hevent: P0) -> ::windows::core::Result<u32>
    where
        P0: ::std::convert::Into<super::super::Foundation::HANDLE>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).RegisterStereoStatusEvent)(::windows::core::Vtable::as_raw(self), hevent.into(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<u32>(result__)
    }
    pub unsafe fn UnregisterStereoStatus(&self, dwcookie: u32) {
        (::windows::core::Vtable::vtable(self).UnregisterStereoStatus)(::windows::core::Vtable::as_raw(self), dwcookie)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn RegisterOcclusionStatusWindow<'a, P0>(&self, windowhandle: P0, wmsg: u32) -> ::windows::core::Result<u32>
    where
        P0: ::std::convert::Into<super::super::Foundation::HWND>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).RegisterOcclusionStatusWindow)(::windows::core::Vtable::as_raw(self), windowhandle.into(), wmsg, ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<u32>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn RegisterOcclusionStatusEvent<'a, P0>(&self, hevent: P0) -> ::windows::core::Result<u32>
    where
        P0: ::std::convert::Into<super::super::Foundation::HANDLE>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).RegisterOcclusionStatusEvent)(::windows::core::Vtable::as_raw(self), hevent.into(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<u32>(result__)
    }
    pub unsafe fn UnregisterOcclusionStatus(&self, dwcookie: u32) {
        (::windows::core::Vtable::vtable(self).UnregisterOcclusionStatus)(::windows::core::Vtable::as_raw(self), dwcookie)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn CreateSwapChainForComposition<'a, P0, P1>(&self, pdevice: P0, pdesc: *const DXGI_SWAP_CHAIN_DESC1, prestricttooutput: P1) -> ::windows::core::Result<IDXGISwapChain1>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
        P1: ::std::convert::Into<::windows::core::InParam<'a, IDXGIOutput>>,
    {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).CreateSwapChainForComposition)(::windows::core::Vtable::as_raw(self), pdevice.into().abi(), ::core::mem::transmute(pdesc), prestricttooutput.into().abi(), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGISwapChain1>(result__)
    }
}
impl ::core::cmp::Eq for IDXGIFactory2 {}
impl ::core::cmp::PartialEq for IDXGIFactory2 {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGIFactory2 {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGIFactory2 {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGIFactory2").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGIFactory2 {
    type Vtable = IDXGIFactory2_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGIFactory2 {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x50c83a1c_e072_4c48_87b0_3630fa36a6d0);
}

#[repr(C)]
pub struct IDXGIFactory2_Vtbl {
    pub base__: IDXGIFactory1_Vtbl,
    #[cfg(feature = "Win32_Foundation")]
    pub IsWindowedStereoEnabled: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> super::super::Foundation::BOOL,
    #[cfg(not(feature = "Win32_Foundation"))]
    IsWindowedStereoEnabled: usize,
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub CreateSwapChainForHwnd: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, hwnd: super::super::Foundation::HWND, pdesc: *const DXGI_SWAP_CHAIN_DESC1, pfullscreendesc: *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC, prestricttooutput: *mut ::core::ffi::c_void, ppswapchain: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common")))]
    CreateSwapChainForHwnd: usize,
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub CreateSwapChainForCoreWindow: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, pwindow: *mut ::core::ffi::c_void, pdesc: *const DXGI_SWAP_CHAIN_DESC1, prestricttooutput: *mut ::core::ffi::c_void, ppswapchain: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common")))]
    CreateSwapChainForCoreWindow: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub GetSharedResourceAdapterLuid: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, hresource: super::super::Foundation::HANDLE, pluid: *mut super::super::Foundation::LUID) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    GetSharedResourceAdapterLuid: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub RegisterStereoStatusWindow: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, windowhandle: super::super::Foundation::HWND, wmsg: u32, pdwcookie: *mut u32) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    RegisterStereoStatusWindow: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub RegisterStereoStatusEvent: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, hevent: super::super::Foundation::HANDLE, pdwcookie: *mut u32) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    RegisterStereoStatusEvent: usize,
    pub UnregisterStereoStatus: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, dwcookie: u32),
    #[cfg(feature = "Win32_Foundation")]
    pub RegisterOcclusionStatusWindow: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, windowhandle: super::super::Foundation::HWND, wmsg: u32, pdwcookie: *mut u32) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    RegisterOcclusionStatusWindow: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub RegisterOcclusionStatusEvent: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, hevent: super::super::Foundation::HANDLE, pdwcookie: *mut u32) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    RegisterOcclusionStatusEvent: usize,
    pub UnregisterOcclusionStatus: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, dwcookie: u32),
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub CreateSwapChainForComposition: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, pdesc: *const DXGI_SWAP_CHAIN_DESC1, prestricttooutput: *mut ::core::ffi::c_void, ppswapchain: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common")))]
    CreateSwapChainForComposition: usize,
}

pub trait IDXGIFactory2_Impl: Sized + IDXGIFactory1_Impl {
    fn IsWindowedStereoEnabled(&self) -> super::super::Foundation::BOOL;
    fn CreateSwapChainForHwnd(&self, pdevice: &::core::option::Option<::windows::core::IUnknown>, hwnd: super::super::Foundation::HWND, pdesc: *const DXGI_SWAP_CHAIN_DESC1, pfullscreendesc: *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC, prestricttooutput: &::core::option::Option<IDXGIOutput>) -> ::windows::core::Result<IDXGISwapChain1>;
    fn CreateSwapChainForCoreWindow(&self, pdevice: &::core::option::Option<::windows::core::IUnknown>, pwindow: &::core::option::Option<::windows::core::IUnknown>, pdesc: *const DXGI_SWAP_CHAIN_DESC1, prestricttooutput: &::core::option::Option<IDXGIOutput>) -> ::windows::core::Result<IDXGISwapChain1>;
    fn GetSharedResourceAdapterLuid(&self, hresource: super::super::Foundation::HANDLE) -> ::windows::core::Result<super::super::Foundation::LUID>;
    fn RegisterStereoStatusWindow(&self, windowhandle: super::super::Foundation::HWND, wmsg: u32) -> ::windows::core::Result<u32>;
    fn RegisterStereoStatusEvent(&self, hevent: super::super::Foundation::HANDLE) -> ::windows::core::Result<u32>;
    fn UnregisterStereoStatus(&self, dwcookie: u32);
    fn RegisterOcclusionStatusWindow(&self, windowhandle: super::super::Foundation::HWND, wmsg: u32) -> ::windows::core::Result<u32>;
    fn RegisterOcclusionStatusEvent(&self, hevent: super::super::Foundation::HANDLE) -> ::windows::core::Result<u32>;
    fn UnregisterOcclusionStatus(&self, dwcookie: u32);
    fn CreateSwapChainForComposition(&self, pdevice: &::core::option::Option<::windows::core::IUnknown>, pdesc: *const DXGI_SWAP_CHAIN_DESC1, prestricttooutput: &::core::option::Option<IDXGIOutput>) -> ::windows::core::Result<IDXGISwapChain1>;
}

impl IDXGIFactory2_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>() -> IDXGIFactory2_Vtbl {
        unsafe extern "system" fn IsWindowedStereoEnabled<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> super::super::Foundation::BOOL {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IsWindowedStereoEnabled()
        }
        unsafe extern "system" fn CreateSwapChainForHwnd<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, hwnd: super::super::Foundation::HWND, pdesc: *const DXGI_SWAP_CHAIN_DESC1, pfullscreendesc: *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC, prestricttooutput: *mut ::core::ffi::c_void, ppswapchain: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateSwapChainForHwnd(::core::mem::transmute(&pdevice), ::core::mem::transmute_copy(&hwnd), ::core::mem::transmute_copy(&pdesc), ::core::mem::transmute_copy(&pfullscreendesc), ::core::mem::transmute(&prestricttooutput)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppswapchain, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn CreateSwapChainForCoreWindow<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, pwindow: *mut ::core::ffi::c_void, pdesc: *const DXGI_SWAP_CHAIN_DESC1, prestricttooutput: *mut ::core::ffi::c_void, ppswapchain: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateSwapChainForCoreWindow(::core::mem::transmute(&pdevice), ::core::mem::transmute(&pwindow), ::core::mem::transmute_copy(&pdesc), ::core::mem::transmute(&prestricttooutput)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppswapchain, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn GetSharedResourceAdapterLuid<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, hresource: super::super::Foundation::HANDLE, pluid: *mut super::super::Foundation::LUID) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetSharedResourceAdapterLuid(::core::mem::transmute_copy(&hresource)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pluid, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn RegisterStereoStatusWindow<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, windowhandle: super::super::Foundation::HWND, wmsg: u32, pdwcookie: *mut u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.RegisterStereoStatusWindow(::core::mem::transmute_copy(&windowhandle), ::core::mem::transmute_copy(&wmsg)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdwcookie, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn RegisterStereoStatusEvent<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, hevent: super::super::Foundation::HANDLE, pdwcookie: *mut u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.RegisterStereoStatusEvent(::core::mem::transmute_copy(&hevent)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdwcookie, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn UnregisterStereoStatus<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, dwcookie: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.UnregisterStereoStatus(::core::mem::transmute_copy(&dwcookie))
        }
        unsafe extern "system" fn RegisterOcclusionStatusWindow<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, windowhandle: super::super::Foundation::HWND, wmsg: u32, pdwcookie: *mut u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.RegisterOcclusionStatusWindow(::core::mem::transmute_copy(&windowhandle), ::core::mem::transmute_copy(&wmsg)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdwcookie, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn RegisterOcclusionStatusEvent<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, hevent: super::super::Foundation::HANDLE, pdwcookie: *mut u32) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.RegisterOcclusionStatusEvent(::core::mem::transmute_copy(&hevent)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdwcookie, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn UnregisterOcclusionStatus<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, dwcookie: u32) {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.UnregisterOcclusionStatus(::core::mem::transmute_copy(&dwcookie))
        }
        unsafe extern "system" fn CreateSwapChainForComposition<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGIFactory2_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdevice: *mut ::core::ffi::c_void, pdesc: *const DXGI_SWAP_CHAIN_DESC1, prestricttooutput: *mut ::core::ffi::c_void, ppswapchain: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.CreateSwapChainForComposition(::core::mem::transmute(&pdevice), ::core::mem::transmute_copy(&pdesc), ::core::mem::transmute(&prestricttooutput)) {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(ppswapchain, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        Self {
            base__: IDXGIFactory1_Vtbl::new::<Identity, Impl, OFFSET>(),
            IsWindowedStereoEnabled: IsWindowedStereoEnabled::<Identity, Impl, OFFSET>,
            CreateSwapChainForHwnd: CreateSwapChainForHwnd::<Identity, Impl, OFFSET>,
            CreateSwapChainForCoreWindow: CreateSwapChainForCoreWindow::<Identity, Impl, OFFSET>,
            GetSharedResourceAdapterLuid: GetSharedResourceAdapterLuid::<Identity, Impl, OFFSET>,
            RegisterStereoStatusWindow: RegisterStereoStatusWindow::<Identity, Impl, OFFSET>,
            RegisterStereoStatusEvent: RegisterStereoStatusEvent::<Identity, Impl, OFFSET>,
            UnregisterStereoStatus: UnregisterStereoStatus::<Identity, Impl, OFFSET>,
            RegisterOcclusionStatusWindow: RegisterOcclusionStatusWindow::<Identity, Impl, OFFSET>,
            RegisterOcclusionStatusEvent: RegisterOcclusionStatusEvent::<Identity, Impl, OFFSET>,
            UnregisterOcclusionStatus: UnregisterOcclusionStatus::<Identity, Impl, OFFSET>,
            CreateSwapChainForComposition: CreateSwapChainForComposition::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGIFactory2 as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID || iid == &<IDXGIFactory as ::windows::core::Interface>::IID || iid == &<IDXGIFactory1 as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGIFactory2, ::windows::core::IUnknown, IDXGIObject, IDXGIFactory, IDXGIFactory1);

#[repr(transparent)]
pub struct IDXGISwapChain1(::windows::core::IUnknown);
impl IDXGISwapChain1 {
    pub unsafe fn SetPrivateData(&self, name: *const ::windows::core::GUID, datasize: u32, pdata: *const ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.SetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), datasize, ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn SetPrivateDataInterface<'a, P0>(&self, name: *const ::windows::core::GUID, punknown: P0) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<::windows::core::InParam<'a, ::windows::core::IUnknown>>,
    {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.SetPrivateDataInterface)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), punknown.into().abi()).ok()
    }
    pub unsafe fn GetPrivateData(&self, name: *const ::windows::core::GUID, pdatasize: *mut u32, pdata: *mut ::core::ffi::c_void) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.base__.base__.GetPrivateData)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(name), ::core::mem::transmute(pdatasize), ::core::mem::transmute(pdata)).ok()
    }
    pub unsafe fn GetParent<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.base__.base__.GetParent)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn GetDevice<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.base__.GetDevice)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    pub unsafe fn Present(&self, syncinterval: u32, flags: u32) -> ::windows::core::HRESULT {
        (::windows::core::Vtable::vtable(self).base__.Present)(::windows::core::Vtable::as_raw(self), syncinterval, flags)
    }
    pub unsafe fn GetBuffer<T>(&self, buffer: u32) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).base__.GetBuffer)(::windows::core::Vtable::as_raw(self), buffer, &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn SetFullscreenState<'a, P0, P1>(&self, fullscreen: P0, ptarget: P1) -> ::windows::core::Result<()>
    where
        P0: ::std::convert::Into<super::super::Foundation::BOOL>,
        P1: ::std::convert::Into<::windows::core::InParam<'a, IDXGIOutput>>,
    {
        (::windows::core::Vtable::vtable(self).base__.SetFullscreenState)(::windows::core::Vtable::as_raw(self), fullscreen.into(), ptarget.into().abi()).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetFullscreenState(&self, pfullscreen: ::core::option::Option<*mut super::super::Foundation::BOOL>, pptarget: ::core::option::Option<*mut ::core::option::Option<IDXGIOutput>>) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.GetFullscreenState)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pfullscreen.unwrap_or(::std::ptr::null_mut())), ::core::mem::transmute(pptarget.unwrap_or(::std::ptr::null_mut()))).ok()
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn GetDesc(&self) -> ::windows::core::Result<DXGI_SWAP_CHAIN_DESC> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.GetDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_SWAP_CHAIN_DESC>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn ResizeBuffers(&self, buffercount: u32, width: u32, height: u32, newformat: Common::DXGI_FORMAT, swapchainflags: u32) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.ResizeBuffers)(::windows::core::Vtable::as_raw(self), buffercount, width, height, newformat, swapchainflags).ok()
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn ResizeTarget(&self, pnewtargetparameters: *const Common::DXGI_MODE_DESC) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).base__.ResizeTarget)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pnewtargetparameters)).ok()
    }
    pub unsafe fn GetContainingOutput(&self) -> ::windows::core::Result<IDXGIOutput> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.GetContainingOutput)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIOutput>(result__)
    }
    pub unsafe fn GetFrameStatistics(&self) -> ::windows::core::Result<DXGI_FRAME_STATISTICS> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.GetFrameStatistics)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_FRAME_STATISTICS>(result__)
    }
    pub unsafe fn GetLastPresentCount(&self) -> ::windows::core::Result<u32> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).base__.GetLastPresentCount)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<u32>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn GetDesc1(&self) -> ::windows::core::Result<DXGI_SWAP_CHAIN_DESC1> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetDesc1)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_SWAP_CHAIN_DESC1>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`, `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub unsafe fn GetFullscreenDesc(&self) -> ::windows::core::Result<DXGI_SWAP_CHAIN_FULLSCREEN_DESC> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetFullscreenDesc)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_SWAP_CHAIN_FULLSCREEN_DESC>(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn GetHwnd(&self) -> ::windows::core::Result<super::super::Foundation::HWND> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetHwnd)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<super::super::Foundation::HWND>(result__)
    }
    pub unsafe fn GetCoreWindow<T>(&self) -> ::windows::core::Result<T>
    where
        T: ::windows::core::Interface,
    {
        let mut result__ = ::core::option::Option::None;
        (::windows::core::Vtable::vtable(self).GetCoreWindow)(::windows::core::Vtable::as_raw(self), &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn Present1(&self, syncinterval: u32, presentflags: u32, ppresentparameters: *const DXGI_PRESENT_PARAMETERS) -> ::windows::core::HRESULT {
        (::windows::core::Vtable::vtable(self).Present1)(::windows::core::Vtable::as_raw(self), syncinterval, presentflags, ::core::mem::transmute(ppresentparameters))
    }
    #[doc = "*Required features: `\"Win32_Foundation\"`*"]
    #[cfg(feature = "Win32_Foundation")]
    pub unsafe fn IsTemporaryMonoSupported(&self) -> super::super::Foundation::BOOL {
        (::windows::core::Vtable::vtable(self).IsTemporaryMonoSupported)(::windows::core::Vtable::as_raw(self))
    }
    pub unsafe fn GetRestrictToOutput(&self) -> ::windows::core::Result<IDXGIOutput> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetRestrictToOutput)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<IDXGIOutput>(result__)
    }
    pub unsafe fn SetBackgroundColor(&self, pcolor: *const DXGI_RGBA) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).SetBackgroundColor)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(pcolor)).ok()
    }
    pub unsafe fn GetBackgroundColor(&self) -> ::windows::core::Result<DXGI_RGBA> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetBackgroundColor)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<DXGI_RGBA>(result__)
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn SetRotation(&self, rotation: Common::DXGI_MODE_ROTATION) -> ::windows::core::Result<()> {
        (::windows::core::Vtable::vtable(self).SetRotation)(::windows::core::Vtable::as_raw(self), rotation).ok()
    }
    #[doc = "*Required features: `\"Win32_Graphics_Dxgi_Common\"`*"]
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub unsafe fn GetRotation(&self) -> ::windows::core::Result<Common::DXGI_MODE_ROTATION> {
        let mut result__ = ::core::mem::MaybeUninit::zeroed();
        (::windows::core::Vtable::vtable(self).GetRotation)(::windows::core::Vtable::as_raw(self), ::core::mem::transmute(result__.as_mut_ptr())).from_abi::<Common::DXGI_MODE_ROTATION>(result__)
    }
}
impl ::core::cmp::Eq for IDXGISwapChain1 {}
impl ::core::cmp::PartialEq for IDXGISwapChain1 {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl ::core::clone::Clone for IDXGISwapChain1 {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl ::core::fmt::Debug for IDXGISwapChain1 {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("IDXGISwapChain1").field(&self.0).finish()
    }
}
unsafe impl ::windows::core::Vtable for IDXGISwapChain1 {
    type Vtable = IDXGISwapChain1_Vtbl;
}
unsafe impl ::windows::core::Interface for IDXGISwapChain1 {
    const IID: ::windows::core::GUID = ::windows::core::GUID::from_u128(0x790a45f7_0d42_4876_983a_0a55cfe6f4aa);
}

#[repr(C)]
pub struct IDXGISwapChain1_Vtbl {
    pub base__: IDXGISwapChain_Vtbl,
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub GetDesc1: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_SWAP_CHAIN_DESC1) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common")))]
    GetDesc1: usize,
    #[cfg(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common"))]
    pub GetFullscreenDesc: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_SWAP_CHAIN_FULLSCREEN_DESC) -> ::windows::core::HRESULT,
    #[cfg(not(all(feature = "Win32_Foundation", feature = "Win32_Graphics_Dxgi_Common")))]
    GetFullscreenDesc: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub GetHwnd: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, phwnd: *mut super::super::Foundation::HWND) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    GetHwnd: usize,
    pub GetCoreWindow: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, refiid: *const ::windows::core::GUID, ppunk: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Foundation")]
    pub Present1: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, syncinterval: u32, presentflags: u32, ppresentparameters: *const DXGI_PRESENT_PARAMETERS) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Foundation"))]
    Present1: usize,
    #[cfg(feature = "Win32_Foundation")]
    pub IsTemporaryMonoSupported: unsafe extern "system" fn(this: *mut ::core::ffi::c_void) -> super::super::Foundation::BOOL,
    #[cfg(not(feature = "Win32_Foundation"))]
    IsTemporaryMonoSupported: usize,
    pub GetRestrictToOutput: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pprestricttooutput: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT,
    pub SetBackgroundColor: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pcolor: *const DXGI_RGBA) -> ::windows::core::HRESULT,
    pub GetBackgroundColor: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, pcolor: *mut DXGI_RGBA) -> ::windows::core::HRESULT,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub SetRotation: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, rotation: Common::DXGI_MODE_ROTATION) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    SetRotation: usize,
    #[cfg(feature = "Win32_Graphics_Dxgi_Common")]
    pub GetRotation: unsafe extern "system" fn(this: *mut ::core::ffi::c_void, protation: *mut Common::DXGI_MODE_ROTATION) -> ::windows::core::HRESULT,
    #[cfg(not(feature = "Win32_Graphics_Dxgi_Common"))]
    GetRotation: usize,
}

pub trait IDXGISwapChain1_Impl: Sized + IDXGISwapChain_Impl {
    fn GetDesc1(&self) -> ::windows::core::Result<DXGI_SWAP_CHAIN_DESC1>;
    fn GetFullscreenDesc(&self) -> ::windows::core::Result<DXGI_SWAP_CHAIN_FULLSCREEN_DESC>;
    fn GetHwnd(&self) -> ::windows::core::Result<super::super::Foundation::HWND>;
    fn GetCoreWindow(&self, refiid: *const ::windows::core::GUID, ppunk: *mut *mut ::core::ffi::c_void) -> ::windows::core::Result<()>;
    fn Present1(&self, syncinterval: u32, presentflags: u32, ppresentparameters: *const DXGI_PRESENT_PARAMETERS) -> ::windows::core::HRESULT;
    fn IsTemporaryMonoSupported(&self) -> super::super::Foundation::BOOL;
    fn GetRestrictToOutput(&self) -> ::windows::core::Result<IDXGIOutput>;
    fn SetBackgroundColor(&self, pcolor: *const DXGI_RGBA) -> ::windows::core::Result<()>;
    fn GetBackgroundColor(&self) -> ::windows::core::Result<DXGI_RGBA>;
    fn SetRotation(&self, rotation: Common::DXGI_MODE_ROTATION) -> ::windows::core::Result<()>;
    fn GetRotation(&self) -> ::windows::core::Result<Common::DXGI_MODE_ROTATION>;
}

impl IDXGISwapChain1_Vtbl {
    pub const fn new<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>() -> IDXGISwapChain1_Vtbl {
        unsafe extern "system" fn GetDesc1<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_SWAP_CHAIN_DESC1) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetDesc1() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdesc, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn GetFullscreenDesc<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pdesc: *mut DXGI_SWAP_CHAIN_FULLSCREEN_DESC) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetFullscreenDesc() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pdesc, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn GetHwnd<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, phwnd: *mut super::super::Foundation::HWND) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetHwnd() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(phwnd, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn GetCoreWindow<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, refiid: *const ::windows::core::GUID, ppunk: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.GetCoreWindow(::core::mem::transmute_copy(&refiid), ::core::mem::transmute_copy(&ppunk)).into()
        }
        unsafe extern "system" fn Present1<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, syncinterval: u32, presentflags: u32, ppresentparameters: *const DXGI_PRESENT_PARAMETERS) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.Present1(::core::mem::transmute_copy(&syncinterval), ::core::mem::transmute_copy(&presentflags), ::core::mem::transmute_copy(&ppresentparameters))
        }
        unsafe extern "system" fn IsTemporaryMonoSupported<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void) -> super::super::Foundation::BOOL {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.IsTemporaryMonoSupported()
        }
        unsafe extern "system" fn GetRestrictToOutput<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pprestricttooutput: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetRestrictToOutput() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pprestricttooutput, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn SetBackgroundColor<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pcolor: *const DXGI_RGBA) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetBackgroundColor(::core::mem::transmute_copy(&pcolor)).into()
        }
        unsafe extern "system" fn GetBackgroundColor<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, pcolor: *mut DXGI_RGBA) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetBackgroundColor() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(pcolor, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        unsafe extern "system" fn SetRotation<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, rotation: Common::DXGI_MODE_ROTATION) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            this.SetRotation(::core::mem::transmute_copy(&rotation)).into()
        }
        unsafe extern "system" fn GetRotation<Identity: ::windows::core::IUnknownImpl<Impl = Impl>, Impl: IDXGISwapChain1_Impl, const OFFSET: isize>(this: *mut ::core::ffi::c_void, protation: *mut Common::DXGI_MODE_ROTATION) -> ::windows::core::HRESULT {
            let this = (this as *const *const ()).offset(OFFSET) as *const Identity;
            let this = (*this).get_impl();
            match this.GetRotation() {
                ::core::result::Result::Ok(ok__) => {
                    ::core::ptr::write(protation, ::core::mem::transmute(ok__));
                    ::windows::core::HRESULT(0)
                }
                ::core::result::Result::Err(err) => err.into(),
            }
        }
        Self {
            base__: IDXGISwapChain_Vtbl::new::<Identity, Impl, OFFSET>(),
            GetDesc1: GetDesc1::<Identity, Impl, OFFSET>,
            GetFullscreenDesc: GetFullscreenDesc::<Identity, Impl, OFFSET>,
            GetHwnd: GetHwnd::<Identity, Impl, OFFSET>,
            GetCoreWindow: GetCoreWindow::<Identity, Impl, OFFSET>,
            Present1: Present1::<Identity, Impl, OFFSET>,
            IsTemporaryMonoSupported: IsTemporaryMonoSupported::<Identity, Impl, OFFSET>,
            GetRestrictToOutput: GetRestrictToOutput::<Identity, Impl, OFFSET>,
            SetBackgroundColor: SetBackgroundColor::<Identity, Impl, OFFSET>,
            GetBackgroundColor: GetBackgroundColor::<Identity, Impl, OFFSET>,
            SetRotation: SetRotation::<Identity, Impl, OFFSET>,
            GetRotation: GetRotation::<Identity, Impl, OFFSET>,
        }
    }
    pub fn matches(iid: &windows::core::GUID) -> bool {
        iid == &<IDXGISwapChain1 as ::windows::core::Interface>::IID || iid == &<IDXGIObject as ::windows::core::Interface>::IID || iid == &<IDXGIDeviceSubObject as ::windows::core::Interface>::IID || iid == &<IDXGISwapChain as ::windows::core::Interface>::IID
    }
}

::windows::core::interface_hierarchy!(IDXGISwapChain1, ::windows::core::IUnknown, IDXGIObject, IDXGIDeviceSubObject, IDXGISwapChain);

pub unsafe fn CreateDXGIFactory2<T>(flags: u32) -> ::windows::core::Result<T>
where
    T: ::windows::core::Interface,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn CreateDXGIFactory2(flags: u32, riid: *const ::windows::core::GUID, ppfactory: *mut *mut ::core::ffi::c_void) -> ::windows::core::HRESULT;
    }
    let mut result__ = ::core::option::Option::None;
    CreateDXGIFactory2(flags, &<T as ::windows::core::Interface>::IID, &mut result__ as *mut _ as *mut _).and_some(result__)
}

#[repr(C)]
pub struct DXGI_SWAP_CHAIN_DESC1 {
    pub Width: u32,
    pub Height: u32,
    pub Format: Common::DXGI_FORMAT,
    pub Stereo: super::super::Foundation::BOOL,
    pub SampleDesc: Common::DXGI_SAMPLE_DESC,
    pub BufferUsage: u32,
    pub BufferCount: u32,
    pub Scaling: DXGI_SCALING,
    pub SwapEffect: DXGI_SWAP_EFFECT,
    pub AlphaMode: Common::DXGI_ALPHA_MODE,
    pub Flags: u32,
}
impl ::core::marker::Copy for DXGI_SWAP_CHAIN_DESC1 {}
impl ::core::cmp::Eq for DXGI_SWAP_CHAIN_DESC1 {}
impl ::core::cmp::PartialEq for DXGI_SWAP_CHAIN_DESC1 {
    fn eq(&self, other: &Self) -> bool {
        unsafe { ::windows::core::memcmp(self as *const _ as _, other as *const _ as _, core::mem::size_of::<DXGI_SWAP_CHAIN_DESC1>()) == 0 }
    }
}
impl ::core::clone::Clone for DXGI_SWAP_CHAIN_DESC1 {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_SWAP_CHAIN_DESC1 {
    fn default() -> Self {
        unsafe { ::core::mem::zeroed() }
    }
}
unsafe impl ::windows::core::Abi for DXGI_SWAP_CHAIN_DESC1 {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_SWAP_CHAIN_DESC1 {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct("DXGI_SWAP_CHAIN_DESC1").field("Width", &self.Width).field("Height", &self.Height).field("Format", &self.Format).field("Stereo", &self.Stereo).field("SampleDesc", &self.SampleDesc).field("BufferUsage", &self.BufferUsage).field("BufferCount", &self.BufferCount).field("Scaling", &self.Scaling).field("SwapEffect", &self.SwapEffect).field("AlphaMode", &self.AlphaMode).field("Flags", &self.Flags).finish()
    }
}

pub const DXGI_USAGE_RENDER_TARGET_OUTPUT: u32 = 32u32;

pub const DXGI_SCALING_NONE: DXGI_SCALING = DXGI_SCALING(1i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct DXGI_SCALING(pub i32);
impl ::core::marker::Copy for DXGI_SCALING {}
impl ::core::clone::Clone for DXGI_SCALING {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_SCALING {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for DXGI_SCALING {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_SCALING {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("DXGI_SCALING").field(&self.0).finish()
    }
}

pub const DXGI_SWAP_EFFECT_FLIP_DISCARD: DXGI_SWAP_EFFECT = DXGI_SWAP_EFFECT(4i32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct DXGI_SWAP_EFFECT(pub i32);
impl ::core::marker::Copy for DXGI_SWAP_EFFECT {}
impl ::core::clone::Clone for DXGI_SWAP_EFFECT {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for DXGI_SWAP_EFFECT {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for DXGI_SWAP_EFFECT {
    type Abi = Self;
}
impl ::core::fmt::Debug for DXGI_SWAP_EFFECT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("DXGI_SWAP_EFFECT").field(&self.0).finish()
    }
}

}
}
pub mod System{
pub mod LibraryLoader{
pub unsafe fn GetModuleHandleW<'a, P0>(lpmodulename: P0) -> ::windows::core::Result<super::super::Foundation::HINSTANCE>
where
    P0: ::std::convert::Into<::windows::core::PCWSTR>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetModuleHandleW(lpmodulename: ::windows::core::PCWSTR) -> super::super::Foundation::HINSTANCE;
    }
    let result__ = GetModuleHandleW(lpmodulename.into());
    (!result__.is_invalid()).then(|| result__).ok_or_else(::windows::core::Error::from_win32)
}

pub unsafe fn LoadLibraryA<'a, P0>(lplibfilename: P0) -> ::windows::core::Result<super::super::Foundation::HINSTANCE>
where
    P0: ::std::convert::Into<::windows::core::PCSTR>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn LoadLibraryA(lplibfilename: ::windows::core::PCSTR) -> super::super::Foundation::HINSTANCE;
    }
    let result__ = LoadLibraryA(lplibfilename.into());
    (!result__.is_invalid()).then(|| result__).ok_or_else(::windows::core::Error::from_win32)
}

pub unsafe fn GetProcAddress<'a, P0, P1>(hmodule: P0, lpprocname: P1) -> super::super::Foundation::FARPROC
where
    P0: ::std::convert::Into<super::super::Foundation::HINSTANCE>,
    P1: ::std::convert::Into<::windows::core::PCSTR>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetProcAddress(hmodule: super::super::Foundation::HINSTANCE, lpprocname: ::windows::core::PCSTR) -> super::super::Foundation::FARPROC;
    }
    GetProcAddress(hmodule.into(), lpprocname.into())
}

}
pub mod Performance{
pub unsafe fn QueryPerformanceCounter(lpperformancecount: *mut i64) -> super::super::Foundation::BOOL {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn QueryPerformanceCounter(lpperformancecount: *mut i64) -> super::super::Foundation::BOOL;
    }
    QueryPerformanceCounter(::core::mem::transmute(lpperformancecount))
}

pub unsafe fn QueryPerformanceFrequency(lpfrequency: *mut i64) -> super::super::Foundation::BOOL {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn QueryPerformanceFrequency(lpfrequency: *mut i64) -> super::super::Foundation::BOOL;
    }
    QueryPerformanceFrequency(::core::mem::transmute(lpfrequency))
}

}
pub mod Memory{
pub unsafe fn GlobalLock(hmem: isize) -> *mut ::core::ffi::c_void {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GlobalLock(hmem: isize) -> *mut ::core::ffi::c_void;
    }
    GlobalLock(hmem)
}

pub unsafe fn GlobalAlloc(uflags: GLOBAL_ALLOC_FLAGS, dwbytes: usize) -> isize {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GlobalAlloc(uflags: GLOBAL_ALLOC_FLAGS, dwbytes: usize) -> isize;
    }
    GlobalAlloc(uflags, dwbytes)
}

pub unsafe fn GlobalSize(hmem: isize) -> usize {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GlobalSize(hmem: isize) -> usize;
    }
    GlobalSize(hmem)
}

pub unsafe fn GlobalUnlock(hmem: isize) -> super::super::Foundation::BOOL {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GlobalUnlock(hmem: isize) -> super::super::Foundation::BOOL;
    }
    GlobalUnlock(hmem)
}

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct GLOBAL_ALLOC_FLAGS(pub u32);
impl ::core::marker::Copy for GLOBAL_ALLOC_FLAGS {}
impl ::core::clone::Clone for GLOBAL_ALLOC_FLAGS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for GLOBAL_ALLOC_FLAGS {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for GLOBAL_ALLOC_FLAGS {
    type Abi = Self;
}
impl ::core::fmt::Debug for GLOBAL_ALLOC_FLAGS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("GLOBAL_ALLOC_FLAGS").field(&self.0).finish()
    }
}
impl ::core::ops::BitOr for GLOBAL_ALLOC_FLAGS {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl ::core::ops::BitAnd for GLOBAL_ALLOC_FLAGS {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl ::core::ops::BitOrAssign for GLOBAL_ALLOC_FLAGS {
    fn bitor_assign(&mut self, other: Self) {
        self.0.bitor_assign(other.0)
    }
}
impl ::core::ops::BitAndAssign for GLOBAL_ALLOC_FLAGS {
    fn bitand_assign(&mut self, other: Self) {
        self.0.bitand_assign(other.0)
    }
}
impl ::core::ops::Not for GLOBAL_ALLOC_FLAGS {
    type Output = Self;
    fn not(self) -> Self {
        Self(self.0.not())
    }
}

}
pub mod SystemServices{
pub const CF_UNICODETEXT: CLIPBOARD_FORMATS = CLIPBOARD_FORMATS(13u32);

#[derive(PartialEq, Eq)]#[repr(transparent)]
pub struct CLIPBOARD_FORMATS(pub u32);
impl ::core::marker::Copy for CLIPBOARD_FORMATS {}
impl ::core::clone::Clone for CLIPBOARD_FORMATS {
    fn clone(&self) -> Self {
        *self
    }
}
impl ::core::default::Default for CLIPBOARD_FORMATS {
    fn default() -> Self {
        Self(0)
    }
}
unsafe impl ::windows::core::Abi for CLIPBOARD_FORMATS {
    type Abi = Self;
}
impl ::core::fmt::Debug for CLIPBOARD_FORMATS {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_tuple("CLIPBOARD_FORMATS").field(&self.0).finish()
    }
}

}
pub mod WindowsProgramming{
pub const GMEM_DDESHARE: u32 = 8192u32;

}
pub mod DataExchange{
pub unsafe fn OpenClipboard<'a, P0>(hwndnewowner: P0) -> super::super::Foundation::BOOL
where
    P0: ::std::convert::Into<super::super::Foundation::HWND>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn OpenClipboard(hwndnewowner: super::super::Foundation::HWND) -> super::super::Foundation::BOOL;
    }
    OpenClipboard(hwndnewowner.into())
}

pub unsafe fn EmptyClipboard() -> super::super::Foundation::BOOL {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn EmptyClipboard() -> super::super::Foundation::BOOL;
    }
    EmptyClipboard()
}

pub unsafe fn GetClipboardData(uformat: u32) -> ::windows::core::Result<super::super::Foundation::HANDLE> {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn GetClipboardData(uformat: u32) -> super::super::Foundation::HANDLE;
    }
    let result__ = GetClipboardData(uformat);
    (!result__.is_invalid()).then(|| result__).ok_or_else(::windows::core::Error::from_win32)
}

pub unsafe fn SetClipboardData<'a, P0>(uformat: u32, hmem: P0) -> ::windows::core::Result<super::super::Foundation::HANDLE>
where
    P0: ::std::convert::Into<super::super::Foundation::HANDLE>,
{
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn SetClipboardData(uformat: u32, hmem: super::super::Foundation::HANDLE) -> super::super::Foundation::HANDLE;
    }
    let result__ = SetClipboardData(uformat, hmem.into());
    (!result__.is_invalid()).then(|| result__).ok_or_else(::windows::core::Error::from_win32)
}

pub unsafe fn CloseClipboard() -> super::super::Foundation::BOOL {
    #[cfg_attr(windows, link(name = "windows"))]
    extern "system" {
        fn CloseClipboard() -> super::super::Foundation::BOOL;
    }
    CloseClipboard()
}

}
}

}
