//! Native evdev parser and touchpad processing rehomed from os/linux/direct/raw_input.rs.
//! Device reports, key mapping, absolute touch-as-pointer and gesture rules are unchanged.
use super::{direct_event::Modifiers, sys as libc_sys, touchpad::{PadButton, PadContact, PadFrame, PadGesture, PadOutput}};
use crate::{buffers::EventQueue, types::input, Id};
use makepad_key_code::KeyCode;
use makepad_math::*;
use std::{collections::HashSet, fs::{File, OpenOptions}, io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt}, path::PathBuf};

#[allow(non_snake_case)]
mod InputEventType {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
}

#[allow(non_snake_case)]
mod EvSynCodes {
    pub const SYN_REPORT: u16 = 0x00;
    pub const SYN_DROPPED: u16 = 0x03;
}

#[allow(non_snake_case)]
mod EvKeyCodes {
    pub const KEY_ESC: u16 = 1;
    pub const KEY_1: u16 = 2;
    pub const KEY_2: u16 = 3;
    pub const KEY_3: u16 = 4;
    pub const KEY_4: u16 = 5;
    pub const KEY_5: u16 = 6;
    pub const KEY_6: u16 = 7;
    pub const KEY_7: u16 = 8;
    pub const KEY_8: u16 = 9;
    pub const KEY_9: u16 = 10;
    pub const KEY_0: u16 = 11;
    pub const KEY_MINUS: u16 = 12;
    pub const KEY_EQUAL: u16 = 13;
    pub const KEY_BACKSPACE: u16 = 14;
    pub const KEY_TAB: u16 = 15;
    pub const KEY_Q: u16 = 16;
    pub const KEY_W: u16 = 17;
    pub const KEY_E: u16 = 18;
    pub const KEY_R: u16 = 19;
    pub const KEY_T: u16 = 20;
    pub const KEY_Y: u16 = 21;
    pub const KEY_U: u16 = 22;
    pub const KEY_I: u16 = 23;
    pub const KEY_O: u16 = 24;
    pub const KEY_P: u16 = 25;
    pub const KEY_LEFTBRACE: u16 = 26;
    pub const KEY_RIGHTBRACE: u16 = 27;
    pub const KEY_ENTER: u16 = 28;
    pub const KEY_LEFTCTRL: u16 = 29;
    pub const KEY_A: u16 = 30;
    pub const KEY_S: u16 = 31;
    pub const KEY_D: u16 = 32;
    pub const KEY_F: u16 = 33;
    pub const KEY_G: u16 = 34;
    pub const KEY_H: u16 = 35;
    pub const KEY_J: u16 = 36;
    pub const KEY_K: u16 = 37;
    pub const KEY_L: u16 = 38;
    pub const KEY_SEMICOLON: u16 = 39;
    pub const KEY_APOSTROPHE: u16 = 40;
    pub const KEY_GRAVE: u16 = 41;
    pub const KEY_LEFTSHIFT: u16 = 42;
    pub const KEY_BACKSLASH: u16 = 43;
    pub const KEY_Z: u16 = 44;
    pub const KEY_X: u16 = 45;
    pub const KEY_C: u16 = 46;
    pub const KEY_V: u16 = 47;
    pub const KEY_B: u16 = 48;
    pub const KEY_N: u16 = 49;
    pub const KEY_M: u16 = 50;
    pub const KEY_COMMA: u16 = 51;
    pub const KEY_DOT: u16 = 52;
    pub const KEY_SLASH: u16 = 53;
    pub const KEY_RIGHTSHIFT: u16 = 54;
    pub const KEY_KPASTERISK: u16 = 55;
    pub const KEY_LEFTALT: u16 = 56;
    pub const KEY_SPACE: u16 = 57;
    pub const KEY_CAPSLOCK: u16 = 58;
    pub const KEY_F1: u16 = 59;
    pub const KEY_F2: u16 = 60;
    pub const KEY_F3: u16 = 61;
    pub const KEY_F4: u16 = 62;
    pub const KEY_F5: u16 = 63;
    pub const KEY_F6: u16 = 64;
    pub const KEY_F7: u16 = 65;
    pub const KEY_F8: u16 = 66;
    pub const KEY_F9: u16 = 67;
    pub const KEY_F10: u16 = 68;
    pub const KEY_NUMLOCK: u16 = 69;
    pub const KEY_SCROLLLOCK: u16 = 70;
    pub const KEY_KP7: u16 = 71;
    pub const KEY_KP8: u16 = 72;
    pub const KEY_KP9: u16 = 73;
    pub const KEY_KPMINUS: u16 = 74;
    pub const KEY_KP4: u16 = 75;
    pub const KEY_KP5: u16 = 76;
    pub const KEY_KP6: u16 = 77;
    pub const KEY_KPPLUS: u16 = 78;
    pub const KEY_KP1: u16 = 79;
    pub const KEY_KP2: u16 = 80;
    pub const KEY_KP3: u16 = 81;
    pub const KEY_KP0: u16 = 82;
    pub const KEY_KPDOT: u16 = 83;
    pub const KEY_ZENKAKUHANKAKU: u16 = 85;
    pub const KEY_102ND: u16 = 86;
    pub const KEY_F11: u16 = 87;
    pub const KEY_F12: u16 = 88;
    pub const KEY_RO: u16 = 89;
    pub const KEY_KPENTER: u16 = 96;
    pub const KEY_RIGHTCTRL: u16 = 97;
    pub const KEY_KPSLASH: u16 = 98;
    pub const KEY_SYSRQ: u16 = 99;
    pub const KEY_RIGHTALT: u16 = 100;
    pub const KEY_HOME: u16 = 102;
    pub const KEY_UP: u16 = 103;
    pub const KEY_PAGEUP: u16 = 104;
    pub const KEY_LEFT: u16 = 105;
    pub const KEY_RIGHT: u16 = 106;
    pub const KEY_END: u16 = 107;
    pub const KEY_DOWN: u16 = 108;
    pub const KEY_PAGEDOWN: u16 = 109;
    pub const KEY_INSERT: u16 = 110;
    pub const KEY_DELETE: u16 = 111;
    pub const KEY_LEFTMETA: u16 = 125;
    pub const KEY_RIGHTMETA: u16 = 126;
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;
    pub const BTN_SIDE: u16 = 0x113;
    pub const BTN_EXTRA: u16 = 0x114;
    pub const BTN_TOOL_PEN: u16 = 0x140;
    pub const BTN_TOOL_FINGER: u16 = 0x145;
    pub const BTN_TOOL_QUINTTAP: u16 = 0x148;
    pub const BTN_TOUCH: u16 = 0x14a;
    pub const BTN_TOOL_DOUBLETAP: u16 = 0x14d;
    pub const BTN_TOOL_TRIPLETAP: u16 = 0x14e;
    pub const BTN_TOOL_QUADTAP: u16 = 0x14f;
}

#[allow(non_snake_case)]
mod EvRelCodes {
    pub const REL_X: u16 = 0x00;
    pub const REL_Y: u16 = 0x01;
    pub const REL_HWHEEL: u16 = 0x06;
    pub const REL_WHEEL: u16 = 0x08;
}

#[allow(non_snake_case)]
mod EvAbsCodes {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
    pub const ABS_MT_SLOT: u16 = 0x2f;
    pub const ABS_MT_POSITION_X: u16 = 0x35;
    pub const ABS_MT_POSITION_Y: u16 = 0x36;
    pub const ABS_MT_TOOL_TYPE: u16 = 0x37;
    pub const ABS_MT_TRACKING_ID: u16 = 0x39;
}

