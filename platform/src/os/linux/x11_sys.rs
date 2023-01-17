pub type Display = _XDisplay;

pub type XID = ::std::os::raw::c_ulong;
pub type Window = XID;
pub type Drawable = XID;
pub type Colormap = XID;
pub type KeySym = XID;
pub type Pixmap = XID;
pub type Cursor = XID;

pub type Time = ::std::os::raw::c_ulong;
pub type XIM = *mut _XIM;
pub type Atom = ::std::os::raw::c_ulong;
pub type XEvent = _XEvent;
pub type XIC = *mut _XIC;
pub type XExtData = _XExtData;
pub type XPointer = *mut ::std::os::raw::c_char;
pub type VisualID = ::std::os::raw::c_ulong;
pub type XrmDatabase = *mut _XrmHashBucketRec;
pub type XKeyPressedEvent = XKeyEvent;
pub type XComposeStatus = _XComposeStatus;
pub type GC = *mut _XGC;

pub const None: u32 = 0;
pub const True: u32 = 1;
pub const False: u32 = 0;

pub const CurrentTime: u32 = 0;
pub const SelectionNotify: u32 = 31;
pub const AnyPropertyType: u32 = 0;
pub const SelectionRequest: u32 = 30;
pub const PropModeReplace: u32 = 0;
pub const DestroyNotify: u32 = 17;
pub const ConfigureNotify: u32 = 22;
pub const EnterNotify: u32 = 7;
pub const LeaveNotify: u32 = 8;
pub const MotionNotify: u32 = 6;
pub const AllocNone: u32 = 0;
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

pub const SubstructureNotifyMask: u32 = 524288;
pub const SubstructureRedirectMask: u32 = 1048576;
pub const NoEventMask: u32 = 0;

pub const ExposureMask: u32 = 32768;
pub const StructureNotifyMask: u32 = 131072;
pub const ButtonMotionMask: u32 = 8192;
pub const PointerMotionMask: u32 = 64;
pub const ButtonPressMask: u32 = 4;
pub const ButtonReleaseMask: u32 = 8;
pub const KeyPressMask: u32 = 1;
pub const KeyReleaseMask: u32 = 2;
pub const VisibilityChangeMask: u32 = 65536;
pub const FocusChangeMask: u32 = 2097152;
pub const EnterWindowMask: u32 = 16;
pub const LeaveWindowMask: u32 = 32;
pub const XBufferOverflow: i32 = -1;

pub const XIMPreeditNothing: u32 = 8;
pub const XIMStatusNothing: u32 = 1024;

pub const XNInputStyle: &'static [u8; 11usize] = b"inputStyle\0";
pub const XNClientWindow: &'static [u8; 13usize] = b"clientWindow\0";
pub const XNFocusWindow: &'static [u8; 12usize] = b"focusWindow\0";

pub const Mod1Mask: u32 = 8;
pub const ShiftMask: u32 = 1;
pub const ControlMask: u32 = 4;
pub const Mod4Mask: u32 = 64;

