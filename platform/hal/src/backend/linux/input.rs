//! Input shared by the two desktop protocols. Key indices come from makepad-key-code;
//! keysyms and the compose API follow os/linux/wayland/xkb_sys.rs.
use crate::{clipboard::ClipboardData, types::input};
use makepad_key_code::KeyCode;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::time::{Duration, Instant};

#[link(name = "xkbcommon")]
extern "C" {
    fn xkb_context_new(flags: u32) -> *mut c_void;
    fn xkb_context_unref(context: *mut c_void);
    fn xkb_keymap_new_from_string(
        context: *mut c_void,
        text: *const c_char,
        format: u32,
        flags: u32,
    ) -> *mut c_void;
    fn xkb_keymap_unref(keymap: *mut c_void);
    fn xkb_state_new(keymap: *mut c_void) -> *mut c_void;
    fn xkb_state_unref(state: *mut c_void);
    fn xkb_state_update_mask(
        state: *mut c_void,
        depressed: u32,
        latched: u32,
        locked: u32,
        dep_group: u32,
        lat_group: u32,
        group: u32,
    ) -> u32;
    fn xkb_state_key_get_one_sym(state: *mut c_void, key: u32) -> u32;
    fn xkb_state_mod_name_is_active(
        state: *mut c_void,
        name: *const c_char,
        component: u32,
    ) -> c_int;
    fn xkb_keymap_key_repeats(keymap: *mut c_void, key: u32) -> c_int;
    fn xkb_keysym_to_utf8(sym: u32, buffer: *mut c_char, size: usize) -> c_int;
    fn xkb_compose_table_new_from_locale(
        context: *mut c_void,
        locale: *const c_char,
        flags: u32,
    ) -> *mut c_void;
    fn xkb_compose_table_unref(table: *mut c_void);
    fn xkb_compose_state_new(table: *mut c_void, flags: u32) -> *mut c_void;
    fn xkb_compose_state_unref(state: *mut c_void);
    fn xkb_compose_state_feed(state: *mut c_void, sym: u32) -> u32;
    fn xkb_compose_state_get_status(state: *mut c_void) -> u32;
    fn xkb_compose_state_get_utf8(state: *mut c_void, buffer: *mut c_char, size: usize) -> c_int;
    fn xkb_compose_state_reset(state: *mut c_void);
}

