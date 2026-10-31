//! The cross-platform `KeyCode`. makepad-studio-protocol re-exports it (its key
//! events carry it) and makepad-script implements its Script traits (`KeyCode.F10`
//! in the DSL). It is its own crate so that neither the protocol nor script waits
//! for the other to compile: script needs only this small crate.
use makepad_micro_serde::*;

/// FNV-1a over `bytes`, continuing from `hash` (start with
/// [`FINGERPRINT_SEED`]). A `const fn`, so a crate can fingerprint its own
/// source at compile time.
pub const fn fingerprint(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut at = 0;
    while at < bytes.len() {
        hash ^= bytes[at] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        at += 1;
    }
    hash
}

pub const FINGERPRINT_SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// This file's fingerprint: `KeyCode` travels the hosted wire by ordinal,
/// so it is part of makepad-studio-protocol's `WIRE`.
pub const SOURCE_FINGERPRINT: u64 = fingerprint(FINGERPRINT_SEED, include_bytes!("lib.rs"));

impl Default for KeyCode {
    fn default() -> Self {
        KeyCode::Unknown
    }
}

// lowest common denominator keymap between desktop and web
// Note: Using manual SerJson/DeJson impl with integer encoding to reduce code bloat
// (derive-based string matching generates ~9500 lines of LLVM IR for 80 variants)
#[derive(Clone, Copy, Debug, SerBin, DeBin, Eq, PartialEq)]
pub enum KeyCode {
    Escape,

    Back,

    Backtick,
    Key0,
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
    Minus,
    Equals,

    Backspace,
    Tab,

    KeyQ,
    KeyW,
    KeyE,
    KeyR,
    KeyT,
    KeyY,
    KeyU,
    KeyI,
    KeyO,
    KeyP,
    LBracket,
    RBracket,
    ReturnKey,

    KeyA,
    KeyS,
    KeyD,
    KeyF,
    KeyG,
    KeyH,
    KeyJ,
    KeyK,
    KeyL,
    Semicolon,
    Quote,
    Backslash,

    KeyZ,
    KeyX,
    KeyC,
    KeyV,
    KeyB,
    KeyN,
    KeyM,
    Comma,
    Period,
    Slash,

    Control,
    Alt,
    Shift,
    Logo,

    Space,
    Capslock,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,

    PrintScreen,
    ScrollLock,
    Pause,

    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,

    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,

    NumpadEquals,
    NumpadSubtract,
    NumpadAdd,
    NumpadDecimal,
    NumpadMultiply,
    NumpadDivide,
    Numlock,
    NumpadEnter,

    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,

    Unknown,
}

