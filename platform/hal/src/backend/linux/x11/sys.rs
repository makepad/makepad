//! Reachable Xlib/Xcursor ABI declarations from os/linux/x11/x11_sys.rs.
//! Window and XEvent layout are the native bindings; unrelated GLX/Xlib APIs are omitted.
#![allow(non_upper_case_globals)]

use std::os::raw::{c_char, c_int, c_long, c_short, c_uchar, c_uint, c_ulong, c_ushort, c_void};

#[repr(C)]
pub struct XColor {
    pub pixel: c_ulong,
    pub red: c_ushort,
    pub green: c_ushort,
    pub blue: c_ushort,
    pub flags: c_char,
    pub pad: c_char,
}

pub type Display = _XDisplay;

pub type XID = c_ulong;

pub type Window = XID;

pub type Colormap = XID;

pub type KeySym = XID;

pub type Pixmap = XID;

pub type Cursor = XID;

pub type Time = c_ulong;

pub type XIM = *mut _XIM;

pub type Atom = c_ulong;

pub type XEvent = _XEvent;

pub type XIC = *mut _XIC;

pub type XFontSet = *mut _XOC;

pub type XExtData = _XExtData;

pub type XPointer = *mut c_char;

pub type VisualID = c_ulong;

pub type XrmDatabase = *mut _XrmHashBucketRec;

pub type XKeyPressedEvent = XKeyEvent;

pub type GC = *mut _XGC;

pub const SelectionNotify: u32 = 31;

pub const SelectionRequest: u32 = 30;

pub const DestroyNotify: u32 = 17;

pub const ConfigureNotify: u32 = 22;

pub const LeaveNotify: u32 = 8;

pub const FocusOut: u32 = 10;

pub const MotionNotify: u32 = 6;

pub const InputOutput: u32 = 1;

pub const ClientMessage: u32 = 33;

pub const KeyPress: u32 = 2;

pub const KeyRelease: u32 = 3;

pub const ButtonPress: u32 = 4;

pub const ButtonRelease: u32 = 5;

pub const Expose: u32 = 12;

pub const CWBorderPixel: u32 = 8;

pub const CWColormap: u32 = 8192;

pub const CWEventMask: u32 = 2048;

pub const CWOverrideRedirect: u32 = 512;

pub const GrabModeAsync: i32 = 1;

pub const SubstructureNotifyMask: u32 = 524288;

pub const SubstructureRedirectMask: u32 = 1048576;

pub const ExposureMask: u32 = 32768;

pub const StructureNotifyMask: u32 = 131072;

pub const PointerMotionMask: u32 = 64;

pub const ButtonPressMask: u32 = 4;

pub const ButtonReleaseMask: u32 = 8;

pub const KeyPressMask: u32 = 1;

pub const KeyReleaseMask: u32 = 2;

pub const FocusChangeMask: u32 = 2097152;

pub const EnterWindowMask: u32 = 16;

pub const LeaveWindowMask: u32 = 32;

pub const XBufferOverflow: i32 = -1;

pub const FocusIn: u32 = 9;

pub const XIMPreeditCallbacks: u32 = 2;

pub const XIMPreeditPosition: u32 = 4;

pub const XIMPreeditNothing: u32 = 8;

pub const XIMStatusNothing: u32 = 1024;

pub const XIMStatusNone: u32 = 2048;

pub type XIMStyle = c_ulong;

#[repr(C)]
pub struct XIMStyles {
    pub count_styles: c_ushort,
    pub supported_styles: *mut XIMStyle,
}

pub const XNInputStyle: &'static [u8; 11usize] = b"inputStyle\0";

pub const XNQueryInputStyle: &'static [u8; 16usize] = b"queryInputStyle\0";

pub const XNClientWindow: &'static [u8; 13usize] = b"clientWindow\0";

pub const XNFocusWindow: &'static [u8; 12usize] = b"focusWindow\0";

pub const XNPreeditAttributes: &'static [u8; 18usize] = b"preeditAttributes\0";

pub const XNSpotLocation: &'static [u8; 13usize] = b"spotLocation\0";

pub const XNArea: &'static [u8; 5usize] = b"area\0";

