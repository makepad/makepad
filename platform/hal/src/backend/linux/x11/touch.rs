//! Optional XI 2.2 touch input. Core Xlib remains usable on servers without XI2/libXi.
//! The ABI is the XInput2 device-event layout; the owning Xlib cookie is freed after copying.
use super::sys as x;
use crate::{types::input::touch, TouchPoint};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};

#[link(name = "dl")]
extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(lib: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(lib: *mut c_void) -> c_int;
}
#[link(name = "X11")]
extern "C" {
    fn XQueryExtension(
        d: *mut x::Display,
        name: *const c_char,
        opcode: *mut c_int,
        event: *mut c_int,
        error: *mut c_int,
    ) -> c_int;
    fn XGetEventData(d: *mut x::Display, cookie: *mut x::XGenericEventCookie) -> c_int;
    fn XFreeEventData(d: *mut x::Display, cookie: *mut x::XGenericEventCookie);
}
#[repr(C)]
struct Mask {
    deviceid: c_int,
    mask_len: c_int,
    mask: *mut u8,
}
#[repr(C)]
struct Buttons {
    mask_len: c_int,
    mask: *mut u8,
}
#[repr(C)]
struct Valuators {
    mask_len: c_int,
    mask: *mut u8,
    values: *mut f64,
}
#[repr(C)]
struct Modifiers {
    base: c_int,
    latched: c_int,
    locked: c_int,
    effective: c_int,
}
#[repr(C)]
struct DeviceEvent {
    kind: c_int,
    serial: std::ffi::c_ulong,
    send_event: c_int,
    display: *mut x::Display,
    extension: c_int,
    event_type: c_int,
    time: x::Time,
    device: c_int,
    source: c_int,
    detail: c_int,
    root: x::Window,
    event: x::Window,
    child: x::Window,
    root_x: f64,
    root_y: f64,
    x: f64,
    y: f64,
    flags: c_int,
    buttons: Buttons,
    valuators: Valuators,
    mods: Modifiers,
    group: Modifiers,
}
type Select = unsafe extern "C" fn(*mut x::Display, x::Window, *mut Mask, c_int) -> c_int;
type Version = unsafe extern "C" fn(*mut x::Display, *mut c_int, *mut c_int) -> c_int;
pub(super) struct Touch {
    lib: *mut c_void,
    opcode: c_int,
    select: Select,
    contacts: HashMap<(c_int, c_int), (x::Window, TouchPoint)>,
    next: u32,
}
impl Touch {
    pub fn new(display: *mut x::Display) -> Option<Self> {
        unsafe {
            let lib = dlopen(c"libXi.so.6".as_ptr(), 1);
            if lib.is_null() {
                return None;
            }
            let select = dlsym(lib, c"XISelectEvents".as_ptr());
            let version = dlsym(lib, c"XIQueryVersion".as_ptr());
            if select.is_null() || version.is_null() {
                dlclose(lib);
                return None;
            }
            let select: Select = std::mem::transmute(select);
            let version: Version = std::mem::transmute(version);
            let mut opcode = 0;
            let mut event = 0;
            let mut error = 0;
            let mut major = 2;
            let mut minor = 2;
            if XQueryExtension(
                display,
                c"XInputExtension".as_ptr(),
                &mut opcode,
                &mut event,
                &mut error,
            ) == 0
                || version(display, &mut major, &mut minor) != 0
                || (major, minor) < (2, 2)
            {
                dlclose(lib);
                return None;
            }
            Some(Self {
                lib,
                opcode,
                select,
                contacts: HashMap::new(),
                next: 0,
            })
        }
    }
    pub fn select(&self, display: *mut x::Display, window: x::Window) {
        let mut bytes = [0u8; 3];
        for bit in [18, 19, 20] {
            bytes[bit / 8] |= 1 << (bit % 8);
        }
        let mut mask = Mask {
            deviceid: 1,
            mask_len: bytes.len() as c_int,
            mask: bytes.as_mut_ptr(),
        };
        let status = unsafe { (self.select)(display, window, &mut mask, 1) };
        if status != 0 {
            eprintln!("[hal-x11] XISelectEvents failed: {status}");
        }
    }
    pub fn event(
        &mut self,
        display: *mut x::Display,
        event: &mut x::XEvent,
    ) -> Option<(x::Window, Vec<TouchPoint>)> {
        unsafe {
            if event.type_ != 35
                || event.xcookie.extension != self.opcode
                || !(18..=20).contains(&event.xcookie.evtype)
            {
                return None;
            }
            if XGetEventData(display, &mut event.xcookie) == 0 {
                return None;
            }
            let data = event.xcookie.data as *const DeviceEvent;
            if data.is_null() {
                XFreeEventData(display, &mut event.xcookie);
                return None;
            }
            let e = &*data;
            let key = (e.device, e.detail);
            let window = e.event;
            let phase = match e.event_type {
                18 => touch::START,
                19 => touch::MOVE,
                _ => touch::STOP,
            };
            if phase == touch::START {
                while self.contacts.values().any(|(_, t)| t.id == self.next) {
                    self.next = self.next.wrapping_add(1);
                }
                let id = self.next;
                self.next = self.next.wrapping_add(1);
                self.contacts.insert(
                    key,
                    (
                        window,
                        TouchPoint {
                            id,
                            phase,
                            x: e.x as f32,
                            y: e.y as f32,
                            force: 1.0,
                        },
                    ),
                );
            } else if let Some((_, p)) = self.contacts.get_mut(&key) {
                p.phase = phase;
                p.x = e.x as f32;
                p.y = e.y as f32;
            }
            let points = self
                .contacts
                .values()
                .filter(|(w, _)| *w == window)
                .map(|(_, p)| *p)
                .collect();
            if phase == touch::STOP {
                self.contacts.remove(&key);
            }
            for (_, p) in self.contacts.values_mut() {
                p.phase = touch::STABLE;
            }
            XFreeEventData(display, &mut event.xcookie);
            Some((window, points))
        }
    }
    pub fn active(&self, window: x::Window) -> bool {
        self.contacts.values().any(|(w, _)| *w == window)
    }
    pub fn cancel(&mut self, window: x::Window) -> Vec<TouchPoint> {
        let mut points = Vec::new();
        self.contacts.retain(|_, (w, p)| {
            if *w == window {
                p.phase = touch::STOP;
                points.push(*p);
                false
            } else {
                true
            }
        });
        points
    }
}
impl Drop for Touch {
    fn drop(&mut self) {
        unsafe {
            dlclose(self.lib);
        }
    }
}
