use {
    crate::{
        area::Area, cx::Cx, event::finger::KeyModifiers, live_traits::*, makepad_derive_live::*,
        makepad_live_compiler::*, makepad_micro_serde::*,
    },
    std::cell::RefCell,
    std::ops::Range,
    std::rc::Rc,
};

#[derive(Default)]
pub struct CxKeyboard {
    pub(crate) prev_key_focus: Area,
    pub(crate) next_key_focus: Area,
    pub(crate) key_focus: Area,
    #[allow(dead_code)]
    pub(crate) keys_down: Vec<KeyEvent>,
    pub(crate) text_ime_dismissed: bool,
}

impl CxKeyboard {
    pub fn modifiers(&self) -> KeyModifiers {
        if let Some(key) = self.keys_down.first() {
            key.modifiers
        } else {
            Default::default()
        }
    }

    pub fn set_key_focus(&mut self, focus_area: Area) {
        self.text_ime_dismissed = false;
        self.next_key_focus = focus_area;
    }

    pub fn key_focus(&self) -> Area {
        self.key_focus
    }

    pub fn revert_key_focus(&mut self) {
        self.next_key_focus = self.prev_key_focus;
    }

    pub fn has_key_focus(&self, focus_area: Area) -> bool {
        self.key_focus == focus_area
    }

    pub fn set_text_ime_dismissed(&mut self) {
        self.text_ime_dismissed = true;
    }

    pub fn reset_text_ime_dismissed(&mut self) {
        self.text_ime_dismissed = false;
    }

    pub(crate) fn update_area(&mut self, old_area: Area, new_area: Area) {
        if self.key_focus == old_area {
            self.key_focus = new_area
        }
        if self.prev_key_focus == old_area {
            self.prev_key_focus = new_area
        }
        if self.next_key_focus == old_area {
            self.next_key_focus = new_area
        }
    }

    #[allow(dead_code)]
    pub(crate) fn all_keys_up(&mut self) -> Vec<KeyEvent> {
        let mut keys_down = Vec::new();
        std::mem::swap(&mut keys_down, &mut self.keys_down);
        keys_down
    }

    pub(crate) fn cycle_key_focus_changed(&mut self) -> Option<(Area, Area)> {
        if self.next_key_focus != self.key_focus {
            self.prev_key_focus = self.key_focus;
            self.key_focus = self.next_key_focus;
            return Some((self.prev_key_focus, self.key_focus));
        }
        None
    }

    #[allow(dead_code)]
    pub fn is_key_down(&mut self, key_code: KeyCode) -> bool {
        self.keys_down.iter().any(|k| k.key_code == key_code)
    }

    pub(crate) fn process_key_down(&mut self, key_event: KeyEvent) {
        if self
            .keys_down
            .iter()
            .any(|k| k.key_code == key_event.key_code)
        {
            return;
        }
        self.keys_down.push(key_event);
    }

    pub(crate) fn process_key_up(&mut self, key_event: KeyEvent) {
        if let Some(pos) = self
            .keys_down
            .iter()
            .position(|k| k.key_code == key_event.key_code)
        {
            self.keys_down.remove(pos);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct KeyEvent {
    pub key_code: KeyCode,
    pub is_repeat: bool,
    pub modifiers: KeyModifiers,
    pub time: f64,
}

#[derive(Clone, Debug)]
pub struct KeyFocusEvent {
    pub prev: Area,
    pub focus: Area,
}

/// Character offset (Unicode scalar values)
/// Platform-independent index type for text positions
/// One character = one Unicode scalar (one emoji = one character)
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
    /// UTF-16 uses 1 unit for BMP chars, 2 units for emoji/supplementary
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
/// Used when platform is authoritative source of text state
/// Not serializable - only used for in-process events
#[derive(Clone, Debug, PartialEq)]
pub struct FullTextState {
    /// Full text content
    pub text: String,
    /// Selection range in character offsets
    pub selection: Range<CharOffset>,
    /// Composition range in character offsets (within text)
    /// None = no active composition
    pub composition: Option<Range<CharOffset>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextInputEvent {
    /// Text to insert or replace
    pub input: String,
    /// If true, replaces the previous composition/input
    /// Used for IME composition updates
    pub replace_last: bool,
    /// True if this input came from paste operation
    pub was_paste: bool,
    /// Composition range in character offsets (within input string)
    /// Some(range) = text is being composed (show underline)
    /// None = text is committed (no composition active)
    /// Not serializable - only used for in-process events
    pub composition: Option<Range<usize>>,
    /// Full text state synchronization (Android only)
    /// When Some, this is authoritative full state from platform
    /// Widget should replace entire text buffer and selection
    /// Not serializable - only used for in-process events
    pub full_state_sync: Option<FullTextState>,
    /// Range to replace in existing text (iOS autocorrect/paste)
    /// When Some, replace text[start..end] with input
    /// Character offsets in the widget's current text
    /// Not serializable - only used for in-process events
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

#[derive(Clone, Debug)]
pub struct TextClipboardEvent {
    pub response: Rc<RefCell<Option<String>>>,
}

/// IME editor action type (from mobile soft keyboard action buttons)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImeAction {
    /// Default action (not specified)
    Unspecified,
    None,
    /// "Go" button - typically for URL bars
    Go,
    /// "Search" button - typically for search fields
    Search,
    /// "Send" button - typically for messaging
    Send,
    /// "Next" button - move to next field
    Next,
    /// "Done" button - finish input
    Done,
    /// "Previous" button - move to previous field
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
/// Triggered when user presses the action button on the soft keyboard
#[derive(Clone, Debug)]
pub struct ImeActionEvent {
    /// The action that was triggered
    pub action: ImeAction,
}

impl Default for KeyCode {
    fn default() -> Self {
        KeyCode::Unknown
    }
}

// lowest common denominator keymap between desktop and web
#[derive(Live, LiveHook, Clone, Copy, Debug, SerBin, DeBin, SerJson, DeJson, Eq, PartialEq)]
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
