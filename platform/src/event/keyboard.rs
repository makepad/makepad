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

// shortcuts to check for keypresses
impl KeyEvent {
    pub fn backtick(&self) -> bool {if let KeyCode::Backtick = self.key_code {true}else {false}}
    pub fn key_0(&self) -> bool {if let KeyCode::Key0 = self.key_code {true}else {false}}
    pub fn key_1(&self) -> bool {if let KeyCode::Key1 = self.key_code {true}else {false}}
    pub fn key_2(&self) -> bool {if let KeyCode::Key2 = self.key_code {true}else {false}}
    pub fn key_3(&self) -> bool {if let KeyCode::Key3 = self.key_code {true}else {false}}
    pub fn key_4(&self) -> bool {if let KeyCode::Key4 = self.key_code {true}else {false}}
    pub fn key_5(&self) -> bool {if let KeyCode::Key5 = self.key_code {true}else {false}}
    pub fn key_6(&self) -> bool {if let KeyCode::Key6 = self.key_code {true}else {false}}
    pub fn key_7(&self) -> bool {if let KeyCode::Key7 = self.key_code {true}else {false}}
    pub fn key_8(&self) -> bool {if let KeyCode::Key8 = self.key_code {true}else {false}}
    pub fn key_9(&self) -> bool {if let KeyCode::Key9 = self.key_code {true}else {false}}
    pub fn minus(&self) -> bool {if let KeyCode::Minus = self.key_code {true}else {false}}
    pub fn equals(&self) -> bool {if let KeyCode::Equals = self.key_code {true}else {false}}
    
    pub fn backspace(&self) -> bool {if let KeyCode::Backspace = self.key_code {true}else {false}}
    pub fn tab(&self) -> bool {if let KeyCode::Tab = self.key_code {true}else {false}}
    
    pub fn key_q(&self) -> bool {if let KeyCode::KeyQ = self.key_code {true}else {false}}
    pub fn key_w(&self) -> bool {if let KeyCode::KeyW = self.key_code {true}else {false}}
    pub fn key_e(&self) -> bool {if let KeyCode::KeyE = self.key_code {true}else {false}}
    pub fn key_r(&self) -> bool {if let KeyCode::KeyR = self.key_code {true}else {false}}
    pub fn key_t(&self) -> bool {if let KeyCode::KeyT = self.key_code {true}else {false}}
    pub fn key_y(&self) -> bool {if let KeyCode::KeyY = self.key_code {true}else {false}}
    pub fn key_u(&self) -> bool {if let KeyCode::KeyU = self.key_code {true}else {false}}
    pub fn key_i(&self) -> bool {if let KeyCode::KeyI = self.key_code {true}else {false}}
    pub fn key_o(&self) -> bool {if let KeyCode::KeyO = self.key_code {true}else {false}}
    pub fn key_p(&self) -> bool {if let KeyCode::KeyP = self.key_code {true}else {false}}
    pub fn l_bracket(&self) -> bool {if let KeyCode::LBracket = self.key_code {true}else {false}}
    pub fn r_bracket(&self) -> bool {if let KeyCode::RBracket = self.key_code {true}else {false}}
    pub fn return_key(&self) -> bool {if let KeyCode::ReturnKey = self.key_code {true}else {false}}
    
