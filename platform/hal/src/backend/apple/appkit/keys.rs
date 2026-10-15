//! Key and modifier translation: NSEvent -> the shared key-code table and the modifier bits of
//! `input`. Moved from os/apple/apple_util.rs (`get_event_keycode`, `keycode_to_menu_key`).

use makepad_apple_sys::*;
use crate::types::input::*;
use makepad_key_code::KeyCode;

/// `mods` bits of an `NSEventModifierFlags` word.
pub fn modifier_bits(flags: u64) -> u32 {
    let mut bits = 0;
    if flags & NSEventModifierFlags::NSShiftKeyMask as u64 != 0 {
        bits |= MOD_SHIFT;
    }
    if flags & NSEventModifierFlags::NSControlKeyMask as u64 != 0 {
        bits |= MOD_CONTROL;
    }
    if flags & NSEventModifierFlags::NSAlternateKeyMask as u64 != 0 {
        bits |= MOD_ALT;
    }
    if flags & NSEventModifierFlags::NSCommandKeyMask as u64 != 0 {
        bits |= MOD_LOGO;
    }
    bits
}

/// The modifier bits of an event.
pub fn event_mods(event: ObjcId) -> u32 {
    let flags: u64 = unsafe { msg_send![event, modifierFlags] };
    modifier_bits(flags)
}

/// Mouse button bit (Cx's `MouseButton` mask: primary 1, secondary 2, middle 4, back 8, forward 16)
/// of an NSEvent button number.
pub fn button_bit(number: usize) -> u32 {
    match number {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        n if n < 32 => 1 << n,
        _ => 0,
    }
}

pub fn event_button_number(event: ObjcId) -> usize {
    unsafe { msg_send![event, buttonNumber] }
}