pub(super) struct Keyboard {
    context: *mut c_void,
    keymap: *mut c_void,
    state: *mut c_void,
    compose: *mut c_void,
    pub mods: u32,
    pub repeat: Option<(u32, Instant)>,
    pub rate: i32,
    pub delay: i32,
}
impl Keyboard {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let context = xkb_context_new(0);
            if context.is_null() {
                return Err("xkb_context_new failed".into());
            }
            let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
                .into_iter()
                .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
                .unwrap_or_else(|| "C".into());
            let locale = CString::new(locale).unwrap_or_else(|_| c"C".into());
            let table = xkb_compose_table_new_from_locale(context, locale.as_ptr(), 0);
            let compose = if table.is_null() {
                ptr::null_mut()
            } else {
                let state = xkb_compose_state_new(table, 0);
                xkb_compose_table_unref(table);
                state
            };
            Ok(Self {
                context,
                keymap: ptr::null_mut(),
                state: ptr::null_mut(),
                compose,
                mods: 0,
                repeat: None,
                rate: 25,
                delay: 600,
            })
        }
    }
    pub fn keymap(&mut self, bytes: &[u8]) -> Result<(), String> {
        let text =
            CString::new(bytes.strip_suffix(&[0]).unwrap_or(bytes)).map_err(|e| e.to_string())?;
        unsafe {
            let map = xkb_keymap_new_from_string(self.context, text.as_ptr(), 1, 0);
            if map.is_null() {
                return Err("invalid compositor XKB keymap".into());
            }
            let state = xkb_state_new(map);
            if state.is_null() {
                xkb_keymap_unref(map);
                return Err("xkb_state_new failed".into());
            }
            if !self.state.is_null() {
                xkb_state_unref(self.state);
            }
            if !self.keymap.is_null() {
                xkb_keymap_unref(self.keymap);
            }
            self.state = state;
            self.keymap = map;
        }
        self.reset();
        Ok(())
    }
    pub fn mask(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        if self.state.is_null() {
            return;
        }
        unsafe {
            xkb_state_update_mask(self.state, depressed, latched, locked, 0, 0, group);
        }
        self.mods = [
            (c"Shift", input::MOD_SHIFT),
            (c"Control", input::MOD_CONTROL),
            (c"Mod1", input::MOD_ALT),
            (c"Mod4", input::MOD_LOGO),
        ]
        .into_iter()
        .fold(0, |bits, (name, bit)| {
            bits | if unsafe { xkb_state_mod_name_is_active(self.state, name.as_ptr(), 1 << 3) } > 0
            {
                bit
            } else {
                0
            }
        });
    }
    pub fn sym(&self, key: u32) -> u32 {
        if self.state.is_null() {
            0
        } else {
            unsafe { xkb_state_key_get_one_sym(self.state, key) }
        }
    }
    pub fn pressed(&mut self, key: u32) {
        if self.rate > 0
            && !self.keymap.is_null()
            && unsafe { xkb_keymap_key_repeats(self.keymap, key) } != 0
        {
            self.repeat = Some((
                key,
                Instant::now() + Duration::from_millis(self.delay.max(0) as u64),
            ));
        }
    }
    pub fn released(&mut self, key: u32) {
        if self.repeat.is_some_and(|(k, _)| k == key) {
            self.repeat = None;
        }
    }
    pub fn tick(&mut self) -> Option<u32> {
        let (key, due) = self.repeat?;
        if self.rate <= 0 {
            self.repeat = None;
            return None;
        }
        if Instant::now() < due {
            return None;
        }
        // Missed beats coalesce, like pointer motion; never burst stale repeat input after resize.
        self.repeat = Some((
            key,
            Instant::now() + Duration::from_secs_f64(1.0 / self.rate as f64),
        ));
        Some(key)
    }
    pub fn reset(&mut self) {
        self.repeat = None;
        if !self.compose.is_null() {
            unsafe {
                xkb_compose_state_reset(self.compose);
            }
        }
    }
    pub fn text(&mut self, sym: u32) -> String {
        unsafe {
            if !self.compose.is_null() {
                xkb_compose_state_feed(self.compose, sym);
                match xkb_compose_state_get_status(self.compose) {
                    1 => return String::new(), // composing
                    2 => {
                        let n = xkb_compose_state_get_utf8(self.compose, ptr::null_mut(), 0).max(0)
                            as usize;
                        let mut bytes = vec![0; n + 1];
                        xkb_compose_state_get_utf8(
                            self.compose,
                            bytes.as_mut_ptr().cast(),
                            bytes.len(),
                        );
                        xkb_compose_state_reset(self.compose);
                        bytes.truncate(n);
                        return String::from_utf8_lossy(&bytes).into_owned();
                    }
                    3 => {
                        xkb_compose_state_reset(self.compose);
                        return String::new();
                    }
                    _ => {}
                }
            }
            let mut bytes = [0u8; 8]; // one keysym's UTF-8 scalar plus its terminator
            let n = xkb_keysym_to_utf8(sym, bytes.as_mut_ptr().cast(), bytes.len());
            if n > 0 {
                String::from_utf8_lossy(&bytes[..n as usize - 1]).into_owned()
            } else {
                String::new()
            }
        }
    }
}
impl Drop for Keyboard {
    fn drop(&mut self) {
        unsafe {
            if !self.compose.is_null() {
                xkb_compose_state_unref(self.compose);
            }
            if !self.state.is_null() {
                xkb_state_unref(self.state);
            }
            if !self.keymap.is_null() {
                xkb_keymap_unref(self.keymap);
            }
            xkb_context_unref(self.context);
        }
    }
}

pub(super) fn cstring(text: &str) -> CString {
    CString::new(text.replace('\0', "")).unwrap()
}
pub(super) fn shortcut(modifiers: u32) -> bool {
    modifiers & (input::MOD_CONTROL | input::MOD_LOGO) != 0
}
pub(super) fn printable(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}
pub(super) fn button(code: u32) -> u32 {
    match code {
        0x110 | 1 => 1,
        0x111 | 3 => 2,
        0x112 | 2 => 4,
        0x113 | 8 => 8,
        0x114 | 9 => 16,
        _ => 0,
    }
}

