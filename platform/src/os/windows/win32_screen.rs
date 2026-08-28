//! Display enumeration for the Win32 backend.

#![allow(non_snake_case)]

use {
    crate::{
        makepad_math::*,
        screen::ScreenGeom,
        windows::Win32::{
            Foundation::{LPARAM, RECT},
            Graphics::Gdi::{HDC, HMONITOR},
        },
    },
    std::{mem::size_of, ptr},
};

/// `MONITORINFO`, absent from the vendored `windows` bindings. `cb_size` tells
/// `GetMonitorInfoW` which layout it was handed, so it must be filled in before the call.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MonitorInfo {
    cb_size: u32,
    rc_monitor: RECT,
    rc_work: RECT,
    dw_flags: u32,
}

/// `MONITORINFOF_PRIMARY`: the display holding the origin of the virtual screen.
const MONITORINFOF_PRIMARY: u32 = 1;

type MonitorEnumProc =
    unsafe extern "system" fn(HMONITOR, HDC, *mut RECT, LPARAM) -> windows_core::BOOL;

#[inline]
unsafe fn EnumDisplayMonitors(
    hdc: HDC,
    clip: *const RECT,
    callback: MonitorEnumProc,
    data: LPARAM,
) -> windows_core::BOOL {
    windows_core::link!("user32.dll" "system" fn EnumDisplayMonitors(hdc : HDC, clip : *const RECT, callback : MonitorEnumProc, data : LPARAM) -> windows_core::BOOL);
    unsafe { EnumDisplayMonitors(hdc, clip, callback, data) }
}

#[inline]
unsafe fn GetMonitorInfoW(monitor: HMONITOR, info: *mut MonitorInfo) -> windows_core::BOOL {
    windows_core::link!("user32.dll" "system" fn GetMonitorInfoW(monitor : HMONITOR, info : *mut MonitorInfo) -> windows_core::BOOL);
    unsafe { GetMonitorInfoW(monitor, info) }
}

/// Converts a Win32 edge-addressed rectangle to the origin-plus-size form makepad uses.
fn rect_of(r: RECT) -> Rect {
    Rect {
        pos: dvec2(r.left as f64, r.top as f64),
        size: dvec2((r.right - r.left) as f64, (r.bottom - r.top) as f64),
    }
}

/// The displays currently attached, in physical screen pixels — the coordinate space
/// `CreateWindowExW` and `MoveWindow` take window positions in.
pub fn win32_screens() -> Vec<ScreenGeom> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> windows_core::BOOL {
        let screens = unsafe { &mut *(data.0 as *mut Vec<ScreenGeom>) };
        let mut info = MonitorInfo {
            cb_size: size_of::<MonitorInfo>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            screens.push(ScreenGeom {
                bounds: rect_of(info.rc_monitor),
                work_area: rect_of(info.rc_work),
                is_primary: info.dw_flags & MONITORINFOF_PRIMARY != 0,
            });
        }
        // Keep enumerating; a display whose info could not be read is simply skipped.
        windows_core::BOOL(1)
    }

    let mut screens = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            ptr::null(),
            collect,
            LPARAM(&mut screens as *mut Vec<ScreenGeom> as isize),
        );
    }
    screens
}

/// Converts a rectangle in Win32 "workspace" coordinates to screen coordinates.
///
/// `WINDOWPLACEMENT` reports a normal top-level window in workspace coordinates: screen
/// coordinates shifted by the primary display's reserved edges. The two spaces coincide for
/// the usual bottom-docked taskbar and differ by its thickness when it sits at the top or on
/// the left, so the shift is read from the primary display rather than assumed to be zero.
pub fn workspace_rect_to_screen(r: RECT) -> RECT {
    let Some(primary) = win32_screens().into_iter().find(|s| s.is_primary) else {
        return r;
    };
    let dx = (primary.work_area.pos.x - primary.bounds.pos.x) as i32;
    let dy = (primary.work_area.pos.y - primary.bounds.pos.y) as i32;
    RECT {
        left: r.left + dx,
        top: r.top + dy,
        right: r.right + dx,
        bottom: r.bottom + dy,
    }
}
