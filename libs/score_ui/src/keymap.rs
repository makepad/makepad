use crate::{ProductMode, ScoreAction, ScoreTool};
use makepad_widgets::*;

/// Stable, discoverable product keymap. Bare note-entry keys are active only
/// in editor mode; global transport/navigation remains useful in pianist mode.
///
/// `tool` gates the operations that change music. Transpose and delete are
/// real edits, so they answer to the keyboard only where a pointer could have
/// made them too: never under Navigate, which is the mode the application
/// rests in.
pub fn action_for_key(
    event: &KeyEvent,
    mode: ProductMode,
    tool: ScoreTool,
    text_focused: bool,
) -> Option<ScoreAction> {
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
            KeyCode::KeyL => Some(ScoreAction::OpenDialog(crate::DialogKind::Library)),
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
    // The tools. Single keys, in the conventional spirit: H is the hand that
    // moves the page, V the arrow that chooses, N the one that writes notes.
    // They work in either product mode — arming an editing tool is itself the
    // request to leave the reading face.
    match event.key_code {
        KeyCode::KeyH => return Some(ScoreAction::SetTool(ScoreTool::Navigate)),
        KeyCode::KeyV => return Some(ScoreAction::SetTool(ScoreTool::Select)),
        KeyCode::KeyN => return Some(ScoreAction::SetTool(ScoreTool::Edit)),
        _ => {}
    }
    // Transpose and delete operate on the selection. They are edits, so the
    // safe tool does not answer to them: Navigate can never change the music,
    // by pointer or by key.
    if mode == ProductMode::Editor && tool != ScoreTool::Navigate {
        match event.key_code {
            KeyCode::ArrowUp => {
                return Some(ScoreAction::Transpose(if m.shift { 12 } else { 1 }))
            }
            KeyCode::ArrowDown => {
                return Some(ScoreAction::Transpose(if m.shift { -12 } else { -1 }))
            }
            KeyCode::Backspace | KeyCode::Delete => {
                return Some(ScoreAction::DeleteSelection)
            }
            _ => {}
        }
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
    ("H / V / N", "Navigate / select / edit tool"),
    ("↑ / ↓", "Transpose selection a semitone (⇧ an octave)"),
    ("Delete", "Delete the selection"),
    ("Escape", "Back to Navigate, then clear the selection"),
    ("⌘E", "Pianist / editor mode"),
    ("⌘N", "New score"),
    ("⌘O", "Open…"),
    ("⌘L", "Music library…"),
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
        assert!(action_for_key(&event, ProductMode::Pianist, ScoreTool::Edit, false).is_none());
        assert!(matches!(
            action_for_key(&event, ProductMode::Editor, ScoreTool::Edit, false),
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
            ("H / V / N", key(KeyCode::KeyH, false, false), ProductMode::Pianist),
            ("↑ / ↓", key(KeyCode::ArrowUp, false, false), ProductMode::Editor),
            ("Delete", key(KeyCode::Backspace, false, false), ProductMode::Editor),
            ("Escape", key(KeyCode::Escape, false, false), ProductMode::Pianist),
            ("⌘E", key(KeyCode::KeyE, false, true), ProductMode::Pianist),
            ("⌘N", key(KeyCode::KeyN, false, true), ProductMode::Pianist),
            ("⌘O", key(KeyCode::KeyO, false, true), ProductMode::Pianist),
            ("⌘L", key(KeyCode::KeyL, false, true), ProductMode::Pianist),
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
            // Every row is checked under the tool it is documented for; the
            // editing rows are the two that need one armed.
            let tool = if *mode == ProductMode::Editor {
                ScoreTool::Edit
            } else {
                ScoreTool::Navigate
            };
            assert!(
                action_for_key(event, *mode, tool, false).is_some(),
                "the keymap dialog shows {label}, but nothing is bound to it"
            );
        }
    }

    /// The whole point of the tool split: the mode the application rests in
    /// cannot change the music, by pointer OR by key.
    #[test]
    fn navigate_answers_to_nothing_that_changes_the_music() {
        for code in [KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::Backspace, KeyCode::Delete] {
            let event = key(code, false, false);
            let under_navigate =
                action_for_key(&event, ProductMode::Editor, ScoreTool::Navigate, false);
            assert!(
                !matches!(
                    under_navigate,
                    Some(ScoreAction::Transpose(_)) | Some(ScoreAction::DeleteSelection)
                ),
                "{code:?} must not edit under the Navigate tool, got {under_navigate:?}"
            );
        }
        // With a tool armed they are exactly the operations they promise.
        let up = key(KeyCode::ArrowUp, false, false);
        assert!(matches!(
            action_for_key(&up, ProductMode::Editor, ScoreTool::Select, false),
            Some(ScoreAction::Transpose(1))
        ));
        let octave = key(KeyCode::ArrowUp, true, false);
        assert!(matches!(
            action_for_key(&octave, ProductMode::Editor, ScoreTool::Select, false),
            Some(ScoreAction::Transpose(12))
        ));
        let down = key(KeyCode::ArrowDown, true, false);
        assert!(matches!(
            action_for_key(&down, ProductMode::Editor, ScoreTool::Edit, false),
            Some(ScoreAction::Transpose(-12))
        ));
        // Pianist mode is the reading face and never edits, tool or no tool.
        assert!(!matches!(
            action_for_key(&up, ProductMode::Pianist, ScoreTool::Edit, false),
            Some(ScoreAction::Transpose(_))
        ));
    }

    /// The tool keys are one press away from anywhere, including the reading
    /// face — being stuck in a tool is worse than the accident it prevents.
    #[test]
    fn the_tool_keys_reach_every_tool_from_either_mode() {
        for mode in [ProductMode::Pianist, ProductMode::Editor] {
            for (code, tool) in [
                (KeyCode::KeyH, ScoreTool::Navigate),
                (KeyCode::KeyV, ScoreTool::Select),
                (KeyCode::KeyN, ScoreTool::Edit),
            ] {
                let action = action_for_key(&key(code, false, false), mode, ScoreTool::Edit, false);
                assert!(
                    matches!(action, Some(ScoreAction::SetTool(armed)) if armed == tool),
                    "{code:?} in {mode:?} armed {action:?}"
                );
            }
        }
    }
}
