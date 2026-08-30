use crate::{ProductMode, ScoreAction};
use makepad_widgets::*;

/// Stable, discoverable product keymap. Bare note-entry keys are active only
/// in editor mode; global transport/navigation remains useful in pianist mode.
pub fn action_for_key(event: &KeyEvent, mode: ProductMode, text_focused: bool) -> Option<ScoreAction> {
    let m = event.modifiers;
    if event.is_repeat && event.key_code != KeyCode::ArrowLeft && event.key_code != KeyCode::ArrowRight {
        return None;
    }

    if event.key_code == KeyCode::Escape {
        return Some(ScoreAction::Dismiss);
    }
    if m.is_primary() {
        return match event.key_code {
            KeyCode::KeyN => Some(ScoreAction::NewDemo),
            KeyCode::KeyE => Some(ScoreAction::ToggleMode),
            KeyCode::KeyO => Some(ScoreAction::OpenDialog(crate::DialogKind::Open)),
            KeyCode::KeyS if m.shift => Some(ScoreAction::OpenDialog(crate::DialogKind::SaveAs)),
            KeyCode::KeyS => Some(ScoreAction::Save),
            KeyCode::KeyZ if m.shift => Some(ScoreAction::Redo),
            KeyCode::KeyZ => Some(ScoreAction::Undo),
            KeyCode::KeyY => Some(ScoreAction::Redo),
            KeyCode::KeyA => Some(ScoreAction::SelectAll),
            KeyCode::Equals | KeyCode::NumpadAdd => Some(ScoreAction::ZoomBy(1.12)),
            KeyCode::Minus | KeyCode::NumpadSubtract => Some(ScoreAction::ZoomBy(1.0 / 1.12)),
            KeyCode::Key0 => Some(ScoreAction::FitPage),
            KeyCode::KeyQ => Some(ScoreAction::Quit),
            _ => None,
        };
    }
    if text_focused {
        return None;
    }
    match event.key_code {
        KeyCode::F1 => Some(ScoreAction::OpenDialog(crate::DialogKind::Keymap)),
        KeyCode::Space => Some(ScoreAction::PlayPause),
        KeyCode::ArrowLeft | KeyCode::PageUp => Some(ScoreAction::PageDelta(-1)),
        KeyCode::ArrowRight | KeyCode::PageDown => Some(ScoreAction::PageDelta(1)),
        KeyCode::Home => Some(ScoreAction::FirstPage),
        KeyCode::End => Some(ScoreAction::LastPage),
        KeyCode::KeyM => Some(ScoreAction::ToggleMetronome),
        KeyCode::KeyL => Some(ScoreAction::ToggleLoop),
        KeyCode::KeyF if mode == ProductMode::Pianist || m.shift => {
            Some(ScoreAction::ToggleFollow)
        }
        KeyCode::KeyR if mode == ProductMode::Editor => Some(ScoreAction::SelectMore),
        KeyCode::Key1 if mode == ProductMode::Editor => Some(ScoreAction::SetDuration(1)),
        KeyCode::Key2 if mode == ProductMode::Editor => Some(ScoreAction::SetDuration(2)),
        KeyCode::Key3 if mode == ProductMode::Editor => Some(ScoreAction::SetDuration(3)),
        KeyCode::Key4 if mode == ProductMode::Editor => Some(ScoreAction::SetDuration(4)),
        KeyCode::Key5 if mode == ProductMode::Editor => Some(ScoreAction::SetDuration(5)),
        KeyCode::Key6 if mode == ProductMode::Editor => Some(ScoreAction::SetDuration(6)),
        KeyCode::Key7 if mode == ProductMode::Editor => Some(ScoreAction::SetDuration(7)),
        KeyCode::KeyC if mode == ProductMode::Editor => Some(ScoreAction::EnterPitch('C')),
        KeyCode::KeyD if mode == ProductMode::Editor => Some(ScoreAction::EnterPitch('D')),
        KeyCode::KeyE if mode == ProductMode::Editor => Some(ScoreAction::EnterPitch('E')),
        KeyCode::KeyF if mode == ProductMode::Editor => Some(ScoreAction::EnterPitch('F')),
        KeyCode::KeyG if mode == ProductMode::Editor => Some(ScoreAction::EnterPitch('G')),
        KeyCode::KeyA if mode == ProductMode::Editor => Some(ScoreAction::EnterPitch('A')),
        KeyCode::KeyB if mode == ProductMode::Editor => Some(ScoreAction::EnterPitch('B')),
        _ => None,
    }
}