impl KeyCode {
    /// A key by its name (`a`, `KeyA`, `enter`, `ArrowLeft`, `F5`, ...), as a driver types it.
    pub fn from_name(name: &str) -> Option<KeyCode> {
        let lower = name.to_ascii_lowercase();
        // single letter / digit shorthands: "a" -> KeyA, "1" -> Key1
        let normalized = if lower.len() == 1 {
            let c = lower.chars().next().unwrap();
            if c.is_ascii_alphanumeric() {
                format!("key{c}")
            } else {
                lower.clone()
            }
        } else {
            lower.clone()
        };
        Some(match normalized.as_str() {
            "escape" | "esc" => KeyCode::Escape,
            "back" => KeyCode::Back,
            "backtick" | "`" => KeyCode::Backtick,
            "key0" => KeyCode::Key0,
            "key1" => KeyCode::Key1,
            "key2" => KeyCode::Key2,
            "key3" => KeyCode::Key3,
            "key4" => KeyCode::Key4,
            "key5" => KeyCode::Key5,
            "key6" => KeyCode::Key6,
            "key7" => KeyCode::Key7,
            "key8" => KeyCode::Key8,
            "key9" => KeyCode::Key9,
            "minus" | "-" => KeyCode::Minus,
            "equals" | "=" => KeyCode::Equals,
            "backspace" => KeyCode::Backspace,
            "tab" => KeyCode::Tab,
            "keyq" => KeyCode::KeyQ,
            "keyw" => KeyCode::KeyW,
            "keye" => KeyCode::KeyE,
            "keyr" => KeyCode::KeyR,
            "keyt" => KeyCode::KeyT,
            "keyy" => KeyCode::KeyY,
            "keyu" => KeyCode::KeyU,
            "keyi" => KeyCode::KeyI,
            "keyo" => KeyCode::KeyO,
            "keyp" => KeyCode::KeyP,
            "lbracket" | "[" => KeyCode::LBracket,
            "rbracket" | "]" => KeyCode::RBracket,
            "return" | "returnkey" | "enter" => KeyCode::ReturnKey,
            "keya" => KeyCode::KeyA,
            "keys" => KeyCode::KeyS,
            "keyd" => KeyCode::KeyD,
            "keyf" => KeyCode::KeyF,
            "keyg" => KeyCode::KeyG,
            "keyh" => KeyCode::KeyH,
            "keyj" => KeyCode::KeyJ,
            "keyk" => KeyCode::KeyK,
            "keyl" => KeyCode::KeyL,
            "semicolon" | ";" => KeyCode::Semicolon,
            "quote" | "'" => KeyCode::Quote,
            "backslash" => KeyCode::Backslash,
            "keyz" => KeyCode::KeyZ,
            "keyx" => KeyCode::KeyX,
            "keyc" => KeyCode::KeyC,
            "keyv" => KeyCode::KeyV,
            "keyb" => KeyCode::KeyB,
            "keyn" => KeyCode::KeyN,
            "keym" => KeyCode::KeyM,
            "comma" | "," => KeyCode::Comma,
            "period" | "." => KeyCode::Period,
            "slash" | "/" => KeyCode::Slash,
            "control" | "ctrl" => KeyCode::Control,
            "alt" | "option" => KeyCode::Alt,
            "shift" => KeyCode::Shift,
            "logo" | "cmd" | "command" | "meta" => KeyCode::Logo,
            "space" => KeyCode::Space,
            "capslock" => KeyCode::Capslock,
            "f1" => KeyCode::F1,
            "f2" => KeyCode::F2,
            "f3" => KeyCode::F3,
            "f4" => KeyCode::F4,
            "f5" => KeyCode::F5,
            "f6" => KeyCode::F6,
            "f7" => KeyCode::F7,
            "f8" => KeyCode::F8,
            "f9" => KeyCode::F9,
            "f10" => KeyCode::F10,
            "f11" => KeyCode::F11,
            "f12" => KeyCode::F12,
            "printscreen" => KeyCode::PrintScreen,
            "scrolllock" => KeyCode::ScrollLock,
            "pause" => KeyCode::Pause,
            "insert" => KeyCode::Insert,
            "delete" | "del" => KeyCode::Delete,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "numpadenter" => KeyCode::NumpadEnter,
            "arrowup" | "up" => KeyCode::ArrowUp,
            "arrowdown" | "down" => KeyCode::ArrowDown,
            "arrowleft" | "left" => KeyCode::ArrowLeft,
            "arrowright" | "right" => KeyCode::ArrowRight,
            _ => return None,
        })
    }

    /// The key's index in the table every wire format shares (JSON, the HAL's key events).
    pub fn to_index(self) -> u32 {
        KEYCODE_VARIANTS.iter().position(|k| *k == self).unwrap_or(KEYCODE_VARIANTS.len() - 1) as u32
    }

    /// The key at `index` of the shared table; `Unknown` past its end.
    pub fn from_index(index: u32) -> KeyCode {
        KEYCODE_VARIANTS.get(index as usize).copied().unwrap_or(KeyCode::Unknown)
    }

    pub fn is_unknown(&self) -> bool {
        match self {
            Self::Unknown => true,
            _ => false,
        }
    }

