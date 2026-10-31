//! Makepad input as Chromium's (CEF's) input: event flags, mouse buttons,
//! Windows virtual key codes, the platform's native key codes, the
//! characters a key types. Shared by everything that drives a page with
//! CEF-level input (the browser host's Browser widget, Stage's tab
//! commands) without linking CEF itself; the numbers are CEF's ABI.

use makepad_platform::*;

pub const EVENTFLAG_NONE: u32 = 0;
pub const EVENTFLAG_CAPS_LOCK_ON: u32 = 1 << 0;
pub const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
pub const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
pub const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
pub const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
pub const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
pub const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
pub const EVENTFLAG_COMMAND_DOWN: u32 = 1 << 7;
pub const EVENTFLAG_NUM_LOCK_ON: u32 = 1 << 8;
pub const EVENTFLAG_IS_KEY_PAD: u32 = 1 << 9;
pub const EVENTFLAG_IS_REPEAT: u32 = 1 << 13;
pub const EVENTFLAG_PRECISION_SCROLLING_DELTA: u32 = 1 << 14;

pub const KEY_EVENT_RAWKEYDOWN: i32 = 0;
pub const KEY_EVENT_KEYDOWN: i32 = 1;
pub const KEY_EVENT_KEYUP: i32 = 2;
pub const KEY_EVENT_CHAR: i32 = 3;

pub const MOUSE_BUTTON_LEFT: i32 = 0;
pub const MOUSE_BUTTON_MIDDLE: i32 = 1;
pub const MOUSE_BUTTON_RIGHT: i32 = 2;

pub fn cef_modifiers(modifiers: KeyModifiers, pressed_buttons: MouseButton) -> u32 {
    let mut out = EVENTFLAG_NONE;
    if modifiers.shift {
        out |= EVENTFLAG_SHIFT_DOWN;
    }
    if modifiers.control {
        out |= EVENTFLAG_CONTROL_DOWN;
    }
    if modifiers.alt {
        out |= EVENTFLAG_ALT_DOWN;
    }
    if modifiers.logo {
        out |= EVENTFLAG_COMMAND_DOWN;
    }
    if pressed_buttons.is_primary() {
        out |= EVENTFLAG_LEFT_MOUSE_BUTTON;
    }
    if pressed_buttons.is_middle() {
        out |= EVENTFLAG_MIDDLE_MOUSE_BUTTON;
    }
    if pressed_buttons.is_secondary() {
        out |= EVENTFLAG_RIGHT_MOUSE_BUTTON;
    }
    out
}

/// Makepad ScrollEvent uses +y for down and +x for right; CEF uses the
/// opposite. Fractional pixels stay in `remainder` across events.
pub fn cef_scroll_delta(delta: Vec2d, remainder: &mut Vec2d) -> (i32, i32) {
    *remainder += -delta;
    let dx = remainder.x as i32;
    let dy = remainder.y as i32;
    remainder.x -= dx as f64;
    remainder.y -= dy as f64;
    (dx, dy)
}

pub fn cef_mouse_button(button: Option<MouseButton>) -> i32 {
    match button {
        Some(button) if button.is_secondary() => MOUSE_BUTTON_RIGHT,
        Some(button) if button.is_middle() => MOUSE_BUTTON_MIDDLE,
        _ => MOUSE_BUTTON_LEFT,
    }
}

