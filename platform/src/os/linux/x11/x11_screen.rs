//! Display geometry for the X11 backend.

use {
    self::super::{x11_sys, xlib_app::get_xlib_app_global},
    crate::{makepad_math::*, screen::ScreenGeom},
    std::{
        ffi::CString,
        mem,
        os::raw::{c_int, c_long, c_uchar, c_ulong},
        ptr,
    },
};

/// Reads a `CARDINAL` array property from the root window.
///
/// Returns an empty vector when the property is absent, which is the normal answer from a
/// window manager that does not implement the hint.
unsafe fn root_cardinals(name: &str) -> Vec<c_long> {
    let display = get_xlib_app_global().display;
    let Ok(name) = CString::new(name) else {
        return Vec::new();
    };
    // `only_if_exists` = true: never define the atom, only look one up.
    let atom = unsafe { x11_sys::XInternAtom(display, name.as_ptr(), 1) };
    if atom == 0 {
        return Vec::new();
    }
    let root = unsafe {
        let screen = x11_sys::XDefaultScreen(display);
        x11_sys::XRootWindow(display, screen)
    };

    let mut actual_type: x11_sys::Atom = 0;
    let mut actual_format: c_int = 0;
    let mut n_items: c_ulong = 0;
    let mut bytes_after: c_ulong = 0;
    let mut data: *mut c_uchar = ptr::null_mut();
    // A long_length of 64 covers 16 desktops' worth of four-value work areas; anything past
    // that is left unread rather than paged in.
    let status = unsafe {
        x11_sys::XGetWindowProperty(
            display,
            root,
            atom,
            0,
            64,
            0,
            x11_sys::AnyPropertyType as c_ulong,
            &mut actual_type,
            &mut actual_format,
            &mut n_items,
            &mut bytes_after,
            &mut data,
        )
    };
    // Xlib's `Success` is zero; the constant itself is not in the bindings.
    if status != 0 || data.is_null() {
        return Vec::new();
    }
    // Xlib hands back 32-bit properties widened to `long`, whatever the wire format says.
    let out = if actual_format == 32 {
        unsafe { std::slice::from_raw_parts(data as *const c_long, n_items as usize).to_vec() }
    } else {
        Vec::new()
    };
    unsafe { x11_sys::XFree(data as *mut _) };
    out
}

/// The X screen's full extent, from the root window's geometry.
unsafe fn root_bounds() -> Option<Rect> {
    let display = get_xlib_app_global().display;
    let root = unsafe {
        let screen = x11_sys::XDefaultScreen(display);
        x11_sys::XRootWindow(display, screen)
    };
    let mut xwa = mem::MaybeUninit::<x11_sys::XWindowAttributes>::uninit();
    if unsafe { x11_sys::XGetWindowAttributes(display, root, xwa.as_mut_ptr()) } == 0 {
        return None;
    }
    let xwa = unsafe { xwa.assume_init() };
    if xwa.width <= 0 || xwa.height <= 0 {
        return None;
    }
    Some(Rect {
        pos: dvec2(0.0, 0.0),
        size: dvec2(xwa.width as f64, xwa.height as f64),
    })
}

/// The desktop area a window may occupy, in physical pixels — the coordinate space
/// `XMoveWindow` and `XCreateWindow` take positions in.
///
/// This is one entry covering the whole X screen, not one per physical monitor: splitting a
/// Xinerama screen into its heads needs libXinerama or libXrandr, and makepad links neither.
/// It still keeps a window on the desktop and clear of the panels, which is what a restored
/// position can get wrong. The extent comes from the root window, and the reserved edges
/// from the EWMH `_NET_WORKAREA` hint of the current desktop, falling back to the full extent
/// under a window manager that publishes neither.
pub fn x11_screens() -> Vec<ScreenGeom> {
    let Some(bounds) = (unsafe { root_bounds() }) else {
        return Vec::new();
    };

    let desktop = unsafe { root_cardinals("_NET_CURRENT_DESKTOP") }
        .first()
        .copied()
        .unwrap_or(0)
        .max(0) as usize;
    let areas = unsafe { root_cardinals("_NET_WORKAREA") };
    let work_area = areas
        .chunks_exact(4)
        .nth(desktop)
        .or_else(|| areas.chunks_exact(4).next())
        .map(|a| Rect {
            pos: dvec2(a[0] as f64, a[1] as f64),
            size: dvec2(a[2] as f64, a[3] as f64),
        })
        .filter(|r| r.size.x > 0.0 && r.size.y > 0.0)
        .unwrap_or(bounds);

    vec![ScreenGeom {
        bounds,
        work_area,
        is_primary: true,
    }]
}
