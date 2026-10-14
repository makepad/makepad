//! wl_seat: pointer and keyboard focus and event delivery.
use super::*;
use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd};
use xkb_keymap::{Layout, Tracker};

pub(crate) struct InputState {
    tracker: Tracker,
    keymap: std::fs::File,
    keymap_size: u32,
    pointers: HashMap<ClientId, Vec<u32>>,
    keyboards: HashMap<ClientId, Vec<u32>>,
    pub pointer_focus: Option<SurfKey>,
    pub kb_focus: Option<SurfKey>,
}

impl InputState {
    pub fn new(layout: &'static Layout) -> std::io::Result<Self> {
        let bytes = xkb_keymap::text::keymap_bytes(layout);
        let fd = unsafe { crate::sys::memfd_create(c"xkb-keymap".as_ptr(), crate::sys::MFD_CLOEXEC) };
        if fd < 0 { return Err(std::io::Error::last_os_error()); }
        let mut file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        file.write_all(&bytes)?;
        Ok(Self { tracker: Tracker::new(layout), keymap: file, keymap_size: bytes.len() as u32,
                  pointers: HashMap::new(), keyboards: HashMap::new(), pointer_focus: None, kb_focus: None })
    }
    pub fn client_gone(&mut self, c: ClientId) {
        self.pointers.remove(&c);
        self.keyboards.remove(&c);
        if self.pointer_focus.is_some_and(|k| k.0 == c) { self.pointer_focus = None; }
        if self.kb_focus.is_some_and(|k| k.0 == c) { self.kb_focus = None; }
    }
    pub fn surface_gone(&mut self, k: SurfKey) {
        if self.pointer_focus == Some(k) { self.pointer_focus = None; }
        if self.kb_focus == Some(k) { self.kb_focus = None; }
    }
    pub fn pointer_gone(&mut self, c: ClientId, id: u32) { if let Some(v) = self.pointers.get_mut(&c) { v.retain(|&p| p != id); } }
    pub fn keyboard_gone(&mut self, c: ClientId, id: u32) { if let Some(v) = self.keyboards.get_mut(&c) { v.retain(|&p| p != id); } }
}

