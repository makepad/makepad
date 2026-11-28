use {
    crate::{area::Area, event::{finger::KeyModifiers, VirtualKeyboardEvent}, window::WindowId},
    std::cell::RefCell,
    std::rc::Rc,
};

pub use crate::clipboard_read::ClipboardWrite;
pub use makepad_studio_protocol::{
    CharOffset, FullTextState, ImeAction, ImeActionEvent, KeyCode, KeyEvent, TextInputEvent,
};

/// The modifier of the text-editing shortcuts (select all, copy, cut, paste, undo, redo).
/// A trait of the platform's, not a method in the studio wire crate: an edit there changes the
/// hosted wire.
pub trait EditModifier {
    /// Whether it is held: Cmd on Apple platforms, where Control in a text field means
    /// something else; Control or the Logo key everywhere else. Logo is the Mac-style key (a
    /// Mac's Cmd reaches a Linux desktop over Desk Link as Logo), and Control keeps working as
    /// it always did. Every other shortcut follows `KeyModifiers::is_primary`.
    fn is_edit_primary(&self) -> bool;
}

impl EditModifier for KeyModifiers {
    fn is_edit_primary(&self) -> bool {
        #[cfg(all(target_vendor = "apple", not(target_arch = "wasm32")))]
        {
            self.logo
        }
        #[cfg(not(all(target_vendor = "apple", not(target_arch = "wasm32"))))]
        {
            self.logo || self.control
        }
    }
}

#[derive(Default)]
pub struct CxKeyboard {
    pub(crate) prev_key_focus: Area,
    pub(crate) next_key_focus: Area,
    pub(crate) key_focus: Area,
    /// Whether the user gave `key_focus` its focus, e.g., with a press or Tab, rather than code.
    pub(crate) is_key_focus_from_user: bool,
    pub(crate) is_prev_key_focus_from_user: bool,
    pub(crate) is_next_key_focus_from_user: bool,
    /// Whether the holder of `key_focus` takes its keys as typed text, as it said with
    /// `set_key_focus_takes_text`. Every focus move clears it: the new holder says it again
    /// from its `Hit::KeyFocus`, however the focus came (a press, Tab, an accessibility
    /// action, a restore, a window getting the keyboard back).
    pub(crate) key_focus_takes_text: bool,
    /// The key focus of each window that lost the keyboard and whether the user gave it, restored
    /// when it gets it back. Kept here, not in the window widget, so a redraw while it is away moves
    /// it like the live focus (update_area): a stale copy would never match the widget's area again.
    pub(crate) parked_key_focus: Vec<(WindowId, Area, bool)>,
    #[allow(dead_code)]
    pub(crate) keys_down: Vec<KeyEvent>,
    pub(crate) text_ime_dismissed: bool,
    /// Whether a hide of the soft keyboard is waiting for the last finger to lift.
    pub(crate) is_text_ime_hide_deferred: bool,
    /// Whether that hide was cancelled while the key focus was changing, see `Cx::cancel_text_ime_hide`.
    pub(crate) is_text_ime_hide_cancelled: bool,
    pub(crate) has_physical_keyboard: bool,
    /// The soft keyboard's height from the latest `VirtualKeyboard` event, or 0 once it's hiding.
    pub(crate) virtual_keyboard_height: f64,
}

impl CxKeyboard {
    pub fn has_physical_keyboard(&self) -> bool {
        self.has_physical_keyboard
    }

    pub fn virtual_keyboard_height(&self) -> f64 {
        self.virtual_keyboard_height
    }

    /// Keeps a `VirtualKeyboard` event's height. Anything that hands one straight to a `KeyboardView`
    /// has to call this too, since that view catches up with this height when it draws.
    pub fn note_virtual_keyboard_event(&mut self, event: &VirtualKeyboardEvent) {
        self.virtual_keyboard_height = match event {
            VirtualKeyboardEvent::WillShow { height, .. } | VirtualKeyboardEvent::DidShow { height, .. } => *height,
            VirtualKeyboardEvent::WillHide { .. } | VirtualKeyboardEvent::DidHide { .. } => 0.0,
        };
    }

    pub fn modifiers(&self) -> KeyModifiers {
        if let Some(key) = self.keys_down.first() {
            key.modifiers
        } else {
            Default::default()
        }
    }

