//! Layout tables. One row per evdev key: (evdev code, key name, symbols).
/// Key type, which decides how shift and lock select a level.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    /// One symbol.
    One,
    /// Shift selects the second symbol.
    Two,
    /// Letters: Shift or Caps Lock selects the second symbol.
    Alphabetic,
    /// Keypad: Num Lock or Shift selects the second symbol.
    Keypad,
    /// Up to four symbols: Shift and AltGr (Level3) select among them.
    Four,
    /// Four symbols where Caps Lock also shifts levels 1 and 2 (letters with an AltGr symbol).
    FourAlpha,
}
pub struct Key {
    pub evdev: u16,
    pub kind: Level,
    /// Keysym names (xkb spelling) for level 1 and 2.
    pub syms: &'static [&'static str],
}
pub struct Layout {
    pub name: &'static str,
    /// Keys not listed in `keys` come from here.
    pub base: Option<&'static Layout>,
    /// This layout's own keys (all of them for a base layout, the changed ones otherwise).
    pub keys: &'static [Key],
}
macro_rules! k {
    ($code:expr, One, $a:expr) => { Key { evdev: $code, kind: Level::One, syms: &[$a] } };
    ($code:expr, $kind:ident, $a:expr, $b:expr) => { Key { evdev: $code, kind: Level::$kind, syms: &[$a, $b] } };
    ($code:expr, $kind:ident, $a:expr, $b:expr, $c:expr) => { Key { evdev: $code, kind: Level::$kind, syms: &[$a, $b, $c] } };
    ($code:expr, $kind:ident, $a:expr, $b:expr, $c:expr, $d:expr) => { Key { evdev: $code, kind: Level::$kind, syms: &[$a, $b, $c, $d] } };
}
pub static US: Layout = Layout { name: "us", base: None, keys: &[
    k!(1, One, "Escape"),
    k!(2, Two, "1", "exclam"), k!(3, Two, "2", "at"), k!(4, Two, "3", "numbersign"), k!(5, Two, "4", "dollar"),
    k!(6, Two, "5", "percent"), k!(7, Two, "6", "asciicircum"), k!(8, Two, "7", "ampersand"), k!(9, Two, "8", "asterisk"),
    k!(10, Two, "9", "parenleft"), k!(11, Two, "0", "parenright"), k!(12, Two, "minus", "underscore"), k!(13, Two, "equal", "plus"),
    k!(14, One, "BackSpace"), k!(15, One, "Tab"),
    k!(16, Alphabetic, "q", "Q"), k!(17, Alphabetic, "w", "W"), k!(18, Alphabetic, "e", "E"), k!(19, Alphabetic, "r", "R"),
    k!(20, Alphabetic, "t", "T"), k!(21, Alphabetic, "y", "Y"), k!(22, Alphabetic, "u", "U"), k!(23, Alphabetic, "i", "I"),
    k!(24, Alphabetic, "o", "O"), k!(25, Alphabetic, "p", "P"),
    k!(26, Two, "bracketleft", "braceleft"), k!(27, Two, "bracketright", "braceright"),
    k!(28, One, "Return"), k!(29, One, "Control_L"),
    k!(30, Alphabetic, "a", "A"), k!(31, Alphabetic, "s", "S"), k!(32, Alphabetic, "d", "D"), k!(33, Alphabetic, "f", "F"),
    k!(34, Alphabetic, "g", "G"), k!(35, Alphabetic, "h", "H"), k!(36, Alphabetic, "j", "J"), k!(37, Alphabetic, "k", "K"),
    k!(38, Alphabetic, "l", "L"),
    k!(39, Two, "semicolon", "colon"), k!(40, Two, "apostrophe", "quotedbl"), k!(41, Two, "grave", "asciitilde"),
    k!(42, One, "Shift_L"), k!(43, Two, "backslash", "bar"),
    k!(44, Alphabetic, "z", "Z"), k!(45, Alphabetic, "x", "X"), k!(46, Alphabetic, "c", "C"), k!(47, Alphabetic, "v", "V"),
    k!(48, Alphabetic, "b", "B"), k!(49, Alphabetic, "n", "N"), k!(50, Alphabetic, "m", "M"),
    k!(51, Two, "comma", "less"), k!(52, Two, "period", "greater"), k!(53, Two, "slash", "question"),
    k!(54, One, "Shift_R"), k!(55, One, "KP_Multiply"), k!(56, One, "Alt_L"), k!(57, One, "space"), k!(58, One, "Caps_Lock"),
    k!(59, One, "F1"), k!(60, One, "F2"), k!(61, One, "F3"), k!(62, One, "F4"), k!(63, One, "F5"),
    k!(64, One, "F6"), k!(65, One, "F7"), k!(66, One, "F8"), k!(67, One, "F9"), k!(68, One, "F10"),
    k!(69, One, "Num_Lock"), k!(70, One, "Scroll_Lock"),
    k!(71, Keypad, "KP_Home", "KP_7"), k!(72, Keypad, "KP_Up", "KP_8"), k!(73, Keypad, "KP_Prior", "KP_9"),
    k!(74, One, "KP_Subtract"),
    k!(75, Keypad, "KP_Left", "KP_4"), k!(76, Keypad, "KP_Begin", "KP_5"), k!(77, Keypad, "KP_Right", "KP_6"),
    k!(78, One, "KP_Add"),
    k!(79, Keypad, "KP_End", "KP_1"), k!(80, Keypad, "KP_Down", "KP_2"), k!(81, Keypad, "KP_Next", "KP_3"),
    k!(82, Keypad, "KP_Insert", "KP_0"), k!(83, Keypad, "KP_Delete", "KP_Decimal"),
    k!(87, One, "F11"), k!(88, One, "F12"),
    k!(96, One, "KP_Enter"), k!(97, One, "Control_R"), k!(98, One, "KP_Divide"), k!(99, One, "Print"),
    k!(100, One, "Alt_R"), k!(102, One, "Home"), k!(103, One, "Up"), k!(104, One, "Prior"),
    k!(105, One, "Left"), k!(106, One, "Right"), k!(107, One, "End"), k!(108, One, "Down"), k!(109, One, "Next"),
    k!(110, One, "Insert"), k!(111, One, "Delete"), k!(119, One, "Pause"),
    k!(125, One, "Super_L"), k!(126, One, "Super_R"), k!(127, One, "Menu"),
] };