pub(super) fn cursor_name(cursor: u32) -> &'static CStr {
    match cursor {
        2 => c"crosshair",
        3 => c"pointer",
        5 => c"move",
        6 => c"text",
        7 => c"wait",
        8 => c"help",
        9 => c"not-allowed",
        10 => c"grab",
        11 => c"grabbing",
        12 => c"n-resize",
        13 => c"ne-resize",
        14 => c"e-resize",
        15 => c"se-resize",
        16 => c"s-resize",
        17 => c"sw-resize",
        18 => c"w-resize",
        19 => c"nw-resize",
        20 => c"ns-resize",
        21 => c"nesw-resize",
        22 => c"ew-resize",
        23 => c"nwse-resize",
        24 => c"col-resize",
        25 => c"row-resize",
        _ => c"default",
    }
}

pub(super) const TEXT_MIMES: [&str; 4] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
];
pub(super) const URI_MIME: &str = "text/uri-list";
pub(super) const PNG_MIME: &str = "image/png";

pub(super) fn decode_clipboard(content: &mut ClipboardData, mime: &str, bytes: Vec<u8>) {
    if mime == PNG_MIME {
        content.png = bytes;
    } else if mime == URI_MIME {
        for uri in String::from_utf8_lossy(&bytes).lines().map(str::trim) {
            if let Some(path) = file_url_path(uri) {
                content.files.push((uri.to_string(), path));
            }
        }
    } else if TEXT_MIMES.contains(&mime) {
        content.text = String::from_utf8_lossy(&bytes)
            .trim_end_matches('\0')
            .to_string();
    }
}