#[allow(non_snake_case)]
mod KeyAction {
    pub const KEY_UP: i32 = 0x00;
    pub const KEY_DOWN: i32 = 0x01;
    pub const KEY_REPEAT: i32 = 0x02;
}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
struct InputEvent {
    time: libc_sys::timeval,
    ty: u16,
    code: u16,
    value: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

// Linux evdev's _IOR('E', nr, size), using the existing in-house ioctl FFI.
fn evdev_read(file: &File, nr: u32, bytes: &mut [u8]) -> bool {
    let request = 0x8000_0000u32 | ((bytes.len() as u32) << 16) | (b'E' as u32) << 8 | nr;
    unsafe { libc_sys::ioctl(file.as_raw_fd(), request as std::os::raw::c_ulong, bytes.as_mut_ptr()) >= 0 }
}

fn abs_info(file: &File, axis: u16) -> Option<AbsInfo> {
    let mut supported = [0u8; 8];
    if !evdev_read(file, 0x23, &mut supported) || !has_bit(&supported, axis) {
        return None;
    }
    let mut bytes = [0u8; std::mem::size_of::<AbsInfo>()];
    evdev_read(file, 0x40 + axis as u32, &mut bytes)
        .then(|| unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<AbsInfo>()) })
}

fn has_bit(bytes: &[u8], bit: u16) -> bool {
    bytes.get(bit as usize / 8).is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
}

#[derive(Default, Clone)]
struct Contact {
    id: Option<i32>,
    x: Option<i32>,
    y: Option<i32>,
    palm: bool,
}

/// `ABS_MT_TOOL_TYPE`'s palm value.
const MT_TOOL_PALM: i32 = 2;

/// An indirect touchpad: evdev parsing here, the gestures (pointer, scroll
/// phases and momentum, pinch) in `linux_touchpad`.
struct Touchpad {
    axes: [AbsInfo; 2],
    /// Units per millimetre on each axis.
    resolution: [f64; 2],
    slots: Vec<Contact>,
    slot: Option<usize>,
    has_touch: bool,
    /// A clickpad (`INPUT_PROP_BUTTONPAD`): its one button is the whole surface.
    buttonpad: bool,
    contact: bool,
    tools: [bool; 5],
    /// The pad's button (`BTN_LEFT`) is held.
    button: bool,
    /// The button the clickpad press went out as (`BTN_LEFT`, `BTN_RIGHT`
    /// or `BTN_MIDDLE`), kept until its release.
    press: u16,
    gesture: PadGesture,
}

impl Touchpad {
    fn open(file: &File, props: &[u8], keys: &[u8]) -> Option<Self> {
        if has_bit(props, 1) || !has_bit(keys, EvKeyCodes::BTN_TOOL_FINGER) {
            return None;
        }
        let mt = abs_info(file, EvAbsCodes::ABS_MT_SLOT).filter(|axis| {
            axis.minimum == 0 && (0..64).contains(&axis.maximum)
        });
        let codes = if mt.is_some() {
            [EvAbsCodes::ABS_MT_POSITION_X, EvAbsCodes::ABS_MT_POSITION_Y]
        } else {
            [EvAbsCodes::ABS_X, EvAbsCodes::ABS_Y]
        };
        let axes = [abs_info(file, codes[0])?, abs_info(file, codes[1])?];
        if axes.iter().any(|axis| axis.maximum <= axis.minimum) { return None; }
        let mut pad = Self::new(axes, mt.map_or(0, |axis| (axis.maximum + 1) as usize),
            has_bit(keys, EvKeyCodes::BTN_TOUCH), has_bit(props, 2));
        pad.resync(file, &mut Vec::new());
        Some(pad)
    }

    fn new(axes: [AbsInfo; 2], slots: usize, has_touch: bool, buttonpad: bool) -> Self {
        // Kernel resolution is units/mm. Old pads without it use their axis
        // extent as a conservative physical-size estimate, not screen size.
        let resolution = |index: usize, size_mm: f64| {
            let axis = axes[index];
            if axis.resolution > 0 { axis.resolution as f64 }
            else { (axis.maximum - axis.minimum) as f64 / size_mm }
        };
        let resolution = [resolution(0, 100.0), resolution(1, 65.0)];
        let size = dvec2(
            (axes[0].maximum - axes[0].minimum) as f64 / resolution[0],
            (axes[1].maximum - axes[1].minimum) as f64 / resolution[1],
        );
        Self {
            axes,
            resolution,
            slots: vec![Contact::default(); slots],
            // The kernel starts every device on slot 0 (resync reads it back).
            slot: (slots > 0).then_some(0),
            has_touch,
            buttonpad,
            contact: false,
            tools: [false; 5],
            button: false,
            press: EvKeyCodes::BTN_LEFT,
            gesture: PadGesture::new(size),
        }
    }

    fn tool_index(code: u16) -> Option<usize> {
        match code {
            EvKeyCodes::BTN_TOOL_FINGER => Some(0),
            EvKeyCodes::BTN_TOOL_DOUBLETAP => Some(1),
            EvKeyCodes::BTN_TOOL_TRIPLETAP => Some(2),
            EvKeyCodes::BTN_TOOL_QUADTAP => Some(3),
            EvKeyCodes::BTN_TOOL_QUINTTAP => Some(4),
            _ => None,
        }
    }

    /// Reads the pad's whole state again (after `SYN_DROPPED`, and when
    /// opened). A gesture in flight ends without a coast and nothing moves
    /// until every finger lifts.
    fn resync(&mut self, file: &File, out: &mut Vec<PadOutput>) {
        self.gesture.cancel(out);
        let mut keys = [0u8; 96];
        evdev_read(file, 0x18, &mut keys); // EVIOCGKEY
        self.contact = has_bit(&keys, EvKeyCodes::BTN_TOUCH);
        self.button = has_bit(&keys, EvKeyCodes::BTN_LEFT);
        for (index, code) in [EvKeyCodes::BTN_TOOL_FINGER, EvKeyCodes::BTN_TOOL_DOUBLETAP,
            EvKeyCodes::BTN_TOOL_TRIPLETAP, EvKeyCodes::BTN_TOOL_QUADTAP,
            EvKeyCodes::BTN_TOOL_QUINTTAP].into_iter().enumerate() {
            self.tools[index] = has_bit(&keys, code);
        }
        if self.slots.is_empty() {
            for (index, code) in [EvAbsCodes::ABS_X, EvAbsCodes::ABS_Y].into_iter().enumerate() {
                if let Some(axis) = abs_info(file, code) { self.axes[index] = axis; }
            }
        } else {
            self.slots.fill(Contact::default());
            self.slot = abs_info(file, EvAbsCodes::ABS_MT_SLOT).and_then(|axis| self.slot_index(axis.value));
            for code in [EvAbsCodes::ABS_MT_TRACKING_ID, EvAbsCodes::ABS_MT_POSITION_X,
                EvAbsCodes::ABS_MT_POSITION_Y, EvAbsCodes::ABS_MT_TOOL_TYPE] {
                // EVIOCGMTSLOTS takes the desired axis followed by one i32 per slot.
                let mut bytes = vec![0u8; (1 + self.slots.len()) * 4];
                bytes[..4].copy_from_slice(&(code as i32).to_ne_bytes());
                if !evdev_read(file, 0x0a, &mut bytes) { continue; }
                for (slot, value) in self.slots.iter_mut().zip(bytes[4..].chunks_exact(4)) {
                    let value = i32::from_ne_bytes(value.try_into().unwrap());
                    match code {
                        EvAbsCodes::ABS_MT_TRACKING_ID => slot.id = (value >= 0).then_some(value),
                        EvAbsCodes::ABS_MT_POSITION_X => slot.x = Some(value),
                        EvAbsCodes::ABS_MT_POSITION_Y => slot.y = Some(value),
                        EvAbsCodes::ABS_MT_TOOL_TYPE => slot.palm = value == MT_TOOL_PALM,
                        _ => {}
                    }
                }
            }
        }
    }

