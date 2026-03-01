use {
    crate::{area::Area, event::finger::KeyModifiers},
    std::cell::RefCell,
    std::rc::Rc,
};

pub use makepad_studio_protocol::{
    CharOffset, FullTextState, ImeAction, ImeActionEvent, KeyCode, KeyEvent, TextInputEvent,
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
}

#[derive(Clone, Debug)]
pub struct KeyFocusEvent {
    pub prev: Area,
    pub focus: Area,
}

#[derive(Clone, Debug)]
pub struct TextClipboardEvent {
    pub response: Rc<RefCell<Option<String>>>,
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
}
