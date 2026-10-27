//! A Desk Link viewer's input as real local devices: uinput virtual devices the HAL's evdev
//! readers find like any plugged-in hardware (the direct backend's raw input rescans
//! `/dev/input` every second, each app's gamepad reader too). Chords, modifiers, focus routing to
//! hosted apps and Wayland clients, and gamepads in every app therefore work exactly as they do
//! for a keyboard, mouse or pad on the machine.
//!
//! - pointer: an absolute device (`INPUT_PROP_DIRECT`, ABS_X/Y 0..65535 over the whole wide
//!   desktop, which raw input maps to the desktop), five buttons and both wheels. Its id
//!   (`DESK_LINK_INPUT_VENDOR`, `DESK_LINK_POINTER_PRODUCT`) tells raw input that the wheel is
//!   in the viewer's content direction already, so the WM's natural scrolling never turns it;
//! - keyboard: every key the WM's key map knows;
//! - one gamepad per viewer pad (made on its first state, removed with `GamepadGone`).
//!
//! Devices live as long as the session; dropping the injector destroys them. Without write
//! access to `/dev/uinput` (a udev rule gives the `input` group it), the session is view-only.

use makepad_desk_link::{button, pad, GamepadState};
use makepad_widgets::makepad_platform::linux_input::{DESK_LINK_INPUT_VENDOR, DESK_LINK_POINTER_PRODUCT};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

extern "C" {
    fn ioctl(fd: i32, request: u64, ...) -> i32;
}

const UI_DEV_CREATE: u64 = 0x5501;
const UI_DEV_DESTROY: u64 = 0x5502;
const UI_DEV_SETUP: u64 = 0x405c_5503;
const UI_ABS_SETUP: u64 = 0x401c_5504;
const UI_SET_EVBIT: u64 = 0x4004_5564;
const UI_SET_KEYBIT: u64 = 0x4004_5565;
const UI_SET_RELBIT: u64 = 0x4004_5566;
const UI_SET_ABSBIT: u64 = 0x4004_5567;
const UI_SET_PROPBIT: u64 = 0x4004_556e;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const REL_WHEEL_HI_RES: u16 = 0x0b;
const REL_HWHEEL_HI_RES: u16 = 0x0c;

const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const INPUT_PROP_DIRECT: i32 = 1;
const BUS_VIRTUAL: u16 = 0x06;
const O_NONBLOCK: i32 = 0o4000;

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
#[derive(Default)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[repr(C)]
struct UinputAbsSetup {
    code: u16,
    absinfo: AbsInfo,
}

#[repr(C)]
struct InputEvent {
    sec: i64,
    usec: i64,
    ty: u16,
    code: u16,
    value: i32,
}

struct Device {
    file: File,
}

impl Device {
    fn new(name: &str, product: u16, setup: impl FnOnce(&Device) -> std::io::Result<()>) -> std::io::Result<Device> {
        let file = OpenOptions::new().write(true).custom_flags(O_NONBLOCK).open("/dev/uinput")?;
        let dev = Device { file };
        setup(&dev)?;
        let mut s = UinputSetup { id: InputId { bustype: BUS_VIRTUAL, vendor: DESK_LINK_INPUT_VENDOR, product, version: 1 }, name: [0; 80], ff_effects_max: 0 };
        let bytes = name.as_bytes();
        s.name[..bytes.len().min(79)].copy_from_slice(&bytes[..bytes.len().min(79)]);
        dev.ioctl_ptr(UI_DEV_SETUP, &s as *const UinputSetup as usize)?;
        dev.ioctl_ptr(UI_DEV_CREATE, 0)?;
        Ok(dev)
    }

