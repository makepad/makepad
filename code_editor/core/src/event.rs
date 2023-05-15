#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Key(KeyEvent),
    Text(TextEvent),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyEvent {
    pub modifiers: KeyModifiers,
    pub code: KeyCode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextEvent {
    pub string: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyModifiers {
    pub command: bool,
    pub shift: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyCode {
    Backspace,
    Enter,
    Left,
    Up,
    Right,
    Down,
    Z
}