pub fn windows_key_code(key_code: KeyCode) -> i32 {
    match key_code {
        KeyCode::Escape => 0x1B,
        KeyCode::Back => 0x08,
        KeyCode::Backtick => 0xC0,
        KeyCode::Key0 => 0x30,
        KeyCode::Key1 => 0x31,
        KeyCode::Key2 => 0x32,
        KeyCode::Key3 => 0x33,
        KeyCode::Key4 => 0x34,
        KeyCode::Key5 => 0x35,
        KeyCode::Key6 => 0x36,
        KeyCode::Key7 => 0x37,
        KeyCode::Key8 => 0x38,
        KeyCode::Key9 => 0x39,
        KeyCode::Minus => 0xBD,
        KeyCode::Equals => 0xBB,
        KeyCode::Backspace => 0x08,
        KeyCode::Tab => 0x09,
        KeyCode::KeyQ => 0x51,
        KeyCode::KeyW => 0x57,
        KeyCode::KeyE => 0x45,
        KeyCode::KeyR => 0x52,
        KeyCode::KeyT => 0x54,
        KeyCode::KeyY => 0x59,
        KeyCode::KeyU => 0x55,
        KeyCode::KeyI => 0x49,
        KeyCode::KeyO => 0x4F,
        KeyCode::KeyP => 0x50,
        KeyCode::LBracket => 0xDB,
        KeyCode::RBracket => 0xDD,
        KeyCode::ReturnKey => 0x0D,
        KeyCode::KeyA => 0x41,
        KeyCode::KeyS => 0x53,
        KeyCode::KeyD => 0x44,
        KeyCode::KeyF => 0x46,
        KeyCode::KeyG => 0x47,
        KeyCode::KeyH => 0x48,
        KeyCode::KeyJ => 0x4A,
        KeyCode::KeyK => 0x4B,
        KeyCode::KeyL => 0x4C,
        KeyCode::Semicolon => 0xBA,
        KeyCode::Quote => 0xDE,
        KeyCode::Backslash => 0xDC,
        KeyCode::KeyZ => 0x5A,
        KeyCode::KeyX => 0x58,
        KeyCode::KeyC => 0x43,
        KeyCode::KeyV => 0x56,
        KeyCode::KeyB => 0x42,
        KeyCode::KeyN => 0x4E,
        KeyCode::KeyM => 0x4D,
        KeyCode::Comma => 0xBC,
        KeyCode::Period => 0xBE,
        KeyCode::Slash => 0xBF,
        KeyCode::Control => 0x11,
        KeyCode::Alt => 0x12,
        KeyCode::Shift => 0x10,
        KeyCode::Logo => 0x5B,
        KeyCode::Space => 0x20,
        KeyCode::Capslock => 0x14,
        KeyCode::F1 => 0x70,
        KeyCode::F2 => 0x71,
        KeyCode::F3 => 0x72,
        KeyCode::F4 => 0x73,
        KeyCode::F5 => 0x74,
        KeyCode::F6 => 0x75,
        KeyCode::F7 => 0x76,
        KeyCode::F8 => 0x77,
        KeyCode::F9 => 0x78,
        KeyCode::F10 => 0x79,
        KeyCode::F11 => 0x7A,
        KeyCode::F12 => 0x7B,
        KeyCode::PrintScreen => 0x2C,
        KeyCode::ScrollLock => 0x91,
        KeyCode::Pause => 0x13,
        KeyCode::Insert => 0x2D,
        KeyCode::Delete => 0x2E,
        KeyCode::Home => 0x24,
        KeyCode::End => 0x23,
        KeyCode::PageUp => 0x21,
        KeyCode::PageDown => 0x22,
        KeyCode::Numpad0 => 0x60,
        KeyCode::Numpad1 => 0x61,
        KeyCode::Numpad2 => 0x62,
        KeyCode::Numpad3 => 0x63,
        KeyCode::Numpad4 => 0x64,
        KeyCode::Numpad5 => 0x65,
        KeyCode::Numpad6 => 0x66,
        KeyCode::Numpad7 => 0x67,
        KeyCode::Numpad8 => 0x68,
        KeyCode::Numpad9 => 0x69,
        KeyCode::NumpadEquals => 0x92,
        KeyCode::NumpadSubtract => 0x6D,
        KeyCode::NumpadAdd => 0x6B,
        KeyCode::NumpadDecimal => 0x6E,
        KeyCode::NumpadMultiply => 0x6A,
        KeyCode::NumpadDivide => 0x6F,
        KeyCode::Numlock => 0x90,
        KeyCode::NumpadEnter => 0x0D,
        KeyCode::ArrowUp => 0x26,
        KeyCode::ArrowDown => 0x28,
        KeyCode::ArrowLeft => 0x25,
        KeyCode::ArrowRight => 0x27,
        KeyCode::Unknown => 0,
    }
}

/// What Chromium's Mac keycode table answers with `DomCode::NONE`
/// (`kInvalidNativeKeycode` in `keycode_converter.cc`). The only value a
/// key without a Carbon code may carry: the Windows VK range overlaps the
/// Carbon range everywhere, and a VK passed off as a Carbon code is a
/// different key -- VK_L (0x4C) is kVK_ANSI_KeypadEnter, VK_NUMPAD0
/// (0x60) is F5, VK_F13 (0x7C) is the right arrow.
#[cfg(target_os = "macos")]
pub const NO_MAC_KEY: i32 = 0xFFFF;