pub fn get_event_keycode(event: ObjcId) -> Option<KeyCode> {
    let scan_code: std::os::raw::c_ushort = unsafe { msg_send![event, keyCode] };

    Some(match scan_code {
        0x00 => KeyCode::KeyA,
        0x01 => KeyCode::KeyS,
        0x02 => KeyCode::KeyD,
        0x03 => KeyCode::KeyF,
        0x04 => KeyCode::KeyH,
        0x05 => KeyCode::KeyG,
        0x06 => KeyCode::KeyZ,
        0x07 => KeyCode::KeyX,
        0x08 => KeyCode::KeyC,
        0x09 => KeyCode::KeyV,
        //0x0a => World 1,
        0x0b => KeyCode::KeyB,
        0x0c => KeyCode::KeyQ,
        0x0d => KeyCode::KeyW,
        0x0e => KeyCode::KeyE,
        0x0f => KeyCode::KeyR,
        0x10 => KeyCode::KeyY,
        0x11 => KeyCode::KeyT,
        0x12 => KeyCode::Key1,
        0x13 => KeyCode::Key2,
        0x14 => KeyCode::Key3,
        0x15 => KeyCode::Key4,
        0x16 => KeyCode::Key6,
        0x17 => KeyCode::Key5,
        0x18 => KeyCode::Equals,
        0x19 => KeyCode::Key9,
        0x1a => KeyCode::Key7,
        0x1b => KeyCode::Minus,
        0x1c => KeyCode::Key8,
        0x1d => KeyCode::Key0,
        0x1e => KeyCode::RBracket,
        0x1f => KeyCode::KeyO,
        0x20 => KeyCode::KeyU,
        0x21 => KeyCode::LBracket,
        0x22 => KeyCode::KeyI,
        0x23 => KeyCode::KeyP,
        0x24 => KeyCode::ReturnKey,
        0x25 => KeyCode::KeyL,
        0x26 => KeyCode::KeyJ,
        0x27 => KeyCode::Quote,
        0x28 => KeyCode::KeyK,
        0x29 => KeyCode::Semicolon,
        0x2a => KeyCode::Backslash,
        0x2b => KeyCode::Comma,
        0x2c => KeyCode::Slash,
        0x2d => KeyCode::KeyN,
        0x2e => KeyCode::KeyM,
        0x2f => KeyCode::Period,
        0x30 => KeyCode::Tab,
        0x31 => KeyCode::Space,
        0x32 => KeyCode::Backtick,
        0x33 => KeyCode::Backspace,
        //0x34 => unkown,
        0x35 => KeyCode::Escape,
        //0x36 => KeyCode::RLogo,
        //0x37 => KeyCode::LLogo,
        //0x38 => KeyCode::LShift,
        0x39 => KeyCode::Capslock,
        //0x3a => KeyCode::LAlt,
        //0x3b => KeyCode::LControl,
        //0x3c => KeyCode::RShift,
        //0x3d => KeyCode::RAlt,
        //0x3e => KeyCode::RControl,
        //0x3f => Fn key,
        //0x40 => KeyCode::F17,
        0x41 => KeyCode::NumpadDecimal,
        //0x42 -> unkown,
        0x43 => KeyCode::NumpadMultiply,
        //0x44 => unkown,
        0x45 => KeyCode::NumpadAdd,
        //0x46 => unkown,
        0x47 => KeyCode::Numlock,
        //0x48 => KeypadClear,
        //0x49 => KeyCode::VolumeUp,
        //0x4a => KeyCode::VolumeDown,
        0x4b => KeyCode::NumpadDivide,
        0x4c => KeyCode::NumpadEnter,
        0x4e => KeyCode::NumpadSubtract,
        //0x4d => unkown,
        //0x4e => KeyCode::Subtract,
        //0x4f => KeyCode::F18,
        //0x50 => KeyCode::F19,
        0x51 => KeyCode::NumpadEquals,
        0x52 => KeyCode::Numpad0,
        0x53 => KeyCode::Numpad1,
        0x54 => KeyCode::Numpad2,
        0x55 => KeyCode::Numpad3,
        0x56 => KeyCode::Numpad4,
        0x57 => KeyCode::Numpad5,
        0x58 => KeyCode::Numpad6,
        0x59 => KeyCode::Numpad7,
        //0x5a => KeyCode::F20,
        0x5b => KeyCode::Numpad8,
        0x5c => KeyCode::Numpad9,
        //0x5d => KeyCode::Yen,
        //0x5e => JIS Ro,
        //0x5f => unkown,
        0x60 => KeyCode::F5,
        0x61 => KeyCode::F6,
        0x62 => KeyCode::F7,
        0x63 => KeyCode::F3,
        0x64 => KeyCode::F8,
        0x65 => KeyCode::F9,
        //0x66 => JIS Eisuu (macOS),
        0x67 => KeyCode::F11,
        //0x68 => JIS Kana (macOS),
        0x69 => KeyCode::PrintScreen,
        //0x6a => KeyCode::F16,
        //0x6b => KeyCode::F14,
        //0x6c => unkown,
        0x6d => KeyCode::F10,
        //0x6e => unkown,
        0x6f => KeyCode::F12,
        //0x70 => unkown,
        //0x71 => KeyCode::F15,
        0x72 => KeyCode::Insert,
        0x73 => KeyCode::Home,
        0x74 => KeyCode::PageUp,
        0x75 => KeyCode::Delete,
        0x76 => KeyCode::F4,
        0x77 => KeyCode::End,
        0x78 => KeyCode::F2,
        0x79 => KeyCode::PageDown,
        0x7a => KeyCode::F1,
        0x7b => KeyCode::ArrowLeft,
        0x7c => KeyCode::ArrowRight,
        0x7d => KeyCode::ArrowDown,
        0x7e => KeyCode::ArrowUp,
        //0x7f =>  unkown,
        //0xa => KeyCode::Caret,
        _ => return None,
    })
}