    fn slot_index(&self, value: i32) -> Option<usize> {
        // The slot axis is opened only with a zero minimum.
        (value >= 0 && (value as usize) < self.slots.len()).then_some(value as usize)
    }

    // Consume only touch coordinates/contact keys. Physical button events
    // still take the ordinary mouse path. In particular BTN_TOUCH is NOT a
    // click on an indirect touchpad. A clickpad press clicks the button the
    // gesture picks from the contacts (two fingers or the bottom right corner:
    // secondary), held as that button until its release.
    fn consume(&mut self, event: &mut InputEvent) -> bool {
        if event.ty == InputEventType::EV_REL
            && matches!(event.code, EvRelCodes::REL_X | EvRelCodes::REL_Y) {
            return true;
        }
        if event.ty == InputEventType::EV_KEY {
            if event.code == EvKeyCodes::BTN_TOUCH {
                self.contact = event.value != 0;
                return true;
            }
            if let Some(index) = Self::tool_index(event.code) {
                self.tools[index] = event.value != 0;
                return true;
            }
            if event.code == EvKeyCodes::BTN_LEFT {
                self.button = event.value != 0;
                if self.buttonpad {
                    if event.value != 0 {
                        self.press = pad_button_code(self.gesture.click_button());
                    }
                    event.code = self.press;
                }
            }
            return false;
        }
        if event.ty != InputEventType::EV_ABS { return false; }
        if self.slots.is_empty() {
            match event.code {
                EvAbsCodes::ABS_X => self.axes[0].value = event.value,
                EvAbsCodes::ABS_Y => self.axes[1].value = event.value,
                _ => {}
            }
        } else if event.code == EvAbsCodes::ABS_MT_SLOT {
            self.slot = self.slot_index(event.value);
        } else if let Some(slot) = self.slot.and_then(|index| self.slots.get_mut(index)) {
            match event.code {
                EvAbsCodes::ABS_MT_TRACKING_ID => {
                    slot.id = (event.value >= 0).then_some(event.value);
                    if slot.id.is_none() { slot.palm = false; }
                }
                EvAbsCodes::ABS_MT_POSITION_X => slot.x = Some(event.value),
                EvAbsCodes::ABS_MT_POSITION_Y => slot.y = Some(event.value),
                EvAbsCodes::ABS_MT_TOOL_TYPE => slot.palm = event.value == MT_TOOL_PALM,
                _ => {}
            }
        }
        true // Never process both legacy ABS_X/Y and the MT coordinates.
    }

    /// The report is complete: hand its contacts to the gesture.
    fn finish_report(&mut self, time: f64, natural: bool, tap: bool, out: &mut Vec<PadOutput>) {
        let mm = |pad: &Self, index: usize, value: i32| {
            (value - pad.axes[index].minimum) as f64 / pad.resolution[index]
        };
        let mut contacts = Vec::new();
        // Some pads keep reporting proximity positions while a finger is
        // hovering. Only actual surface contact counts.
        if !self.has_touch || self.contact {
            if self.slots.is_empty() {
                // Single-touch pads: the finger count from the tool keys, all
                // at the one reported position.
                let count = self.tools.iter().rposition(|tool| *tool).map_or(self.contact as usize, |i| i + 1);
                let pos = dvec2(mm(self, 0, self.axes[0].value), mm(self, 1, self.axes[1].value));
                for slot in 0..count {
                    contacts.push(PadContact { slot, id: 0, pos, palm: false });
                }
            } else {
                for (slot, contact) in self.slots.iter().enumerate() {
                    let Some(id) = contact.id else { continue };
                    // A contact whose position has not arrived yet: wait for
                    // a complete report rather than reading a false lift.
                    let (Some(x), Some(y)) = (contact.x, contact.y) else { return };
                    contacts.push(PadContact { slot, id, pos: dvec2(mm(self, 0, x), mm(self, 1, y)), palm: contact.palm });
                }
            }
        }
        let frame = PadFrame { time, contacts: &contacts, button: self.button };
        self.gesture.frame(&frame, natural, tap, out);
    }
}

fn pad_button_code(button: PadButton) -> u16 {
    EvKeyCodes::BTN_LEFT + button as u16
}

enum DeviceEvent {
    Raw(InputEvent),
    Touchpad(PadOutput),
}

struct InputDevice {
    id: u64,
    path: PathBuf,
    file: File,
    pending: Vec<InputEvent>,
    dropped: bool,
    reported_input: bool,
    touchpad: Option<Touchpad>,
    absolute_axes: Option<[AbsInfo; 2]>,
    absolute_mt: bool,
}

pub struct RawInput {
    pub modifiers: Modifiers,
    devices: Vec<InputDevice>,
    next_device_id: u64,
    modifier_keys: HashSet<(u64, u16)>,
    denied: HashSet<PathBuf>,
    next_scan: f64,
    width: f64,
    height: f64,
    dpi_factor: f64,
    abs: Vec2d,
    /// The wide desktop's screens, as `[x, y, width, height]` in raw input's own
    /// coordinates: relative pointer motion stays on them, so screens of
    /// different heights leave no reachable dead corners.
    screen_rects: Vec<[f64; 4]>,
}

impl RawInput {
    pub fn new(width: f64, height: f64, dpi_factor: f64) -> Self {
        // Guard like `set_bounds` does: a non-positive or non-finite factor
        // carries no usable information (a zeroed or corrupt state value),
        // and dividing by it later would poison every pointer coordinate.
        let dpi_factor = if dpi_factor.is_finite() && dpi_factor > 0.0 { dpi_factor } else { 1.0 };
        let mut input = Self {
            devices: Vec::new(),
            next_device_id: 0,
            modifier_keys: HashSet::new(),
            denied: HashSet::new(),
            next_scan: 1.0,
            width,
            height,
            dpi_factor,
            abs: dvec2(0.0, 0.0),
            screen_rects: Vec::new(),
            modifiers: Default::default(),
        };
        input.scan_devices();
        input
    }

    /// The pointer's clamp rectangle follows the wide desktop. Called when a
    /// hotplug reconcile changes the desktop size; the current pointer position
    /// is kept and clamped so it stays on screen.
    pub fn set_bounds(&mut self, width: f64, height: f64, dpi_factor: f64) {
        self.width = width;
        self.height = height;
        if dpi_factor.is_finite() && dpi_factor > 0.0 {
            self.dpi_factor = dpi_factor;
        }
        self.abs.x = self.abs.x.clamp(0.0, width.max(0.0));
        self.abs.y = self.abs.y.clamp(0.0, height.max(0.0));
        self.clamp_to_screens();
    }

    /// The screens of the wide desktop, as `[x, y, width, height]` in the same
    /// coordinates as the pointer; an empty list keeps the plain bounds clamp.
    pub fn set_screen_rects(&mut self, rects: Vec<[f64; 4]>) {
        self.screen_rects = rects;
        self.clamp_to_screens();
    }

    fn clamp_to_screens(&mut self) {
        if !self.screen_rects.is_empty() {
            let (x, y) = crate::wide_desktop::clamp_to_rects(&self.screen_rects, self.abs.x, self.abs.y);
            self.abs.x = x;
            self.abs.y = y;
        }
    }