/// Platform scan / virtual key Chromium wants in `native_key_code`.
/// On macOS that is the Carbon/NSEvent keyCode, never the Windows VK.
pub fn native_key_code(key_code: KeyCode) -> i32 {
    #[cfg(target_os = "macos")]
    {
        return match key_code {
            KeyCode::KeyA => 0x00,
            KeyCode::KeyS => 0x01,
            KeyCode::KeyD => 0x02,
            KeyCode::KeyF => 0x03,
            KeyCode::KeyH => 0x04,
            KeyCode::KeyG => 0x05,
            KeyCode::KeyZ => 0x06,
            KeyCode::KeyX => 0x07,
            KeyCode::KeyC => 0x08,
            KeyCode::KeyV => 0x09,
            KeyCode::KeyB => 0x0b,
            KeyCode::KeyQ => 0x0c,
            KeyCode::KeyW => 0x0d,
            KeyCode::KeyE => 0x0e,
            KeyCode::KeyR => 0x0f,
            KeyCode::KeyY => 0x10,
            KeyCode::KeyT => 0x11,
            KeyCode::Key1 => 0x12,
            KeyCode::Key2 => 0x13,
            KeyCode::Key3 => 0x14,
            KeyCode::Key4 => 0x15,
            KeyCode::Key6 => 0x16,
            KeyCode::Key5 => 0x17,
            KeyCode::Equals => 0x18,
            KeyCode::Key9 => 0x19,
            KeyCode::Key7 => 0x1a,
            KeyCode::Minus => 0x1b,
            KeyCode::Key8 => 0x1c,
            KeyCode::Key0 => 0x1d,
            KeyCode::RBracket => 0x1e,
            KeyCode::KeyO => 0x1f,
            KeyCode::KeyU => 0x20,
            KeyCode::LBracket => 0x21,
            KeyCode::KeyI => 0x22,
            KeyCode::KeyP => 0x23,
            KeyCode::ReturnKey => 0x24,
            KeyCode::KeyL => 0x25,
            KeyCode::KeyJ => 0x26,
            KeyCode::Quote => 0x27,
            KeyCode::KeyK => 0x28,
            KeyCode::Semicolon => 0x29,
            KeyCode::Backslash => 0x2a,
            KeyCode::Comma => 0x2b,
            KeyCode::Slash => 0x2c,
            KeyCode::KeyN => 0x2d,
            KeyCode::KeyM => 0x2e,
            KeyCode::Period => 0x2f,
            KeyCode::Tab => 0x30,
            KeyCode::Space => 0x31,
            KeyCode::Backtick => 0x32,
            KeyCode::Backspace | KeyCode::Back => 0x33,
            KeyCode::Escape => 0x35,
            KeyCode::Capslock => 0x39,
            KeyCode::Shift => 0x38,
            KeyCode::Alt => 0x3a,
            KeyCode::Control => 0x3b,
            KeyCode::Logo => 0x37,
            KeyCode::F5 => 0x60,
            KeyCode::F6 => 0x61,
            KeyCode::F7 => 0x62,
            KeyCode::F3 => 0x63,
            KeyCode::F8 => 0x64,
            KeyCode::F9 => 0x65,
            KeyCode::F11 => 0x67,
            KeyCode::F10 => 0x6d,
            KeyCode::F12 => 0x6f,
            KeyCode::Insert => 0x72,
            KeyCode::Home => 0x73,
            KeyCode::PageUp => 0x74,
            KeyCode::Delete => 0x75,
            KeyCode::F4 => 0x76,
            KeyCode::End => 0x77,
            KeyCode::F2 => 0x78,
            KeyCode::PageDown => 0x79,
            KeyCode::F1 => 0x7a,
            KeyCode::ArrowLeft => 0x7b,
            KeyCode::ArrowRight => 0x7c,
            KeyCode::ArrowDown => 0x7d,
            KeyCode::ArrowUp => 0x7e,
            KeyCode::NumpadDecimal => 0x41,
            KeyCode::NumpadMultiply => 0x43,
            KeyCode::NumpadAdd => 0x45,
            KeyCode::Numlock => 0x47,
            KeyCode::NumpadDivide => 0x4b,
            KeyCode::NumpadEnter => 0x4c,
            KeyCode::NumpadSubtract => 0x4e,
            KeyCode::NumpadEquals => 0x51,
            KeyCode::Numpad0 => 0x52,
            KeyCode::Numpad1 => 0x53,
            KeyCode::Numpad2 => 0x54,
            KeyCode::Numpad3 => 0x55,
            KeyCode::Numpad4 => 0x56,
            KeyCode::Numpad5 => 0x57,
            KeyCode::Numpad6 => 0x58,
            KeyCode::Numpad7 => 0x59,
            KeyCode::Numpad8 => 0x5b,
            KeyCode::Numpad9 => 0x5c,
            // A Mac keyboard has F13-F15 where a PC one has these.
            KeyCode::PrintScreen => 0x69,
            KeyCode::ScrollLock => 0x6b,
            KeyCode::Pause => 0x71,
            KeyCode::Unknown => NO_MAC_KEY,
        };
    }
    #[cfg(not(target_os = "macos"))]
    windows_key_code(key_code)
}