    pub fn to_char(&self, uc: bool) -> Option<char> {
        match self {
            KeyCode::KeyA => {
                if uc {
                    Some('A')
                } else {
                    Some('a')
                }
            }
            KeyCode::KeyB => {
                if uc {
                    Some('B')
                } else {
                    Some('b')
                }
            }
            KeyCode::KeyC => {
                if uc {
                    Some('C')
                } else {
                    Some('c')
                }
            }
            KeyCode::KeyD => {
                if uc {
                    Some('D')
                } else {
                    Some('d')
                }
            }
            KeyCode::KeyE => {
                if uc {
                    Some('E')
                } else {
                    Some('e')
                }
            }
            KeyCode::KeyF => {
                if uc {
                    Some('F')
                } else {
                    Some('f')
                }
            }
            KeyCode::KeyG => {
                if uc {
                    Some('G')
                } else {
                    Some('g')
                }
            }
            KeyCode::KeyH => {
                if uc {
                    Some('H')
                } else {
                    Some('h')
                }
            }
            KeyCode::KeyI => {
                if uc {
                    Some('I')
                } else {
                    Some('i')
                }
            }
            KeyCode::KeyJ => {
                if uc {
                    Some('J')
                } else {
                    Some('j')
                }
            }
            KeyCode::KeyK => {
                if uc {
                    Some('K')
                } else {
                    Some('k')
                }
            }
            KeyCode::KeyL => {
                if uc {
                    Some('L')
                } else {
                    Some('l')
                }
            }
            KeyCode::KeyM => {
                if uc {
                    Some('M')
                } else {
                    Some('m')
                }
            }
            KeyCode::KeyN => {
                if uc {
                    Some('N')
                } else {
                    Some('n')
                }
            }
            KeyCode::KeyO => {
                if uc {
                    Some('O')
                } else {
                    Some('o')
                }
            }
            KeyCode::KeyP => {
                if uc {
                    Some('P')
                } else {
                    Some('p')
                }
            }
            KeyCode::KeyQ => {
                if uc {
                    Some('Q')
                } else {
                    Some('q')
                }
            }
            KeyCode::KeyR => {
                if uc {
                    Some('R')
                } else {
                    Some('r')
                }
            }
            KeyCode::KeyS => {
                if uc {
                    Some('S')
                } else {
                    Some('s')
                }
            }
            KeyCode::KeyT => {
                if uc {
                    Some('T')
                } else {
                    Some('t')
                }
            }
            KeyCode::KeyU => {
                if uc {
                    Some('U')
                } else {
                    Some('u')
                }
            }
            KeyCode::KeyV => {
                if uc {
                    Some('V')
                } else {
                    Some('v')
                }
            }
            KeyCode::KeyW => {
                if uc {
                    Some('W')
                } else {
                    Some('w')
                }
            }
            KeyCode::KeyX => {
                if uc {
                    Some('X')
                } else {
                    Some('x')
                }
            }
            KeyCode::KeyY => {
                if uc {
                    Some('Y')
                } else {
                    Some('y')
                }
            }
            KeyCode::KeyZ => {
                if uc {
                    Some('Z')
                } else {
                    Some('z')
                }
            }
            KeyCode::Key0 => {
                if uc {
                    Some(')')
                } else {
                    Some('0')
                }
            }
            KeyCode::Key1 => {
                if uc {
                    Some('!')
                } else {
                    Some('1')
                }
            }
            KeyCode::Key2 => {
                if uc {
                    Some('@')
                } else {
                    Some('2')
                }
            }
            KeyCode::Key3 => {
                if uc {
                    Some('#')
                } else {
                    Some('3')
                }
            }
            KeyCode::Key4 => {
                if uc {
                    Some('$')
                } else {
                    Some('4')
                }
            }
            KeyCode::Key5 => {
                if uc {
                    Some('%')
                } else {
                    Some('5')
                }
            }
            KeyCode::Key6 => {
                if uc {
                    Some('^')
                } else {
                    Some('6')
                }
            }
            KeyCode::Key7 => {
                if uc {
                    Some('&')
                } else {
                    Some('7')
                }
            }
            KeyCode::Key8 => {
                if uc {
                    Some('*')
                } else {
                    Some('8')
                }
            }
            KeyCode::Key9 => {
                if uc {
                    Some('(')
                } else {
                    Some('9')
                }
            }
            KeyCode::Equals => {
                if uc {
                    Some('+')
                } else {
                    Some('=')
                }
            }
            KeyCode::Minus => {
                if uc {
                    Some('_')
                } else {
                    Some('-')
                }
            }
            KeyCode::RBracket => {
                if uc {
                    Some('{')
                } else {
                    Some('[')
                }
            }
            KeyCode::LBracket => {
                if uc {
                    Some('}')
                } else {
                    Some(']')
                }
            }
            KeyCode::ReturnKey => Some('\n'),
            KeyCode::Backtick => {
                if uc {
                    Some('~')
                } else {
                    Some('`')
                }
            }
            KeyCode::Semicolon => {
                if uc {
                    Some(':')
                } else {
                    Some(';')
                }
            }
            KeyCode::Backslash => {
                if uc {
                    Some('|')
                } else {
                    Some('\\')
                }
            }
            KeyCode::Comma => {
                if uc {
                    Some('<')
                } else {
                    Some(',')
                }
            }
            KeyCode::Slash => {
                if uc {
                    Some('?')
                } else {
                    Some('/')
                }
            }
            KeyCode::Period => {
                if uc {
                    Some('>')
                } else {
                    Some('.')
                }
            }
            KeyCode::Tab => Some('\t'),
            KeyCode::Space => Some(' '),
            KeyCode::NumpadDecimal => Some('.'),
            KeyCode::NumpadMultiply => Some('*'),
            KeyCode::NumpadAdd => Some('+'),
            KeyCode::NumpadDivide => Some('/'),
            KeyCode::NumpadEnter => Some('\n'),
            KeyCode::NumpadSubtract => Some('-'),
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
            _ => None,
        }
    }
}

