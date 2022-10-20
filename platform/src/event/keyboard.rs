use {
    std::rc::Rc,
    std::cell::RefCell,
    crate::{
        event::{
            finger::KeyModifiers,
        },
        area::Area,
    },
};


#[derive(Default)]
pub struct CxKeyboard {
    pub (crate) prev_key_focus: Area,
    pub (crate) next_key_focus: Area,
    pub (crate) key_focus: Area,
    pub (crate) keys_down: Vec<KeyEvent>,
}

impl CxKeyboard {
    
    pub fn set_key_focus(&mut self, focus_area: Area) {
        self.next_key_focus = focus_area;
    }
    
    pub fn revert_key_focus(&mut self) {
        self.next_key_focus = self.prev_key_focus;
    }
    
    pub fn has_key_focus(&self, focus_area: Area) -> bool {
        self.key_focus == focus_area
    }
    
    pub (crate) fn update_area(&mut self, old_area: Area, new_area: Area) {
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
    
    pub (crate) fn all_keys_up(&mut self) -> Vec<KeyEvent> {
        let mut keys_down = Vec::new();
        std::mem::swap(&mut keys_down, &mut self.keys_down);
        keys_down
    }
    
    pub (crate) fn cycle_key_focus_changed(&mut self) -> Option<(Area, Area)> {
        if self.next_key_focus != self.key_focus {
            self.prev_key_focus = self.key_focus;
            self.key_focus = self.next_key_focus;
            return Some((self.prev_key_focus, self.key_focus))
        }
        None
    }
    
    pub (crate) fn process_key_down(&mut self, key_event: KeyEvent) {
        if let Some(_) = self.keys_down.iter().position( | k | k.key_code == key_event.key_code) {
            return;
        }
        self.keys_down.push(key_event);
    }
    
    pub (crate) fn process_key_up(&mut self, key_event: KeyEvent) {
        if let Some(pos) = self.keys_down.iter().position( | k | k.key_code == key_event.key_code) {
            self.keys_down.remove(pos);
        }
    }
}

#[derive(Clone, Debug)]
pub struct KeyEvent {
    pub key_code: KeyCode,
    pub is_repeat: bool,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug)]
pub struct KeyFocusEvent {
    pub prev: Area,
    pub focus: Area,
}

#[derive(Clone, Debug)]
pub struct TextInputEvent {
    pub input: String,
    pub replace_last: bool,
    pub was_paste: bool
}

#[derive(Clone, Debug)]
pub struct TextCopyEvent {
    pub response: Rc<RefCell<Option<String>>>
}

impl Default for KeyCode {
    fn default() -> Self {KeyCode::Unknown}
}


// lowest common denominator keymap between desktop and web
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum KeyCode {
    Escape,
    
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
    None
}