    fn ioctl_int(&self, request: u64, value: i32) -> std::io::Result<()> {
        if unsafe { ioctl(self.file.as_raw_fd(), request, value) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn ioctl_ptr(&self, request: u64, ptr: usize) -> std::io::Result<()> {
        if unsafe { ioctl(self.file.as_raw_fd(), request, ptr) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn abs(&self, code: u16, minimum: i32, maximum: i32) -> std::io::Result<()> {
        self.ioctl_int(UI_SET_ABSBIT, code as i32)?;
        let setup = UinputAbsSetup { code, absinfo: AbsInfo { minimum, maximum, ..Default::default() } };
        self.ioctl_ptr(UI_ABS_SETUP, &setup as *const UinputAbsSetup as usize)
    }

    /// The events and their SYN_REPORT in one write: the reader sees them as one report.
    fn emit(&mut self, events: &[(u16, u16, i32)]) {
        let mut buf = Vec::with_capacity((events.len() + 1) * std::mem::size_of::<InputEvent>());
        for &(ty, code, value) in events.iter().chain(std::iter::once(&(EV_SYN, 0, 0))) {
            let e = InputEvent { sec: 0, usec: 0, ty, code, value };
            buf.extend_from_slice(unsafe { std::slice::from_raw_parts(&e as *const InputEvent as *const u8, std::mem::size_of::<InputEvent>()) });
        }
        if let Err(e) = self.file.write_all(&buf) {
            super::note(&format!("desk link input: {e}"));
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = self.ioctl_ptr(UI_DEV_DESTROY, 0);
    }
}

const BTN_LEFT: u16 = 0x110;

/// The gamepad codes the HAL's reader maps (backend/linux/media/game_input.rs `key`).
const PAD_BUTTONS: [(u32, u16); 15] = [
    (pad::A, 0x130),
    (pad::B, 0x131),
    (pad::X, 0x133),
    (pad::Y, 0x134),
    (pad::LEFT_SHOULDER, 0x136),
    (pad::RIGHT_SHOULDER, 0x137),
    (pad::SELECT, 0x13a),
    (pad::START, 0x13b),
    (pad::HOME, 0x13c),
    (pad::LEFT_THUMB, 0x13d),
    (pad::RIGHT_THUMB, 0x13e),
    (pad::DPAD_UP, 0x220),
    (pad::DPAD_DOWN, 0x221),
    (pad::DPAD_LEFT, 0x222),
    (pad::DPAD_RIGHT, 0x223),
];

/// The viewer's scroll (1/120 notches, the content direction its user saw: +dy the view moves
/// up, +dx right) as one wheel report: a high-resolution wheel (smooth, trackpad pixels and
/// momentum included) plus the whole notches a wheel also reports, `rest` carrying parts of a
/// notch. The values go out unturned, in evdev's own sense (+1 = the view moves up, tilted
/// right): raw input recognises this pointer and never applies the WM's natural scrolling to it.
fn wheel_report(rest: &mut [i32; 2], dx: i32, dy: i32) -> Vec<(u16, u16, i32)> {
    rest[0] += dx;
    rest[1] += dy;
    let notches = [rest[0] / makepad_desk_link::SCROLL_UNITS_PER_NOTCH, rest[1] / makepad_desk_link::SCROLL_UNITS_PER_NOTCH];
    rest[0] -= notches[0] * makepad_desk_link::SCROLL_UNITS_PER_NOTCH;
    rest[1] -= notches[1] * makepad_desk_link::SCROLL_UNITS_PER_NOTCH;
    let mut events = Vec::new();
    // high resolution first: the WM's raw input then leaves this device's whole notches out
    if dx != 0 {
        events.push((EV_REL, REL_HWHEEL_HI_RES, dx));
    }
    if dy != 0 {
        events.push((EV_REL, REL_WHEEL_HI_RES, dy));
    }
    if notches[0] != 0 {
        events.push((EV_REL, REL_HWHEEL, notches[0]));
    }
    if notches[1] != 0 {
        events.push((EV_REL, REL_WHEEL, notches[1]));
    }
    events
}

pub struct Injector {
    pointer: Device,
    keyboard: Device,
    pads: Vec<Option<Device>>,
    /// Wheel motion below a notch, carried to the next scroll.
    wheel: [i32; 2],
}

impl Injector {
    pub fn new() -> std::io::Result<Injector> {
        let pointer = Device::new("Desk Link pointer", DESK_LINK_POINTER_PRODUCT, |d| {
            d.ioctl_int(UI_SET_PROPBIT, INPUT_PROP_DIRECT)?;
            d.ioctl_int(UI_SET_EVBIT, EV_KEY as i32)?;
            d.ioctl_int(UI_SET_EVBIT, EV_REL as i32)?;
            d.ioctl_int(UI_SET_EVBIT, EV_ABS as i32)?;
            for b in 0..5 {
                d.ioctl_int(UI_SET_KEYBIT, (BTN_LEFT + b) as i32)?;
            }
            d.ioctl_int(UI_SET_RELBIT, REL_WHEEL as i32)?;
            d.ioctl_int(UI_SET_RELBIT, REL_HWHEEL as i32)?;
            d.ioctl_int(UI_SET_RELBIT, REL_WHEEL_HI_RES as i32)?;
            d.ioctl_int(UI_SET_RELBIT, REL_HWHEEL_HI_RES as i32)?;
            d.abs(ABS_X, 0, 65535)?;
            d.abs(ABS_Y, 0, 65535)
        })?;
        let keyboard = Device::new("Desk Link keyboard", 2, |d| {
            d.ioctl_int(UI_SET_EVBIT, EV_KEY as i32)?;
            for code in 1..249 {
                d.ioctl_int(UI_SET_KEYBIT, code)?;
            }
            Ok(())
        })?;
        Ok(Injector { pointer, keyboard, pads: Vec::new(), wheel: [0; 2] })
    }

    /// The pointer at a desktop-normalised position (0..=65535 over the wide desktop).
    pub fn pointer(&mut self, x: u16, y: u16) {
        self.pointer.emit(&[(EV_ABS, ABS_X, x as i32), (EV_ABS, ABS_Y, y as i32)]);
    }

    pub fn button(&mut self, down: bool, b: u8) {
        let code = BTN_LEFT
            + match b {
                button::LEFT => 0,
                button::RIGHT => 1,
                button::MIDDLE => 2,
                button::BACK => 3,
                button::FORWARD => 4,
                _ => return,
            };
        self.pointer.emit(&[(EV_KEY, code, down as i32)]);
    }

    /// The viewer's scroll as this pointer's wheel (`wheel_report`).
    pub fn scroll(&mut self, dx: i32, dy: i32) {
        let events = wheel_report(&mut self.wheel, dx, dy);
        if !events.is_empty() {
            self.pointer.emit(&events);
        }
    }

    /// A key by makepad `KeyCode` index (the viewer's), through the WM's evdev key map.
    pub fn key(&mut self, down: bool, keycode: u32) {
        let Some(code) = crate::wayland_view::evdev_of(makepad_widgets::KeyCode::from_index(keycode)) else { return };
        self.keyboard.emit(&[(EV_KEY, code, down as i32)]);
    }

    pub fn gamepad(&mut self, slot: u8, state: &GamepadState) {
        let slot = slot as usize;
        if slot >= 8 {
            return;
        }
        while self.pads.len() <= slot {
            self.pads.push(None);
        }
        if self.pads[slot].is_none() {
            let made = Device::new(&format!("Desk Link gamepad {}", slot + 1), 0x10 + slot as u16, |d| {
                d.ioctl_int(UI_SET_EVBIT, EV_KEY as i32)?;
                d.ioctl_int(UI_SET_EVBIT, EV_ABS as i32)?;
                for (_, code) in PAD_BUTTONS {
                    d.ioctl_int(UI_SET_KEYBIT, code as i32)?;
                }
                for code in [0, 1, 3, 4] {
                    d.abs(code, -32767, 32767)?;
                }
                d.abs(2, 0, 255)?;
                d.abs(5, 0, 255)
            });
            match made {
                Ok(d) => self.pads[slot] = Some(d),
                Err(e) => {
                    super::note(&format!("desk link gamepad: {e}"));
                    return;
                }
            }
        }
        let mut events = Vec::with_capacity(24);
        for (bit, code) in PAD_BUTTONS {
            events.push((EV_KEY, code, (state.buttons & bit != 0) as i32));
        }
        // makepad's stick y is up-positive, evdev's down-positive: the wire carries makepad's
        events.push((EV_ABS, 0, state.left[0] as i32));
        events.push((EV_ABS, 1, -(state.left[1] as i32)));
        events.push((EV_ABS, 3, state.right[0] as i32));
        events.push((EV_ABS, 4, -(state.right[1] as i32)));
        events.push((EV_ABS, 2, state.triggers[0] as i32));
        events.push((EV_ABS, 5, state.triggers[1] as i32));
        if let Some(d) = self.pads[slot].as_mut() {
            d.emit(&events);
        }
    }

    pub fn gamepad_gone(&mut self, slot: u8) {
        if let Some(p) = self.pads.get_mut(slot as usize) {
            *p = None;
        }
    }
}

impl makepad_desk_link::server::Input for Injector {
    fn pointer(&mut self, x: u16, y: u16) {
        Injector::pointer(self, x, y)
    }
    fn button(&mut self, down: bool, button: u8) {
        Injector::button(self, down, button)
    }
    fn scroll(&mut self, dx: i32, dy: i32) {
        Injector::scroll(self, dx, dy)
    }
    fn key(&mut self, down: bool, keycode: u32) {
        Injector::key(self, down, keycode)
    }
    fn gamepad(&mut self, slot: u8, state: &GamepadState) {
        Injector::gamepad(self, slot, state)
    }
    fn gamepad_gone(&mut self, slot: u8) {
        Injector::gamepad_gone(self, slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_viewers_scroll_goes_out_unturned() {
        let mut rest = [0; 2];
        // one notch "view moves up" (a Mac user's two fingers down, natural): evdev +1 (away)
        assert_eq!(wheel_report(&mut rest, 0, 120), vec![(EV_REL, REL_WHEEL_HI_RES, 120), (EV_REL, REL_WHEEL, 1)]);
        // "view moves down", in parts of a notch: the whole notch once they add up
        assert_eq!(wheel_report(&mut rest, 0, -60), vec![(EV_REL, REL_WHEEL_HI_RES, -60)]);
        assert_eq!(wheel_report(&mut rest, 0, -60), vec![(EV_REL, REL_WHEEL_HI_RES, -60), (EV_REL, REL_WHEEL, -1)]);
        // "view moves right": tilted right
        assert_eq!(wheel_report(&mut rest, 120, 0), vec![(EV_REL, REL_HWHEEL_HI_RES, 120), (EV_REL, REL_HWHEEL, 1)]);
        assert_eq!(rest, [0, 0]);
    }
}
