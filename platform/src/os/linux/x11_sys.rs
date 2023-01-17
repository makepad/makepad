pub type Display = _XDisplay;

pub type XID = ::std::os::raw::c_ulong;
pub type Window = XID;
pub type Time = ::std::os::raw::c_ulong;
pub type Drawable = XID;
pub type Colormap = XID;
pub type XIM = *mut _XIM;
pub type Atom = ::std::os::raw::c_ulong;
pub type XEvent = _XEvent;
pub type XIC = *mut _XIC;
pub type Pixmap = XID;
pub type Cursor = XID;

pub const SelectionNotify: u32 = 31;
pub const AnyPropertyType: u32 = 0;
pub const SelectionRequest: u32 = 30;
pub const PropModeReplace: u32 = 0;
pub const DestroyNotify: u32 = 17;
pub const ConfigureNotify: u32 = 22;
pub const EnterNotify: u32 = 7;
pub const LeaveNotify: u32 = 8;
pub const MotionNotify: u32 = 6;

extern "C" {
    pub fn XOpenDisplay(arg1: *const ::std::os::raw::c_char) -> *mut Display;
    pub fn XConnectionNumber(arg1: *mut Display) -> ::std::os::raw::c_int;
    pub fn XOpenIM(
        arg1: *mut Display,
        arg2: *mut _XrmHashBucketRec,
        arg3: *mut ::std::os::raw::c_char,
        arg4: *mut ::std::os::raw::c_char,
    ) -> XIM;
    pub fn XInternAtom(
        arg1: *mut Display,
        arg2: *const ::std::os::raw::c_char,
        arg3: ::std::os::raw::c_int,
    ) -> Atom;
    pub fn XrmInitialize();
    pub fn XCloseIM(arg1: XIM) -> ::std::os::raw::c_int;
    pub fn XCloseDisplay(arg1: *mut Display) -> ::std::os::raw::c_int;
    pub fn XPending(arg1: *mut Display) -> ::std::os::raw::c_int;
    pub fn XNextEvent(arg1: *mut Display, arg2: *mut XEvent) -> ::std::os::raw::c_int;
    pub fn XGetWindowProperty(
        arg1: *mut Display,
        arg2: Window,
        arg3: Atom,
        arg4: ::std::os::raw::c_long,
        arg5: ::std::os::raw::c_long,
        arg6: ::std::os::raw::c_int,
        arg7: Atom,
        arg8: *mut Atom,
        arg9: *mut ::std::os::raw::c_int,
        arg10: *mut ::std::os::raw::c_ulong,
        arg11: *mut ::std::os::raw::c_ulong,
        arg12: *mut *mut ::std::os::raw::c_uchar,
    ) -> ::std::os::raw::c_int;
    pub fn XFree(arg1: *mut ::std::os::raw::c_void) -> ::std::os::raw::c_int;    
    pub fn XChangeProperty(
        arg1: *mut Display,
        arg2: Window,
        arg3: Atom,
        arg4: Atom,
        arg5: ::std::os::raw::c_int,
        arg6: ::std::os::raw::c_int,
        arg7: *const ::std::os::raw::c_uchar,
        arg8: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;    
    pub fn XSendEvent(
        arg1: *mut Display,
        arg2: Window,
        arg3: ::std::os::raw::c_int,
        arg4: ::std::os::raw::c_long,
        arg5: *mut XEvent,
    ) -> ::std::os::raw::c_int;
    pub fn XDefaultScreen(arg1: *mut Display) -> ::std::os::raw::c_int;
    pub fn XRootWindow(arg1: *mut Display, arg2: ::std::os::raw::c_int) -> Window;
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
pub struct _XrmHashBucketRec {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XIC {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XSetWindowAttributes {
    pub background_pixmap: Pixmap,
    pub background_pixel: ::std::os::raw::c_ulong,
    pub border_pixmap: Pixmap,
    pub border_pixel: ::std::os::raw::c_ulong,
    pub bit_gravity: ::std::os::raw::c_int,
    pub win_gravity: ::std::os::raw::c_int,
    pub backing_store: ::std::os::raw::c_int,
    pub backing_planes: ::std::os::raw::c_ulong,
    pub backing_pixel: ::std::os::raw::c_ulong,
    pub save_under: ::std::os::raw::c_int,
    pub event_mask: ::std::os::raw::c_long,
    pub do_not_propagate_mask: ::std::os::raw::c_long,
    pub override_redirect: ::std::os::raw::c_int,
    pub colormap: Colormap,
    pub cursor: Cursor,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XVisualInfo {
    pub visual: *mut Visual,
    pub visualid: VisualID,
    pub screen: ::std::os::raw::c_int,
    pub depth: ::std::os::raw::c_int,
    pub class: ::std::os::raw::c_int,
    pub red_mask: ::std::os::raw::c_ulong,
    pub green_mask: ::std::os::raw::c_ulong,
    pub blue_mask: ::std::os::raw::c_ulong,
    pub colormap_size: ::std::os::raw::c_int,
    pub bits_per_rgb: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XSelectionEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub requestor: Window,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: Time,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XNoExposeEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub drawable: Drawable,
    pub major_code: ::std::os::raw::c_int,
    pub minor_code: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XCrossingEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub x_root: ::std::os::raw::c_int,
    pub y_root: ::std::os::raw::c_int,
    pub mode: ::std::os::raw::c_int,
    pub detail: ::std::os::raw::c_int,
    pub same_screen: ::std::os::raw::c_int,
    pub focus: ::std::os::raw::c_int,
    pub state: ::std::os::raw::c_uint,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XMotionEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub x_root: ::std::os::raw::c_int,
    pub y_root: ::std::os::raw::c_int,
    pub state: ::std::os::raw::c_uint,
    pub is_hint: ::std::os::raw::c_char,
    pub same_screen: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XButtonEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub x_root: ::std::os::raw::c_int,
    pub y_root: ::std::os::raw::c_int,
    pub state: ::std::os::raw::c_uint,
    pub button: ::std::os::raw::c_uint,
    pub same_screen: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XKeyEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub x_root: ::std::os::raw::c_int,
    pub y_root: ::std::os::raw::c_int,
    pub state: ::std::os::raw::c_uint,
    pub keycode: ::std::os::raw::c_uint,
    pub same_screen: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XAnyEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XFocusChangeEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub mode: ::std::os::raw::c_int,
    pub detail: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XExposeEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub width: ::std::os::raw::c_int,
    pub height: ::std::os::raw::c_int,
    pub count: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XGraphicsExposeEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub drawable: Drawable,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub width: ::std::os::raw::c_int,
    pub height: ::std::os::raw::c_int,
    pub count: ::std::os::raw::c_int,
    pub major_code: ::std::os::raw::c_int,
    pub minor_code: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XVisibilityEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub state: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XCreateWindowEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub parent: Window,
    pub window: Window,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub width: ::std::os::raw::c_int,
    pub height: ::std::os::raw::c_int,
    pub border_width: ::std::os::raw::c_int,
    pub override_redirect: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XDestroyWindowEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XUnmapEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub from_configure: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XMapEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub override_redirect: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XMapRequestEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub parent: Window,
    pub window: Window,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XReparentEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub parent: Window,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub override_redirect: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XConfigureEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub width: ::std::os::raw::c_int,
    pub height: ::std::os::raw::c_int,
    pub border_width: ::std::os::raw::c_int,
    pub above: Window,
    pub override_redirect: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XGravityEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XResizeRequestEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub width: ::std::os::raw::c_int,
    pub height: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XConfigureRequestEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub parent: Window,
    pub window: Window,
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub width: ::std::os::raw::c_int,
    pub height: ::std::os::raw::c_int,
    pub border_width: ::std::os::raw::c_int,
    pub above: Window,
    pub detail: ::std::os::raw::c_int,
    pub value_mask: ::std::os::raw::c_ulong,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XCirculateEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub place: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XCirculateRequestEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub parent: Window,
    pub window: Window,
    pub place: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XPropertyEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub atom: Atom,
    pub time: Time,
    pub state: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XSelectionClearEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub selection: Atom,
    pub time: Time,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XSelectionRequestEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub owner: Window,
    pub requestor: Window,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: Time,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XColormapEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub colormap: Colormap,
    pub new: ::std::os::raw::c_int,
    pub state: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct XClientMessageEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub message_type: Atom,
    pub format: ::std::os::raw::c_int,
    pub data: XClientMessageEvent__bindgen_ty_1,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union XClientMessageEvent__bindgen_ty_1 {
    pub b: [::std::os::raw::c_char; 20usize],
    pub s: [::std::os::raw::c_short; 10usize],
    pub l: [::std::os::raw::c_long; 5usize],
    _bindgen_union_align: [u64; 5usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XMappingEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub request: ::std::os::raw::c_int,
    pub first_keycode: ::std::os::raw::c_int,
    pub count: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XErrorEvent {
    pub type_: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub resourceid: XID,
    pub serial: ::std::os::raw::c_ulong,
    pub error_code: ::std::os::raw::c_uchar,
    pub request_code: ::std::os::raw::c_uchar,
    pub minor_code: ::std::os::raw::c_uchar,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XKeymapEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub window: Window,
    pub key_vector: [::std::os::raw::c_char; 32usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XGenericEvent {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub extension: ::std::os::raw::c_int,
    pub evtype: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XGenericEventCookie {
    pub type_: ::std::os::raw::c_int,
    pub serial: ::std::os::raw::c_ulong,
    pub send_event: ::std::os::raw::c_int,
    pub display: *mut Display,
    pub extension: ::std::os::raw::c_int,
    pub evtype: ::std::os::raw::c_int,
    pub cookie: ::std::os::raw::c_uint,
    pub data: *mut ::std::os::raw::c_void,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union _XEvent {
    pub type_: ::std::os::raw::c_int,
    pub xany: XAnyEvent,
    pub xkey: XKeyEvent,
    pub xbutton: XButtonEvent,
    pub xmotion: XMotionEvent,
    pub xcrossing: XCrossingEvent,
    pub xfocus: XFocusChangeEvent,
    pub xexpose: XExposeEvent,
    pub xgraphicsexpose: XGraphicsExposeEvent,
    pub xnoexpose: XNoExposeEvent,
    pub xvisibility: XVisibilityEvent,
    pub xcreatewindow: XCreateWindowEvent,
    pub xdestroywindow: XDestroyWindowEvent,
    pub xunmap: XUnmapEvent,
    pub xmap: XMapEvent,
    pub xmaprequest: XMapRequestEvent,
    pub xreparent: XReparentEvent,
    pub xconfigure: XConfigureEvent,
    pub xgravity: XGravityEvent,
    pub xresizerequest: XResizeRequestEvent,
    pub xconfigurerequest: XConfigureRequestEvent,
    pub xcirculate: XCirculateEvent,
    pub xcirculaterequest: XCirculateRequestEvent,
    pub xproperty: XPropertyEvent,
    pub xselectionclear: XSelectionClearEvent,
    pub xselectionrequest: XSelectionRequestEvent,
    pub xselection: XSelectionEvent,
    pub xcolormap: XColormapEvent,
    pub xclient: XClientMessageEvent,
    pub xmapping: XMappingEvent,
    pub xerror: XErrorEvent,
    pub xkeymap: XKeymapEvent,
    pub xgeneric: XGenericEvent,
    pub xcookie: XGenericEventCookie,
    pub pad: [::std::os::raw::c_long; 24usize],
    _bindgen_union_align: [u64; 24usize],
}