pub fn keycode_to_menu_key(keycode: KeyCode, shift: bool) -> &'static str {
    if !shift {
        match keycode {
            KeyCode::Backtick => "`",
            KeyCode::Key0 => "0",
            KeyCode::Key1 => "1",
            KeyCode::Key2 => "2",
            KeyCode::Key3 => "3",
            KeyCode::Key4 => "4",
            KeyCode::Key5 => "5",
            KeyCode::Key6 => "6",
            KeyCode::Key7 => "7",
            KeyCode::Key8 => "8",
            KeyCode::Key9 => "9",
            KeyCode::Minus => "-",
            KeyCode::Equals => "=",

            KeyCode::KeyQ => "q",
            KeyCode::KeyW => "w",
            KeyCode::KeyE => "e",
            KeyCode::KeyR => "r",
            KeyCode::KeyT => "t",
            KeyCode::KeyY => "y",
            KeyCode::KeyU => "u",
            KeyCode::KeyI => "i",
            KeyCode::KeyO => "o",
            KeyCode::KeyP => "p",
            KeyCode::LBracket => "[",
            KeyCode::RBracket => "]",

            KeyCode::KeyA => "a",
            KeyCode::KeyS => "s",
            KeyCode::KeyD => "d",
            KeyCode::KeyF => "f",
            KeyCode::KeyG => "g",
            KeyCode::KeyH => "h",
            KeyCode::KeyJ => "j",
            KeyCode::KeyK => "k",
            KeyCode::KeyL => "l",
            KeyCode::Semicolon => ";",
            KeyCode::Quote => "'",
            KeyCode::Backslash => "\\",

            KeyCode::KeyZ => "z",
            KeyCode::KeyX => "x",
            KeyCode::KeyC => "c",
            KeyCode::KeyV => "v",
            KeyCode::KeyB => "b",
            KeyCode::KeyN => "n",
            KeyCode::KeyM => "m",
            KeyCode::Comma => ",",
            KeyCode::Period => ".",
            KeyCode::Slash => "/",
            _ => "",
        }
    } else {
        match keycode {
            KeyCode::Backtick => "~",
            KeyCode::Key0 => "!",
            KeyCode::Key1 => "@",
            KeyCode::Key2 => "#",
            KeyCode::Key3 => "$",
            KeyCode::Key4 => "%",
            KeyCode::Key5 => "^",
            KeyCode::Key6 => "&",
            KeyCode::Key7 => "*",
            KeyCode::Key8 => "(",
            KeyCode::Key9 => ")",
            KeyCode::Minus => "_",
            KeyCode::Equals => "+",

            KeyCode::KeyQ => "Q",
            KeyCode::KeyW => "W",
            KeyCode::KeyE => "E",
            KeyCode::KeyR => "R",
            KeyCode::KeyT => "T",
            KeyCode::KeyY => "Y",
            KeyCode::KeyU => "U",
            KeyCode::KeyI => "I",
            KeyCode::KeyO => "O",
            KeyCode::KeyP => "P",
            KeyCode::LBracket => "{",
            KeyCode::RBracket => "}",

            KeyCode::KeyA => "A",
            KeyCode::KeyS => "S",
            KeyCode::KeyD => "D",
            KeyCode::KeyF => "F",
            KeyCode::KeyG => "G",
            KeyCode::KeyH => "H",
            KeyCode::KeyJ => "J",
            KeyCode::KeyK => "K",
            KeyCode::KeyL => "L",
            KeyCode::Semicolon => ":",
            KeyCode::Quote => "\"",
            KeyCode::Backslash => "|",

            KeyCode::KeyZ => "Z",
            KeyCode::KeyX => "X",
            KeyCode::KeyC => "C",
            KeyCode::KeyV => "V",
            KeyCode::KeyB => "B",
            KeyCode::KeyN => "N",
            KeyCode::KeyM => "M",
            KeyCode::Comma => "<",
            KeyCode::Period => ">",
            KeyCode::Slash => "?",
            _ => "",
        }
    }
}