/// Carbon keyCode for a Unicode character on Mac. Must not use the
/// Windows VK: 0x4C is VK_L and also kVK_ANSI_KeypadEnter.
pub fn native_key_code_for_char(ch: char) -> i32 {
    #[cfg(target_os = "macos")]
    {
        let code = match ch.to_ascii_lowercase() {
            'a' => KeyCode::KeyA,
            'b' => KeyCode::KeyB,
            'c' => KeyCode::KeyC,
            'd' => KeyCode::KeyD,
            'e' => KeyCode::KeyE,
            'f' => KeyCode::KeyF,
            'g' => KeyCode::KeyG,
            'h' => KeyCode::KeyH,
            'i' => KeyCode::KeyI,
            'j' => KeyCode::KeyJ,
            'k' => KeyCode::KeyK,
            'l' => KeyCode::KeyL,
            'm' => KeyCode::KeyM,
            'n' => KeyCode::KeyN,
            'o' => KeyCode::KeyO,
            'p' => KeyCode::KeyP,
            'q' => KeyCode::KeyQ,
            'r' => KeyCode::KeyR,
            's' => KeyCode::KeyS,
            't' => KeyCode::KeyT,
            'u' => KeyCode::KeyU,
            'v' => KeyCode::KeyV,
            'w' => KeyCode::KeyW,
            'x' => KeyCode::KeyX,
            'y' => KeyCode::KeyY,
            'z' => KeyCode::KeyZ,
            '0' => KeyCode::Key0,
            '1' => KeyCode::Key1,
            '2' => KeyCode::Key2,
            '3' => KeyCode::Key3,
            '4' => KeyCode::Key4,
            '5' => KeyCode::Key5,
            '6' => KeyCode::Key6,
            '7' => KeyCode::Key7,
            '8' => KeyCode::Key8,
            '9' => KeyCode::Key9,
            ' ' => KeyCode::Space,
            '\r' | '\n' => KeyCode::ReturnKey,
            '\t' => KeyCode::Tab,
            '\u{8}' => KeyCode::Backspace,
            // The US layout's punctuation, shifted or not: the code
            // point of every one of these is some other Carbon key
            // ('.' is 0x2E = M, '-' is 0x2D = N, ';' is 0x3B = Control).
            '`' | '~' => KeyCode::Backtick,
            '-' | '_' => KeyCode::Minus,
            '=' | '+' => KeyCode::Equals,
            '[' | '{' => KeyCode::LBracket,
            ']' | '}' => KeyCode::RBracket,
            '\\' | '|' => KeyCode::Backslash,
            ';' | ':' => KeyCode::Semicolon,
            '\'' | '"' => KeyCode::Quote,
            ',' | '<' => KeyCode::Comma,
            '.' | '>' => KeyCode::Period,
            '/' | '?' => KeyCode::Slash,
            ')' => KeyCode::Key0,
            '!' => KeyCode::Key1,
            '@' => KeyCode::Key2,
            '#' => KeyCode::Key3,
            '$' => KeyCode::Key4,
            '%' => KeyCode::Key5,
            '^' => KeyCode::Key6,
            '&' => KeyCode::Key7,
            '*' => KeyCode::Key8,
            '(' => KeyCode::Key9,
            // A character with no key on the US layout (an accented
            // letter from a dead key, anything the IME composed): no
            // Carbon code at all, never its code point.
            _ => return NO_MAC_KEY,
        };
        return native_key_code(code);
    }
    #[cfg(not(target_os = "macos"))]
    {
        if ch.is_ascii_alphabetic() {
            ch.to_ascii_uppercase() as i32
        } else {
            ch as i32
        }
    }
}