    fn scan_devices(&mut self) {
        let entries = match std::fs::read_dir("/dev/input") {
            Ok(entries) => entries,
            Err(error) => {
                if self.denied.insert(PathBuf::from("/dev/input")) {
                    eprintln!("Direct input: enumerate /dev/input: {error}");
                }
                return;
            }
        };
        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.file_name().and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("event"))
                .is_some_and(|number| !number.is_empty() && number.bytes().all(|c| c.is_ascii_digit()))
        }).collect();
        paths.sort();
        self.denied.retain(|path| paths.contains(path));
        for path in paths {
            if self.devices.iter().any(|device| device.path == path) {
                continue;
            }
            match OpenOptions::new().read(true).custom_flags(libc_sys::O_NONBLOCK).open(&path) {
                Ok(file) => {
                    self.denied.remove(&path);
                    let mut props = [0u8; 8];
                    let mut keys = [0u8; 96];
                    evdev_read(&file, 0x09, &mut props); // EVIOCGPROP
                    evdev_read(&file, 0x21, &mut keys); // EVIOCGBIT(EV_KEY)
                    let touchpad = Touchpad::open(&file, &props, &keys);
                    let mut absolute_mt = false;
                    let absolute_axes = if touchpad.is_none() && (has_bit(&props, 1)
                        || has_bit(&keys, EvKeyCodes::BTN_TOOL_PEN) || has_bit(&keys, EvKeyCodes::BTN_TOUCH)) {
                        abs_info(&file, EvAbsCodes::ABS_X).zip(abs_info(&file, EvAbsCodes::ABS_Y))
                            .or_else(|| {
                                absolute_mt = true;
                                abs_info(&file, EvAbsCodes::ABS_MT_POSITION_X)
                                    .zip(abs_info(&file, EvAbsCodes::ABS_MT_POSITION_Y))
                            })
                            .filter(|(x, y)| x.maximum > x.minimum && y.maximum > y.minimum)
                            .map(|(x, y)| [x, y])
                    } else { None };
                    eprintln!("Direct input: opened {}{}", path.display(),
                        if touchpad.is_some() { " (relative touchpad, two-finger scroll)" } else { "" });
                    self.next_device_id += 1;
                    self.devices.push(InputDevice { id: self.next_device_id, path, file,
                        pending: Vec::new(), dropped: false, reported_input: false, touchpad, absolute_axes, absolute_mt });
                }
                Err(error) => {
                    if self.denied.insert(path.clone()) {
                        eprintln!("Direct input: open {}: {error}", path.display());
                    }
                }
            }
        }
    }

    pub fn suspend(&mut self) {
        self.devices.clear();
        self.modifier_keys.clear();
        self.modifiers = Modifiers::default();
        self.next_scan = 0.0;
    }

    pub fn coasting(&self) -> bool {
        self.devices.iter().any(|d| d.touchpad.as_ref().is_some_and(|p| p.gesture.is_coasting()))
    }

    pub fn fds(&self) -> impl Iterator<Item = i32> + '_ {
        self.devices.iter().map(|device| device.file.as_raw_fd())
    }

    pub fn poll_raw_input(&mut self, time: f64, window_id: Id) -> EventQueue {
        if time >= self.next_scan {
            self.next_scan = time + 1.0;
            self.scan_devices();
        }
        let mut dir_evts = EventQueue::default();
        dir_evts.set_time((time * 1e9) as u64);
        let mut packets = Vec::new();
        let mut pad_out = Vec::new();
        let natural = super::preferences::natural_scroll();
        let tap = super::preferences::tap_to_click();
        // Poll nonblocking descriptors directly. Packet state belongs to each
        // device, so a partial report never spins the UI or mixes two devices.
        self.devices.retain_mut(|device| {
            const EVENT_SIZE: usize = std::mem::size_of::<InputEvent>();
            let mut buffer = [0u8; EVENT_SIZE * 64];
            // Bound work per device so an input flood cannot starve rendering.
            for _ in 0..16 {
                let len = match device.file.read(&mut buffer) {
                    Ok(0) => return false,
                    Ok(len) => len,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        eprintln!("Direct input: disconnected {}: {error}", device.path.display());
                        return false;
                    }
                };
                for bytes in buffer[..len].chunks_exact(EVENT_SIZE) {
                    // input_event consists only of integers; read_unaligned
                    // also works when the byte buffer has no struct alignment.
                    let event = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<InputEvent>()) };
                    if event.ty == InputEventType::EV_SYN {
                        if event.code == EvSynCodes::SYN_DROPPED {
                            device.pending.clear();
                            device.dropped = true;
                            eprintln!("Direct input: event queue overrun on {}", device.path.display());
                        } else if event.code == EvSynCodes::SYN_REPORT {
                            if !device.dropped {
                                if !device.reported_input && device.pending.iter().any(|event| {
                                    matches!(event.ty, InputEventType::EV_KEY | InputEventType::EV_REL)
                                }) {
                                    eprintln!("Direct input: receiving keyboard/pointer reports from {}", device.path.display());
                                    device.reported_input = true;
                                }
                                if let Some(pad) = device.touchpad.as_mut() {
                                    device.pending.retain_mut(|event| !pad.consume(event));
                                    let report_time = event.time.tv_sec as f64 + event.time.tv_usec as f64 * 1e-6;
                                    pad.finish_report(report_time, natural, tap, &mut pad_out);
                                    packets.extend(pad_out.drain(..).map(|out| (device.id, event.time, DeviceEvent::Touchpad(out))));
                                }
                                packets.extend(device.pending.drain(..)
                                    .map(|event| (device.id, event.time, DeviceEvent::Raw(event))));
                            } else if let Some(pad) = device.touchpad.as_mut() {
                                // The kernel requires a state query after SYN_DROPPED.
                                // Hold a gesture still until lift instead of jumping.
                                pad.resync(&device.file, &mut pad_out);
                                packets.extend(pad_out.drain(..).map(|out| (device.id, event.time, DeviceEvent::Touchpad(out))));
                            }
                            device.dropped = false;
                        }
                    } else if !device.dropped {
                        if device.pending.len() < 1024 {
                            device.pending.push(event);
                        } else {
                            device.pending.clear();
                            device.dropped = true;
                        }
                    }
                }
            }
            true
        });
        // Composite keyboards can report modifiers and keys on different
        // event nodes. Preserve kernel order, not device enumeration order.
        packets.sort_by_key(|(_, time, _)| (time.tv_sec, time.tv_usec));
        // A touchpad coast advances on the event loop's clock (it wakes at
        // least every 8 ms), after this poll's reports: a touch in them has
        // already stopped it.
        for device in &mut self.devices {
            if let Some(pad) = device.touchpad.as_mut() {
                pad.gesture.tick(time, &mut pad_out);
                packets.extend(pad_out.drain(..).map(|out| (device.id, Default::default(), DeviceEvent::Touchpad(out))));
            }
        }
        self.modifier_keys.retain(|(id, _)| self.devices.iter().any(|device| device.id == *id));
        self.update_modifiers();
        self.process_event(&mut packets, &mut dir_evts, time, window_id);
        dir_evts
    }

    fn process_event(
        &mut self,
        evts: &mut Vec<(u64, libc_sys::timeval, DeviceEvent)>,
        dir_evts: &mut EventQueue,
        time: f64,
        window_id: Id,
    ) {
        for (device_id, _, event) in evts.drain(..) {
            let evt = match event {
                DeviceEvent::Raw(evt) => evt,
                DeviceEvent::Touchpad(PadOutput::Pointer(millimeters)) => {
                    let delta = millimeters * (20.0 * super::preferences::pointer_speed(true) as f64 / 100.0 / self.dpi_factor);
                    self.abs.x = (self.abs.x + delta.x).clamp(0.0, self.width.max(0.0));
                    self.abs.y = (self.abs.y + delta.y).clamp(0.0, self.height.max(0.0));
                    self.clamp_to_screens();
                    dir_evts.mouse_move(window_id, self.abs.x, self.abs.y, 0.0, 0.0, self.modifiers.bits());
                    continue;
                }
                DeviceEvent::Touchpad(PadOutput::Scroll { delta, phase }) => {
                    // Points already: smooth pixel deltas, never line steps.
                    dir_evts.scroll(window_id, self.abs.x, self.abs.y, delta.x, delta.y, self.modifiers.bits(), true, phase);
                    continue;
                }
                DeviceEvent::Touchpad(PadOutput::Pinch { scale, phase }) => {
                    dir_evts.pinch(window_id, self.abs.x, self.abs.y, scale, phase);
                    continue;
                }
                DeviceEvent::Touchpad(PadOutput::Tap(button)) => {
                    let mask = 1 << (pad_button_code(button) - EvKeyCodes::BTN_LEFT);
                    dir_evts.mouse_down(window_id, self.abs.x, self.abs.y, mask, self.modifiers.bits());
                    dir_evts.mouse_up(window_id, self.abs.x, self.abs.y, mask, self.modifiers.bits());
                    continue;
                }
            };
            match evt.ty {
                InputEventType::EV_REL => {
                    // relative input
                    self.process_rel_event(evt, dir_evts, time, window_id)
                }
                InputEventType::EV_ABS => {
                    // absolute input
                    self.process_abs_event(device_id, evt, dir_evts, time, window_id)
                }
                InputEventType::EV_KEY => {
                    // key press
                    self.process_key_event(device_id, evt, dir_evts, time, window_id)
                }
                _ => (),
            }
        }
    }

    fn process_rel_event(
        &mut self,
        evt: InputEvent,
        dir_evts: &mut EventQueue,
        _time: f64,
        window_id: Id,
    ) {
        let code = evt.code;
        match code {
            EvRelCodes::REL_X => {
                self.abs.x += evt.value as f64 * super::preferences::pointer_speed(false) as f64 / 100.0 / self.dpi_factor;
                if self.abs.x < 0.0 {
                    self.abs.x = 0.0
                }
                if self.abs.x > self.width {
                    self.abs.x = self.width
                }
                self.clamp_to_screens();
            }
            EvRelCodes::REL_Y => {
                self.abs.y += evt.value as f64 * super::preferences::pointer_speed(false) as f64 / 100.0 / self.dpi_factor;
                if self.abs.y < 0.0 {
                    self.abs.y = 0.0
                }
                if self.abs.y > self.height {
                    self.abs.y = self.height
                }
                self.clamp_to_screens();
            }
            EvRelCodes::REL_WHEEL | EvRelCodes::REL_HWHEEL => {
                let delta = evt.value as f64 * 40.0;
                dir_evts.scroll(window_id, self.abs.x, self.abs.y,
                    if code == EvRelCodes::REL_HWHEEL { delta } else { 0.0 },
                    if code == EvRelCodes::REL_WHEEL { -delta } else { 0.0 },
                    self.modifiers.bits(), false, input::scroll::NONE);
                return;
            }
            _ => return (),
        }
        dir_evts.mouse_move(window_id, self.abs.x, self.abs.y, 0.0, 0.0, self.modifiers.bits())
    }

    fn process_abs_event(
        &mut self,
        device_id: u64,
        evt: InputEvent,
        dir_evts: &mut EventQueue,
        _time: f64,
        window_id: Id,
    ) {
        // Wheels, pedals and gamepad axes are not absolute pointing devices.
        let Some((axes, mt)) = self.devices.iter().find(|device| device.id == device_id)
            .and_then(|device| device.absolute_axes.map(|axes| (axes, device.absolute_mt))) else { return; };
        let normalized = |axis: AbsInfo| {
            ((evt.value as f64 - axis.minimum as f64) / (axis.maximum as f64 - axis.minimum as f64)).clamp(0.0, 1.0)
        };
        match (evt.code, mt) {
            (EvAbsCodes::ABS_X, false) | (EvAbsCodes::ABS_MT_POSITION_X, true) => {
                self.abs.x = normalized(axes[0]) * self.width;
            }
            (EvAbsCodes::ABS_Y, false) | (EvAbsCodes::ABS_MT_POSITION_Y, true) => {
                self.abs.y = normalized(axes[1]) * self.height;
            }
            _ => return (),
        }
        dir_evts.mouse_move(window_id, self.abs.x, self.abs.y, 0.0, 0.0, self.modifiers.bits())
    }

    fn update_modifiers(&mut self) {
        let held = |left, right| self.modifier_keys.iter().any(|(_, key)| *key == left || *key == right);
        self.modifiers = Modifiers {
            shift: held(EvKeyCodes::KEY_LEFTSHIFT, EvKeyCodes::KEY_RIGHTSHIFT),
            control: held(EvKeyCodes::KEY_LEFTCTRL, EvKeyCodes::KEY_RIGHTCTRL),
            alt: held(EvKeyCodes::KEY_LEFTALT, EvKeyCodes::KEY_RIGHTALT),
            logo: held(EvKeyCodes::KEY_LEFTMETA, EvKeyCodes::KEY_RIGHTMETA),
        };
    }

    fn process_key_event(
        &mut self,
        device_id: u64,
        evt: InputEvent,
        dir_evts: &mut EventQueue,
        _time: f64,
        window_id: Id,
    ) {
        let code = evt.code;
        let key_action = evt.value;
        if !(KeyAction::KEY_UP..=KeyAction::KEY_REPEAT).contains(&key_action) {
            return;
        }
        let key_code = match code {
            EvKeyCodes::KEY_ESC => KeyCode::Escape,
            EvKeyCodes::KEY_1 => KeyCode::Key1,
            EvKeyCodes::KEY_2 => KeyCode::Key2,
            EvKeyCodes::KEY_3 => KeyCode::Key3,
            EvKeyCodes::KEY_4 => KeyCode::Key4,
            EvKeyCodes::KEY_5 => KeyCode::Key5,
            EvKeyCodes::KEY_6 => KeyCode::Key6,
            EvKeyCodes::KEY_7 => KeyCode::Key7,
            EvKeyCodes::KEY_8 => KeyCode::Key8,
            EvKeyCodes::KEY_9 => KeyCode::Key9,
            EvKeyCodes::KEY_0 => KeyCode::Key0,
            EvKeyCodes::KEY_MINUS => KeyCode::Minus,
            EvKeyCodes::KEY_EQUAL => KeyCode::Equals,
            EvKeyCodes::KEY_BACKSPACE => KeyCode::Backspace,
            EvKeyCodes::KEY_TAB => KeyCode::Tab,
            EvKeyCodes::KEY_Q => KeyCode::KeyQ,
            EvKeyCodes::KEY_W => KeyCode::KeyW,
            EvKeyCodes::KEY_E => KeyCode::KeyE,
            EvKeyCodes::KEY_R => KeyCode::KeyR,
            EvKeyCodes::KEY_T => KeyCode::KeyT,
            EvKeyCodes::KEY_Y => KeyCode::KeyY,
            EvKeyCodes::KEY_U => KeyCode::KeyU,
            EvKeyCodes::KEY_I => KeyCode::KeyI,
            EvKeyCodes::KEY_O => KeyCode::KeyO,
            EvKeyCodes::KEY_P => KeyCode::KeyP,
            EvKeyCodes::KEY_LEFTBRACE => KeyCode::LBracket,
            EvKeyCodes::KEY_RIGHTBRACE => KeyCode::RBracket,
            EvKeyCodes::KEY_ENTER => KeyCode::ReturnKey,
            EvKeyCodes::KEY_LEFTCTRL => KeyCode::Control,
            EvKeyCodes::KEY_A => KeyCode::KeyA,
            EvKeyCodes::KEY_S => KeyCode::KeyS,
            EvKeyCodes::KEY_D => KeyCode::KeyD,
            EvKeyCodes::KEY_F => KeyCode::KeyF,
            EvKeyCodes::KEY_G => KeyCode::KeyG,
            EvKeyCodes::KEY_H => KeyCode::KeyH,
            EvKeyCodes::KEY_J => KeyCode::KeyJ,
            EvKeyCodes::KEY_K => KeyCode::KeyK,
            EvKeyCodes::KEY_L => KeyCode::KeyL,
            EvKeyCodes::KEY_SEMICOLON => KeyCode::Semicolon,
            EvKeyCodes::KEY_APOSTROPHE => KeyCode::Quote,
            EvKeyCodes::KEY_GRAVE => KeyCode::Backtick,
            EvKeyCodes::KEY_LEFTSHIFT => KeyCode::Shift,
            EvKeyCodes::KEY_BACKSLASH => KeyCode::Backslash,
            EvKeyCodes::KEY_Z => KeyCode::KeyZ,
            EvKeyCodes::KEY_X => KeyCode::KeyX,
            EvKeyCodes::KEY_C => KeyCode::KeyC,
            EvKeyCodes::KEY_V => KeyCode::KeyV,
            EvKeyCodes::KEY_B => KeyCode::KeyB,
            EvKeyCodes::KEY_N => KeyCode::KeyN,
            EvKeyCodes::KEY_M => KeyCode::KeyM,
            EvKeyCodes::KEY_COMMA => KeyCode::Comma,
            EvKeyCodes::KEY_DOT => KeyCode::Period,
            EvKeyCodes::KEY_SLASH => KeyCode::Slash,
            EvKeyCodes::KEY_RIGHTSHIFT => KeyCode::Shift,
            EvKeyCodes::KEY_KPASTERISK => KeyCode::NumpadMultiply,
            EvKeyCodes::KEY_LEFTALT => KeyCode::Alt,
            EvKeyCodes::KEY_SPACE => KeyCode::Space,
            EvKeyCodes::KEY_CAPSLOCK => KeyCode::Capslock,
            EvKeyCodes::KEY_F1 => KeyCode::F1,
            EvKeyCodes::KEY_F2 => KeyCode::F2,
            EvKeyCodes::KEY_F3 => KeyCode::F3,
            EvKeyCodes::KEY_F4 => KeyCode::F4,
            EvKeyCodes::KEY_F5 => KeyCode::F5,
            EvKeyCodes::KEY_F6 => KeyCode::F6,
            EvKeyCodes::KEY_F7 => KeyCode::F7,
            EvKeyCodes::KEY_F8 => KeyCode::F8,
            EvKeyCodes::KEY_F9 => KeyCode::F9,
            EvKeyCodes::KEY_F10 => KeyCode::F10,
            EvKeyCodes::KEY_NUMLOCK => KeyCode::Numlock,
            EvKeyCodes::KEY_SCROLLLOCK => KeyCode::ScrollLock,
            EvKeyCodes::KEY_KP7 => KeyCode::Numpad7,
            EvKeyCodes::KEY_KP8 => KeyCode::Numpad8,
            EvKeyCodes::KEY_KP9 => KeyCode::Numpad9,
            EvKeyCodes::KEY_KPMINUS => KeyCode::NumpadSubtract,
            EvKeyCodes::KEY_KP4 => KeyCode::Numpad4,
            EvKeyCodes::KEY_KP5 => KeyCode::Numpad5,
            EvKeyCodes::KEY_KP6 => KeyCode::Numpad6,
            EvKeyCodes::KEY_KPPLUS => KeyCode::NumpadAdd,
            EvKeyCodes::KEY_KP1 => KeyCode::Numpad1,
            EvKeyCodes::KEY_KP2 => KeyCode::Numpad2,
            EvKeyCodes::KEY_KP3 => KeyCode::Numpad3,
            EvKeyCodes::KEY_KP0 => KeyCode::Numpad0,
            EvKeyCodes::KEY_KPDOT => KeyCode::NumpadDecimal,
            EvKeyCodes::KEY_ZENKAKUHANKAKU => KeyCode::Unknown,
            EvKeyCodes::KEY_102ND => KeyCode::Backtick, //Seems odd but this was in the code this replaced
            EvKeyCodes::KEY_F11 => KeyCode::F11,
            EvKeyCodes::KEY_F12 => KeyCode::F12,
            EvKeyCodes::KEY_RO => KeyCode::NumpadDivide, //Seems odd but this was in the code this replaced
            EvKeyCodes::KEY_KPENTER => KeyCode::NumpadEnter,
            EvKeyCodes::KEY_RIGHTCTRL => KeyCode::Control,
            EvKeyCodes::KEY_KPSLASH => KeyCode::NumpadDivide,
            EvKeyCodes::KEY_SYSRQ => KeyCode::PrintScreen,
            EvKeyCodes::KEY_RIGHTALT => KeyCode::Alt,
            EvKeyCodes::KEY_HOME => KeyCode::Home,
            EvKeyCodes::KEY_UP => KeyCode::ArrowUp,
            EvKeyCodes::KEY_PAGEUP => KeyCode::PageUp,
            EvKeyCodes::KEY_LEFT => KeyCode::ArrowLeft,
            EvKeyCodes::KEY_RIGHT => KeyCode::ArrowRight,
            EvKeyCodes::KEY_END => KeyCode::End,
            EvKeyCodes::KEY_DOWN => KeyCode::ArrowDown,
            EvKeyCodes::KEY_PAGEDOWN => KeyCode::PageDown,
            EvKeyCodes::KEY_INSERT => KeyCode::Insert,
            EvKeyCodes::KEY_DELETE => KeyCode::Delete,
            EvKeyCodes::KEY_LEFTMETA => KeyCode::Logo,
            EvKeyCodes::KEY_RIGHTMETA => KeyCode::Logo,
            _ => KeyCode::Unknown,
        };
        if matches!(code, EvKeyCodes::KEY_LEFTSHIFT | EvKeyCodes::KEY_RIGHTSHIFT
            | EvKeyCodes::KEY_LEFTCTRL | EvKeyCodes::KEY_RIGHTCTRL
            | EvKeyCodes::KEY_LEFTALT | EvKeyCodes::KEY_RIGHTALT
            | EvKeyCodes::KEY_LEFTMETA | EvKeyCodes::KEY_RIGHTMETA) {
            if key_action == KeyAction::KEY_UP { self.modifier_keys.remove(&(device_id, code)); }
            else { self.modifier_keys.insert((device_id, code)); }
            self.update_modifiers();
        }

        match key_action {
            KeyAction::KEY_DOWN => {
                match code {
                    EvKeyCodes::BTN_LEFT
                    | EvKeyCodes::BTN_RIGHT
                    | EvKeyCodes::BTN_MIDDLE
                    | EvKeyCodes::BTN_SIDE
                    | EvKeyCodes::BTN_EXTRA => {
                        dir_evts.mouse_down(window_id, self.abs.x, self.abs.y, 1 << (evt.code - EvKeyCodes::BTN_LEFT), self.modifiers.bits())
                    }
                    EvKeyCodes::BTN_TOUCH => {
                        dir_evts.mouse_down(window_id, self.abs.x, self.abs.y, 1, self.modifiers.bits())
                    }
                    _ => {
                        if !self.modifiers.control && !self.modifiers.alt && !self.modifiers.logo {
                            let uc = self.modifiers.shift;
                            let inp = key_code.to_char(uc);
                            if let Some(inp) = inp {
                                dir_evts.text(window_id, input::text::TYPED, inp.to_string().as_bytes());
                            }
                        }
                        dir_evts.key_down(window_id, key_code.to_index(), false, self.modifiers.bits())
                    }
                }
            }
            KeyAction::KEY_UP => {
                match code {
                    EvKeyCodes::BTN_LEFT
                    | EvKeyCodes::BTN_RIGHT
                    | EvKeyCodes::BTN_MIDDLE
                    | EvKeyCodes::BTN_SIDE
                    | EvKeyCodes::BTN_EXTRA => dir_evts.mouse_up(window_id, self.abs.x, self.abs.y, 1 << (evt.code - EvKeyCodes::BTN_LEFT), self.modifiers.bits()),
                    EvKeyCodes::BTN_TOUCH => dir_evts.mouse_up(window_id, self.abs.x, self.abs.y, 1, self.modifiers.bits()),
                    _ => dir_evts.key_up(window_id, key_code.to_index(), self.modifiers.bits()),
                }
            }
            KeyAction::KEY_REPEAT => {
                if !self.modifiers.control && !self.modifiers.alt && !self.modifiers.logo {
                    let uc = self.modifiers.shift;
                    let inp = key_code.to_char(uc);
                    if let Some(inp) = inp {
                        dir_evts.text(window_id, input::text::TYPED, inp.to_string().as_bytes());
                    }
                }
                dir_evts.key_down(window_id, key_code.to_index(), true, self.modifiers.bits())
            }
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis(maximum: i32) -> AbsInfo {
        AbsInfo { maximum, resolution: 32, ..Default::default() }
    }

    /// The clickpad on node 149: 4762 x 3099 units at 32 units/mm, 5 slots.
    fn clickpad() -> Touchpad {
        Touchpad::new([axis(4762), axis(3099)], 5, true, true)
    }

    fn event(time: f64, ty: u16, code: u16, value: i32) -> InputEvent {
        let time = libc_sys::timeval { tv_sec: time.floor() as _, tv_usec: ((time - time.floor()) * 1e6).round() as _ };
        InputEvent { time, ty, code, value }
    }

    /// `evtest` output as events (headers and comments skipped).
    fn parse_evtest(text: &str) -> Vec<InputEvent> {
        text.lines().filter_map(|line| {
            let rest = line.strip_prefix("Event: time ")?;
            let (time, rest) = rest.split_once(", ")?;
            let time: f64 = time.parse().ok()?;
            if rest.contains("SYN_REPORT") {
                return Some(event(time, InputEventType::EV_SYN, EvSynCodes::SYN_REPORT, 0));
            }
            let number = |key: &str| -> Option<i32> {
                let at = rest.find(key)? + key.len();
                rest[at..].split(|c: char| !(c.is_ascii_digit() || c == '-')).next()?.parse().ok()
            };
            Some(event(time, number("type ")? as u16, number("code ")? as u16, number("value ")?))
        }).collect()
    }

    /// Runs events through the pad report by report, like `poll_raw_input`:
    /// the gesture outputs, and the events left for the ordinary path.
    fn feed(pad: &mut Touchpad, events: &[InputEvent]) -> (Vec<PadOutput>, Vec<InputEvent>) {
        let (mut out, mut raw, mut pending) = (Vec::new(), Vec::new(), Vec::new());
        for event in events {
            if event.ty == InputEventType::EV_SYN {
                pending.retain_mut(|event| !pad.consume(event));
                let time = event.time.tv_sec as f64 + event.time.tv_usec as f64 * 1e-6;
                pad.finish_report(time, true, true, &mut out);
                raw.append(&mut pending);
            } else {
                pending.push(*event);
            }
        }
        (out, raw)
    }

    #[test]
    fn recorded_one_finger_move_and_click_points_and_clicks_primary() {
        let events = parse_evtest(include_str!("testdata/clickpad_one_finger_click.evtest"));
        assert!(events.len() > 400);
        let mut pad = clickpad();
        let (out, raw) = feed(&mut pad, &events);
        let touched = out.iter().filter(|o| matches!(o, PadOutput::Scroll { phase: input::scroll::TOUCHED, .. })).count();
        assert_eq!(touched, 1, "one touch");
        assert!(!out.iter().any(|o| matches!(o, PadOutput::Scroll { phase, .. } if *phase != input::scroll::TOUCHED)),
            "one finger never scrolls");
        let moved = out.iter().filter_map(|o| match o { PadOutput::Pointer(d) => Some(*d), _ => None })
            .fold(dvec2(0.0, 0.0), |s, d| s + d);
        assert!(moved.length() > 5.0, "the finger moved the pointer: {moved:?} mm");
        let buttons: Vec<(u16, i32)> = raw.iter().filter(|e| e.ty == InputEventType::EV_KEY).map(|e| (e.code, e.value)).collect();
        assert_eq!(buttons, [(EvKeyCodes::BTN_LEFT, 1), (EvKeyCodes::BTN_LEFT, 0)]);
    }

    /// Two fingers (protocol B) moving `step` units per report for `reports`
    /// reports from y = `y`, every 7 ms from `t`.
    fn two_fingers(t: &mut f64, y: i32, step: i32, reports: i32, events: &mut Vec<InputEvent>) {
        use EvAbsCodes::*;
        for i in 0..reports {
            *t += 0.007;
            let y = y + step * i;
            if i == 0 {
                for (slot, x, id) in [(0, 1500, 7), (1, 2200, 8)] {
                    events.push(event(*t, InputEventType::EV_ABS, ABS_MT_SLOT, slot));
                    events.push(event(*t, InputEventType::EV_ABS, ABS_MT_TRACKING_ID, id));
                    events.push(event(*t, InputEventType::EV_ABS, ABS_MT_POSITION_X, x));
                    events.push(event(*t, InputEventType::EV_ABS, ABS_MT_POSITION_Y, y));
                }
                events.push(event(*t, InputEventType::EV_KEY, EvKeyCodes::BTN_TOUCH, 1));
                events.push(event(*t, InputEventType::EV_KEY, EvKeyCodes::BTN_TOOL_DOUBLETAP, 1));
            } else {
                for slot in [0, 1] {
                    events.push(event(*t, InputEventType::EV_ABS, ABS_MT_SLOT, slot));
                    events.push(event(*t, InputEventType::EV_ABS, ABS_MT_POSITION_Y, y));
                }
            }
            events.push(event(*t, InputEventType::EV_SYN, EvSynCodes::SYN_REPORT, 0));
        }
    }

    fn lift(t: &mut f64, events: &mut Vec<InputEvent>) {
        *t += 0.007;
        for slot in [0, 1] {
            events.push(event(*t, InputEventType::EV_ABS, EvAbsCodes::ABS_MT_SLOT, slot));
            events.push(event(*t, InputEventType::EV_ABS, EvAbsCodes::ABS_MT_TRACKING_ID, -1));
        }
        events.push(event(*t, InputEventType::EV_KEY, EvKeyCodes::BTN_TOUCH, 0));
        events.push(event(*t, InputEventType::EV_KEY, EvKeyCodes::BTN_TOOL_DOUBLETAP, 0));
        events.push(event(*t, InputEventType::EV_SYN, EvSynCodes::SYN_REPORT, 0));
    }

    #[test]
    fn two_finger_protocol_b_swipe_scrolls_naturally_and_coasts() {
        let (mut t, mut events) = (10.0, Vec::new());
        // Up the pad at 0.8 mm per report.
        two_fingers(&mut t, 2500, -26, 40, &mut events);
        lift(&mut t, &mut events);
        let mut pad = clickpad();
        let (out, raw) = feed(&mut pad, &events);
        assert!(raw.is_empty(), "touch keys and coordinates are the pad's own");
        let phases: Vec<u32> = out.iter().filter_map(|o| match o {
            PadOutput::Scroll { phase, .. } => Some(*phase),
            _ => None,
        }).collect();
        assert_eq!(phases.first(), Some(&input::scroll::TOUCHED));
        assert_eq!(phases.get(1), Some(&input::scroll::BEGAN));
        assert_eq!(phases.last(), Some(&input::scroll::ENDED));
        let scrolled = out.iter().filter_map(|o| match o { PadOutput::Scroll { delta, .. } => Some(delta.y), _ => None }).sum::<f64>();
        assert!(scrolled > 0.0, "natural: fingers up scroll the view down: {scrolled}");
        assert!(pad.gesture.is_coasting());
        assert!(!out.iter().any(|o| matches!(o, PadOutput::Pointer(_))));
    }

    #[test]
    fn a_two_finger_clickpad_press_is_the_secondary_button() {
        let (mut t, mut events) = (10.0, Vec::new());
        two_fingers(&mut t, 2000, 0, 5, &mut events);
        t += 0.007;
        events.push(event(t, InputEventType::EV_KEY, EvKeyCodes::BTN_LEFT, 1));
        events.push(event(t, InputEventType::EV_SYN, EvSynCodes::SYN_REPORT, 0));
        t += 0.007;
        // Released after one finger already lifted: still the secondary button.
        events.push(event(t, InputEventType::EV_ABS, EvAbsCodes::ABS_MT_SLOT, 1));
        events.push(event(t, InputEventType::EV_ABS, EvAbsCodes::ABS_MT_TRACKING_ID, -1));
        events.push(event(t, InputEventType::EV_KEY, EvKeyCodes::BTN_LEFT, 0));
        events.push(event(t, InputEventType::EV_SYN, EvSynCodes::SYN_REPORT, 0));
        let mut pad = clickpad();
        let (_, raw) = feed(&mut pad, &events);
        let buttons: Vec<(u16, i32)> = raw.iter().map(|e| (e.code, e.value)).collect();
        assert_eq!(buttons, [(EvKeyCodes::BTN_RIGHT, 1), (EvKeyCodes::BTN_RIGHT, 0)]);
    }

    /// One finger (protocol B) at `x`, `y` units, a click, a drag of
    /// `drag` units right, and the release and lift.
    fn one_finger_click_drag(x: i32, y: i32, drag: i32) -> Vec<InputEvent> {
        use EvAbsCodes::*;
        let (mut t, mut events) = (10.0, Vec::new());
        let syn = |t: f64, events: &mut Vec<InputEvent>| events.push(event(t, InputEventType::EV_SYN, EvSynCodes::SYN_REPORT, 0));
        t += 0.007;
        events.push(event(t, InputEventType::EV_ABS, ABS_MT_SLOT, 0));
        events.push(event(t, InputEventType::EV_ABS, ABS_MT_TRACKING_ID, 3));
        events.push(event(t, InputEventType::EV_ABS, ABS_MT_POSITION_X, x));
        events.push(event(t, InputEventType::EV_ABS, ABS_MT_POSITION_Y, y));
        events.push(event(t, InputEventType::EV_KEY, EvKeyCodes::BTN_TOUCH, 1));
        events.push(event(t, InputEventType::EV_KEY, EvKeyCodes::BTN_TOOL_FINGER, 1));
        syn(t, &mut events);
        t += 0.007;
        events.push(event(t, InputEventType::EV_KEY, EvKeyCodes::BTN_LEFT, 1));
        syn(t, &mut events);
        for i in 1..=10 {
            t += 0.007;
            events.push(event(t, InputEventType::EV_ABS, ABS_MT_POSITION_X, x + drag * i / 10));
            syn(t, &mut events);
        }
        t += 0.007;
        events.push(event(t, InputEventType::EV_KEY, EvKeyCodes::BTN_LEFT, 0));
        syn(t, &mut events);
        t += 0.007;
        events.push(event(t, InputEventType::EV_ABS, ABS_MT_TRACKING_ID, -1));
        events.push(event(t, InputEventType::EV_KEY, EvKeyCodes::BTN_TOUCH, 0));
        events.push(event(t, InputEventType::EV_KEY, EvKeyCodes::BTN_TOOL_FINGER, 0));
        syn(t, &mut events);
        events
    }

    #[test]
    fn a_bottom_right_click_is_the_secondary_button_for_its_whole_drag() {
        // 4400, 3000 units = 137, 94 mm on the 149 x 97 mm pad; the drag leaves the corner.
        let mut pad = clickpad();
        let (out, raw) = feed(&mut pad, &one_finger_click_drag(4400, 3000, -1600));
        let buttons: Vec<(u16, i32)> = raw.iter().map(|e| (e.code, e.value)).collect();
        assert_eq!(buttons, [(EvKeyCodes::BTN_RIGHT, 1), (EvKeyCodes::BTN_RIGHT, 0)]);
        let moved = out.iter().filter_map(|o| match o { PadOutput::Pointer(d) => Some(*d), _ => None })
            .fold(dvec2(0.0, 0.0), |s, d| s + d);
        assert!(moved.x < -40.0, "dragged with the button held: {moved:?} mm");
    }

    #[test]
    fn a_click_elsewhere_is_the_primary_button() {
        for (x, y) in [(2400, 1500), (800, 3000), (4400, 2000)] {
            let mut pad = clickpad();
            let (_, raw) = feed(&mut pad, &one_finger_click_drag(x, y, 600));
            let buttons: Vec<(u16, i32)> = raw.iter().map(|e| (e.code, e.value)).collect();
            assert_eq!(buttons, [(EvKeyCodes::BTN_LEFT, 1), (EvKeyCodes::BTN_LEFT, 0)], "at {x}, {y}");
        }
    }

    #[test]
    fn a_two_finger_tap_is_a_secondary_tap() {
        let (mut t, mut events) = (10.0, Vec::new());
        two_fingers(&mut t, 2000, 0, 3, &mut events);
        lift(&mut t, &mut events);
        let mut pad = clickpad();
        let (out, raw) = feed(&mut pad, &events);
        assert!(raw.is_empty());
        let taps: Vec<&PadOutput> = out.iter().filter(|o| matches!(o, PadOutput::Tap(_))).collect();
        assert_eq!(taps, [&PadOutput::Tap(PadButton::Secondary)]);
    }

    #[test]
    fn a_palm_slot_is_not_a_finger() {
        let (mut t, mut events) = (10.0, Vec::new());
        two_fingers(&mut t, 2500, -26, 3, &mut events);
        t += 0.007;
        events.push(event(t, InputEventType::EV_ABS, EvAbsCodes::ABS_MT_SLOT, 1));
        events.push(event(t, InputEventType::EV_ABS, EvAbsCodes::ABS_MT_TOOL_TYPE, MT_TOOL_PALM));
        events.push(event(t, InputEventType::EV_SYN, EvSynCodes::SYN_REPORT, 0));
        let mut pad = clickpad();
        feed(&mut pad, &events);
        assert_eq!(pad.gesture.finger_count(), 1);
    }
}