impl Layout {
    pub fn key(&self, evdev: u16) -> Option<&Key> {
        self.keys.iter().find(|k| k.evdev == evdev).or_else(|| self.base.and_then(|b| b.key(evdev)))
    }
    /// Every key of the layout (own and inherited), by evdev code.
    pub fn all_keys(&self) -> Vec<&Key> {
        let mut out: Vec<&Key> = self.keys.iter().collect();
        if let Some(b) = self.base {
            for k in b.all_keys() { if !self.keys.iter().any(|o| o.evdev == k.evdev) { out.push(k); } }
        }
        out.sort_by_key(|k| k.evdev);
        out
    }
    /// Keysym name for a key at a level (0-based), if it has one.
    pub fn sym(&self, evdev: u16, level: usize) -> Option<&'static str> { self.key(evdev)?.syms.get(level).copied() }
    /// The text a key types for the given modifier state, `None` for non-text keys (and dead keys).
    pub fn char_for(&self, evdev: u16, shift: bool, caps: bool, numlock: bool, altgr: bool) -> Option<char> {
        let key = self.key(evdev)?;
        let level = match key.kind {
            Level::One => 0,
            Level::Two => shift as usize,
            Level::Alphabetic => (shift != caps) as usize,
            Level::Keypad => (shift != numlock) as usize,
            Level::Four => altgr as usize * 2 + shift as usize,
            Level::FourAlpha => altgr as usize * 2 + (shift != (caps && !altgr)) as usize,
        };
        // A missing level falls back to the first, as xkb does for short symbol lists.
        let name = key.syms.get(level).or(key.syms.first())?;
        sym_char(name)
    }
}
/// Character a keysym name types, for the names this layout uses.
pub fn sym_char(name: &str) -> Option<char> {
    let mut it = name.chars();
    if let (Some(c), None) = (it.next(), it.next()) { return Some(c); }
    Some(match name {
        "exclam" => '!', "at" => '@', "numbersign" => '#', "dollar" => '$', "percent" => '%', "asciicircum" => '^',
        "ampersand" => '&', "asterisk" => '*', "parenleft" => '(', "parenright" => ')', "minus" => '-', "underscore" => '_',
        "equal" => '=', "plus" => '+', "bracketleft" => '[', "braceleft" => '{', "bracketright" => ']', "braceright" => '}',
        "semicolon" => ';', "colon" => ':', "apostrophe" => '\'', "quotedbl" => '"', "grave" => '`', "asciitilde" => '~',
        "backslash" => '\\', "bar" => '|', "comma" => ',', "less" => '<', "period" => '.', "greater" => '>', "slash" => '/',
        "question" => '?', "space" => ' ', "Tab" => '\t', "Return" | "KP_Enter" => '\r', "BackSpace" => '\u{8}',
        "adiaeresis" => 'ä', "Adiaeresis" => 'Ä', "odiaeresis" => 'ö', "Odiaeresis" => 'Ö', "udiaeresis" => 'ü', "Udiaeresis" => 'Ü', "ssharp" => 'ß',
        "eacute" => 'é', "egrave" => 'è', "ccedilla" => 'ç', "agrave" => 'à', "ugrave" => 'ù', "degree" => '°', "section" => '§',
        "sterling" => '£', "EuroSign" => '€', "mu" => 'µ', "twosuperior" => '²', "threesuperior" => '³', "currency" => '¤', "notsign" => '¬',
        "KP_Multiply" => '*', "KP_Subtract" => '-', "KP_Add" => '+', "KP_Divide" => '/', "KP_Decimal" => '.',
        n if n.starts_with("KP_") && n.len() == 4 && n.as_bytes()[3].is_ascii_digit() => n.chars().last().unwrap(),
        _ => return None,
    })
}

