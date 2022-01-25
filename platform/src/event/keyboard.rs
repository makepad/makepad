use {
    crate::{
        event::{
            finger::KeyModifiers,
        },
        area::Area,
    },
};


#[derive(Clone, Debug, PartialEq)]
pub struct KeyEvent {
    pub key_code: KeyCode,
    pub is_repeat: bool,
    pub modifiers: KeyModifiers,
    pub time: f64
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyFocusEvent {
    pub prev: Area,
    pub focus: Area,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextInputEvent {
    pub input: String,
    pub replace_last: bool,
    pub was_paste: bool
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextCopyEvent {
    pub response: Option<String>
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
    Return,
    
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
    Scrolllock,
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
    
    Unknown
}