    /// The modifier keys held down now, by their own key events (a copy chord's `TextCopy` and
    /// a paste chord's text come before the chord's key, so its modifiers are known only so).
    pub fn held_modifiers(&self) -> KeyModifiers {
        let mut held = KeyModifiers::default();
        for key in &self.keys_down {
            match key.key_code {
                KeyCode::Shift => held.shift = true,
                KeyCode::Control => held.control = true,
                KeyCode::Alt => held.alt = true,
                KeyCode::Logo => held.logo = true,
                _ => {}
            }
        }
        held
    }

    pub fn set_key_focus(&mut self, focus_area: Area) {
        self.text_ime_dismissed = false;
        self.next_key_focus = focus_area;
        self.is_next_key_focus_from_user = false;
    }

    /// Like [`Self::set_key_focus`], for focus the user gave, e.g., by pressing on `focus_area` or tabbing to it.
    pub fn set_key_focus_from_user(&mut self, focus_area: Area) {
        self.set_key_focus(focus_area);
        self.is_next_key_focus_from_user = true;
    }

    /// Gives `focus_area` the key focus back, as the user's if `is_from_user`,
    /// e.g., where it was before a popup took it.
    pub fn restore_key_focus(&mut self, focus_area: Area, is_from_user: bool) {
        self.set_key_focus(focus_area);
        self.is_next_key_focus_from_user = is_from_user;
    }

    pub fn key_focus(&self) -> Area {
        self.key_focus
    }

    /// The area queued to hold key focus once the current event settles
    /// (equals [`Self::key_focus`] when no focus change is pending).
    pub fn next_key_focus(&self) -> Area {
        self.next_key_focus
    }

    /// Says whether `focus_area`, which holds the key focus, takes its keys as typed text. An area
    /// that does not hold the key focus changes nothing. See [`crate::Cx::set_key_focus_takes_text`].
    pub fn set_key_focus_takes_text(&mut self, focus_area: Area, takes_text: bool) {
        if !focus_area.is_empty() && self.key_focus == focus_area {
            self.key_focus_takes_text = takes_text;
        }
    }

    /// Whether the key focus belongs to a widget that takes its keys as typed text.
    pub fn key_focus_takes_text(&self) -> bool {
        self.key_focus_takes_text
    }

    pub fn revert_key_focus(&mut self) {
        self.next_key_focus = self.prev_key_focus;
        self.is_next_key_focus_from_user = self.is_prev_key_focus_from_user;
    }

    pub fn has_key_focus(&self, focus_area: Area) -> bool {
        self.key_focus == focus_area
    }

    /// Whether `focus_area` has key focus that the user gave it, rather than code.
    pub fn has_key_focus_from_user(&self, focus_area: Area) -> bool {
        // Nothing focused is never the user's, e.g., when a restore finds its area gone.
        !focus_area.is_empty() && self.key_focus == focus_area && self.is_key_focus_from_user
    }

    pub fn set_text_ime_dismissed(&mut self) {
        self.text_ime_dismissed = true;
    }

    pub fn reset_text_ime_dismissed(&mut self) {
        self.text_ime_dismissed = false;
    }

    pub fn park_key_focus(&mut self, window: WindowId) {
        // A second loss of the keyboard while nothing took the focus since keeps what the first
        // parked: a WM that said so twice as a returning app's tile came (its window ready, then
        // its first frame) parked the empty focus over the terminal's, which then came back to
        // nothing and dropped the first keys (11-22).
        if self.next_key_focus.is_empty() && self.parked_key_focus.iter().any(|(w, ..)| *w == window) {
            return;
        }
        self.parked_key_focus.retain(|(w, ..)| *w != window);
        self.parked_key_focus.push((window, self.key_focus, self.is_key_focus_from_user));
        self.set_key_focus(Area::Empty);
    }

    pub fn unpark_key_focus(&mut self, window: WindowId) {
        if let Some(i) = self.parked_key_focus.iter().position(|(w, ..)| *w == window) {
            let (_, area, is_from_user) = self.parked_key_focus.remove(i);
            self.restore_key_focus(area, is_from_user);
        }
    }

