use crate::mouse::KeyModifiers;
use makepad_micro_serde::*;
use makepad_script::*;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct KeyEvent {
    pub key_code: KeyCode,
    pub is_repeat: bool,
    pub modifiers: KeyModifiers,
    pub time: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextInputEvent {
    /// Text to insert or replace
    pub input: String,
    /// If true, replaces the previous composition/input
    pub replace_last: bool,
    /// True if this input came from paste operation
    pub was_paste: bool,
    /// Composition range in character offsets (within input string)
    pub composition: Option<Range<usize>>,
    /// Full text state synchronization (Android only)
    pub full_state_sync: Option<FullTextState>,
    /// Range to replace in existing text (iOS autocorrect/paste)
    pub replace_range: Option<(CharOffset, CharOffset)>,
}

impl Default for TextInputEvent {
    fn default() -> Self {
        Self {
            input: String::new(),
            replace_last: false,
            was_paste: false,
            composition: None,
            full_state_sync: None,
            replace_range: None,
        }
    }
}

// Manual serialization: only serialize the 3 wire-compatible fields.
// The IME-specific fields are only used in-process, not over the stdin protocol.
#[derive(SerBin, DeBin, SerJson, DeJson)]
struct TextInputEventWire {
    input: String,
    replace_last: bool,
    was_paste: bool,
}

impl SerBin for TextInputEvent {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        let wire = TextInputEventWire {
            input: self.input.clone(),
            replace_last: self.replace_last,
            was_paste: self.was_paste,
        };
        wire.ser_bin(s);
    }
}

impl DeBin for TextInputEvent {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        let wire = TextInputEventWire::de_bin(o, d)?;
        Ok(Self {
            input: wire.input,
            replace_last: wire.replace_last,
            was_paste: wire.was_paste,
            ..Default::default()
        })
    }
}

impl SerJson for TextInputEvent {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        let wire = TextInputEventWire {
            input: self.input.clone(),
            replace_last: self.replace_last,
            was_paste: self.was_paste,
        };
        wire.ser_json(d, s);
    }
}

impl DeJson for TextInputEvent {
    fn de_json(s: &mut DeJsonState, i: &mut std::str::Chars) -> Result<Self, DeJsonErr> {
        let wire = TextInputEventWire::de_json(s, i)?;
        Ok(Self {
            input: wire.input,
            replace_last: wire.replace_last,
            was_paste: wire.was_paste,
            ..Default::default()
        })
    }
}

/// Character offset (Unicode scalar values)
/// Platform-independent index type for text positions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, SerBin, DeBin, SerJson, DeJson)]
pub struct CharOffset(pub usize);

impl CharOffset {
    /// Convert to byte index in UTF-8 string
    pub fn to_byte_index(self, text: &str) -> usize {
        text.char_indices()
            .nth(self.0)
            .map(|(byte_idx, _)| byte_idx)
            .unwrap_or(text.len())
    }

    /// Convert from UTF-16 index (Android/Java)
    pub fn from_utf16_index(text: &str, utf16_idx: usize) -> Self {
        let mut utf16_count = 0;
        for (char_idx, c) in text.chars().enumerate() {
            if utf16_count >= utf16_idx {
                return CharOffset(char_idx);
            }
            utf16_count += c.len_utf16();
        }
        CharOffset(text.chars().count())
    }

    /// Convert to UTF-16 index (for Android/Java)
    pub fn to_utf16_index(self, text: &str) -> usize {
        text.chars().take(self.0).map(|c| c.len_utf16()).sum()
    }

    /// Convert Range<CharOffset> to Range<usize> (byte indices)
    pub fn range_to_bytes(range: &Range<CharOffset>, text: &str) -> Range<usize> {
        range.start.to_byte_index(text)..range.end.to_byte_index(text)
    }
}

/// Full text state from platform IME (Android InputConnection)
#[derive(Clone, Debug, PartialEq)]
pub struct FullTextState {
    pub text: String,
    pub selection: Range<CharOffset>,
    pub composition: Option<Range<CharOffset>>,
}

/// IME editor action type (from mobile soft keyboard action buttons)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImeAction {
    Unspecified,
    None,
    Go,
    Search,
    Send,
    Next,
    Done,
    Previous,
}

impl ImeAction {
    /// Convert from Android EditorInfo action codes
    pub fn from_android_action_code(code: i32) -> Self {
        match code {
            0 => ImeAction::Unspecified,
            1 => ImeAction::None,
            2 => ImeAction::Go,
            3 => ImeAction::Search,
            4 => ImeAction::Send,
            5 => ImeAction::Next,
            6 => ImeAction::Done,
            7 => ImeAction::Previous,
            _ => ImeAction::Unspecified,
        }
    }
}

/// Event for IME editor action (Done, Go, Search, etc.)
#[derive(Clone, Debug)]
pub struct ImeActionEvent {
    pub action: ImeAction,
}

impl Default for KeyCode {
    fn default() -> Self {
        KeyCode::Unknown
    }
}

// lowest common denominator keymap between desktop and web
// Note: Using manual SerJson/DeJson impl with integer encoding to reduce code bloat
// (derive-based string matching generates ~9500 lines of LLVM IR for 80 variants)
#[derive(Script, ScriptHook, Clone, Copy, Debug, SerBin, DeBin, Eq, PartialEq)]
pub enum KeyCode {
    #[pick]
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
const KEYCODE_VARIANTS: [KeyCode; 102] = [
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