pub fn key_char(key_code: KeyCode, shift: bool) -> Option<char> {
    match key_code {
        KeyCode::Backspace | KeyCode::Back => Some('\u{8}'),
        KeyCode::Backtick => Some(if shift { '~' } else { '`' }),
        KeyCode::Key0 => Some(if shift { ')' } else { '0' }),
        KeyCode::Key1 => Some(if shift { '!' } else { '1' }),
        KeyCode::Key2 => Some(if shift { '@' } else { '2' }),
        KeyCode::Key3 => Some(if shift { '#' } else { '3' }),
        KeyCode::Key4 => Some(if shift { '$' } else { '4' }),
        KeyCode::Key5 => Some(if shift { '%' } else { '5' }),
        KeyCode::Key6 => Some(if shift { '^' } else { '6' }),
        KeyCode::Key7 => Some(if shift { '&' } else { '7' }),
        KeyCode::Key8 => Some(if shift { '*' } else { '8' }),
        KeyCode::Key9 => Some(if shift { '(' } else { '9' }),
        KeyCode::Minus => Some(if shift { '_' } else { '-' }),
        KeyCode::Equals => Some(if shift { '+' } else { '=' }),
        KeyCode::KeyQ => Some(if shift { 'Q' } else { 'q' }),
        KeyCode::KeyW => Some(if shift { 'W' } else { 'w' }),
        KeyCode::KeyE => Some(if shift { 'E' } else { 'e' }),
        KeyCode::KeyR => Some(if shift { 'R' } else { 'r' }),
        KeyCode::KeyT => Some(if shift { 'T' } else { 't' }),
        KeyCode::KeyY => Some(if shift { 'Y' } else { 'y' }),
        KeyCode::KeyU => Some(if shift { 'U' } else { 'u' }),
        KeyCode::KeyI => Some(if shift { 'I' } else { 'i' }),
        KeyCode::KeyO => Some(if shift { 'O' } else { 'o' }),
        KeyCode::KeyP => Some(if shift { 'P' } else { 'p' }),
        KeyCode::LBracket => Some(if shift { '{' } else { '[' }),
        KeyCode::RBracket => Some(if shift { '}' } else { ']' }),
        KeyCode::KeyA => Some(if shift { 'A' } else { 'a' }),
        KeyCode::KeyS => Some(if shift { 'S' } else { 's' }),
        KeyCode::KeyD => Some(if shift { 'D' } else { 'd' }),
        KeyCode::KeyF => Some(if shift { 'F' } else { 'f' }),
        KeyCode::KeyG => Some(if shift { 'G' } else { 'g' }),
        KeyCode::KeyH => Some(if shift { 'H' } else { 'h' }),
        KeyCode::KeyJ => Some(if shift { 'J' } else { 'j' }),
        KeyCode::KeyK => Some(if shift { 'K' } else { 'k' }),
        KeyCode::KeyL => Some(if shift { 'L' } else { 'l' }),
        KeyCode::Semicolon => Some(if shift { ':' } else { ';' }),
        KeyCode::Quote => Some(if shift { '"' } else { '\'' }),
        KeyCode::Backslash => Some(if shift { '|' } else { '\\' }),
        KeyCode::KeyZ => Some(if shift { 'Z' } else { 'z' }),
        KeyCode::KeyX => Some(if shift { 'X' } else { 'x' }),
        KeyCode::KeyC => Some(if shift { 'C' } else { 'c' }),
        KeyCode::KeyV => Some(if shift { 'V' } else { 'v' }),
        KeyCode::KeyB => Some(if shift { 'B' } else { 'b' }),
        KeyCode::KeyN => Some(if shift { 'N' } else { 'n' }),
        KeyCode::KeyM => Some(if shift { 'M' } else { 'm' }),
        KeyCode::Comma => Some(if shift { '<' } else { ',' }),
        KeyCode::Period => Some(if shift { '>' } else { '.' }),
        KeyCode::Slash => Some(if shift { '?' } else { '/' }),
        KeyCode::Tab => Some('\t'),
        KeyCode::ReturnKey => Some('\r'),
        KeyCode::Space => Some(' '),
        KeyCode::Numpad0 => Some('0'),
        KeyCode::Numpad1 => Some('1'),
        KeyCode::Numpad2 => Some('2'),
        KeyCode::Numpad3 => Some('3'),
        KeyCode::Numpad4 => Some('4'),
        KeyCode::Numpad5 => Some('5'),
        KeyCode::Numpad6 => Some('6'),
        KeyCode::Numpad7 => Some('7'),
        KeyCode::Numpad8 => Some('8'),
        KeyCode::Numpad9 => Some('9'),
        KeyCode::NumpadEquals => Some('='),
        KeyCode::NumpadSubtract => Some('-'),
        KeyCode::NumpadAdd => Some('+'),
        KeyCode::NumpadDecimal => Some('.'),
        KeyCode::NumpadMultiply => Some('*'),
        KeyCode::NumpadDivide => Some('/'),
        KeyCode::NumpadEnter => Some('\r'),
        _ => None,
    }
}