    pub(crate) fn update_area(&mut self, old_area: Area, new_area: Area) {
        for (_, parked, _) in self.parked_key_focus.iter_mut() {
            if *parked == old_area {
                *parked = new_area;
            }
        }
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

    pub(crate) fn cycle_key_focus_changed(&mut self) -> Option<(Area, Area)> {
        if self.next_key_focus != self.key_focus {
            self.prev_key_focus = self.key_focus;
            self.is_prev_key_focus_from_user = self.is_key_focus_from_user;
            self.key_focus = self.next_key_focus;
            self.is_key_focus_from_user = self.is_next_key_focus_from_user;
            self.key_focus_takes_text = false;
            return Some((self.prev_key_focus, self.key_focus));
        }
        None
    }

    #[allow(dead_code)]
    pub fn is_key_down(&mut self, key_code: KeyCode) -> bool {
        self.keys_down.iter().any(|k| k.key_code == key_code)
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub(crate) fn process_key_up(&mut self, key_event: KeyEvent) {
        if let Some(pos) = self
            .keys_down
            .iter()
            .position(|k| k.key_code == key_event.key_code)
        {
            self.keys_down.remove(pos);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_physical_keyboard_state(&mut self, connected: bool) {
        self.has_physical_keyboard = connected;
    }

    #[allow(dead_code)]
    pub(crate) fn update_physical_keyboard_state(
        &mut self,
        connected: bool,
    ) -> Option<PhysicalKeyboardEvent> {
        if self.has_physical_keyboard == connected {
            return None;
        }
        self.has_physical_keyboard = connected;
        Some(PhysicalKeyboardEvent { connected })
    }
}

#[derive(Clone, Debug)]
pub struct KeyFocusEvent {
    pub prev: Area,
    pub focus: Area,
}

/// A copy or cut asks the focused widget for its selection: the widget answers in `response`
/// with plain text, or with `answer` for its richer flavours too.
#[derive(Clone, Debug, Default)]
pub struct TextClipboardEvent {
    pub response: Rc<RefCell<Option<String>>>,
    /// The answer's richer flavours (HTML, Markdown) beside `response`'s text, set by `answer`.
    pub rich: Rc<RefCell<Option<ClipboardWrite>>>,
}

impl TextClipboardEvent {
    /// Answers with the selection's text and its richer flavours.
    pub fn answer(&self, write: ClipboardWrite) {
        *self.response.borrow_mut() = Some(write.text.clone());
        *self.rich.borrow_mut() = Some(write);
    }

    /// What the copy puts on the clipboard; `None` (nothing selected) leaves the clipboard as it
    /// is. The richer flavours go along only with the text they were answered with.
    pub fn written(&self) -> Option<ClipboardWrite> {
        let text = self.response.borrow().clone().filter(|t| !t.is_empty())?;
        match self.rich.borrow().clone() {
            Some(write) if write.text == text => Some(write),
            _ => Some(ClipboardWrite::text(text)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalKeyboardEvent {
    pub connected: bool,
}

/// Event for replacing a specific range of text
#[derive(Clone, Debug)]
pub struct TextRangeReplaceEvent {
    /// Start index (in characters, not bytes) of range to replace
    pub start: usize,
    /// End index (in characters, not bytes) of range to replace
    pub end: usize,
    /// Text to insert at the range
    pub text: String,
    /// Text that the platform saw in the replaced range before editing.
    ///
    /// Used to reject or re-anchor stale autocorrection ranges.
    pub replaced_text: Option<String>,
    /// If the range no longer matches, treat this as ordinary text input.
    pub fallback_to_insert: bool,
}

#[cfg(test)]
mod takes_text_tests {
    use super::*;
    use crate::{area::RectArea, draw_list::CxDrawListPool};

    /// A text field, a button and the window's id, on one draw list.
    fn areas() -> (Area, Area, WindowId) {
        let mut pool = CxDrawListPool::default();
        let list = pool.alloc();
        let rect = |rect_id| Area::Rect(RectArea { draw_list_id: list.id(), rect_id, redraw_id: 0 });
        (rect(0), rect(1), WindowId(0, 0))
    }

    /// Moves the key focus the way the platform does between events, then lets the new holder
    /// answer its `Hit::KeyFocus` the way a text field does (the button says nothing).
    fn settle(kb: &mut CxKeyboard, field: Area) -> bool {
        if let Some((_, focus)) = kb.cycle_key_focus_changed() {
            kb.set_key_focus_takes_text(focus, focus == field);
        }
        kb.key_focus_takes_text()
    }

    #[test]
    fn the_flag_belongs_to_the_holder_and_every_focus_move_clears_it() {
        let (field, button, window) = areas();
        let mut kb = CxKeyboard::default();
        kb.set_key_focus_takes_text(field, true);
        assert!(!kb.key_focus_takes_text(), "an area without the key focus cannot say it takes text");
        kb.set_key_focus_from_user(field);
        assert_eq!(kb.next_key_focus(), field, "next_key_focus exposes pending focus before settle");
        assert!(settle(&mut kb, field), "a press on the field: it takes text once it has the focus");
        kb.set_key_focus(field);
        assert!(settle(&mut kb, field), "a plain set_key_focus on the holder moves nothing, so it keeps the flag");
        kb.set_key_focus(button);
        assert!(kb.cycle_key_focus_changed().is_some());
        assert!(!kb.key_focus_takes_text(), "a plain set_key_focus elsewhere clears it, before anyone answers");
        kb.revert_key_focus();
        assert!(settle(&mut kb, field), "a revert hands the field its focus back, and the field says it again");
        kb.set_key_focus_takes_text(field, false);
        assert!(!kb.key_focus_takes_text(), "a holder turned read-only says it takes no text");
        kb.set_key_focus_takes_text(field, true);
        kb.park_key_focus(window);
        assert!(!settle(&mut kb, field), "a window that loses the keyboard leaves nothing focused, and no text");
        kb.park_key_focus(window);
        assert!(!settle(&mut kb, field), "told again that it lost the keyboard: still nothing focused");
        kb.unpark_key_focus(window);
        assert!(settle(&mut kb, field), "getting the keyboard back restores the field, which says it again (the second loss parked nothing over it)");
        kb.park_key_focus(window);
        settle(&mut kb, field);
        kb.set_key_focus(button);
        settle(&mut kb, field);
        kb.park_key_focus(window);
        settle(&mut kb, field);
        kb.unpark_key_focus(window);
        assert!(!settle(&mut kb, field) && kb.key_focus() == button, "a focus given while the window was away is what a second loss parks");
        kb.restore_key_focus(field, true);
        assert!(settle(&mut kb, field));
        kb.restore_key_focus(button, true);
        assert!(!settle(&mut kb, field), "a restore to the button: no text");
        kb.restore_key_focus(field, true);
        assert!(settle(&mut kb, field), "a restore to the field: text");
    }

    #[test]
    fn held_modifiers_come_from_the_modifier_keys_themselves() {
        let key = |key_code| KeyEvent { key_code, ..Default::default() };
        let mut kb = CxKeyboard::default();
        kb.process_key_down(key(KeyCode::Control));
        // the chord's own key reports Control in its modifiers; only the held keys count
        kb.process_key_down(KeyEvent { key_code: KeyCode::KeyC, modifiers: KeyModifiers { control: true, logo: true, ..Default::default() }, ..Default::default() });
        assert_eq!(kb.held_modifiers(), KeyModifiers { control: true, ..Default::default() }, "Control held, nothing else");
        kb.process_key_down(key(KeyCode::Logo));
        kb.process_key_up(key(KeyCode::Control));
        assert_eq!(kb.held_modifiers(), KeyModifiers { logo: true, ..Default::default() }, "Control went up, Logo is held");
        kb.process_key_up(key(KeyCode::Logo));
        kb.process_key_up(key(KeyCode::KeyC));
        assert_eq!(kb.held_modifiers(), KeyModifiers::default(), "nothing held");
    }

    #[test]
    fn edit_shortcuts_take_logo_and_keep_control_off_apple() {
        let logo = KeyModifiers { logo: true, ..Default::default() };
        let control = KeyModifiers { control: true, ..Default::default() };
        let shift_logo = KeyModifiers { shift: true, logo: true, ..Default::default() };
        assert!(logo.is_edit_primary(), "Logo (a Mac's Cmd) is the editing key everywhere");
        assert!(shift_logo.is_edit_primary(), "Shift does not take it away (redo is Shift+Z)");
        assert!(!KeyModifiers::default().is_edit_primary() && !KeyModifiers { alt: true, shift: true, ..Default::default() }.is_edit_primary());
        #[cfg(all(target_vendor = "apple", not(target_arch = "wasm32")))]
        assert!(!control.is_edit_primary(), "on a Mac Control in a text field is not the editing key");
        #[cfg(not(all(target_vendor = "apple", not(target_arch = "wasm32"))))]
        {
            assert!(control.is_edit_primary(), "Control keeps working off Apple");
            assert_eq!(logo.is_primary(), cfg!(target_arch = "wasm32"), "the other shortcuts keep their primary key");
        }
    }
}
