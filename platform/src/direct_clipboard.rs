//! The clipboard of a linux_direct session.
//!
//! On DRM/KMS there is no display server, so there is no system clipboard to
//! ask: the process that owns the display (the WM) owns the clipboard. It
//! lives here, in-process. The keyboard chords work as on X11 (Ctrl or Logo
//! with C/X/V), `cx.copy_to_clipboard` writes it, and hosted children reach it
//! through the existing protocol: their copies arrive as
//! `AppToStudio::SetClipboard` (→ `copy_to_clipboard`, i.e. here) and a paste
//! is the WM's `TextInput { was_paste }` forwarded to the focused child.

use crate::event::{KeyCode, KeyEvent};

/// What a key-down means for the clipboard.
#[derive(Debug, PartialEq)]
pub(crate) enum ClipboardKey {
    Copy,
    Cut,
    /// Paste this text (the chord still reaches the app as a key-down).
    Paste(String),
}

#[derive(Default)]
pub(crate) struct DirectClipboard {
    text: String,
}

impl DirectClipboard {
    pub fn set(&mut self, text: String) {
        self.text = text;
    }

    /// The clipboard action of this key-down, if any. Repeats of a held
    /// chord do not copy or paste again.
    pub fn key_down(&self, e: &KeyEvent) -> Option<ClipboardKey> {
        if e.is_repeat || !(e.modifiers.control || e.modifiers.logo) || e.modifiers.alt {
            return None;
        }
        match e.key_code {
            KeyCode::KeyC => Some(ClipboardKey::Copy),
            KeyCode::KeyX => Some(ClipboardKey::Cut),
            KeyCode::KeyV if !self.text.is_empty() => Some(ClipboardKey::Paste(self.text.clone())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::KeyModifiers;

    fn key(key_code: KeyCode, control: bool) -> KeyEvent {
        KeyEvent {
            key_code,
            is_repeat: false,
            modifiers: KeyModifiers { control, ..Default::default() },
            time: 0.0,
        }
    }

    #[test]
    fn chords_copy_cut_and_paste_the_stored_text() {
        let mut clip = DirectClipboard::default();
        assert_eq!(clip.key_down(&key(KeyCode::KeyC, true)), Some(ClipboardKey::Copy));
        assert_eq!(clip.key_down(&key(KeyCode::KeyX, true)), Some(ClipboardKey::Cut));
        // Nothing to paste yet.
        assert_eq!(clip.key_down(&key(KeyCode::KeyV, true)), None);
        clip.set("hello".into());
        assert_eq!(clip.key_down(&key(KeyCode::KeyV, true)), Some(ClipboardKey::Paste("hello".into())));
        // Plain keys and held repeats do nothing.
        assert_eq!(clip.key_down(&key(KeyCode::KeyV, false)), None);
        let mut repeat = key(KeyCode::KeyV, true);
        repeat.is_repeat = true;
        assert_eq!(clip.key_down(&repeat), None);
    }
}