pub fn sends_char_on_keydown(key_code: KeyCode) -> bool {
    matches!(
        key_code,
        KeyCode::Backspace
            | KeyCode::Back
            | KeyCode::Tab
            | KeyCode::ReturnKey
            | KeyCode::NumpadEnter
    )
}

/// Whether a key-down goes on to the page. For the copy, cut and paste
/// chords every platform raises `TextCopy` / `TextCut` / a `TextInput`
/// with `was_paste` (and the WM forwards its own to a hosted app); the
/// page is served from those ([`answer_clipboard`], the paste
/// commit), so running the chord in the page as well would do it twice.
/// A paste chord still reaches the page on macOS and Windows when no text
/// paste came first (an image on the clipboard: Chromium reads the system
/// clipboard itself); never on Linux, where the windowless Chromium runs
/// without a display server and its clipboard is private to the process.
pub fn page_takes_key_down(key_event: &KeyEvent, pasted_text: &mut bool) -> bool {
    let modifiers = &key_event.modifiers;
    if !(modifiers.control || modifiers.logo) || modifiers.alt {
        return true;
    }
    match key_event.key_code {
        KeyCode::KeyC | KeyCode::KeyX => false,
        KeyCode::KeyV => !std::mem::take(pasted_text) && !cfg!(target_os = "linux"),
        _ => true,
    }
}

pub fn char_event_data(text: &str) -> Option<(i32, u16)> {
    let mut chars = text.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let mut utf16 = text.encode_utf16();
    let unit = utf16.next()?;
    if utf16.next().is_some() {
        return None;
    }
    let windows_key_code = if ch.is_ascii_alphabetic() {
        ch.to_ascii_uppercase() as i32
    } else {
        unit as i32
    };
    Some((windows_key_code, unit))
}

pub fn key_event_modifiers(key_event: &KeyEvent) -> u32 {
    let mut modifiers = cef_modifiers(key_event.modifiers, MouseButton::empty());
    if key_event.is_repeat {
        modifiers |= EVENTFLAG_IS_REPEAT;
    }
    if matches!(
        key_event.key_code,
        KeyCode::Numpad0
            | KeyCode::Numpad1
            | KeyCode::Numpad2
            | KeyCode::Numpad3
            | KeyCode::Numpad4
            | KeyCode::Numpad5
            | KeyCode::Numpad6
            | KeyCode::Numpad7
            | KeyCode::Numpad8
            | KeyCode::Numpad9
            | KeyCode::NumpadEquals
            | KeyCode::NumpadSubtract
            | KeyCode::NumpadAdd
            | KeyCode::NumpadDecimal
            | KeyCode::NumpadMultiply
            | KeyCode::NumpadDivide
            | KeyCode::NumpadEnter
    ) {
        modifiers |= EVENTFLAG_IS_KEY_PAD;
    }
    modifiers
}