// Const array for efficient index-to-variant conversion (all 102 variants in order)
pub const KEYCODE_VARIANTS: [KeyCode; 102] = [
    KeyCode::Escape,
    KeyCode::Back,
    KeyCode::Backtick,
    KeyCode::Key0,
    KeyCode::Key1,
    KeyCode::Key2,
    KeyCode::Key3,
    KeyCode::Key4,
    KeyCode::Key5,
    KeyCode::Key6,
    KeyCode::Key7,
    KeyCode::Key8,
    KeyCode::Key9,
    KeyCode::Minus,
    KeyCode::Equals,
    KeyCode::Backspace,
    KeyCode::Tab,
    KeyCode::KeyQ,
    KeyCode::KeyW,
    KeyCode::KeyE,
    KeyCode::KeyR,
    KeyCode::KeyT,
    KeyCode::KeyY,
    KeyCode::KeyU,
    KeyCode::KeyI,
    KeyCode::KeyO,
    KeyCode::KeyP,
    KeyCode::LBracket,
    KeyCode::RBracket,
    KeyCode::ReturnKey,
    KeyCode::KeyA,
    KeyCode::KeyS,
    KeyCode::KeyD,
    KeyCode::KeyF,
    KeyCode::KeyG,
    KeyCode::KeyH,
    KeyCode::KeyJ,
    KeyCode::KeyK,
    KeyCode::KeyL,
    KeyCode::Semicolon,
    KeyCode::Quote,
    KeyCode::Backslash,
    KeyCode::KeyZ,
    KeyCode::KeyX,
    KeyCode::KeyC,
    KeyCode::KeyV,
    KeyCode::KeyB,
    KeyCode::KeyN,
    KeyCode::KeyM,
    KeyCode::Comma,
    KeyCode::Period,
    KeyCode::Slash,
    KeyCode::Control,
    KeyCode::Alt,
    KeyCode::Shift,
    KeyCode::Logo,
    KeyCode::Space,
    KeyCode::Capslock,
    KeyCode::F1,
    KeyCode::F2,
    KeyCode::F3,
    KeyCode::F4,
    KeyCode::F5,
    KeyCode::F6,
    KeyCode::F7,
    KeyCode::F8,
    KeyCode::F9,
    KeyCode::F10,
    KeyCode::F11,
    KeyCode::F12,
    KeyCode::PrintScreen,
    KeyCode::ScrollLock,
    KeyCode::Pause,
    KeyCode::Insert,
    KeyCode::Delete,
    KeyCode::Home,
    KeyCode::End,
    KeyCode::PageUp,
    KeyCode::PageDown,
    KeyCode::Numpad0,
    KeyCode::Numpad1,
    KeyCode::Numpad2,
    KeyCode::Numpad3,
    KeyCode::Numpad4,
    KeyCode::Numpad5,
    KeyCode::Numpad6,
    KeyCode::Numpad7,
    KeyCode::Numpad8,
    KeyCode::Numpad9,
    KeyCode::NumpadEquals,
    KeyCode::NumpadSubtract,
    KeyCode::NumpadAdd,
    KeyCode::NumpadDecimal,
    KeyCode::NumpadMultiply,
    KeyCode::NumpadDivide,
    KeyCode::Numlock,
    KeyCode::NumpadEnter,
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::Unknown,
];

// Manual SerJson/DeJson implementations using integer encoding
// This reduces LLVM IR from ~9500 lines (string matching) to ~100 lines (integer parsing + array lookup)
impl SerJson for KeyCode {
    fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
        let idx = KEYCODE_VARIANTS
            .iter()
            .position(|k| k == self)
            .unwrap_or(101);
        s.out.push_str(&idx.to_string());
    }
}

impl DeJson for KeyCode {
    fn de_json(s: &mut DeJsonState, i: &mut std::str::Chars) -> Result<Self, DeJsonErr> {
        let val = u64::de_json(s, i)? as usize;
        Ok(KEYCODE_VARIANTS
            .get(val)
            .copied()
            .unwrap_or(KeyCode::Unknown))
    }
}

pub mod android;