/// The product keymap, as the Help dialog shows it. One table, so the dialog
/// can never drift from [`action_for_key`]; every row below is dispatched
/// there and covered by `keymap_rows_match_the_dispatcher`.
pub const KEYMAP_ROWS: &[(&str, &str)] = &[
    ("Space", "Play / pause"),
    ("← / →", "Turn page"),
    ("Home / End", "First / last page"),
    ("M", "Metronome"),
    ("L", "Practice loop"),
    ("F", "Follow playback cursor (⇧F in editor)"),
    ("Escape", "Close dialog, then tool, then selection"),
    ("⌘E", "Pianist / editor mode"),
    ("⌘N", "New score"),
    ("⌘O", "Open…"),
    ("⌘S / ⇧⌘S", "Save / Save as…"),
    ("⌘0", "Fit page"),
    ("⌘+ / ⌘−", "Zoom"),
    ("⌘A", "Select all"),
    ("C D E F G A B", "Enter pitches at the caret (editor)"),
    ("1…7", "64th, 32nd, 16th, eighth, quarter, half, whole"),
    ("R", "Select more: note → chord → bar → score"),
    ("⌘Z / ⇧⌘Z", "Undo / redo"),
    ("⌘Q", "Quit"),
    ("F1", "Show this keymap"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_letters_are_editor_only() {
        let event = KeyEvent {
            key_code: KeyCode::KeyC,
            is_repeat: false,
            modifiers: KeyModifiers::default(),
            time: 0.0,
        };
        assert!(action_for_key(&event, ProductMode::Pianist, false).is_none());
        assert!(matches!(
            action_for_key(&event, ProductMode::Editor, false),
            Some(ScoreAction::EnterPitch('C'))
        ));
    }

    fn key(key_code: KeyCode, shift: bool, primary: bool) -> KeyEvent {
        let mut modifiers = KeyModifiers::default();
        modifiers.shift = shift;
        if primary {
            modifiers.logo = true;
        }
        KeyEvent {
            key_code,
            is_repeat: false,
            modifiers,
            time: 0.0,
        }
    }

    /// The Help dialog reads [`KEYMAP_ROWS`]; a row that no longer dispatches
    /// is the UI lying about what the app can do.
    #[test]
    fn keymap_rows_match_the_dispatcher() {
        let bindings: &[(&str, KeyEvent, ProductMode)] = &[
            ("Space", key(KeyCode::Space, false, false), ProductMode::Pianist),
            ("← / →", key(KeyCode::ArrowLeft, false, false), ProductMode::Pianist),
            ("Home / End", key(KeyCode::Home, false, false), ProductMode::Pianist),
            ("M", key(KeyCode::KeyM, false, false), ProductMode::Pianist),
            ("L", key(KeyCode::KeyL, false, false), ProductMode::Pianist),
            ("F", key(KeyCode::KeyF, false, false), ProductMode::Pianist),
            ("Escape", key(KeyCode::Escape, false, false), ProductMode::Pianist),
            ("⌘E", key(KeyCode::KeyE, false, true), ProductMode::Pianist),
            ("⌘N", key(KeyCode::KeyN, false, true), ProductMode::Pianist),
            ("⌘O", key(KeyCode::KeyO, false, true), ProductMode::Pianist),
            ("⌘S / ⇧⌘S", key(KeyCode::KeyS, false, true), ProductMode::Pianist),
            ("⌘0", key(KeyCode::Key0, false, true), ProductMode::Pianist),
            ("⌘+ / ⌘−", key(KeyCode::Equals, false, true), ProductMode::Pianist),
            ("⌘A", key(KeyCode::KeyA, false, true), ProductMode::Pianist),
            ("C D E F G A B", key(KeyCode::KeyC, false, false), ProductMode::Editor),
            ("1…7", key(KeyCode::Key1, false, false), ProductMode::Editor),
            ("R", key(KeyCode::KeyR, false, false), ProductMode::Editor),
            ("⌘Z / ⇧⌘Z", key(KeyCode::KeyZ, false, true), ProductMode::Pianist),
            ("⌘Q", key(KeyCode::KeyQ, false, true), ProductMode::Pianist),
            ("F1", key(KeyCode::F1, false, false), ProductMode::Pianist),
        ];
        assert_eq!(bindings.len(), KEYMAP_ROWS.len());
        for (label, event, mode) in bindings {
            assert!(
                KEYMAP_ROWS.iter().any(|(key, _)| key == label),
                "{label} is dispatched but missing from the keymap dialog"
            );
            assert!(
                action_for_key(event, *mode, false).is_some(),
                "the keymap dialog shows {label}, but nothing is bound to it"
            );
        }
    }
}
