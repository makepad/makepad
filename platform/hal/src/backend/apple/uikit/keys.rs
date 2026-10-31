//! Hardware keyboards (iOS): `UIPress` / `UIKey` -> the shared key-code table and the modifier
//! bits of `input`. The view takes the presses the text field does not: Return / Enter, Tab,
//! the navigation keys and Command shortcuts; plain typing reaches the app through the text
//! field (`super::text`). Moved from os/apple/ios/ios_delegates.rs (`hid_usage_to_key_code`,
//! `dispatch_hardware_key_presses`).

use super::app::app;
use super::classes::{shielded, win_of};
use super::text;
use super::window::Win;
use crate::types::input::*;
use makepad_apple_sys::*;
use makepad_key_code::KeyCode;

/// `mods` bits of a `UIKeyModifierFlags` word (the same bits as `NSEventModifierFlags`).
pub fn modifier_bits(flags: u64) -> u32 {
    let mut bits = 0;
    if flags & (1 << 17) != 0 {
        bits |= MOD_SHIFT;
    }
    if flags & (1 << 18) != 0 {
        bits |= MOD_CONTROL;
    }
    if flags & (1 << 19) != 0 {
        bits |= MOD_ALT;
    }
    if flags & (1 << 20) != 0 {
        bits |= MOD_LOGO;
    }
    bits
}

/// A USB HID keyboard usage (`UIKey.keyCode`) as a key code.
pub fn hid_usage_to_key_code(usage: u64) -> KeyCode {
    match usage {
        0x04 => KeyCode::KeyA,
        0x05 => KeyCode::KeyB,
        0x06 => KeyCode::KeyC,
        0x07 => KeyCode::KeyD,
        0x08 => KeyCode::KeyE,
        0x09 => KeyCode::KeyF,
        0x0A => KeyCode::KeyG,
        0x0B => KeyCode::KeyH,
        0x0C => KeyCode::KeyI,
        0x0D => KeyCode::KeyJ,
        0x0E => KeyCode::KeyK,
        0x0F => KeyCode::KeyL,
        0x10 => KeyCode::KeyM,
        0x11 => KeyCode::KeyN,
        0x12 => KeyCode::KeyO,
        0x13 => KeyCode::KeyP,
        0x14 => KeyCode::KeyQ,
        0x15 => KeyCode::KeyR,
        0x16 => KeyCode::KeyS,
        0x17 => KeyCode::KeyT,
        0x18 => KeyCode::KeyU,
        0x19 => KeyCode::KeyV,
        0x1A => KeyCode::KeyW,
        0x1B => KeyCode::KeyX,
        0x1C => KeyCode::KeyY,
        0x1D => KeyCode::KeyZ,
        0x1E => KeyCode::Key1,
        0x1F => KeyCode::Key2,
        0x20 => KeyCode::Key3,
        0x21 => KeyCode::Key4,
        0x22 => KeyCode::Key5,
        0x23 => KeyCode::Key6,
        0x24 => KeyCode::Key7,
        0x25 => KeyCode::Key8,
        0x26 => KeyCode::Key9,
        0x27 => KeyCode::Key0,
        0x28 => KeyCode::ReturnKey,
        0x29 => KeyCode::Escape,
        0x2A => KeyCode::Backspace,
        0x2B => KeyCode::Tab,
        0x2C => KeyCode::Space,
        0x2D => KeyCode::Minus,
        0x2E => KeyCode::Equals,
        0x2F => KeyCode::LBracket,
        0x30 => KeyCode::RBracket,
        0x31 => KeyCode::Backslash,
        0x33 => KeyCode::Semicolon,
        0x34 => KeyCode::Quote,
        0x35 => KeyCode::Backtick,
        0x36 => KeyCode::Comma,
        0x37 => KeyCode::Period,
        0x38 => KeyCode::Slash,
        0x39 => KeyCode::Capslock,
        0x3A => KeyCode::F1,
        0x3B => KeyCode::F2,
        0x3C => KeyCode::F3,
        0x3D => KeyCode::F4,
        0x3E => KeyCode::F5,
        0x3F => KeyCode::F6,
        0x40 => KeyCode::F7,
        0x41 => KeyCode::F8,
        0x42 => KeyCode::F9,
        0x43 => KeyCode::F10,
        0x44 => KeyCode::F11,
        0x45 => KeyCode::F12,
        0x49 => KeyCode::Insert,
        0x4A => KeyCode::Home,
        0x4B => KeyCode::PageUp,
        0x4C => KeyCode::Delete,
        0x4D => KeyCode::End,
        0x4E => KeyCode::PageDown,
        0x4F => KeyCode::ArrowRight,
        0x50 => KeyCode::ArrowLeft,
        0x51 => KeyCode::ArrowDown,
        0x52 => KeyCode::ArrowUp,
        0x53 => KeyCode::Numlock,
        0x54 => KeyCode::NumpadDivide,
        0x55 => KeyCode::NumpadMultiply,
        0x56 => KeyCode::NumpadSubtract,
        0x57 => KeyCode::NumpadAdd,
        0x58 => KeyCode::NumpadEnter,
        0x59 => KeyCode::Numpad1,
        0x5A => KeyCode::Numpad2,
        0x5B => KeyCode::Numpad3,
        0x5C => KeyCode::Numpad4,
        0x5D => KeyCode::Numpad5,
        0x5E => KeyCode::Numpad6,
        0x5F => KeyCode::Numpad7,
        0x60 => KeyCode::Numpad8,
        0x61 => KeyCode::Numpad9,
        0x62 => KeyCode::Numpad0,
        0x63 => KeyCode::NumpadDecimal,
        0x67 => KeyCode::NumpadEquals,
        _ => KeyCode::Unknown,
    }
}

