use super::*;
use crate::buffers::Wire;
use std::os::unix::fs::FileExt;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Axis {
    raw: [f64; 2],
    detents: [f64; 2],
    source: Option<wl_pointer::AxisSource>,
    stop: bool,
}
#[derive(Clone, Copy)]
struct Press {
    key: Key,
    serial: u32,
    button: u32,
    pos: [f64; 2],
}
#[derive(Default)]
struct Ime {
    key: Option<Key>,
    entered: Option<Key>,
    enabled: bool,
    config: u32,
    area: [f32; 4],
    selection: [u32; 2],
    text: String,
    preedit: Option<String>,
    commit: Option<String>,
    delete: Option<(u32, u32)>,
    last: String,
}
pub(super) struct Input {
    pub keyboard: common::Keyboard,
    pub focus: Option<Key>,
    pub pointer: Option<Key>,
    pub serial: u32,
    pub drag_start: Option<(Key, u32)>,
    pub cursors: HashMap<Key, u32>,
    pos: [f64; 2],
    motion: Option<(Key, [f64; 2])>,
    enter_serial: u32,
    displayed_cursor: Option<(u32, u32, u32)>,
    axis: Axis,
    finger_scroll: bool,
    pinch: f64,
    shadow_edge: Option<xdg_toplevel::ResizeEdge>,
    pending: Option<Press>,
    caption: Option<Press>,
    deferred: Vec<(u32, u32, bool)>,
    last_caption: Option<(Key, [f64; 2], Instant)>,
    ime: Ime,
    touches: HashMap<i32, (Key, crate::TouchPoint)>,
}
impl Input {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            keyboard: common::Keyboard::new()?,
            focus: None,
            pointer: None,
            serial: 0,
            drag_start: None,
            cursors: HashMap::new(),
            pos: [0.0; 2],
            motion: None,
            enter_serial: 0,
            displayed_cursor: None,
            axis: Axis::default(),
            finger_scroll: false,
            pinch: 1.0,
            shadow_edge: None,
            pending: None,
            caption: None,
            deferred: Vec::new(),
            last_caption: None,
            ime: Ime::default(),
            touches: HashMap::new(),
        })
    }
}