pub const XNFontSet: &'static [u8; 8usize] = b"fontSet\0";

pub const XNPreeditStartCallback: &[u8] = b"preeditStartCallback\0";

pub const XNPreeditDrawCallback: &[u8] = b"preeditDrawCallback\0";

pub const XNPreeditDoneCallback: &[u8] = b"preeditDoneCallback\0";

pub const XNPreeditCaretCallback: &[u8] = b"preeditCaretCallback\0";

pub type XIMFeedback = c_ulong;

pub type XIMProc = Option<unsafe extern "C" fn(arg1: XIC, arg2: XPointer, arg3: XPointer)>;

pub type XErrorHandler =
    Option<unsafe extern "C" fn(display: *mut Display, event: *mut XErrorEvent) -> c_int>;

#[repr(C)]
pub struct XIMCallback {
    pub client_data: XPointer,
    pub callback: XIMProc,
}

#[repr(C)]
pub struct XIMText {
    pub length: c_ushort,
    pub feedback: *mut XIMFeedback,
    pub encoding_is_wchar: c_int,
    // C union of `char *multi_byte` / `wchar_t *wide_char`; we only read the
    // multi-byte form (guarded by `encoding_is_wchar == 0`).
    pub string: *mut c_char,
}

#[repr(C)]
pub struct XIMPreeditDrawCallbackStruct {
    pub caret: c_int,
    pub chg_first: c_int,
    pub chg_length: c_int,
    pub text: *mut XIMText,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XPoint {
    pub x: c_short,
    pub y: c_short,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XRectangle {
    pub x: c_short,
    pub y: c_short,
    pub width: u16,
    pub height: u16,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XrmHashBucketRec {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XDisplay {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XIM {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XIC {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XOC {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XExtData {
    pub number: c_int,
    pub next: *mut _XExtData,
    pub free_private:
        ::std::option::Option<unsafe extern "C" fn(extension: *mut _XExtData) -> c_int>,
    pub private_data: XPointer,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XrmValue {
    pub size: c_uint,
    pub addr: XPointer,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XGC {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Depth {
    pub depth: c_int,
    pub nvisuals: c_int,
    pub visuals: *mut Visual,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Screen {
    pub ext_data: *mut XExtData,
    pub display: *mut _XDisplay,
    pub root: Window,
    pub width: c_int,
    pub height: c_int,
    pub mwidth: c_int,
    pub mheight: c_int,
    pub ndepths: c_int,
    pub depths: *mut Depth,
    pub root_depth: c_int,
    pub root_visual: *mut Visual,
    pub default_gc: GC,
    pub cmap: Colormap,
    pub white_pixel: c_ulong,
    pub black_pixel: c_ulong,
    pub max_maps: c_int,
    pub min_maps: c_int,
    pub backing_store: c_int,
    pub save_unders: c_int,
    pub root_input_mask: c_long,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XWindowAttributes {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub depth: c_int,
    pub visual: *mut Visual,
    pub root: Window,
    pub class: c_int,
    pub bit_gravity: c_int,
    pub win_gravity: c_int,
    pub backing_store: c_int,
    pub backing_planes: c_ulong,
    pub backing_pixel: c_ulong,
    pub save_under: c_int,
    pub colormap: Colormap,
    pub map_installed: c_int,
    pub map_state: c_int,
    pub all_event_masks: c_long,
    pub your_event_mask: c_long,
    pub do_not_propagate_mask: c_long,
    pub override_redirect: c_int,
    pub screen: *mut Screen,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Visual {
    pub ext_data: *mut XExtData,
    pub visualid: VisualID,
    pub class: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
    pub bits_per_rgb: c_int,
    pub map_entries: c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XSetWindowAttributes {
    pub background_pixmap: Pixmap,
    pub background_pixel: c_ulong,
    pub border_pixmap: Pixmap,
    pub border_pixel: c_ulong,
    pub bit_gravity: c_int,
    pub win_gravity: c_int,
    pub backing_store: c_int,
    pub backing_planes: c_ulong,
    pub backing_pixel: c_ulong,
    pub save_under: c_int,
    pub event_mask: c_long,
    pub do_not_propagate_mask: c_long,
    pub override_redirect: c_int,
    pub colormap: Colormap,
    pub cursor: Cursor,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XSelectionEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub requestor: Window,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: Time,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XMotionEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub is_hint: c_char,
    pub same_screen: c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XButtonEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub button: c_uint,
    pub same_screen: c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XKeyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub keycode: c_uint,
    pub same_screen: c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XAnyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Window,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XFocusChangeEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Window,
    pub mode: c_int,
    pub detail: c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XDestroyWindowEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XConfigureEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub above: Window,
    pub override_redirect: c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XPropertyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Window,
    pub atom: Atom,
    pub time: Time,
    pub state: c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XSelectionRequestEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub owner: Window,
    pub requestor: Window,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: Time,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct XClientMessageEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Window,
    pub message_type: Atom,
    pub format: c_int,
    pub data: XClientMessageEvent__bindgen_ty_1,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union XClientMessageEvent__bindgen_ty_1 {
    pub b: [c_char; 20usize],
    pub s: [c_short; 10usize],
    pub l: [c_long; 5usize],
    _bindgen_union_align: [u64; 5usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XErrorEvent {
    pub type_: c_int,
    pub display: *mut Display,
    pub resourceid: XID,
    pub serial: c_ulong,
    pub error_code: c_uchar,
    pub request_code: c_uchar,
    pub minor_code: c_uchar,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XGenericEventCookie {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub extension: c_int,
    pub evtype: c_int,
    pub cookie: c_uint,
    pub data: *mut c_void,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union _XEvent {
    pub type_: c_int,
    pub xany: XAnyEvent,
    pub xkey: XKeyEvent,
    pub xbutton: XButtonEvent,
    pub xmotion: XMotionEvent,
    pub xfocus: XFocusChangeEvent,
    pub xdestroywindow: XDestroyWindowEvent,
    pub xconfigure: XConfigureEvent,
    pub xproperty: XPropertyEvent,
    pub xselectionrequest: XSelectionRequestEvent,
    pub xselection: XSelectionEvent,
    pub xclient: XClientMessageEvent,
    pub xcookie: XGenericEventCookie,
    pub pad: [c_long; 24usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XWMHints {
    pub flags: c_long,
    pub input: c_int,
    pub initial_state: c_int,
    pub icon_pixmap: Pixmap,
    pub icon_window: Window,
    pub icon_x: c_int,
    pub icon_y: c_int,
    pub icon_mask: Pixmap,
    pub window_group: XID,
}

#[link(name = "X11")]
extern "C" {
    pub fn XInitThreads() -> c_int;
    pub fn XDefaultVisual(display: *mut Display, screen: c_int) -> *mut Visual;
    pub fn XDefaultDepth(display: *mut Display, screen: c_int) -> c_int;
    pub fn XSelectInput(display: *mut Display, window: Window, mask: c_long) -> c_int;
    pub fn XDeleteProperty(display: *mut Display, window: Window, property: Atom) -> c_int;
    pub fn XFreeColormap(display: *mut Display, colormap: Colormap) -> c_int;
    pub fn XFreeFontSet(display: *mut Display, font_set: XFontSet);
    pub fn XrmDestroyDatabase(db: XrmDatabase);
    pub fn XWarpPointer(
        display: *mut Display,
        src: Window,
        dest: Window,
        sx: c_int,
        sy: c_int,
        sw: c_uint,
        sh: c_uint,
        dx: c_int,
        dy: c_int,
    ) -> c_int;
    pub fn XQueryPointer(
        display: *mut Display,
        window: Window,
        root: *mut Window,
        child: *mut Window,
        rx: *mut c_int,
        ry: *mut c_int,
        x: *mut c_int,
        y: *mut c_int,
        mask: *mut c_uint,
    ) -> c_int;
    pub fn XCreateBitmapFromData(
        display: *mut Display,
        window: Window,
        data: *const c_char,
        w: c_uint,
        h: c_uint,
    ) -> Pixmap;
    pub fn XCreatePixmapCursor(
        display: *mut Display,
        src: Pixmap,
        mask: Pixmap,
        fg: *mut XColor,
        bg: *mut XColor,
        x: c_uint,
        y: c_uint,
    ) -> Cursor;
    pub fn XFreePixmap(display: *mut Display, pixmap: Pixmap) -> c_int;
    pub fn XMaxRequestSize(display: *mut Display) -> c_long;
    pub fn XSetWMHints(display: *mut Display, window: Window, hints: *mut XWMHints) -> c_int;
    pub fn XCreateSimpleWindow(
        display: *mut Display,
        parent: Window,
        x: c_int,
        y: c_int,
        w: c_uint,
        h: c_uint,
        border: c_uint,
        border_color: c_ulong,
        bg: c_ulong,
    ) -> Window;
    pub fn XkbKeycodeToKeysym(
        display: *mut Display,
        key: c_uchar,
        group: c_int,
        level: c_int,
    ) -> KeySym;
    pub fn XOpenDisplay(arg1: *const c_char) -> *mut Display;
    pub fn XConnectionNumber(arg1: *mut Display) -> c_int;
    pub fn XOpenIM(
        arg1: *mut Display,
        arg2: *mut _XrmHashBucketRec,
        arg3: *mut c_char,
        arg4: *mut c_char,
    ) -> XIM;
    pub fn XGetIMValues(arg1: XIM, ...) -> *mut c_char;
    pub fn XCreateFontSet(
        arg1: *mut Display,
        arg2: *const c_char,
        arg3: *mut *mut *mut c_char,
        arg4: *mut c_int,
        arg5: *mut *mut c_char,
    ) -> XFontSet;
    pub fn XFreeStringList(arg1: *mut *mut c_char);
    pub fn XInternAtom(arg1: *mut Display, arg2: *const c_char, arg3: c_int) -> Atom;
    pub fn XrmInitialize();
    pub fn XCloseIM(arg1: XIM) -> c_int;
    pub fn XCloseDisplay(arg1: *mut Display) -> c_int;
    pub fn XPending(arg1: *mut Display) -> c_int;
    pub fn XNextEvent(arg1: *mut Display, arg2: *mut XEvent) -> c_int;
    pub fn XPeekEvent(arg1: *mut Display, arg2: *mut XEvent) -> c_int;
    pub fn XGetWindowProperty(
        arg1: *mut Display,
        arg2: Window,
        arg3: Atom,
        arg4: c_long,
        arg5: c_long,
        arg6: c_int,
        arg7: Atom,
        arg8: *mut Atom,
        arg9: *mut c_int,
        arg10: *mut c_ulong,
        arg11: *mut c_ulong,
        arg12: *mut *mut c_uchar,
    ) -> c_int;
    pub fn XFree(arg1: *mut c_void) -> c_int;
    pub fn XChangeProperty(
        display: *mut Display,
        window: Window,
        property: Atom,
        type_: Atom,
        format: c_int,
        mode: c_int,
        data: *const c_uchar,
        num_elements: c_int,
    ) -> c_int;
    pub fn XSendEvent(
        arg1: *mut Display,
        arg2: Window,
        arg3: c_int,
        arg4: c_long,
        arg5: *mut XEvent,
    ) -> c_int;
    pub fn XDefaultScreen(arg1: *mut Display) -> c_int;
    pub fn XRootWindow(arg1: *mut Display, arg2: c_int) -> Window;
    pub fn XCreateColormap(
        arg1: *mut Display,
        arg2: Window,
        arg3: *mut Visual,
        arg4: c_int,
    ) -> Colormap;
    pub fn XCreateWindow(
        arg1: *mut Display,
        arg2: Window,
        arg3: c_int,
        arg4: c_int,
        arg5: c_uint,
        arg6: c_uint,
        arg7: c_uint,
        arg8: c_int,
        arg9: c_uint,
        arg10: *mut Visual,
        arg11: c_ulong,
        arg12: *mut XSetWindowAttributes,
    ) -> Window;
    pub fn XSetWMProtocols(arg1: *mut Display, arg2: Window, arg3: *mut Atom, arg4: c_int)
        -> c_int;
    pub fn XMapWindow(arg1: *mut Display, arg2: Window) -> c_int;
    pub fn XUnmapWindow(arg1: *mut Display, arg2: Window) -> c_int;
    pub fn XMoveWindow(display: *mut Display, window: Window, x: c_int, y: c_int);
    pub fn XResizeWindow(display: *mut Display, window: Window, width: c_uint, height: c_uint);
    pub fn XSetErrorHandler(handler: XErrorHandler) -> XErrorHandler;
    pub fn XFlush(arg1: *mut Display) -> c_int;
    pub fn XCreateIC(arg1: XIM, ...) -> XIC;
    pub fn XDestroyIC(arg1: XIC);
    pub fn XSetLocaleModifiers(arg1: *const c_char) -> *const c_char;
    pub fn XSetICFocus(arg1: XIC);
    pub fn XUnsetICFocus(arg1: XIC);
    pub fn XSetICValues(arg1: XIC, ...) -> *mut c_char;
    pub fn XVaCreateNestedList(arg1: c_int, ...) -> *mut c_void;
    pub fn XFilterEvent(arg1: *mut XEvent, arg2: Window) -> c_int;
    pub fn XDestroyWindow(arg1: *mut Display, arg2: Window) -> c_int;
    pub fn XIconifyWindow(arg1: *mut Display, arg2: Window, arg3: c_int) -> c_int;
    pub fn XGetWindowAttributes(
        arg1: *mut Display,
        arg2: Window,
        arg3: *mut XWindowAttributes,
    ) -> c_int;
    pub fn XTranslateCoordinates(
        display: *mut Display,
        src_window: Window,
        dest_window: Window,
        src_x: c_int,
        src_y: c_int,
        dest_x_return: *mut c_int,
        dest_y_return: *mut c_int,
        child_return: *mut Window,
    ) -> c_int;
    pub fn XResourceManagerString(arg1: *mut Display) -> *mut c_char;
    pub fn XrmGetStringDatabase(arg1: *const c_char) -> XrmDatabase;
    pub fn XrmGetResource(
        arg1: XrmDatabase,
        arg2: *const c_char,
        arg3: *const c_char,
        arg4: *mut *mut c_char,
        arg5: *mut XrmValue,
    ) -> c_int;
    pub fn XConvertSelection(
        arg1: *mut Display,
        arg2: Atom,
        arg3: Atom,
        arg4: Atom,
        arg5: Window,
        arg6: Time,
    ) -> c_int;
    pub fn XGrabPointer(
        arg1: *mut Display,
        arg2: Window,
        arg3: c_int,
        arg4: c_uint,
        arg5: c_int,
        arg6: c_int,
        arg7: Window,
        arg8: Cursor,
        arg9: Time,
    ) -> c_int;
    pub fn XUngrabPointer(arg1: *mut Display, arg2: Time) -> c_int;
    pub fn XSetSelectionOwner(arg1: *mut Display, arg2: Atom, arg3: Window, arg4: Time) -> c_int;
    pub fn Xutf8LookupString(
        arg1: XIC,
        arg2: *mut XKeyPressedEvent,
        arg3: *mut c_char,
        arg4: c_int,
        arg5: *mut KeySym,
        arg6: *mut c_int,
    ) -> c_int;
    pub fn XDefineCursor(arg1: *mut Display, arg2: Window, arg3: Cursor) -> c_int;
    pub fn XFreeCursor(arg1: *mut Display, arg2: Cursor) -> c_int;
}

#[link(name = "Xcursor")]
extern "C" {
    pub fn XcursorLibraryLoadCursor(dpy: *mut Display, file: *const c_char) -> Cursor;
}

#[link(name = "c")]
extern "C" {
    pub fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XVisualInfo {
    pub visual: *mut Visual,
    pub visualid: VisualID,
    pub screen: c_int,
    pub depth: c_int,
    pub class: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
    pub colormap_size: c_int,
    pub bits_per_rgb: c_int,
}
#[link(name = "X11")]
extern "C" {
    pub fn XMatchVisualInfo(
        display: *mut Display,
        screen: c_int,
        depth: c_int,
        class: c_int,
        info: *mut XVisualInfo,
    ) -> c_int;
}

#[link(name = "X11")]
extern "C" {
    pub fn XMapRaised(display: *mut Display, window: Window) -> c_int;
}