pub const XK_A: u32 = 65;
pub const XK_B: u32 = 66;
pub const XK_C: u32 = 67;
pub const XK_D: u32 = 68;
pub const XK_E: u32 = 69;
pub const XK_F: u32 = 70;
pub const XK_G: u32 = 71;
pub const XK_H: u32 = 72;
pub const XK_I: u32 = 73;
pub const XK_J: u32 = 74;
pub const XK_K: u32 = 75;
pub const XK_L: u32 = 76;
pub const XK_M: u32 = 77;
pub const XK_N: u32 = 78;
pub const XK_O: u32 = 79;
pub const XK_P: u32 = 80;
pub const XK_Q: u32 = 81;
pub const XK_R: u32 = 82;
pub const XK_S: u32 = 83;
pub const XK_T: u32 = 84;
pub const XK_U: u32 = 85;
pub const XK_V: u32 = 86;
pub const XK_W: u32 = 87;
pub const XK_X: u32 = 88;
pub const XK_Y: u32 = 89;
pub const XK_Z: u32 = 90;
pub const XK_a: u32 = 97;
pub const XK_b: u32 = 98;
pub const XK_c: u32 = 99;
pub const XK_d: u32 = 100;
pub const XK_e: u32 = 101;
pub const XK_f: u32 = 102;
pub const XK_g: u32 = 103;
pub const XK_h: u32 = 104;
pub const XK_i: u32 = 105;
pub const XK_j: u32 = 106;
pub const XK_k: u32 = 107;
pub const XK_l: u32 = 108;
pub const XK_m: u32 = 109;
pub const XK_n: u32 = 110;
pub const XK_o: u32 = 111;
pub const XK_p: u32 = 112;
pub const XK_q: u32 = 113;
pub const XK_r: u32 = 114;
pub const XK_s: u32 = 115;
pub const XK_t: u32 = 116;
pub const XK_u: u32 = 117;
pub const XK_v: u32 = 118;
pub const XK_w: u32 = 119;
pub const XK_x: u32 = 120;
pub const XK_y: u32 = 121;
pub const XK_z: u32 = 122;
pub const XK_0: u32 = 48;
pub const XK_1: u32 = 49;
pub const XK_2: u32 = 50;
pub const XK_3: u32 = 51;
pub const XK_4: u32 = 52;
pub const XK_5: u32 = 53;
pub const XK_6: u32 = 54;
pub const XK_7: u32 = 55;
pub const XK_8: u32 = 56;
pub const XK_9: u32 = 57;

pub const XK_Meta_L: u32 = 65511;
pub const XK_Meta_R: u32 = 65512;
pub const XK_Alt_L: u32 = 65513;
pub const XK_Alt_R: u32 = 65514;
pub const XK_Shift_L: u32 = 65505;
pub const XK_Shift_R: u32 = 65506;
pub const XK_Control_L: u32 = 65507;
pub const XK_Control_R: u32 = 65508;

pub const XK_equal: u32 = 61;
pub const XK_minus: u32 = 45;
pub const XK_bracketleft: u32 = 91;
pub const XK_bracketright: u32 = 93;
pub const XK_Return: u32 = 65293;
pub const XK_grave: u32 = 96;
pub const XK_semicolon: u32 = 59;
pub const XK_backslash: u32 = 92;
pub const XK_comma: u32 = 44;
pub const XK_slash: u32 = 47;
pub const XK_period: u32 = 46;
pub const XK_Tab: u32 = 65289;
pub const XK_ISO_Left_Tab: u32 = 65056;
pub const XK_space: u32 = 32;
pub const XK_BackSpace: u32 = 65288;
pub const XK_Escape: u32 = 65307;
pub const XK_Caps_Lock: u32 = 65509;

pub const XK_KP_Subtract: u32 = 65453;
pub const XK_KP_Decimal: u32 = 65454;
pub const XK_KP_Divide: u32 = 65455;
pub const XK_KP_Multiply: u32 = 65450;
pub const XK_KP_Add: u32 = 65451;
pub const XK_Num_Lock: u32 = 65407;
pub const XK_KP_Enter: u32 = 65421;

pub const XK_KP_0: u32 = 65456;
pub const XK_KP_1: u32 = 65457;
pub const XK_KP_2: u32 = 65458;
pub const XK_KP_3: u32 = 65459;
pub const XK_KP_4: u32 = 65460;
pub const XK_KP_5: u32 = 65461;
pub const XK_KP_6: u32 = 65462;
pub const XK_KP_7: u32 = 65463;
pub const XK_KP_8: u32 = 65464;
pub const XK_KP_9: u32 = 65465;

pub const XK_F1: u32 = 65470;
pub const XK_F2: u32 = 65471;
pub const XK_F3: u32 = 65472;
pub const XK_F4: u32 = 65473;
pub const XK_F5: u32 = 65474;
pub const XK_F6: u32 = 65475;
pub const XK_F7: u32 = 65476;
pub const XK_F8: u32 = 65477;
pub const XK_F9: u32 = 65478;
pub const XK_F10: u32 = 65479;
pub const XK_F11: u32 = 65480;
pub const XK_F12: u32 = 65481;