impl State {
    pub(super) fn pointer_lock(&mut self, key: Key, mode: u32) {
        let Some(w) = self.windows.get_mut(&key) else {
            return;
        };
        // A persistent Wayland lock remains pinned by the compositor between frames.
        if mode == pointer_lock::REPIN || (mode == pointer_lock::PIN && w.locked.is_some()) {
            return;
        }
        if mode == pointer_lock::UNPIN && !w.pin {
            return;
        }
        if mode == pointer_lock::UNLOCK && w.locked.is_none() {
            return;
        }
        if let Some(old) = w.locked.take() {
            if w.pin {
                old.set_cursor_position_hint(w.pin_restore[0], w.pin_restore[1]);
                w.surface.commit();
            }
            old.destroy();
        }
        w.locked_active = false;
        if w.pin {
            self.input.pos = w.pin_restore;
        }
        w.pin = false;
        if matches!(mode, pointer_lock::UNLOCK | pointer_lock::UNPIN) {
            self.cursor(self.input.cursors.get(&key).copied().unwrap_or(1));
            return;
        }
        if let (Some(manager), Some(pointer), Some(_)) = (
            self.constraints.as_ref(),
            self.pointer.as_ref(),
            self.relative.as_ref(),
        ) {
            w.pin = mode == pointer_lock::PIN;
            w.pin_restore = self.input.pos;
            w.locked = Some(manager.lock_pointer(&w.surface, pointer, None, &self.qh, key));
            w.surface.commit();
        } else {
            eprintln!("[hal-wayland] compositor has no pointer lock/relative-motion protocols");
        }
    }
    pub(super) fn key(&mut self, code: u32, down: bool, repeat: bool) {
        let Some(key) = self.input.focus else {
            return;
        };
        let Some(w) = self.windows.get(&key) else {
            return;
        };
        let recipient = w.recipient;
        let symbol = self.input.keyboard.sym(code);
        let keycode = common::keycode(symbol);
        let mods = self.input.keyboard.mods;
        self.shared.push(recipient, |q, id| {
            if down {
                q.key_down(id, keycode.to_index(), repeat, mods)
            } else {
                q.key_up(id, keycode.to_index(), mods)
            }
        });
        if down && !repeat {
            self.input.keyboard.pressed(code);
        } else if !down {
            self.input.keyboard.released(code);
        }
        if !down {
            return;
        }
        if keycode == makepad_key_code::KeyCode::Escape && w.popup.is_some() {
            self.shared.push(recipient, |q, id| q.popup_dismissed(id));
            return;
        }
        if common::shortcut(mods) {
            if keycode == makepad_key_code::KeyCode::KeyV {
                self.read_clipboard(selection::Purpose::Paste(recipient), false);
            }
            return;
        }
        if self.text.is_some()
            && self.input.ime.key == Some(key)
            && self.input.ime.entered == Some(key)
        {
            return;
        }
        let text = self.input.keyboard.text(symbol);
        if common::printable(&text) {
            self.shared.push(recipient, |q, id| {
                q.text(id, raw::text::TYPED, text.as_bytes())
            });
        }
    }
    pub(super) fn flush_motion(&mut self) {
        let Some((key, pos)) = self.input.motion.take() else {
            return;
        };
        self.input.pos = pos;
        self.cursor(self.input.cursors.get(&key).copied().unwrap_or(1));
        if let Some(press) = self.input.caption {
            if ((pos[0] - press.pos[0]).powi(2) + (pos[1] - press.pos[1]).powi(2)).sqrt() > 5.0 {
                if let (Some(w), Some(seat)) = (self.windows.get(&press.key), self.seat.as_ref()) {
                    if let Some(top) = w.top.as_ref() {
                        top._move(seat, press.serial);
                    }
                }
                self.input.caption = None;
            }
            return;
        }
        if self.input.pending.is_some() {
            return;
        }
        if let Some(w) = self.windows.get(&key).filter(|w| !w.locked_active) {
            self.shared.push(w.recipient, |q, id| {
                q.mouse_move(id, pos[0], pos[1], 0.0, 0.0, self.input.keyboard.mods)
            });
        }
    }
    fn button(&mut self, serial: u32, button: u32, down: bool) {
        self.flush_motion();
        self.input.serial = serial;
        if self.input.pending.is_some() {
            self.input.deferred.push((serial, button, down));
            return;
        }
        let Some(key) = self.input.pointer else {
            return;
        };
        if !down && self.windows.get(&key).is_some_and(|w| w.pin) {
            self.pointer_lock(key, pointer_lock::UNPIN);
        }
        if button == 0x110 {
            self.input.drag_start = down.then_some((key, serial));
        }
        let Some(w) = self.windows.get(&key) else {
            return;
        };
        if down {
            let popups: Vec<_> = self
                .windows
                .values()
                .filter(|w| !w.closed && w.popup.is_some() && w.owner != key)
                .map(|w| w.recipient)
                .collect();
            for popup in popups {
                self.shared.push(popup, |q, id| q.popup_dismissed(id));
            }
            if button == 0x110 && w.resizable {
                let edge = self
                    .input
                    .shadow_edge
                    .or_else(|| resize_edge(w, self.input.pos));
                if let (Some(edge), Some(top), Some(seat)) =
                    (edge, w.top.as_ref(), self.seat.as_ref())
                {
                    top.resize(seat, serial, edge);
                    return;
                }
            }
            if w.csd && w.top.is_some() && matches!(button, 0x110 | 0x111) {
                self.input.pending = Some(Press {
                    key,
                    serial,
                    button,
                    pos: self.input.pos,
                });
                self.shared.push(w.recipient, |q, id| {
                    q.drag_query(id, self.input.pos[0], self.input.pos[1])
                });
                return;
            }
        } else if self.input.caption.take().is_some() {
            return;
        }
        let mask = common::button(button);
        if mask == 0 {
            return;
        }
        self.shared.push(w.recipient, |q, id| {
            if down {
                q.mouse_down(
                    id,
                    self.input.pos[0],
                    self.input.pos[1],
                    mask,
                    self.input.keyboard.mods,
                )
            } else {
                q.mouse_up(
                    id,
                    self.input.pos[0],
                    self.input.pos[1],
                    mask,
                    self.input.keyboard.mods,
                )
            }
        });
        if down && button == 0x112 {
            self.read_clipboard(selection::Purpose::Paste(w.recipient), true);
        }
        self.apply_selection();
    }
    pub(super) fn caption_answer(&mut self, key: Key, answer: u32) {
        let Some(press) = self.input.pending.take() else {
            return;
        };
        if key != press.key {
            self.input.pending = Some(press);
            return;
        }
        let deferred = std::mem::take(&mut self.input.deferred);
        if let (Some(w), Some(seat)) = (self.windows.get(&key), self.seat.as_ref()) {
            if let Some(top) = w.top.as_ref() {
                if answer == raw::drag_query::SYS_MENU
                    || (answer == raw::drag_query::CAPTION && press.button == 0x111)
                {
                    top.show_window_menu(
                        seat,
                        press.serial,
                        press.pos[0] as i32,
                        press.pos[1] as i32,
                    );
                } else if answer == raw::drag_query::CAPTION {
                    let double = self.input.last_caption.is_some_and(|(k, p, t)| {
                        k == key
                            && t.elapsed() < Duration::from_millis(350)
                            && (p[0] - press.pos[0]).abs() < 5.0
                            && (p[1] - press.pos[1]).abs() < 5.0
                    });
                    if double {
                        if w.state == window_state::MAXIMIZE {
                            top.unset_maximized();
                        } else {
                            top.set_maximized();
                        }
                        self.input.last_caption = None;
                    } else {
                        self.input.caption = Some(press);
                        self.input.last_caption = Some((key, press.pos, Instant::now()));
                    }
                } else {
                    self.shared.push(w.recipient, |q, id| {
                        q.mouse_down(
                            id,
                            press.pos[0],
                            press.pos[1],
                            common::button(press.button),
                            self.input.keyboard.mods,
                        )
                    });
                }
            }
        }
        // A release may arrive while the app is answering the query; replay it once, in order.
        for (serial, button, down) in deferred {
            self.button(serial, button, down);
        }
        if self.input.pos != press.pos {
            self.input.motion = Some((key, self.input.pos));
            self.flush_motion();
        }
    }
    fn scroll(&mut self) {
        self.flush_motion();
        let axis = std::mem::take(&mut self.input.axis);
        let Some(key) = self.input.pointer else {
            return;
        };
        let Some(w) = self.windows.get(&key) else {
            return;
        };
        let detents = axis.detents.iter().any(|v| *v != 0.0);
        let wheel = match axis.source {
            Some(wl_pointer::AxisSource::Wheel | wl_pointer::AxisSource::WheelTilt) => true,
            Some(wl_pointer::AxisSource::Finger | wl_pointer::AxisSource::Continuous) => false,
            _ => detents,
        };
        let delta = std::array::from_fn::<_, 2, _>(|i| {
            if wheel && axis.detents[i] != 0.0 {
                axis.detents[i] * 60.0
            } else {
                axis.raw[i]
            }
        });
        let changed = delta.iter().any(|v| *v != 0.0);
        let finger = axis.source == Some(wl_pointer::AxisSource::Finger)
            || (axis.source.is_none() && self.input.finger_scroll && !detents);
        let phase = if finger {
            if changed {
                self.input.finger_scroll = true;
                raw::scroll::CHANGED
            } else if axis.stop {
                self.input.finger_scroll = false;
                raw::scroll::ENDED
            } else {
                raw::scroll::TOUCHED
            }
        } else {
            raw::scroll::NONE
        };
        if changed || (finger && axis.stop) {
            self.shared.push(w.recipient, |q, id| {
                q.scroll(
                    id,
                    self.input.pos[0],
                    self.input.pos[1],
                    delta[0],
                    delta[1],
                    self.input.keyboard.mods,
                    !wheel,
                    phase,
                )
            });
        }
    }
    pub(super) fn cursor(&mut self, requested: u32) {
        let requested = if self
            .input
            .pointer
            .and_then(|k| self.windows.get(&k))
            .is_some_and(|w| w.locked_active)
        {
            0
        } else {
            requested
        };
        let edge = self.input.shadow_edge.or_else(|| {
            self.input
                .pointer
                .and_then(|k| self.windows.get(&k))
                .and_then(|w| resize_edge(w, self.input.pos))
        });
        let requested = if requested == 0 {
            0
        } else {
            edge.map_or(requested, |edge| match edge {
                xdg_toplevel::ResizeEdge::Top => 12,
                xdg_toplevel::ResizeEdge::TopRight => 13,
                xdg_toplevel::ResizeEdge::Right => 14,
                xdg_toplevel::ResizeEdge::BottomRight => 15,
                xdg_toplevel::ResizeEdge::Bottom => 16,
                xdg_toplevel::ResizeEdge::BottomLeft => 17,
                xdg_toplevel::ResizeEdge::Left => 18,
                xdg_toplevel::ResizeEdge::TopLeft => 19,
                _ => requested,
            })
        };
        let Some(pointer) = self.pointer.as_ref() else {
            return;
        };
        let dpi = self
            .input
            .pointer
            .and_then(|k| self.windows.get(&k))
            .map_or(1.0, |w| w.dpi);
        let state = (self.input.enter_serial, requested, dpi.to_bits());
        if self.input.displayed_cursor == Some(state) {
            return;
        }
        self.input.displayed_cursor = Some(state);
        if requested == 0 {
            pointer.set_cursor(self.input.enter_serial, None, 0, 0);
            return;
        }
        if let Some(shape) = self.cursor_shape.as_ref() {
            use wp_cursor_shape_device_v1::Shape;
            let cursor = match requested {
                2 => Shape::Crosshair,
                3 => Shape::Pointer,
                5 => Shape::Move,
                6 => Shape::Text,
                7 => Shape::Wait,
                8 => Shape::Help,
                9 => Shape::NotAllowed,
                10 => Shape::Grab,
                11 => Shape::Grabbing,
                12 => Shape::NResize,
                13 => Shape::NeResize,
                14 => Shape::EResize,
                15 => Shape::SeResize,
                16 => Shape::SResize,
                17 => Shape::SwResize,
                18 => Shape::WResize,
                19 => Shape::NwResize,
                20 => Shape::NsResize,
                21 => Shape::NeswResize,
                22 => Shape::EwResize,
                23 => Shape::NwseResize,
                24 => Shape::ColResize,
                25 => Shape::RowResize,
                _ => Shape::Default,
            };
            shape.set_shape(self.input.enter_serial, cursor);
        } else if let (Some(compositor), Some(shm)) = (self.compositor.as_ref(), self.shm.as_ref())
        {
            if !self.cursor_theme_attempted {
                self.cursor_theme_attempted = true;
                self.cursor_theme = cursor::Theme::new(compositor, &self.qh);
                if self.cursor_theme.is_none() {
                    eprintln!("[hal-wayland] cursor-shape and libwayland-cursor unavailable");
                }
            }
            if let Some(theme) = self.cursor_theme.as_mut() {
                theme.show(
                    &self.conn,
                    shm,
                    pointer,
                    self.input.enter_serial,
                    requested,
                    dpi,
                );
            }
        }
    }
    pub(super) fn ime_show(&mut self, key: Key, area: [f32; 4], config: u32) {
        if self.input.ime.key != Some(key) {
            self.input.ime.enabled = false;
        }
        self.input.ime.key = Some(key);
        self.input.ime.config = config;
        self.input.ime.area = area;
        self.configure_ime();
    }
    fn configure_ime(&mut self) {
        if self.input.ime.key != self.input.ime.entered {
            return;
        }
        if let Some(text) = self.text.as_ref() {
            if self.input.ime.config & 8 != 0 {
                text.disable();
                text.commit();
                self.input.ime.enabled = false;
                return;
            }
            if !self.input.ime.enabled {
                text.enable();
                self.input.ime.enabled = true;
            }
            let area = self.input.ime.area;
            text.set_cursor_rectangle(
                area[0] as i32,
                (area[1] + area[3] + area[3] * 0.6) as i32,
                area[2].max(1.0) as i32,
                area[3].max(1.0) as i32,
            );
            let secure = self.input.ime.config & 2 != 0;
            text.set_content_type(
                if secure {
                    zwp_text_input_v3::ContentHint::HiddenText
                        | zwp_text_input_v3::ContentHint::SensitiveData
                } else {
                    zwp_text_input_v3::ContentHint::None
                },
                if secure {
                    zwp_text_input_v3::ContentPurpose::Password
                } else {
                    zwp_text_input_v3::ContentPurpose::Normal
                },
            );
            // text-input-v3's wire limit is 4000 bytes. Keep the complete selection and
            // as much neighbouring text as fits, without splitting a UTF-8 character.
            let surrounding = &self.input.ime.text;
            let mut cursor = (self.input.ime.selection[1] as usize).min(surrounding.len());
            let mut anchor = (self.input.ime.selection[0] as usize).min(surrounding.len());
            while !surrounding.is_char_boundary(cursor) {
                cursor -= 1;
            }
            while !surrounding.is_char_boundary(anchor) {
                anchor -= 1;
            }
            let low = cursor.min(anchor);
            let high = cursor.max(anchor);
            if high - low <= 4000 {
                let mut start = low.saturating_sub((4000 - (high - low)) / 2);
                let mut end = (start + 4000).min(surrounding.len());
                start = start.min(end.saturating_sub(4000));
                while !surrounding.is_char_boundary(start) {
                    start += 1;
                }
                while !surrounding.is_char_boundary(end) {
                    end -= 1;
                }
                text.set_surrounding_text(
                    surrounding[start..end].into(),
                    (cursor - start) as i32,
                    (anchor - start) as i32,
                );
            } else {
                // The document is retained in the app; only optional IME context is omitted.
                eprintln!("[hal-wayland] selection exceeds text-input-v3 context limit");
                text.set_surrounding_text(String::new(), 0, 0);
            }
            text.commit();
        }
    }
    pub(super) fn ime_hide(&mut self, key: Key) {
        if self.input.ime.key == Some(key) {
            if let Some(text) = self.text.as_ref() {
                text.disable();
                text.commit();
            }
            self.input.ime.key = None;
            self.input.ime.enabled = false;
            self.cancel_ime(key);
        }
    }
    fn cancel_ime(&mut self, key: Key) {
        if !self.input.ime.last.is_empty() {
            self.input.ime.last.clear();
            if let Some(w) = self.windows.get(&key) {
                self.shared
                    .push(w.recipient, |q, id| q.ime(id, raw::ime::CANCEL, 0, 0, &[]));
            }
        }
        self.input.ime.preedit = None;
        self.input.ime.commit = None;
        self.input.ime.delete = None;
    }
    pub(super) fn ime_sync(&mut self, key: Key, selection: [u32; 2], text: String) {
        if self.input.ime.key == Some(key) {
            self.input.ime.selection = selection;
            self.input.ime.text = text;
            self.configure_ime();
        }
    }
    fn touch_frame(&mut self) {
        let mut windows: HashMap<Key, Vec<crate::TouchPoint>> = HashMap::new();
        for (key, touch) in self.input.touches.values() {
            windows.entry(*key).or_default().push(*touch);
        }
        for (key, touches) in windows {
            if let Some(w) = self.windows.get(&key) {
                let mut bytes = Vec::new();
                for touch in &touches {
                    touch.write(&mut bytes);
                }
                self.shared.push(w.recipient, |q, id| {
                    q.touch(id, touches.len() as u32, &bytes)
                });
            }
        }
        self.input
            .touches
            .retain(|_, (_, t)| t.phase != raw::touch::STOP);
        for (_, t) in self.input.touches.values_mut() {
            t.phase = raw::touch::STABLE;
        }
    }
}