    pub fn key_a(&self) -> bool {if let KeyCode::KeyA = self.key_code {true}else {false}}
    pub fn key_s(&self) -> bool {if let KeyCode::KeyS = self.key_code {true}else {false}}
    pub fn key_d(&self) -> bool {if let KeyCode::KeyD = self.key_code {true}else {false}}
    pub fn key_f(&self) -> bool {if let KeyCode::KeyF = self.key_code {true}else {false}}
    pub fn key_g(&self) -> bool {if let KeyCode::KeyG = self.key_code {true}else {false}}
    pub fn key_h(&self) -> bool {if let KeyCode::KeyH = self.key_code {true}else {false}}
    pub fn key_j(&self) -> bool {if let KeyCode::KeyJ = self.key_code {true}else {false}}
    pub fn key_k(&self) -> bool {if let KeyCode::KeyK = self.key_code {true}else {false}}
    pub fn key_l(&self) -> bool {if let KeyCode::KeyL = self.key_code {true}else {false}}
    pub fn semicolon(&self) -> bool {if let KeyCode::Semicolon = self.key_code {true}else {false}}
    pub fn quote(&self) -> bool {if let KeyCode::Quote = self.key_code {true}else {false}}
    pub fn backslash(&self) -> bool {if let KeyCode::Backslash = self.key_code {true}else {false}}
    
    pub fn key_z(&self) -> bool {if let KeyCode::KeyZ = self.key_code {true}else {false}}
    pub fn key_x(&self) -> bool {if let KeyCode::KeyX = self.key_code {true}else {false}}
    pub fn key_c(&self) -> bool {if let KeyCode::KeyC = self.key_code {true}else {false}}
    pub fn key_v(&self) -> bool {if let KeyCode::KeyV = self.key_code {true}else {false}}
    pub fn key_b(&self) -> bool {if let KeyCode::KeyB = self.key_code {true}else {false}}
    pub fn key_n(&self) -> bool {if let KeyCode::KeyN = self.key_code {true}else {false}}
    pub fn key_m(&self) -> bool {if let KeyCode::KeyM = self.key_code {true}else {false}}
    pub fn comma(&self) -> bool {if let KeyCode::Comma = self.key_code {true}else {false}}
    pub fn period(&self) -> bool {if let KeyCode::Period = self.key_code {true}else {false}}
    pub fn slash(&self) -> bool {if let KeyCode::Slash = self.key_code {true}else {false}}
    
    pub fn control(&self) -> bool {if let KeyCode::Control = self.key_code {true}else {false}}
    pub fn alt(&self) -> bool {if let KeyCode::Alt = self.key_code {true}else {false}}
    pub fn shift(&self) -> bool {if let KeyCode::Shift = self.key_code {true}else {false}}
    pub fn logo(&self) -> bool {if let KeyCode::Logo = self.key_code {true}else {false}}
    
    pub fn mod_control(&self) -> bool {self.modifiers.control}
    pub fn mod_alt(&self) -> bool {self.modifiers.alt}
    pub fn mod_shift(&self) -> bool {self.modifiers.shift}
    pub fn mod_logo(&self) -> bool {self.modifiers.logo}
    
    pub fn space(&self) -> bool {if let KeyCode::Space = self.key_code {true}else {false}}
    pub fn capslock(&self) -> bool {if let KeyCode::Capslock = self.key_code {true}else {false}}
    pub fn f1(&self) -> bool {if let KeyCode::F1 = self.key_code {true}else {false}}
    pub fn f2(&self) -> bool {if let KeyCode::F2 = self.key_code {true}else {false}}
    pub fn f3(&self) -> bool {if let KeyCode::F3 = self.key_code {true}else {false}}
    pub fn f4(&self) -> bool {if let KeyCode::F4 = self.key_code {true}else {false}}
    pub fn f5(&self) -> bool {if let KeyCode::F5 = self.key_code {true}else {false}}
    pub fn f6(&self) -> bool {if let KeyCode::F6 = self.key_code {true}else {false}}
    pub fn f7(&self) -> bool {if let KeyCode::F7 = self.key_code {true}else {false}}
    pub fn f8(&self) -> bool {if let KeyCode::F8 = self.key_code {true}else {false}}
    pub fn f9(&self) -> bool {if let KeyCode::F9 = self.key_code {true}else {false}}
    pub fn f10(&self) -> bool {if let KeyCode::F10 = self.key_code {true}else {false}}
    pub fn f11(&self) -> bool {if let KeyCode::F11 = self.key_code {true}else {false}}
    pub fn f12(&self) -> bool {if let KeyCode::F12 = self.key_code {true}else {false}}
    