pub const XK_Print: u32 = 65377;
pub const XK_Home: u32 = 65360;
pub const XK_Page_Up: u32 = 65365;
pub const XK_Delete: u32 = 65535;
pub const XK_End: u32 = 65367;
pub const XK_Page_Down: u32 = 65366;
pub const XK_Left: u32 = 65361;
pub const XK_Right: u32 = 65363;
pub const XK_Down: u32 = 65364;
pub const XK_Up: u32 = 65362;

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
    
    pub fn XCreateColormap(
        arg1: *mut Display,
        arg2: Window,
        arg3: *mut Visual,
        arg4: ::std::os::raw::c_int,
    ) -> Colormap;
    
    pub fn XCreateWindow(
        arg1: *mut Display,
        arg2: Window,
        arg3: ::std::os::raw::c_int,
        arg4: ::std::os::raw::c_int,
        arg5: ::std::os::raw::c_uint,
        arg6: ::std::os::raw::c_uint,
        arg7: ::std::os::raw::c_uint,
        arg8: ::std::os::raw::c_int,
        arg9: ::std::os::raw::c_uint,
        arg10: *mut Visual,
        arg11: ::std::os::raw::c_ulong,
        arg12: *mut XSetWindowAttributes,
    ) -> Window;
    
    pub fn XSetWMProtocols(
        arg1: *mut Display,
        arg2: Window,
        arg3: *mut Atom,
        arg4: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn XMapWindow(arg1: *mut Display, arg2: Window) -> ::std::os::raw::c_int;
    
    pub fn XFlush(arg1: *mut Display) -> ::std::os::raw::c_int;
    
    pub fn XStoreName(
        arg1: *mut Display,
        arg2: Window,
        arg3: *const ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
    
    pub fn XCreateIC(arg1: XIM, ...) -> XIC;
    
    pub fn XDestroyWindow(arg1: *mut Display, arg2: Window) -> ::std::os::raw::c_int;
    
    pub fn XIconifyWindow(
        arg1: *mut Display,
        arg2: Window,
        arg3: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn XGetWindowAttributes(
        arg1: *mut Display,
        arg2: Window,
        arg3: *mut XWindowAttributes,
    ) -> ::std::os::raw::c_int;
    
    pub fn XResourceManagerString(arg1: *mut Display) -> *mut ::std::os::raw::c_char;
    
    pub fn XrmGetStringDatabase(arg1: *const ::std::os::raw::c_char) -> XrmDatabase;
    
    pub fn XrmGetResource(
        arg1: XrmDatabase,
        arg2: *const ::std::os::raw::c_char,
        arg3: *const ::std::os::raw::c_char,
        arg4: *mut *mut ::std::os::raw::c_char,
        arg5: *mut XrmValue,
    ) -> ::std::os::raw::c_int;
    
    pub fn XConvertSelection(
        arg1: *mut Display,
        arg2: Atom,
        arg3: Atom,
        arg4: Atom,
        arg5: Window,
        arg6: Time,
    ) -> ::std::os::raw::c_int;
    
    pub fn XSetInputFocus(
        arg1: *mut Display,
        arg2: Window,
        arg3: ::std::os::raw::c_int,
        arg4: Time,
    ) -> ::std::os::raw::c_int;
    
    pub fn XUngrabPointer(arg1: *mut Display, arg2: Time) -> ::std::os::raw::c_int;
    
    pub fn XSetSelectionOwner(
        arg1: *mut Display,
        arg2: Atom,
        arg3: Window,
        arg4: Time,
    ) -> ::std::os::raw::c_int;
    
    pub fn Xutf8LookupString(
        arg1: XIC,
        arg2: *mut XKeyPressedEvent,
        arg3: *mut ::std::os::raw::c_char,
        arg4: ::std::os::raw::c_int,
        arg5: *mut KeySym,
        arg6: *mut ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn XcursorLibraryLoadCursor(
        dpy: *mut Display,
        file: *const ::std::os::raw::c_char,
    ) -> Cursor;
    
    pub fn XDefineCursor(arg1: *mut Display, arg2: Window, arg3: Cursor) -> ::std::os::raw::c_int;
    pub fn XFreeCursor(arg1: *mut Display, arg2: Cursor) -> ::std::os::raw::c_int;
    pub fn XLookupString(
        arg1: *mut XKeyEvent,
        arg2: *mut ::std::os::raw::c_char,
        arg3: ::std::os::raw::c_int,
        arg4: *mut KeySym,
        arg5: *mut XComposeStatus,
    ) -> ::std::os::raw::c_int;
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
pub struct _XComposeStatus {
    pub compose_ptr: XPointer,
    pub chars_matched: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XExtData {
    pub number: ::std::os::raw::c_int,
    pub next: *mut _XExtData,
    pub free_private: ::std::option::Option<
    unsafe extern "C" fn(extension: *mut _XExtData) -> ::std::os::raw::c_int,
    >,
    pub private_data: XPointer,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XrmValue {
    pub size: ::std::os::raw::c_uint,
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
    pub depth: ::std::os::raw::c_int,
    pub nvisuals: ::std::os::raw::c_int,
    pub visuals: *mut Visual,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Screen {
    pub ext_data: *mut XExtData,
    pub display: *mut _XDisplay,
    pub root: Window,
    pub width: ::std::os::raw::c_int,
    pub height: ::std::os::raw::c_int,
    pub mwidth: ::std::os::raw::c_int,
    pub mheight: ::std::os::raw::c_int,
    pub ndepths: ::std::os::raw::c_int,
    pub depths: *mut Depth,
    pub root_depth: ::std::os::raw::c_int,
    pub root_visual: *mut Visual,
    pub default_gc: GC,
    pub cmap: Colormap,
    pub white_pixel: ::std::os::raw::c_ulong,
    pub black_pixel: ::std::os::raw::c_ulong,
    pub max_maps: ::std::os::raw::c_int,
    pub min_maps: ::std::os::raw::c_int,
    pub backing_store: ::std::os::raw::c_int,
    pub save_unders: ::std::os::raw::c_int,
    pub root_input_mask: ::std::os::raw::c_long,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct XWindowAttributes {
    pub x: ::std::os::raw::c_int,
    pub y: ::std::os::raw::c_int,
    pub width: ::std::os::raw::c_int,
    pub height: ::std::os::raw::c_int,
    pub border_width: ::std::os::raw::c_int,
    pub depth: ::std::os::raw::c_int,
    pub visual: *mut Visual,
    pub root: Window,
    pub class: ::std::os::raw::c_int,
    pub bit_gravity: ::std::os::raw::c_int,
    pub win_gravity: ::std::os::raw::c_int,
    pub backing_store: ::std::os::raw::c_int,
    pub backing_planes: ::std::os::raw::c_ulong,
    pub backing_pixel: ::std::os::raw::c_ulong,
    pub save_under: ::std::os::raw::c_int,
    pub colormap: Colormap,
    pub map_installed: ::std::os::raw::c_int,
    pub map_state: ::std::os::raw::c_int,
    pub all_event_masks: ::std::os::raw::c_long,
    pub your_event_mask: ::std::os::raw::c_long,
    pub do_not_propagate_mask: ::std::os::raw::c_long,
    pub override_redirect: ::std::os::raw::c_int,
    pub screen: *mut Screen,
}

#[derive(Debug, Copy, Clone)]
pub struct Visual {
    pub ext_data: *mut XExtData,
    pub visualid: VisualID,
    pub class: ::std::os::raw::c_int,
    pub red_mask: ::std::os::raw::c_ulong,
    pub green_mask: ::std::os::raw::c_ulong,
    pub blue_mask: ::std::os::raw::c_ulong,
    pub bits_per_rgb: ::std::os::raw::c_int,
    pub map_entries: ::std::os::raw::c_int,
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