fn resize_edge(w: &Window, pos: [f64; 2]) -> Option<xdg_toplevel::ResizeEdge> {
    use xdg_toplevel::ResizeEdge;

    if !w.csd || w.state != window_state::RESTORE {
        return None;
    }
    let [x, y] = pos;
    let [width, height] = [w.geom[2], w.geom[3]];
    let left = x < 5.0 && w.edges & 1 == 0;
    let right = x >= width - 5.0 && w.edges & 4 == 0;
    let top = y < 5.0 && w.edges & 2 == 0;
    let bottom = y >= height - 5.0 && w.edges & 8 == 0;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (_, true, true, _) => Some(ResizeEdge::TopRight),
        (true, _, _, true) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (true, _, _, _) => Some(ResizeEdge::Left),
        (_, true, _, _) => Some(ResizeEdge::Right),
        (_, _, true, _) => Some(ResizeEdge::Top),
        (_, _, _, true) => Some(ResizeEdge::Bottom),
        _ => None,
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        s: &mut Self,
        p: &wl_seat::WlSeat,
        e: wl_seat::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = e
        {
            if caps.contains(wl_seat::Capability::Pointer) {
                if s.pointer.is_none() {
                    s.pointer = Some(p.get_pointer(q, ()));
                }
            } else {
                if let Some(r) = s.relative.take() {
                    for key in s.windows.keys().copied().collect::<Vec<_>>() {
                        s.pointer_lock(key, pointer_lock::UNLOCK);
                    }
                    r.destroy();
                }
                if let Some(c) = s.cursor_shape.take() {
                    c.destroy();
                }
                if let Some(g) = s.pinch.take() {
                    g.destroy();
                }
                if let Some(p) = s.pointer.take() {
                    if p.version() >= 3 {
                        p.release();
                    }
                }
                s.input.pointer = None;
            }
            if caps.contains(wl_seat::Capability::Keyboard) {
                if s.keyboard_proxy.is_none() {
                    s.keyboard_proxy = Some(p.get_keyboard(q, ()));
                }
            } else {
                if let Some(p) = s.keyboard_proxy.take() {
                    if p.version() >= 3 {
                        p.release();
                    }
                }
                s.input.focus = None;
                s.input.keyboard.reset();
            }
            if caps.contains(wl_seat::Capability::Touch) {
                if s.touch_proxy.is_none() {
                    s.touch_proxy = Some(p.get_touch(q, ()));
                }
            } else {
                if let Some(p) = s.touch_proxy.take() {
                    if p.version() >= 3 {
                        p.release();
                    }
                }
                for (_, touch) in s.input.touches.values_mut() {
                    touch.phase = raw::touch::STOP;
                }
                s.touch_frame();
            }
            s.devices();
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        s: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        e: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                // The compositor may hand every client the same sealed file (wl_keyboard v7: map it
                // privately, never consume it): read at offset 0, so the shared file offset stays
                // where it is and the next client still gets the whole keymap.
                let mut bytes = vec![0; size as usize];
                let result = std::fs::File::from(fd)
                    .read_exact_at(&mut bytes, 0)
                    .map_err(|e| e.to_string())
                    .and_then(|_| s.input.keyboard.keymap(&bytes));
                if let Err(e) = result {
                    eprintln!("[hal-wayland] keymap: {e}");
                }
            }
            wl_keyboard::Event::Enter {
                serial,
                surface,
                keys,
            } => {
                s.input.serial = serial;
                s.input.focus = s.surface_key(&surface);
                s.input.keyboard.reset();
                if let Some(w) = s.input.focus.and_then(|k| s.windows.get(&k)) {
                    s.shared.push(w.recipient, |q, id| q.focus(id, true));
                    for k in keys.chunks_exact(4) {
                        let symbol = s
                            .input
                            .keyboard
                            .sym(u32::from_ne_bytes(k.try_into().unwrap()) + 8);
                        s.shared.push(w.recipient, |q, id| {
                            q.key_down(
                                id,
                                common::keycode(symbol).to_index(),
                                false,
                                s.input.keyboard.mods,
                            )
                        });
                    }
                }
                s.apply_selection();
            }
            wl_keyboard::Event::Leave { serial, surface } => {
                s.input.serial = serial;
                if let Some(key) = s.surface_key(&surface) {
                    s.pointer_lock(key, pointer_lock::UNLOCK);
                    if let Some(w) = s.windows.get(&key) {
                        s.shared.push(w.recipient, |q, id| {
                            q.focus(id, false);
                            if w.popup.is_some() {
                                q.popup_dismissed(id);
                            }
                        });
                    }
                    s.cancel_ime(key);
                }
                s.input.focus = None;
                s.input.keyboard.reset();
            }
            wl_keyboard::Event::Key {
                serial, key, state, ..
            } => {
                s.input.serial = serial;
                s.key(
                    key + 8,
                    state == WEnum::Value(wl_keyboard::KeyState::Pressed),
                    false,
                );
                s.apply_selection();
            }
            wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                s.input.serial = serial;
                s.input
                    .keyboard
                    .mask(mods_depressed, mods_latched, mods_locked, group);
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                s.input.keyboard.rate = rate;
                s.input.keyboard.delay = delay;
                if rate <= 0 {
                    s.input.keyboard.repeat = None;
                }
            }
            _ => {}
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        s: &mut Self,
        p: &wl_pointer::WlPointer,
        e: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                s.flush_motion();
                s.input.serial = serial;
                s.input.enter_serial = serial;
                s.input.pointer = s.surface_key(&surface);
                s.input.pos = [surface_x, surface_y];
                s.input.shadow_edge = s
                    .input
                    .pointer
                    .and_then(|k| s.windows.get(&k))
                    .and_then(|w| w.shadow.as_ref())
                    .and_then(|shadow| shadow.edge(&surface));
                let cursor = s
                    .input
                    .pointer
                    .and_then(|k| s.input.cursors.get(&k))
                    .copied()
                    .unwrap_or(1);
                s.cursor(cursor);
                if s.input.shadow_edge.is_none() {
                    if let Some(k) = s.input.pointer {
                        s.input.motion = Some((k, s.input.pos));
                    }
                }
            }
            wl_pointer::Event::Leave { serial, .. } => {
                s.flush_motion();
                s.input.serial = serial;
                if let Some(w) = s.input.pointer.and_then(|k| s.windows.get(&k)) {
                    s.shared.push(w.recipient, |q, id| {
                        q.mouse_leave(id, s.input.pos[0], s.input.pos[1])
                    });
                }
                s.input.pointer = None;
                s.input.shadow_edge = None;
                s.input.displayed_cursor = None;
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if s.input.shadow_edge.is_none() {
                    if let Some(k) = s.input.pointer {
                        s.input.motion = Some((k, [surface_x, surface_y]));
                    }
                }
            }
            wl_pointer::Event::Button {
                serial,
                button,
                state,
                ..
            } => s.button(
                serial,
                button,
                state == WEnum::Value(wl_pointer::ButtonState::Pressed),
            ),
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                s.input.axis.raw[if axis == wl_pointer::Axis::HorizontalScroll {
                    0
                } else {
                    1
                }] += value;
                if p.version() < 5 {
                    s.scroll();
                }
            }
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(source),
            } => s.input.axis.source = Some(source),
            wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            } => {
                if p.version() < 8 {
                    s.input.axis.detents[if axis == wl_pointer::Axis::HorizontalScroll {
                        0
                    } else {
                        1
                    }] += discrete as f64;
                }
            }
            wl_pointer::Event::AxisValue120 {
                axis: WEnum::Value(axis),
                value120,
            } => {
                s.input.axis.detents[if axis == wl_pointer::Axis::HorizontalScroll {
                    0
                } else {
                    1
                }] += value120 as f64 / 120.0
            }
            wl_pointer::Event::AxisStop { .. } => s.input.axis.stop = true,
            wl_pointer::Event::Frame => {
                s.flush_motion();
                s.scroll();
            }
            _ => {}
        }
    }
}
impl Dispatch<zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1, ()> for State {
    fn event(
        s: &mut Self,
        _: &zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1,
        e: zwp_pointer_gesture_pinch_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        s.flush_motion();
        let (scale, phase) = match e {
            zwp_pointer_gesture_pinch_v1::Event::Begin { .. } => {
                s.input.pinch = 1.0;
                (1.0, raw::pinch::BEGIN)
            }
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, .. } => {
                let delta = scale / s.input.pinch;
                s.input.pinch = scale;
                (delta, raw::pinch::UPDATE)
            }
            zwp_pointer_gesture_pinch_v1::Event::End { .. } => (1.0, raw::pinch::END),
            _ => return,
        };
        if let Some(w) = s.input.pointer.and_then(|k| s.windows.get(&k)) {
            s.shared.push(w.recipient, |q, id| {
                q.pinch(id, s.input.pos[0], s.input.pos[1], scale, phase)
            });
        }
    }
}
impl Dispatch<wl_touch::WlTouch, ()> for State {
    fn event(
        s: &mut Self,
        _: &wl_touch::WlTouch,
        e: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_touch::Event::Down {
                serial,
                surface,
                id,
                x,
                y,
                ..
            } => {
                s.input.serial = serial;
                if let Some(key) = s.surface_key(&surface) {
                    s.input.touches.insert(
                        id,
                        (
                            key,
                            crate::TouchPoint {
                                id: id as u32,
                                phase: raw::touch::START,
                                x: x as f32,
                                y: y as f32,
                                force: 1.0,
                            },
                        ),
                    );
                }
            }
            wl_touch::Event::Motion { id, x, y, .. } => {
                if let Some((_, t)) = s.input.touches.get_mut(&id) {
                    t.phase = raw::touch::MOVE;
                    t.x = x as f32;
                    t.y = y as f32;
                }
            }
            wl_touch::Event::Up { serial, id, .. } => {
                s.input.serial = serial;
                if let Some((_, t)) = s.input.touches.get_mut(&id) {
                    t.phase = raw::touch::STOP;
                }
            }
            wl_touch::Event::Frame => s.touch_frame(),
            wl_touch::Event::Cancel => {
                for (_, t) in s.input.touches.values_mut() {
                    t.phase = raw::touch::STOP;
                }
                s.touch_frame();
            }
            _ => {}
        }
    }
}
impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for State {
    fn event(
        s: &mut Self,
        _: &zwp_text_input_v3::ZwpTextInputV3,
        e: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            zwp_text_input_v3::Event::Enter { surface } => {
                s.input.ime.entered = s.surface_key(&surface);
                s.input.ime.enabled = false;
                s.configure_ime();
            }
            zwp_text_input_v3::Event::Leave { surface } => {
                if let Some(k) = s.surface_key(&surface) {
                    s.cancel_ime(k);
                }
                s.input.ime.entered = None;
            }
            zwp_text_input_v3::Event::PreeditString { text, .. } => s.input.ime.preedit = text,
            zwp_text_input_v3::Event::CommitString { text } => s.input.ime.commit = text,
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => s.input.ime.delete = Some((before_length, after_length)),
            zwp_text_input_v3::Event::Done { .. } => {
                let Some(key) = s.input.ime.entered else {
                    return;
                };
                if s.input.ime.key != Some(key) {
                    return;
                }
                let Some(w) = s.windows.get(&key) else {
                    return;
                };
                let ime = &mut s.input.ime;
                if let Some((before, after)) = ime.delete.take() {
                    let cursor = ime.selection[1];
                    s.shared.push(w.recipient, |q, id| {
                        q.ime(
                            id,
                            raw::ime::COMMIT,
                            cursor.saturating_sub(before),
                            cursor.saturating_add(after),
                            &[],
                        )
                    });
                }
                if let Some(text) = ime.commit.take() {
                    if !text.is_empty() {
                        s.shared.push(w.recipient, |q, id| {
                            q.ime(
                                id,
                                raw::ime::COMMIT,
                                0,
                                if ime.last.is_empty() { 0 } else { 1 },
                                text.as_bytes(),
                            )
                        });
                        ime.last.clear();
                    }
                }
                let preedit = ime.preedit.take().unwrap_or_default();
                if preedit != ime.last {
                    s.shared.push(w.recipient, |q, id| {
                        q.ime(
                            id,
                            if preedit.is_empty() {
                                raw::ime::CANCEL
                            } else {
                                raw::ime::UPDATE
                            },
                            0,
                            0,
                            preedit.as_bytes(),
                        )
                    });
                    ime.last = preedit;
                }
            }
            _ => {}
        }
    }
}
impl Dispatch<RelativePointer, ()> for State {
    fn event(
        s: &mut Self,
        _: &RelativePointer,
        e: constraints::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let constraints::Event::RelativeMotion {
            dx_unaccel,
            dy_unaccel,
            ..
        } = e
        {
            if let Some(w) = s
                .input
                .pointer
                .and_then(|k| s.windows.get(&k))
                .filter(|w| w.locked_active)
            {
                if w.pin {
                    s.input.pos[0] += dx_unaccel;
                    s.input.pos[1] += dy_unaccel;
                }
                s.shared.push(w.recipient, |q, id| {
                    q.mouse_move(
                        id,
                        s.input.pos[0],
                        s.input.pos[1],
                        dx_unaccel,
                        dy_unaccel,
                        s.input.keyboard.mods,
                    )
                });
            }
        }
    }
}
impl Dispatch<LockedPointer, Key> for State {
    fn event(
        s: &mut Self,
        _: &LockedPointer,
        e: constraints::Event,
        key: &Key,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let constraints::Event::Active(active) = e {
            if let Some(w) = s.windows.get_mut(key) {
                w.locked_active = active;
            }
            if s.input.pointer == Some(*key) {
                s.cursor(if active {
                    0
                } else {
                    s.input.cursors.get(key).copied().unwrap_or(1)
                });
            }
        }
    }
}