    pub fn print_screen(&self) -> bool {if let KeyCode::PrintScreen = self.key_code {true}else {false}}
    pub fn scroll_lock(&self) -> bool {if let KeyCode::ScrollLock = self.key_code {true}else {false}}
    pub fn pause(&self) -> bool {if let KeyCode::Pause = self.key_code {true}else {false}}
    
    pub fn insert(&self) -> bool {if let KeyCode::Insert = self.key_code {true}else {false}}
    pub fn delete(&self) -> bool {if let KeyCode::Delete = self.key_code {true}else {false}}
    pub fn home(&self) -> bool {if let KeyCode::Home = self.key_code {true}else {false}}
    pub fn end(&self) -> bool {if let KeyCode::End = self.key_code {true}else {false}}
    pub fn page_up(&self) -> bool {if let KeyCode::PageUp = self.key_code {true}else {false}}
    pub fn page_down(&self) -> bool {if let KeyCode::PageDown = self.key_code {true}else {false}}
    
    pub fn numpad_0(&self) -> bool {if let KeyCode::Numpad0 = self.key_code {true}else {false}}
    pub fn numpad_1(&self) -> bool {if let KeyCode::Numpad1 = self.key_code {true}else {false}}
    pub fn numpad_2(&self) -> bool {if let KeyCode::Numpad2 = self.key_code {true}else {false}}
    pub fn numpad_3(&self) -> bool {if let KeyCode::Numpad3 = self.key_code {true}else {false}}
    pub fn numpad_4(&self) -> bool {if let KeyCode::Numpad4 = self.key_code {true}else {false}}
    pub fn numpad_5(&self) -> bool {if let KeyCode::Numpad5 = self.key_code {true}else {false}}
    pub fn numpad_6(&self) -> bool {if let KeyCode::Numpad6 = self.key_code {true}else {false}}
    pub fn numpad_7(&self) -> bool {if let KeyCode::Numpad7 = self.key_code {true}else {false}}
    pub fn numpad_8(&self) -> bool {if let KeyCode::Numpad8 = self.key_code {true}else {false}}
    pub fn numpad_9(&self) -> bool {if let KeyCode::Numpad9 = self.key_code {true}else {false}}
    
    pub fn numpad_equals(&self) -> bool {if let KeyCode::NumpadEquals = self.key_code {true}else {false}}
    pub fn numpad_subtract(&self) -> bool {if let KeyCode::NumpadSubtract = self.key_code {true}else {false}}
    pub fn numpad_add(&self) -> bool {if let KeyCode::NumpadAdd = self.key_code {true}else {false}}
    pub fn numpad_decimal(&self) -> bool {if let KeyCode::NumpadDecimal = self.key_code {true}else {false}}
    pub fn numpad_multiply(&self) -> bool {if let KeyCode::NumpadMultiply = self.key_code {true}else {false}}
    pub fn numpad_divide(&self) -> bool {if let KeyCode::NumpadDivide = self.key_code {true}else {false}}
    pub fn numpad_lock(&self) -> bool {if let KeyCode::Numlock = self.key_code {true}else {false}}
    pub fn numpad_enter(&self) -> bool {if let KeyCode::NumpadEnter = self.key_code {true}else {false}}
    
    pub fn arrow_up(&self) -> bool {if let KeyCode::ArrowUp = self.key_code {true}else {false}}
    pub fn arrow_down(&self) -> bool {if let KeyCode::ArrowDown = self.key_code {true}else {false}}
    pub fn arrow_left(&self) -> bool {if let KeyCode::ArrowLeft = self.key_code {true}else {false}}
    pub fn arrow_right(&self) -> bool {if let KeyCode::ArrowRight = self.key_code {true}else {false}}
    
    pub fn unknown(&self) -> bool {if let KeyCode::Unknown = self.key_code {true}else {false}}
    
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
    
    Unknown
}

