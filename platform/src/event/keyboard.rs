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
    #[allow(dead_code)]
    pub (crate) keys_down: Vec<KeyEvent>,
    pub (crate) text_ime_dismissed: bool
}

impl CxKeyboard {
    
    pub fn set_key_focus(&mut self, focus_area: Area) {
        self.text_ime_dismissed = false;
        self.next_key_focus = focus_area;
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
    /*
    pub (crate) fn all_keys_up(&mut self) -> Vec<KeyEvent> {
        let mut keys_down = Vec::new();
        std::mem::swap(&mut keys_down, &mut self.keys_down);
        keys_down
    }*/
    
    pub (crate) fn cycle_key_focus_changed(&mut self) -> Option<(Area, Area)> {
        if self.next_key_focus != self.key_focus {
            self.prev_key_focus = self.key_focus;
            self.key_focus = self.next_key_focus;
            return Some((self.prev_key_focus, self.key_focus))
        }
        None
    }
    
    #[allow(dead_code)]
    pub (crate) fn process_key_down(&mut self, key_event: KeyEvent) {
        if let Some(_) = self.keys_down.iter().position( | k | k.key_code == key_event.key_code) {
            return;
        }
        self.keys_down.push(key_event);
    }
    
    #[allow(dead_code)]
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
}

impl KeyCode{
    pub fn is_unknown(&self)->bool{
        match self{
            Self::Unknown=>true,
            _=>false
        }
    }
    pub fn to_char(&self, uc:bool)->Option<char>{
        match self {
            KeyCode::KeyA => if uc {Some('A')}else {Some('a')},
            KeyCode::KeyB => if uc {Some('B')}else {Some('b')},
            KeyCode::KeyC => if uc {Some('C')}else {Some('c')},
            KeyCode::KeyD => if uc {Some('D')}else {Some('d')},
            KeyCode::KeyE => if uc {Some('E')}else {Some('e')},
            KeyCode::KeyF => if uc {Some('F')}else {Some('f')},
            KeyCode::KeyG => if uc {Some('G')}else {Some('g')},
            KeyCode::KeyH => if uc {Some('H')}else {Some('h')},
            KeyCode::KeyI => if uc {Some('I')}else {Some('i')},
            KeyCode::KeyJ => if uc {Some('J')}else {Some('j')},
            KeyCode::KeyK => if uc {Some('K')}else {Some('k')},
            KeyCode::KeyL => if uc {Some('L')}else {Some('l')},
            KeyCode::KeyM => if uc {Some('M')}else {Some('m')},
            KeyCode::KeyN => if uc {Some('N')}else {Some('n')},
            KeyCode::KeyO => if uc {Some('O')}else {Some('o')},
            KeyCode::KeyP => if uc {Some('P')}else {Some('p')},
            KeyCode::KeyQ => if uc {Some('Q')}else {Some('q')},
            KeyCode::KeyR => if uc {Some('R')}else {Some('r')},
            KeyCode::KeyS => if uc {Some('S')}else {Some('s')},
            KeyCode::KeyT => if uc {Some('T')}else {Some('t')},
            KeyCode::KeyU => if uc {Some('U')}else {Some('u')},
            KeyCode::KeyV => if uc {Some('V')}else {Some('v')},
            KeyCode::KeyW => if uc {Some('W')}else {Some('w')},
            KeyCode::KeyX => if uc {Some('X')}else {Some('x')},
            KeyCode::KeyY => if uc {Some('Y')}else {Some('y')},
            KeyCode::KeyZ => if uc {Some('Z')}else {Some('z')},
            KeyCode::Key0 => if uc {Some(')')}else {Some('0')},
            KeyCode::Key1 => if uc {Some('!')}else {Some('1')},
            KeyCode::Key2 => if uc {Some('@')}else {Some('2')},
            KeyCode::Key3 => if uc {Some('#')}else {Some('3')},
            KeyCode::Key4 => if uc {Some('$')}else {Some('4')},
            KeyCode::Key5 => if uc {Some('%')}else {Some('5')},
            KeyCode::Key6 => if uc {Some('^')}else {Some('6')},
            KeyCode::Key7 => if uc {Some('&')}else {Some('7')},
            KeyCode::Key8 => if uc {Some('*')}else {Some('8')},
            KeyCode::Key9 => if uc {Some('(')}else {Some('9')},
            KeyCode::Equals => if uc {Some('+')}else {Some('=')},
            KeyCode::Minus => if uc {Some('_')}else {Some('-')},
            KeyCode::RBracket => if uc {Some('{')}else {Some('[')},
            KeyCode::LBracket => if uc {Some('}')}else {Some(']')},
            KeyCode::ReturnKey => Some('\n'),
            KeyCode::Backtick => if uc {Some('~')}else {Some('`')},
            KeyCode::Semicolon => if uc {Some(':')}else {Some(';')},
            KeyCode::Backslash => if uc {Some('|')}else {Some('\\')},
            KeyCode::Comma => if uc {Some('<')}else {Some(',')},
            KeyCode::Slash => if uc {Some('?')}else {Some('/')},
            KeyCode::Period => if uc {Some('>')}else {Some('.')},
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
            _ => None
        }
    }
}
