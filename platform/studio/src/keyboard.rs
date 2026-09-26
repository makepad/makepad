use crate::mouse::KeyModifiers;
use makepad_micro_serde::*;
use std::ops::Range;

// KeyCode lives in its own small crate so that makepad-script, which implements
// its Script traits, does not wait for this crate (nor this crate for script).
pub use makepad_key_code::KeyCode;

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct KeyEvent {
    pub key_code: KeyCode,
    pub is_repeat: bool,
    pub modifiers: KeyModifiers,
    pub time: f64,
}

impl KeyEvent {
    /// The one cross-platform shortcut for the design tweaker.
    pub fn is_tweaker_toggle(&self) -> bool {
        self.key_code == KeyCode::F10
            && self.modifiers.shift
            && !self.modifiers.control
            && !self.modifiers.alt
            && !self.modifiers.logo
    }
}

#[cfg(test)]
mod key_event_tests {
    use super::*;

    fn f10(modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            key_code: KeyCode::F10,
            modifiers,
            ..Default::default()
        }
    }

    #[test]
    fn tweaker_toggle_is_shift_f10_only() {
        assert!(f10(KeyModifiers {
            shift: true,
            ..Default::default()
        })
        .is_tweaker_toggle());
        assert!(!f10(KeyModifiers::default()).is_tweaker_toggle());
        assert!(!f10(KeyModifiers {
            shift: true,
            control: true,
            ..Default::default()
        })
        .is_tweaker_toggle());
    }
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