/// The extra ISO key left of Z (evdev 86) that US boards lack.
const ISO: Key = k!(86, Four, "less", "greater", "bar");
const ALTGR: Key = k!(100, One, "ISO_Level3_Shift");

pub static GB: Layout = Layout { name: "gb", base: Some(&US), keys: &[
    k!(41, Four, "grave", "notsign", "bar"),
    k!(3, Two, "2", "quotedbl"), k!(4, Two, "3", "sterling"), k!(5, Four, "4", "dollar", "EuroSign"),
    k!(40, Two, "apostrophe", "at"), k!(43, Two, "numbersign", "asciitilde"),
    k!(86, Two, "backslash", "bar"),
] };

pub static DE: Layout = Layout { name: "de", base: Some(&US), keys: &[
    k!(41, Two, "dead_circumflex", "degree"),
    k!(3, Four, "2", "quotedbl", "twosuperior"), k!(4, Four, "3", "section", "threesuperior"),
    k!(6, Two, "5", "percent"), k!(7, Two, "6", "ampersand"),
    k!(8, Four, "7", "slash", "braceleft"), k!(9, Four, "8", "parenleft", "bracketleft"),
    k!(10, Four, "9", "parenright", "bracketright"), k!(11, Four, "0", "equal", "braceright"),
    k!(12, Four, "ssharp", "question", "backslash"), k!(13, Two, "dead_acute", "dead_grave"),
    k!(16, FourAlpha, "q", "Q", "at"), k!(18, FourAlpha, "e", "E", "EuroSign"), k!(21, Alphabetic, "z", "Z"),
    k!(26, Alphabetic, "udiaeresis", "Udiaeresis"), k!(27, Four, "plus", "asterisk", "asciitilde"),
    k!(39, Alphabetic, "odiaeresis", "Odiaeresis"), k!(40, Alphabetic, "adiaeresis", "Adiaeresis"),
    k!(43, Two, "numbersign", "apostrophe"),
    k!(44, Alphabetic, "y", "Y"), k!(50, FourAlpha, "m", "M", "mu"),
    k!(51, Two, "comma", "semicolon"), k!(52, Two, "period", "colon"), k!(53, Two, "minus", "underscore"),
    ISO, ALTGR,
] };