fn is_navigation_key(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ArrowUp | KeyCode::ArrowDown | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown
    )
}

/// Navigation keys the text field's auto-repeating key commands own while it has the focus
/// (Home / End / PageUp / PageDown only where their iOS 13.4+ commands exist).
fn is_key_command_key(key: KeyCode) -> bool {
    match key {
        KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ArrowUp | KeyCode::ArrowDown => true,
        KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown => text::doc_nav_key_commands(),
        _ => false,
    }
}

/// The key of a press the view takes, with its modifiers.
unsafe fn view_key(press: ObjcId) -> Option<(KeyCode, u32)> {
    let key: ObjcId = msg_send![press, key];
    if key == nil {
        return None;
    }
    let flags: u64 = msg_send![key, modifierFlags];
    let mods = modifier_bits(flags);
    let usage: u64 = msg_send![key, keyCode];
    let code = hid_usage_to_key_code(usage);
    let enter = code == KeyCode::ReturnKey || code == KeyCode::NumpadEnter;
    if code.is_unknown() || (!enter && code != KeyCode::Tab && mods & MOD_LOGO == 0 && !is_navigation_key(code)) {
        None
    } else {
        Some((code, mods))
    }
}

/// A key the shell took for the app, in `win`. Command C / X ask the app for the selection (the
/// copy / cut chord), Command V pastes the pasteboard's text; their releases are swallowed.
pub fn key(win: &Win, code: KeyCode, mods: u32, down: bool) {
    if mods & MOD_LOGO != 0 {
        match code {
            KeyCode::KeyC | KeyCode::KeyX => {
                if down {
                    let id = win.id.get();
                    win.push(|q| q.key_down(id, code.to_index(), false, mods));
                }
                return;
            }
            KeyCode::KeyV => {
                if down {
                    text::paste(win);
                }
                return;
            }
            _ => {}
        }
    }
    let id = win.id.get();
    if down {
        win.push(|q| q.key_down(id, code.to_index(), false, mods));
    } else {
        win.push(|q| q.key_up(id, code.to_index(), mods));
    }
}

/// The presses of a `pressesBegan` / `Ended` the view takes, all or none: a set with a press the
/// view does not take goes on up the responder chain whole.
unsafe fn take_presses(win: &Win, presses: ObjcId, down: bool) -> bool {
    let mut keys = Vec::new();
    let enumerator: ObjcId = msg_send![presses, objectEnumerator];
    loop {
        let press: ObjcId = msg_send![enumerator, nextObject];
        if press == nil {
            break;
        }
        match view_key(press) {
            Some(key) => keys.push(key),
            None => return false,
        }
    }
    if keys.is_empty() {
        return false;
    }
    let field_focused = text::focused();
    for (code, mods) in keys {
        if field_focused && is_key_command_key(code) {
            continue;
        }
        if code == KeyCode::ReturnKey || code == KeyCode::NumpadEnter {
            // a hardware Enter never reaches the text field's own path: in a multiline field
            // that takes it as a newline it goes into the field, so it syncs in order
            if let Some((multiline, submit_on_enter, read_only)) = text::enter_config() {
                let primary = mods & MOD_LOGO != 0;
                if multiline && !primary && !(submit_on_enter && mods == 0) {
                    if down && !read_only {
                        text::insert_newline();
                    }
                    continue;
                }
            }
        }
        key(win, code, mods, down);
    }
    true
}

macro_rules! press_method {
    ($name:ident, $sel:literal, $down:expr) => {
        pub(super) extern "C" fn $name(this: &Object, _: Sel, presses: ObjcId, event: ObjcId) {
            let win = win_of(this).or_else(|| app().top_window());
            let mut taken = false;
            if let Some(win) = win {
                shielded("keys", || taken = unsafe { take_presses(win, presses, $down) });
            }
            if !taken {
                unsafe {
                    let () = msg_send![super(this, class!(UIView)), $name: presses withEvent: event];
                }
            }
        }
    };
}

#[allow(non_snake_case)]
mod presses {
    use super::*;
    press_method!(pressesBegan, "pressesBegan:withEvent:", true);
    press_method!(pressesChanged, "pressesChanged:withEvent:", true);
    press_method!(pressesEnded, "pressesEnded:withEvent:", false);
    press_method!(pressesCancelled, "pressesCancelled:withEvent:", false);
}

pub(super) fn add_press_methods(decl: &mut ClassDecl) {
    unsafe {
        decl.add_method(sel!(pressesBegan:withEvent:), presses::pressesBegan as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
        decl.add_method(sel!(pressesChanged:withEvent:), presses::pressesChanged as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
        decl.add_method(sel!(pressesEnded:withEvent:), presses::pressesEnded as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
        decl.add_method(sel!(pressesCancelled:withEvent:), presses::pressesCancelled as extern "C" fn(&Object, Sel, ObjcId, ObjcId));
    }
}