impl Compositor {
    pub(crate) fn seat_request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &[Value]) {
        match op {
            wl_seat::REQ_GET_POINTER => {
                let id = a[0].as_uint();
                self.objs.insert((c, id), Obj::Pointer);
                self.input.pointers.entry(c).or_default().push(id);
            }
            wl_seat::REQ_GET_KEYBOARD => {
                let id = a[0].as_uint();
                self.objs.insert((c, id), Obj::Keyboard);
                self.input.keyboards.entry(c).or_default().push(id);
                if let Ok(fd) = self.input.keymap.try_clone() {
                    let _ = core.send(c, id, wl_keyboard::EVT_KEYMAP, vec![Value::Uint(1), Value::Fd(fd.into()), Value::Uint(self.input.keymap_size)]);
                }
                let _ = core.send(c, id, wl_keyboard::EVT_REPEAT_INFO, vec![Value::Int(25), Value::Int(600)]);
            }
            _ => { let _ = o; }
        }
    }

    fn pointers_of(&self, c: ClientId) -> Vec<u32> { self.input.pointers.get(&c).cloned().unwrap_or_default() }
    fn keyboards_of(&self, c: ClientId) -> Vec<u32> { self.input.keyboards.get(&c).cloned().unwrap_or_default() }

    /// Pointer enters `k` at surface-local (x, y), leaving the previous surface.
    pub fn pointer_enter(&mut self, core: &mut Core, k: SurfKey, x: f64, y: f64) {
        if self.input.pointer_focus == Some(k) { return; }
        self.pointer_leave(core);
        let serial = core.next_serial();
        for p in self.pointers_of(k.0) {
            let _ = core.send(k.0, p, wl_pointer::EVT_ENTER, vec![Value::Uint(serial), Value::Object(k.1), Value::fixed(x), Value::fixed(y)]);
            let _ = core.send(k.0, p, wl_pointer::EVT_FRAME, vec![]);
        }
        self.input.pointer_focus = Some(k);
    }
    pub fn pointer_leave(&mut self, core: &mut Core) {
        let Some(k) = self.input.pointer_focus.take() else { return };
        let serial = core.next_serial();
        for p in self.pointers_of(k.0) {
            let _ = core.send(k.0, p, wl_pointer::EVT_LEAVE, vec![Value::Uint(serial), Value::Object(k.1)]);
            let _ = core.send(k.0, p, wl_pointer::EVT_FRAME, vec![]);
        }
    }
    pub fn pointer_motion(&mut self, core: &mut Core, time: u32, x: f64, y: f64) {
        let Some(k) = self.input.pointer_focus else { return };
        for p in self.pointers_of(k.0) {
            let _ = core.send(k.0, p, wl_pointer::EVT_MOTION, vec![Value::Uint(time), Value::fixed(x), Value::fixed(y)]);
            let _ = core.send(k.0, p, wl_pointer::EVT_FRAME, vec![]);
        }
    }
    /// `button` is an evdev code (BTN_LEFT = 0x110). Returns the serial used.
    pub fn pointer_button(&mut self, core: &mut Core, time: u32, button: u32, pressed: bool) -> u32 {
        let serial = core.next_serial();
        let Some(k) = self.input.pointer_focus else { return serial };
        for p in self.pointers_of(k.0) {
            let _ = core.send(k.0, p, wl_pointer::EVT_BUTTON, vec![Value::Uint(serial), Value::Uint(time), Value::Uint(button), Value::Uint(pressed as u32)]);
            let _ = core.send(k.0, p, wl_pointer::EVT_FRAME, vec![]);
        }
        serial
    }
    /// Scroll. `axis`: 0 vertical, 1 horizontal. `value` in surface units; `v120` is the wheel
    /// click count times 120 for devices that report it (0 for smooth/touchpad).
    pub fn pointer_axis(&mut self, core: &mut Core, time: u32, axis: u32, value: f64, v120: i32) {
        let Some(k) = self.input.pointer_focus else { return };
        for p in self.pointers_of(k.0) {
            let ver = core.obj(k.0, p).map_or(1, |o| o.version);
            if ver >= 5 { let _ = core.send(k.0, p, wl_pointer::EVT_AXIS_SOURCE, vec![Value::Uint(if v120 != 0 { 0 } else { 2 })]); }
            let _ = core.send(k.0, p, wl_pointer::EVT_AXIS, vec![Value::Uint(time), Value::Uint(axis), Value::fixed(value)]);
            if v120 != 0 { let _ = core.send(k.0, p, wl_pointer::EVT_AXIS_VALUE120, vec![Value::Uint(axis), Value::Int(v120)]); }
            let _ = core.send(k.0, p, wl_pointer::EVT_FRAME, vec![]);
        }
    }

    fn send_modifiers(&mut self, core: &mut Core, k: SurfKey) {
        let m = self.input.tracker.state();
        let serial = core.next_serial();
        for kb in self.keyboards_of(k.0) {
            let _ = core.send(k.0, kb, wl_keyboard::EVT_MODIFIERS, vec![Value::Uint(serial), Value::Uint(m.depressed), Value::Uint(m.latched), Value::Uint(m.locked), Value::Uint(m.group)]);
        }
    }
    pub fn keyboard_focus(&mut self, core: &mut Core, k: Option<SurfKey>) {
        if self.input.kb_focus == k { return; }
        if let Some(old) = self.input.kb_focus.take() {
            let serial = core.next_serial();
            for kb in self.keyboards_of(old.0) { let _ = core.send(old.0, kb, wl_keyboard::EVT_LEAVE, vec![Value::Uint(serial), Value::Object(old.1)]); }
        }
        let Some(k) = k else { return };
        // the selections come right before the keyboard (wl_data_device.selection)
        self.send_selection(core, k.0);
        let serial = core.next_serial();
        let keys: Vec<u8> = self.input.tracker.pressed().iter().flat_map(|&c| (c as u32).to_ne_bytes()).collect();
        for kb in self.keyboards_of(k.0) {
            let _ = core.send(k.0, kb, wl_keyboard::EVT_ENTER, vec![Value::Uint(serial), Value::Object(k.1), Value::Array(keys.clone())]);
        }
        self.send_modifiers(core, k);
        self.input.kb_focus = Some(k);
    }
    /// Feeds a raw key (evdev code) to the tracker and the focused client.
    pub fn key(&mut self, core: &mut Core, time: u32, evdev: u16, pressed: bool) {
        let changed = self.input.tracker.key(evdev, pressed);
        let Some(k) = self.input.kb_focus else { return };
        let serial = core.next_serial();
        for kb in self.keyboards_of(k.0) {
            let _ = core.send(k.0, kb, wl_keyboard::EVT_KEY, vec![Value::Uint(serial), Value::Uint(time), Value::Uint(evdev as u32), Value::Uint(pressed as u32)]);
        }
        if changed { self.send_modifiers(core, k); }
    }
}