fn file_url_path(uri: &str) -> Option<String> {
    let path = uri
        .strip_prefix("file://localhost/")
        .map(|p| format!("/{p}"))
        .or_else(|| uri.strip_prefix("file:///").map(|p| format!("/{p}")))?;
    let bytes = path.as_bytes();
    let mut decoded = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(a), Some(b)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                decoded.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

pub(super) fn decode_items(mime: &str, bytes: &[u8]) -> Vec<(u32, String)> {
    if mime == URI_MIME {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter_map(|line| {
                let uri = line.trim();
                if uri.is_empty() || uri.starts_with('#') {
                    return None;
                }
                Some(match file_url_path(uri) {
                    Some(path) => (input::drag_item::FILE, path),
                    None => (input::drag_item::STRING, uri.to_string()),
                })
            })
            .collect()
    } else {
        vec![(
            input::drag_item::STRING,
            String::from_utf8_lossy(bytes)
                .trim_end_matches('\0')
                .to_string(),
        )]
    }
}
pub(super) fn encode_uris(items: &[(u32, String)]) -> Vec<u8> {
    let mut out = String::new();
    for (_, path) in items
        .iter()
        .filter(|(kind, _)| *kind == input::drag_item::FILE)
    {
        out.push_str("file://");
        for b in path.bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                out.push(b as char);
            } else {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
        out.push_str("\r\n");
    }
    out.into_bytes()
}
pub(super) fn packed(items: &[(u32, String)]) -> Vec<u8> {
    crate::buffers::pack_drag_items(
        &items
            .iter()
            .map(|(k, s)| (*k, s.as_str()))
            .collect::<Vec<_>>(),
    )
}

// Key table ported from xkb_sys.rs:473-586, shared by X11 and Wayland.
pub(super) fn keycode(keysym: u32) -> KeyCode {
    match keysym {
        0x0061 | 0x0041 => KeyCode::KeyA,
        0x0062 | 0x0042 => KeyCode::KeyB,
        0x0063 | 0x0043 => KeyCode::KeyC,
        0x0064 | 0x0044 => KeyCode::KeyD,
        0x0065 | 0x0045 => KeyCode::KeyE,
        0x0066 | 0x0046 => KeyCode::KeyF,
        0x0067 | 0x0047 => KeyCode::KeyG,
        0x0068 | 0x0048 => KeyCode::KeyH,
        0x0069 | 0x0049 => KeyCode::KeyI,
        0x006a | 0x004a => KeyCode::KeyJ,
        0x006b | 0x004b => KeyCode::KeyK,
        0x006c | 0x004c => KeyCode::KeyL,
        0x006d | 0x004d => KeyCode::KeyM,
        0x006e | 0x004e => KeyCode::KeyN,
        0x006f | 0x004f => KeyCode::KeyO,
        0x0070 | 0x0050 => KeyCode::KeyP,
        0x0071 | 0x0051 => KeyCode::KeyQ,
        0x0072 | 0x0052 => KeyCode::KeyR,
        0x0073 | 0x0053 => KeyCode::KeyS,
        0x0074 | 0x0054 => KeyCode::KeyT,
        0x0075 | 0x0055 => KeyCode::KeyU,
        0x0076 | 0x0056 => KeyCode::KeyV,
        0x0077 | 0x0057 => KeyCode::KeyW,
        0x0078 | 0x0058 => KeyCode::KeyX,
        0x0079 | 0x0059 => KeyCode::KeyY,
        0x007a | 0x005a => KeyCode::KeyZ,

        0x0030 => KeyCode::Key0,
        0x0031 => KeyCode::Key1,
        0x0032 => KeyCode::Key2,
        0x0033 => KeyCode::Key3,
        0x0034 => KeyCode::Key4,
        0x0035 => KeyCode::Key5,
        0x0036 => KeyCode::Key6,
        0x0037 => KeyCode::Key7,
        0x0038 => KeyCode::Key8,
        0x0039 => KeyCode::Key9,

        0xffe9 | 0xffea => KeyCode::Alt,
        0xffe7 | 0xffe8 | 0xffeb | 0xffec => KeyCode::Logo,
        0xffe1 | 0xffe2 => KeyCode::Shift,
        0xffe3 | 0xffe4 => KeyCode::Control,

        0x003d => KeyCode::Equals,
        0x002d => KeyCode::Minus,
        0x005d => KeyCode::RBracket,
        0x005b => KeyCode::LBracket,
        0xff0d => KeyCode::ReturnKey,
        0x0060 => KeyCode::Backtick,
        0x003b => KeyCode::Semicolon,
        0x005c => KeyCode::Backslash,
        0x002c => KeyCode::Comma,
        0x002f => KeyCode::Slash,
        0x002e => KeyCode::Period,
        0xff09 | 0xfe20 => KeyCode::Tab,
        0x0020 => KeyCode::Space,
        0xff08 => KeyCode::Backspace,
        0xff1b => KeyCode::Escape,
        0xffe5 => KeyCode::Capslock,
        0xffae => KeyCode::NumpadDecimal,
        0xffaa => KeyCode::NumpadMultiply,
        0xffab => KeyCode::NumpadAdd,
        0xff7f => KeyCode::Numlock,
        0xffaf => KeyCode::NumpadDivide,
        0xff8d => KeyCode::NumpadEnter,
        0xffad => KeyCode::NumpadSubtract,
        0xffbd => KeyCode::NumpadEquals,
        0xffb0 => KeyCode::Numpad0,
        0xffb1 => KeyCode::Numpad1,
        0xffb2 => KeyCode::Numpad2,
        0xffb3 => KeyCode::Numpad3,
        0xffb4 => KeyCode::Numpad4,
        0xffb5 => KeyCode::Numpad5,
        0xffb6 => KeyCode::Numpad6,
        0xffb7 => KeyCode::Numpad7,
        0xffb8 => KeyCode::Numpad8,
        0xffb9 => KeyCode::Numpad9,

        0xffbe => KeyCode::F1,
        0xffbf => KeyCode::F2,
        0xffc0 => KeyCode::F3,
        0xffc1 => KeyCode::F4,
        0xffc2 => KeyCode::F5,
        0xffc3 => KeyCode::F6,
        0xffc4 => KeyCode::F7,
        0xffc5 => KeyCode::F8,
        0xffc6 => KeyCode::F9,
        0xffc7 => KeyCode::F10,
        0xffc8 => KeyCode::F11,
        0xffc9 => KeyCode::F12,

        0xff61 => KeyCode::PrintScreen,
        0xff14 => KeyCode::ScrollLock,
        0xff13 => KeyCode::Pause,
        0xff50 | 0xff95 => KeyCode::Home,
        0xff55 | 0xff9a => KeyCode::PageUp,
        0xffff | 0xff9f => KeyCode::Delete,
        0xff57 | 0xff9c => KeyCode::End,
        0xff56 | 0xff9b => KeyCode::PageDown,
        0xff51 | 0xff96 => KeyCode::ArrowLeft,
        0xff53 | 0xff98 => KeyCode::ArrowRight,
        0xff54 | 0xff99 => KeyCode::ArrowDown,
        0xff52 | 0xff97 => KeyCode::ArrowUp,
        0xff63 | 0xff9e => KeyCode::Insert,
        0x0027 => KeyCode::Quote,

        _ => KeyCode::Unknown,
    }
}