pub static FR: Layout = Layout { name: "fr", base: Some(&US), keys: &[
    k!(41, One, "twosuperior"),
    k!(2, Two, "ampersand", "1"), k!(3, Four, "eacute", "2", "asciitilde"), k!(4, Four, "quotedbl", "3", "numbersign"),
    k!(5, Four, "apostrophe", "4", "braceleft"), k!(6, Four, "parenleft", "5", "bracketleft"), k!(7, Four, "minus", "6", "bar"),
    k!(8, Four, "egrave", "7", "grave"), k!(9, Four, "underscore", "8", "backslash"), k!(10, Four, "ccedilla", "9", "asciicircum"),
    k!(11, Four, "agrave", "0", "at"), k!(12, Four, "parenright", "degree", "bracketright"), k!(13, Four, "equal", "plus", "braceright"),
    k!(16, Alphabetic, "a", "A"), k!(17, Alphabetic, "z", "Z"), k!(18, FourAlpha, "e", "E", "EuroSign"),
    k!(26, Two, "dead_circumflex", "dead_diaeresis"), k!(27, Four, "dollar", "sterling", "currency"),
    k!(30, Alphabetic, "q", "Q"), k!(39, Alphabetic, "m", "M"), k!(40, Two, "ugrave", "percent"), k!(43, Two, "asterisk", "mu"),
    k!(44, Alphabetic, "w", "W"), k!(50, Two, "comma", "question"), k!(51, Two, "semicolon", "period"),
    k!(52, Two, "colon", "slash"), k!(53, Two, "exclam", "section"),
    ISO, ALTGR,
] };

pub static DVORAK: Layout = Layout { name: "dvorak", base: Some(&US), keys: &[
    k!(12, Two, "bracketleft", "braceleft"), k!(13, Two, "bracketright", "braceright"),
    k!(16, Two, "apostrophe", "quotedbl"), k!(17, Two, "comma", "less"), k!(18, Two, "period", "greater"),
    k!(19, Alphabetic, "p", "P"), k!(20, Alphabetic, "y", "Y"), k!(21, Alphabetic, "f", "F"), k!(22, Alphabetic, "g", "G"),
    k!(23, Alphabetic, "c", "C"), k!(24, Alphabetic, "r", "R"), k!(25, Alphabetic, "l", "L"),
    k!(26, Two, "slash", "question"), k!(27, Two, "equal", "plus"),
    k!(30, Alphabetic, "a", "A"), k!(31, Alphabetic, "o", "O"), k!(32, Alphabetic, "e", "E"), k!(33, Alphabetic, "u", "U"),
    k!(34, Alphabetic, "i", "I"), k!(35, Alphabetic, "d", "D"), k!(36, Alphabetic, "h", "H"), k!(37, Alphabetic, "t", "T"),
    k!(38, Alphabetic, "n", "N"), k!(39, Alphabetic, "s", "S"), k!(40, Two, "minus", "underscore"),
    k!(44, Two, "semicolon", "colon"), k!(45, Alphabetic, "q", "Q"), k!(46, Alphabetic, "j", "J"), k!(47, Alphabetic, "k", "K"),
    k!(48, Alphabetic, "x", "X"), k!(49, Alphabetic, "b", "B"), k!(50, Alphabetic, "m", "M"),
    k!(51, Alphabetic, "w", "W"), k!(52, Alphabetic, "v", "V"), k!(53, Alphabetic, "z", "Z"),
] };

/// Layout by its short name (`us`, `gb`, `de`, `fr`, `dvorak`).
pub fn by_name(name: &str) -> Option<&'static Layout> {
    [&US, &GB, &DE, &FR, &DVORAK].into_iter().find(|l| l.name == name)
}
