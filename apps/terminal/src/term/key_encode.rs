//! Keyboard -> PTY byte encoding: xterm legacy encoding (incl. application
//! cursor/keypad modes, modifier params, alt-as-esc, DECBKM) plus the Kitty
//! keyboard protocol.
//!
//! Port of ghostty `src/input/key_encode.zig` + `src/input/function_keys.zig`
//! + `src/input/kitty.zig` (flags model).
//!
//! LANE CONTRACT: keep the public types below. Port the legacy encoding
//! completely (this is what shells/TUIs live on) and the kitty protocol
//! for the flag set apps actually request (disambiguate, report_events,
//! report_alternates, report_all, report_associated). Port ghostty's tests.
//!
//! Port notes (differences from the Zig, all deliberate):
//!
//!   * ghostty carries left/right *sides* on its modifier bitmask and a
//!     `macos-option-as-alt` config to decide whether the macOS Option key
//!     is a real alt. Neither exists in this contract: the frontend decides
//!     whether Option means alt when it fills in `KeyMods::alt`, so this
//!     encoder always behaves like ghostty with `macos_option_as_alt = true`
//!     (the non-Darwin path). The one Darwin-only rule that survives is
//!     "command+key encodes no text", kept behind `cfg!(target_os = "macos")`.
//!   * ghostty's `KeyEvent.composing` (dead-key/IME in progress) is not in
//!     this contract; the frontend must not call the encoder mid-composition.
//!   * Legacy F13-F24 emit nothing, exactly as ghostty does today (its
//!     function-key table stops at F12); they do encode under kitty.

/// Physical/logical key identity, following W3C KeyboardEvent.code naming
/// like ghostty `src/input/key.zig` `Key`. Only keys a Makepad app can
/// deliver are included; extend as needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    // Writing system keys are carried via `utf8`/`unshifted_codepoint`,
    // with `KeyA`-style identity for kitty alternates.
    KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL,
    KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX,
    KeyY, KeyZ,
    Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8,
    Digit9,
    Minus, Equal, BracketLeft, BracketRight, Backslash, Semicolon, Quote,
    Backquote, Comma, Period, Slash, IntlBackslash,
    Space, Enter, Tab, Backspace, Escape,
    Insert, Delete, Home, End, PageUp, PageDown,
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
    CapsLock, ScrollLock, NumLock, PrintScreen, Pause, Menu,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24,
    Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7,
    Numpad8, Numpad9, NumpadDecimal, NumpadDivide, NumpadMultiply,
    NumpadSubtract, NumpadAdd, NumpadEnter, NumpadEqual,
    ShiftLeft, ShiftRight, ControlLeft, ControlRight, AltLeft, AltRight,
    MetaLeft, MetaRight,
    Unidentified,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyMods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Release,
    Repeat,
}

/// A key event, normalized (ghostty `input.KeyEvent` essentials).
#[derive(Clone, Debug)]
pub struct KeyEvent {
    pub action: KeyAction,
    pub key: Key,
    pub mods: KeyMods,
    /// Mods already consumed to produce `utf8` (e.g. shift that made '%').
    pub consumed_mods: KeyMods,
    /// The text this key produces, if any (empty when none).
    pub utf8: String,
    /// The codepoint without shift applied, 0 if none (kitty alternates).
    pub unshifted_codepoint: u32,
}

/// Kitty keyboard protocol progressive-enhancement flags (CSI > flags u).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KittyFlags(pub u8);

impl KittyFlags {
    pub const DISAMBIGUATE: u8 = 1;
    pub const REPORT_EVENTS: u8 = 2;
    pub const REPORT_ALTERNATES: u8 = 4;
    pub const REPORT_ALL: u8 = 8;
    pub const REPORT_ASSOCIATED: u8 = 16;

    pub fn has(self, flag: u8) -> bool {
        self.0 & flag != 0
    }
}

/// Terminal state the encoder consults (mirrors ghostty KeyEncoder options).
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyEncodeOptions {
    pub cursor_key_application: bool,
    pub keypad_key_application: bool,
    pub ignore_keypad_with_numlock: bool,
    /// DECSET 1036 (default true): ESC-prefix for alt.
    pub alt_esc_prefix: bool,
    /// xterm modifyOtherKeys state 2 (CSI 27;mod;code~ encoding).
    pub modify_other_keys_state_2: bool,
    pub kitty_flags: KittyFlags,
    /// DECBKM: backspace sends BS (0x08) instead of DEL (0x7f).
    pub backarrow_key_mode: bool,
}

/// Encode a key event to the bytes to write to the PTY. Empty = nothing.
pub fn encode_key(event: &KeyEvent, opts: &KeyEncodeOptions) -> Vec<u8> {
    if opts.kitty_flags.0 != 0 {
        kitty(event, opts)
    } else {
        legacy(event, opts)
    }
}

// ---------------------------------------------------------------------------
// Modifier helpers (ghostty `key_mods.zig`)
// ---------------------------------------------------------------------------

const MOD_SHIFT: u8 = 1;
const MOD_CTRL: u8 = 2;
const MOD_ALT: u8 = 4;
const MOD_SUPER: u8 = 8;
const MOD_CAPS: u8 = 16;
const MOD_NUM: u8 = 32;

impl KeyMods {
    /// Raw bitfield, matching ghostty's packed struct order.
    fn int(self) -> u8 {
        (if self.shift { MOD_SHIFT } else { 0 })
            | (if self.ctrl { MOD_CTRL } else { 0 })
            | (if self.alt { MOD_ALT } else { 0 })
            | (if self.super_ { MOD_SUPER } else { 0 })
            | (if self.caps_lock { MOD_CAPS } else { 0 })
            | (if self.num_lock { MOD_NUM } else { 0 })
    }

    fn from_int(v: u8) -> KeyMods {
        KeyMods {
            shift: v & MOD_SHIFT != 0,
            ctrl: v & MOD_CTRL != 0,
            alt: v & MOD_ALT != 0,
            super_: v & MOD_SUPER != 0,
            caps_lock: v & MOD_CAPS != 0,
            num_lock: v & MOD_NUM != 0,
        }
    }

    /// Only the mods relevant for bindings: drops the lock keys.
    fn binding(self) -> KeyMods {
        KeyMods {
            shift: self.shift,
            ctrl: self.ctrl,
            alt: self.alt,
            super_: self.super_,
            caps_lock: false,
            num_lock: false,
        }
    }

    fn is_empty(self) -> bool {
        self.int() == 0
    }

    /// `self &~ other`
    fn unset(self, other: KeyMods) -> KeyMods {
        KeyMods::from_int(self.int() & !other.int())
    }
}

impl KeyEvent {
    /// The mods that remain after the ones consumed to produce `utf8`.
    fn effective_mods(&self) -> KeyMods {
        if self.utf8.is_empty() {
            return self.mods;
        }
        self.mods.unset(self.consumed_mods)
    }
}

/// True for an ASCII control character (matches libc `iscntrl`).
fn is_control(cp: u32) -> bool {
    cp < 0x20 || cp == 0x7F
}

/// True if this string is exactly one control character.
fn is_control_utf8(s: &str) -> bool {
    s.len() == 1 && is_control(s.as_bytes()[0] as u32)
}

// ---------------------------------------------------------------------------
// PC-style function keys (ghostty `function_keys.zig`)
// ---------------------------------------------------------------------------

/// The modifier combinations for "modify other keys" sequences. The mode
/// value sent on the wire is `index + 2`.
const MODIFIERS: [u8; 15] = [
    MOD_SHIFT,
    MOD_ALT,
    MOD_SHIFT | MOD_ALT,
    MOD_CTRL,
    MOD_SHIFT | MOD_CTRL,
    MOD_ALT | MOD_CTRL,
    MOD_SHIFT | MOD_ALT | MOD_CTRL,
    MOD_SUPER,
    MOD_SHIFT | MOD_SUPER,
    MOD_ALT | MOD_SUPER,
    MOD_SHIFT | MOD_ALT | MOD_SUPER,
    MOD_CTRL | MOD_SUPER,
    MOD_SHIFT | MOD_CTRL | MOD_SUPER,
    MOD_ALT | MOD_CTRL | MOD_SUPER,
    MOD_SHIFT | MOD_ALT | MOD_CTRL | MOD_SUPER,
];

fn modifier_index(mods_int: u8) -> Option<usize> {
    MODIFIERS.iter().position(|m| *m == mods_int)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CursorMode {
    Any,
    Normal,
    Application,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeypadMode {
    Any,
    Application,
}

/// The xterm "modify other keys" state an entry requires. `Set` means the
/// entry applies unless state 2 ("other keys") is active, `SetOther` means
/// it only applies when state 2 is active.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModifyKeys {
    Any,
    Set,
    SetOther,
}

struct FnEntry {
    /// Exact binding-mods bitfield this entry requires.
    mods: u8,
    /// When true (the default) empty `mods` matches *any* mods.
    mods_empty_is_any: bool,
    cursor: CursorMode,
    keypad: KeypadMode,
    modify_other_keys: ModifyKeys,
    sequence: String,
    /// Sequence to send instead when DECBKM is set.
    sequence_decbkm: Option<String>,
}

impl FnEntry {
    fn new(sequence: &str) -> FnEntry {
        FnEntry {
            mods: 0,
            mods_empty_is_any: true,
            cursor: CursorMode::Any,
            keypad: KeypadMode::Any,
            modify_other_keys: ModifyKeys::Any,
            sequence: sequence.to_string(),
            sequence_decbkm: None,
        }
    }
    fn mods(mut self, mods: u8) -> FnEntry {
        self.mods = mods;
        self
    }
    fn cursor(mut self, cursor: CursorMode) -> FnEntry {
        self.cursor = cursor;
        self
    }
    fn keypad(mut self, keypad: KeypadMode) -> FnEntry {
        self.keypad = keypad;
        self
    }
    fn mok(mut self, m: ModifyKeys) -> FnEntry {
        self.modify_other_keys = m;
        self
    }
    fn decbkm(mut self, sequence: &str) -> FnEntry {
        self.sequence_decbkm = Some(sequence.to_string());
        self
    }
    fn empty_is_none(mut self) -> FnEntry {
        self.mods_empty_is_any = false;
        self
    }
}

/// Constructs the set of pc-style function key entries for `fmt`, which must
/// contain exactly one `{}` hole for the mods code.
fn pc_style(fmt: &str) -> Vec<FnEntry> {
    MODIFIERS
        .iter()
        .enumerate()
        .map(|(i, mods)| FnEntry::new(&fmt.replace("{}", &(i + 2).to_string())).mods(*mods))
        .collect()
}

/// Entries that depend on the cursor key mode (DECCKM).
fn cursor_key(normal: &str, application: &str) -> Vec<FnEntry> {
    vec![
        FnEntry::new(normal).cursor(CursorMode::Normal),
        FnEntry::new(application).cursor(CursorMode::Application),
    ]
}

/// Entries for a keypad key; `suffix` is the final byte, e.g. "q" for kp_1.
fn kp_keys(suffix: &str) -> Vec<FnEntry> {
    let mut out = vec![FnEntry::new(&format!("\x1bO{}", suffix))
        .keypad(KeypadMode::Application)
        .empty_is_none()];
    for entry in pc_style(&format!("\x1bO{{}}{}", suffix)) {
        out.push(entry.keypad(KeypadMode::Application));
    }
    out
}

fn cat(mut a: Vec<FnEntry>, b: Vec<FnEntry>) -> Vec<FnEntry> {
    a.extend(b);
    a
}

/// The pc-style function key table (ghostty `function_keys.keys`).
fn function_key_entries(k: Key) -> Vec<FnEntry> {
    match k {
        Key::ArrowUp => cat(pc_style("\x1b[1;{}A"), cursor_key("\x1b[A", "\x1bOA")),
        Key::ArrowDown => cat(pc_style("\x1b[1;{}B"), cursor_key("\x1b[B", "\x1bOB")),
        Key::ArrowRight => cat(pc_style("\x1b[1;{}C"), cursor_key("\x1b[C", "\x1bOC")),
        Key::ArrowLeft => cat(pc_style("\x1b[1;{}D"), cursor_key("\x1b[D", "\x1bOD")),
        Key::Home => cat(pc_style("\x1b[1;{}H"), cursor_key("\x1b[H", "\x1bOH")),
        Key::End => cat(pc_style("\x1b[1;{}F"), cursor_key("\x1b[F", "\x1bOF")),
        Key::Insert => cat(pc_style("\x1b[2;{}~"), vec![FnEntry::new("\x1b[2~")]),
        Key::Delete => cat(pc_style("\x1b[3;{}~"), vec![FnEntry::new("\x1b[3~")]),
        Key::PageUp => cat(pc_style("\x1b[5;{}~"), vec![FnEntry::new("\x1b[5~")]),
        Key::PageDown => cat(pc_style("\x1b[6;{}~"), vec![FnEntry::new("\x1b[6~")]),

        // Function keys. Like ghostty, F13+ has no legacy encoding.
        Key::F1 => cat(pc_style("\x1b[1;{}P"), vec![FnEntry::new("\x1bOP")]),
        Key::F2 => cat(pc_style("\x1b[1;{}Q"), vec![FnEntry::new("\x1bOQ")]),
        Key::F3 => cat(pc_style("\x1b[13;{}~"), vec![FnEntry::new("\x1bOR")]),
        Key::F4 => cat(pc_style("\x1b[1;{}S"), vec![FnEntry::new("\x1bOS")]),
        Key::F5 => cat(pc_style("\x1b[15;{}~"), vec![FnEntry::new("\x1b[15~")]),
        Key::F6 => cat(pc_style("\x1b[17;{}~"), vec![FnEntry::new("\x1b[17~")]),
        Key::F7 => cat(pc_style("\x1b[18;{}~"), vec![FnEntry::new("\x1b[18~")]),
        Key::F8 => cat(pc_style("\x1b[19;{}~"), vec![FnEntry::new("\x1b[19~")]),
        Key::F9 => cat(pc_style("\x1b[20;{}~"), vec![FnEntry::new("\x1b[20~")]),
        Key::F10 => cat(pc_style("\x1b[21;{}~"), vec![FnEntry::new("\x1b[21~")]),
        Key::F11 => cat(pc_style("\x1b[23;{}~"), vec![FnEntry::new("\x1b[23~")]),
        Key::F12 => cat(pc_style("\x1b[24;{}~"), vec![FnEntry::new("\x1b[24~")]),

        // Keypad keys
        Key::Numpad0 => kp_keys("p"),
        Key::Numpad1 => kp_keys("q"),
        Key::Numpad2 => kp_keys("r"),
        Key::Numpad3 => kp_keys("s"),
        Key::Numpad4 => kp_keys("t"),
        Key::Numpad5 => kp_keys("u"),
        Key::Numpad6 => kp_keys("v"),
        Key::Numpad7 => kp_keys("w"),
        Key::Numpad8 => kp_keys("x"),
        Key::Numpad9 => kp_keys("y"),
        Key::NumpadDecimal => kp_keys("n"),
        Key::NumpadDivide => kp_keys("o"),
        Key::NumpadMultiply => kp_keys("j"),
        Key::NumpadSubtract => kp_keys("m"),
        Key::NumpadAdd => kp_keys("k"),
        Key::NumpadEnter => cat(kp_keys("M"), vec![FnEntry::new("\r")]),

        Key::Backspace => vec![
            // Modify Keys Normal
            FnEntry::new("\x7f").mods(MOD_SHIFT).mok(ModifyKeys::Set),
            FnEntry::new("\x1b\x7f").mods(MOD_ALT).mok(ModifyKeys::Set),
            FnEntry::new("\x1b\x7f").mods(MOD_ALT | MOD_SHIFT).mok(ModifyKeys::Set),
            FnEntry::new("\x08").mods(MOD_CTRL | MOD_SHIFT).mok(ModifyKeys::Set),
            FnEntry::new("\x1b\x08").mods(MOD_ALT | MOD_CTRL).mok(ModifyKeys::Set),
            FnEntry::new("\x7f").mods(MOD_SUPER).mok(ModifyKeys::Set),
            FnEntry::new("\x7f").mods(MOD_SUPER | MOD_SHIFT).mok(ModifyKeys::Set),
            FnEntry::new("\x1b\x7f").mods(MOD_ALT | MOD_SUPER).mok(ModifyKeys::Set),
            FnEntry::new("\x1b\x7f")
                .mods(MOD_ALT | MOD_SUPER | MOD_SHIFT)
                .mok(ModifyKeys::Set),
            FnEntry::new("\x08").mods(MOD_SUPER | MOD_CTRL).mok(ModifyKeys::Set),
            FnEntry::new("\x08")
                .mods(MOD_SUPER | MOD_CTRL | MOD_SHIFT)
                .mok(ModifyKeys::Set),
            FnEntry::new("\x1b\x08")
                .mods(MOD_ALT | MOD_SUPER | MOD_CTRL)
                .mok(ModifyKeys::Set),
            FnEntry::new("\x1b\x08")
                .mods(MOD_ALT | MOD_SUPER | MOD_CTRL | MOD_SHIFT)
                .mok(ModifyKeys::Set),
            // Modify Keys Other
            FnEntry::new("\x1b[27;2;127~").mods(MOD_SHIFT).mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;3;127~").mods(MOD_ALT).mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;4;127~")
                .mods(MOD_ALT | MOD_SHIFT)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;6;127~")
                .mods(MOD_CTRL | MOD_SHIFT)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;7;127~")
                .mods(MOD_ALT | MOD_CTRL)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;8;127~")
                .mods(MOD_ALT | MOD_SHIFT | MOD_CTRL)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;9;127~").mods(MOD_SUPER).mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;10;127~")
                .mods(MOD_SUPER | MOD_SHIFT)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;11;127~")
                .mods(MOD_ALT | MOD_SUPER)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;12;127~")
                .mods(MOD_ALT | MOD_SUPER | MOD_SHIFT)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;13;127~")
                .mods(MOD_SUPER | MOD_CTRL)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;14;127~")
                .mods(MOD_SUPER | MOD_CTRL | MOD_SHIFT)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;15;127~")
                .mods(MOD_ALT | MOD_SUPER | MOD_CTRL)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;16;127~")
                .mods(MOD_ALT | MOD_SUPER | MOD_CTRL | MOD_SHIFT)
                .mok(ModifyKeys::SetOther),
            FnEntry::new("\x08").mods(MOD_CTRL).decbkm("\x7f"),
            FnEntry::new("\x7f").decbkm("\x08"),
        ],

        Key::Tab => vec![
            // Modify Keys Normal
            FnEntry::new("\x1b[Z").mods(MOD_SHIFT).mok(ModifyKeys::Set),
            FnEntry::new("\x1b\t").mods(MOD_ALT).mok(ModifyKeys::Set),
            // Modify Keys Other
            FnEntry::new("\x1b[27;2;9~").mods(MOD_SHIFT).mok(ModifyKeys::SetOther),
            FnEntry::new("\x1b[27;3;9~").mods(MOD_ALT).mok(ModifyKeys::SetOther),
            // Everything else
            FnEntry::new("\x1b[27;4;9~").mods(MOD_ALT | MOD_SHIFT),
            FnEntry::new("\x1b[27;5;9~").mods(MOD_CTRL),
            FnEntry::new("\x1b[27;6;9~").mods(MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b[27;7;9~").mods(MOD_ALT | MOD_CTRL),
            FnEntry::new("\x1b[27;8;9~").mods(MOD_ALT | MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b[27;9;9~").mods(MOD_SUPER),
            FnEntry::new("\x1b[27;10;9~").mods(MOD_SUPER | MOD_SHIFT),
            FnEntry::new("\x1b[27;11;9~").mods(MOD_ALT | MOD_SUPER),
            FnEntry::new("\x1b[27;12;9~").mods(MOD_ALT | MOD_SUPER | MOD_SHIFT),
            FnEntry::new("\x1b[27;13;9~").mods(MOD_SUPER | MOD_CTRL),
            FnEntry::new("\x1b[27;14;9~").mods(MOD_SUPER | MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b[27;15;9~").mods(MOD_ALT | MOD_SUPER | MOD_CTRL),
            FnEntry::new("\x1b[27;16;9~").mods(MOD_ALT | MOD_SUPER | MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\t"),
        ],

        Key::Enter => vec![
            FnEntry::new("\x1b[27;2;13~").mods(MOD_SHIFT),
            // Modify Keys Normal
            FnEntry::new("\x1b\r").mods(MOD_ALT).mok(ModifyKeys::Set),
            // Modify Keys Other
            FnEntry::new("\x1b[27;3;13~").mods(MOD_ALT).mok(ModifyKeys::SetOther),
            // Everything else
            FnEntry::new("\x1b[27;4;13~").mods(MOD_ALT | MOD_SHIFT),
            FnEntry::new("\x1b[27;5;13~").mods(MOD_CTRL),
            FnEntry::new("\x1b[27;6;13~").mods(MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b[27;7;13~").mods(MOD_ALT | MOD_CTRL),
            FnEntry::new("\x1b[27;8;13~").mods(MOD_ALT | MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b[27;9;13~").mods(MOD_SUPER),
            FnEntry::new("\x1b[27;10;13~").mods(MOD_SUPER | MOD_SHIFT),
            FnEntry::new("\x1b[27;11;13~").mods(MOD_ALT | MOD_SUPER),
            FnEntry::new("\x1b[27;12;13~").mods(MOD_ALT | MOD_SUPER | MOD_SHIFT),
            FnEntry::new("\x1b[27;13;13~").mods(MOD_SUPER | MOD_CTRL),
            FnEntry::new("\x1b[27;14;13~").mods(MOD_SUPER | MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b[27;15;13~").mods(MOD_ALT | MOD_SUPER | MOD_CTRL),
            FnEntry::new("\x1b[27;16;13~").mods(MOD_ALT | MOD_SUPER | MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\r"),
        ],

        Key::Escape => vec![
            FnEntry::new("\x1b[27;2;27~").mods(MOD_SHIFT),
            FnEntry::new("\x1b\x1b").mods(MOD_ALT),
            FnEntry::new("\x1b[27;4;27~").mods(MOD_ALT | MOD_SHIFT),
            FnEntry::new("\x1b[27;5;27~").mods(MOD_CTRL),
            FnEntry::new("\x1b[27;6;27~").mods(MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b[27;7;27~").mods(MOD_ALT | MOD_CTRL),
            FnEntry::new("\x1b[27;8;27~").mods(MOD_ALT | MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b[27;9;27~").mods(MOD_SUPER),
            FnEntry::new("\x1b[27;10;27~").mods(MOD_SUPER | MOD_SHIFT),
            FnEntry::new("\x1b[27;11;27~").mods(MOD_ALT | MOD_SUPER),
            FnEntry::new("\x1b[27;12;27~").mods(MOD_ALT | MOD_SUPER | MOD_SHIFT),
            FnEntry::new("\x1b[27;13;27~").mods(MOD_SUPER | MOD_CTRL),
            FnEntry::new("\x1b[27;14;27~").mods(MOD_SUPER | MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b[27;15;27~").mods(MOD_ALT | MOD_SUPER | MOD_CTRL),
            FnEntry::new("\x1b[27;16;27~").mods(MOD_ALT | MOD_SUPER | MOD_CTRL | MOD_SHIFT),
            FnEntry::new("\x1b"),
        ],

        _ => Vec::new(),
    }
}

/// Whether this key encodes in xterm's "PC-style Function Key" syntax, and
/// with what bytes. Port of ghostty `pcStyleFunctionKey`.
fn pc_style_function_key(
    keyval: Key,
    mods: KeyMods,
    cursor_key_application: bool,
    keypad_key_application_req: bool,
    ignore_keypad_with_numlock: bool,
    modify_other_keys: bool,
    backarrow_key_mode: bool,
) -> Option<String> {
    // Lock keys and modifier sides never matter for pc-style keys.
    let mods_int = mods.binding().int();

    // Mode 1035 (default set) means numlock never puts us in application
    // keypad mode; with it reset we honor the requested state.
    let keypad_key_application = if ignore_keypad_with_numlock {
        false
    } else {
        keypad_key_application_req
    };

    for entry in function_key_entries(keyval) {
        match entry.cursor {
            CursorMode::Any => {}
            CursorMode::Normal => {
                if cursor_key_application {
                    continue;
                }
            }
            CursorMode::Application => {
                if !cursor_key_application {
                    continue;
                }
            }
        }

        match entry.keypad {
            KeypadMode::Any => {}
            KeypadMode::Application => {
                if !keypad_key_application {
                    continue;
                }
            }
        }

        match entry.modify_other_keys {
            ModifyKeys::Any => {}
            ModifyKeys::Set => {
                if modify_other_keys {
                    continue;
                }
            }
            ModifyKeys::SetOther => {
                if !modify_other_keys {
                    continue;
                }
            }
        }

        if entry.mods == 0 {
            // Mods are either empty, or empty means any, so we allow it.
            if mods_int != 0 && !entry.mods_empty_is_any {
                continue;
            }
        } else if entry.mods != mods_int {
            // Any set mods require an exact match.
            continue;
        }

        if backarrow_key_mode {
            if let Some(seq) = entry.sequence_decbkm {
                return Some(seq);
            }
        }

        return Some(entry.sequence);
    }

    None
}

// ---------------------------------------------------------------------------
// Key codepoints (ghostty `Key.codepoint`)
// ---------------------------------------------------------------------------

/// The codepoint this key produces on a US layout, or None if not printable.
fn key_codepoint(k: Key) -> Option<u32> {
    let cp = match k {
        Key::KeyA => 'a',
        Key::KeyB => 'b',
        Key::KeyC => 'c',
        Key::KeyD => 'd',
        Key::KeyE => 'e',
        Key::KeyF => 'f',
        Key::KeyG => 'g',
        Key::KeyH => 'h',
        Key::KeyI => 'i',
        Key::KeyJ => 'j',
        Key::KeyK => 'k',
        Key::KeyL => 'l',
        Key::KeyM => 'm',
        Key::KeyN => 'n',
        Key::KeyO => 'o',
        Key::KeyP => 'p',
        Key::KeyQ => 'q',
        Key::KeyR => 'r',
        Key::KeyS => 's',
        Key::KeyT => 't',
        Key::KeyU => 'u',
        Key::KeyV => 'v',
        Key::KeyW => 'w',
        Key::KeyX => 'x',
        Key::KeyY => 'y',
        Key::KeyZ => 'z',
        Key::Digit0 => '0',
        Key::Digit1 => '1',
        Key::Digit2 => '2',
        Key::Digit3 => '3',
        Key::Digit4 => '4',
        Key::Digit5 => '5',
        Key::Digit6 => '6',
        Key::Digit7 => '7',
        Key::Digit8 => '8',
        Key::Digit9 => '9',
        Key::Semicolon => ';',
        Key::Space => ' ',
        Key::Quote => '\'',
        Key::Comma => ',',
        Key::Backquote => '`',
        Key::Period => '.',
        Key::Slash => '/',
        Key::Minus => '-',
        Key::Equal => '=',
        Key::BracketLeft => '[',
        Key::BracketRight => ']',
        Key::Backslash => '\\',
        Key::Tab => '\t',
        Key::Numpad0 => '0',
        Key::Numpad1 => '1',
        Key::Numpad2 => '2',
        Key::Numpad3 => '3',
        Key::Numpad4 => '4',
        Key::Numpad5 => '5',
        Key::Numpad6 => '6',
        Key::Numpad7 => '7',
        Key::Numpad8 => '8',
        Key::Numpad9 => '9',
        Key::NumpadDecimal => '.',
        Key::NumpadDivide => '/',
        Key::NumpadMultiply => '*',
        Key::NumpadSubtract => '-',
        Key::NumpadAdd => '+',
        Key::NumpadEqual => '=',
        _ => return None,
    };
    Some(cp as u32)
}

// ---------------------------------------------------------------------------
// Legacy encoding
// ---------------------------------------------------------------------------

/// Legacy encoding: traditional terminal behavior plus xterm's
/// `modifyOtherKeys` plus the "fixterms" CSI u spec.
fn legacy(event: &KeyEvent, opts: &KeyEncodeOptions) -> Vec<u8> {
    let all_mods = event.mods;
    let binding_mods = event.effective_mods().binding();

    // Legacy encoding only does press/repeat.
    if !matches!(event.action, KeyAction::Press | KeyAction::Repeat) {
        return Vec::new();
    }

    // If we match a PC style function key then that is our result.
    if let Some(sequence) = pc_style_function_key(
        event.key,
        all_mods,
        opts.cursor_key_application,
        opts.keypad_key_application,
        opts.ignore_keypad_with_numlock,
        opts.modify_other_keys_state_2,
        opts.backarrow_key_mode,
    ) {
        // If we have UTF-8 text then we never emit PC style function keys:
        // escape/enter/backspace all have a dead-key meaning we must not
        // stomp (escape clears/commits IME state, backspace deletes one
        // preedit char, ...). Control characters are excluded because some
        // frontends deliver those as UTF-8 text.
        let mut fall_through = false;
        if !event.utf8.is_empty() && !is_control_utf8(&event.utf8) {
            match event.key {
                // Backspace encodes nothing because we modified the IME.
                Key::Backspace => return Vec::new(),
                // Enter/escape encode the committed text instead.
                Key::Enter | Key::Escape => fall_through = true,
                _ => {}
            }
        }
        if !fall_through {
            return sequence.into_bytes();
        }
    }

    // If we match a control sequence, output it directly. ctrlSeq uses all
    // mods because we want it to only match ctrl+<char>.
    if let Some(ch) = ctrl_seq(event.key, &event.utf8, event.unshifted_codepoint, all_mods) {
        // C0 sequences support alt-as-esc prefixing.
        if binding_mods.alt {
            return vec![0x1B, ch];
        }
        return vec![ch];
    }

    // With no UTF-8 text the only possibility left is alt-prefixing an
    // unshifted codepoint.
    if event.utf8.is_empty() {
        if let Some(byte) = legacy_alt_prefix(event, binding_mods, opts) {
            return vec![0x1B, byte];
        }
        return Vec::new();
    }

    // In modify other keys state 2 we send the CSI 27 sequence for any char
    // with a modifier. Ctrl sequences like ctrl+a are handled above.
    if opts.modify_other_keys_state_2 {
        if let Some(out) = modify_other_keys(event) {
            return out;
        }
    }

    // Apply fixterms to this codepoint. At this stage we only need to do so
    // if ctrl is set.
    if event.mods.ctrl {
        if let Some(out) = csi_u(event) {
            return out;
        }
    }

    // Alt-prefix the utf8 sequence if alt-esc-prefix is enabled.
    if let Some(byte) = legacy_alt_prefix(event, binding_mods, opts) {
        return vec![0x1B, byte];
    }

    // On macOS, command+key never encodes text: it doesn't in native text
    // inputs and it doesn't in other native terminals. On Linux we keep
    // encoding it because that is what GTK terminals do.
    if cfg!(target_os = "macos") && all_mods.super_ {
        return Vec::new();
    }

    event.utf8.as_bytes().to_vec()
}

fn legacy_alt_prefix(
    event: &KeyEvent,
    binding_mods: KeyMods,
    opts: &KeyEncodeOptions,
) -> Option<u8> {
    // This only takes effect with alt pressed.
    if !binding_mods.alt || !opts.alt_esc_prefix {
        return None;
    }

    // We require the utf8 to already have the byte represented.
    let utf8 = event.utf8.as_bytes();
    if utf8.len() == 1 {
        return Some(utf8[0]);
    }

    // If utf8 isn't set, allow unshifted codepoints through.
    if event.unshifted_codepoint > 0 && event.unshifted_codepoint <= 0xFF {
        return Some(event.unshifted_codepoint as u8);
    }

    None
}

/// xterm modifyOtherKeys state 2: `CSI 27 ; mods ; codepoint ~`.
fn modify_other_keys(event: &KeyEvent) -> Option<Vec<u8>> {
    // We only do this if we have a single codepoint.
    let mut it = event.utf8.chars();
    let codepoint = it.next()? as u32;
    if it.next().is_some() {
        return None;
    }

    // The mods we encode are just the binding mods.
    let mods = event.mods.binding();

    // This copies xterm's `ModifyOtherKeys` function that returns whether
    // modify other keys should be encoded for the given input.
    let should_modify = if (0x40..=0x7F).contains(&codepoint) {
        // xterm IsControlInput
        true
    } else {
        let mut mods_no_shift = mods;
        mods_no_shift.shift = false;
        if !mods_no_shift.is_empty() {
            // Anything other than shift pressed: encode.
            true
        } else {
            // Only shift pressed: we only allow space.
            codepoint == ' ' as u32
        }
    };

    if !should_modify {
        return None;
    }

    let code = modifier_index(mods.int())? + 2;
    Some(format!("\x1b[27;{};{}~", code, codepoint).into_bytes())
}

/// This is the bitmask for fixterm CSI u modifiers.
#[derive(Clone, Copy, Default)]
struct CsiUMods {
    shift: bool,
    alt: bool,
    ctrl: bool,
}

impl CsiUMods {
    fn from_input(mods: KeyMods) -> CsiUMods {
        CsiUMods {
            shift: mods.shift,
            alt: mods.alt,
            ctrl: mods.ctrl,
        }
    }

    fn int(self) -> u8 {
        (if self.shift { 1 } else { 0 })
            | (if self.alt { 2 } else { 0 })
            | (if self.ctrl { 4 } else { 0 })
    }

    /// The integer sent as part of the CSI u sequence: bitmask + 1.
    fn seq_int(self) -> u8 {
        self.int() + 1
    }
}

fn csi_u(event: &KeyEvent) -> Option<Vec<u8>> {
    // Important: we use the original mods here, not the effective mods. The
    // fixterms spec states shifted chars should be sent uppercase but Kitty
    // changes that behavior, so we send all the mods.
    let mut mods = CsiUMods::from_input(event.mods);

    // More than one codepoint can't be valid CSIu.
    let mut it = event.utf8.chars();
    let mut char_ = it.next()? as u32;
    if it.next().is_some() {
        return None;
    }

    // If our character is A to Z and we have shift set then we lowercase it.
    // This is Kitty-specific behavior that we follow, diverging from the
    // fixterms spec: it makes it easier for programs to detect shifted
    // letters for keybindings.
    if (0x41..=0x5A).contains(&char_) && mods.shift {
        char_ += 0x20;
    }

    // If our unshifted codepoint is identical to the shifted one then we
    // consider shift. Otherwise we do not, because the shift key was used to
    // obtain the character. This is specified by fixterms.
    if event.unshifted_codepoint != char_ {
        mods.shift = false;
    }

    Some(format!("\x1b[{};{}u", char_, mods.seq_int()).into_bytes())
}

/// Returns the C0 byte for the key event if it should be used. This converts
/// a key event into the expected terminal behavior, such as ctrl+C turning
/// into 0x03. Returns None if the event should not become a C0 byte.
fn ctrl_seq(
    logical_key: Key,
    utf8: &str,
    unshifted_codepoint: u32,
    mods: KeyMods,
) -> Option<u8> {
    const CTRL_ONLY: u8 = MOD_CTRL;

    // If ctrl is not pressed then we never do anything.
    if !mods.ctrl {
        return None;
    }

    // Only binding modifiers: strip lock keys, sides, etc.
    let mut unset_mods = mods.binding();

    // Alt does not impact whether we generate a ctrl sequence; the ESC-prefix
    // logic is handled separately.
    unset_mods.alt = false;

    let mut char_: u8 = if utf8.len() == 1 {
        // Exactly one UTF-8 byte: that is the character to convert.
        utf8.as_bytes()[0]
    } else if let Some(cp) = key_codepoint(logical_key).filter(|cp| *cp <= 0xFF) {
        // A logical key that maps to a single byte printable character.
        // History: this supports cyrillic layouts such as Russian and
        // Mongolian, whose `c` key maps to U+0441 but which every terminal
        // encodes as ctrl+c.
        //
        // For this case we only map the key if we have exactly ctrl pressed,
        // because shift would modify the key and we don't know how to do that
        // properly here (we don't have the layout); we want shift encoded as
        // CSIu.
        if unset_mods.int() != CTRL_ONLY {
            return None;
        }
        cp as u8
    } else {
        return None;
    };

    // Remove shift if we have something outside of the US letter range, so
    // that characters such as `ctrl+shift+-` generate the correct ctrl-seq
    // (used by emacs).
    if unset_mods.shift && !(char_ >= b'A' && char_ <= b'Z') {
        // Special case for the awkward case fixterms specifies.
        if char_ != b'@' {
            unset_mods.shift = false;
        }
    }

    // If the character is uppercase we convert it to lowercase, relying on
    // the unshifted codepoint. This handles caps lock. Shifted characters are
    // handled above: with only shift pressed the ctrl-only check below fails
    // and we don't ctrl-seq encode, which is how programs can tell ctrl+M
    // apart from ctrl+shift+M (Kitty's behavior, a deliberate divergence
    // from fixterms).
    if char_ >= b'A' && char_ <= b'Z' && unshifted_codepoint > 0 && unshifted_codepoint <= 0xFF {
        char_ = unshifted_codepoint as u8;
    }

    // After unsetting, we only continue if we have ONLY control set.
    if unset_mods.int() != CTRL_ONLY {
        return None;
    }

    // From Kitty's key encoding logic. The exact behavior across terminals
    // isn't clear so we just repeat what Kitty does.
    Some(match char_ {
        b' ' => 0,
        b'/' => 31,
        b'0' => 48,
        b'1' => 49,
        b'2' => 0,
        b'3' => 27,
        b'4' => 28,
        b'5' => 29,
        b'6' => 30,
        b'7' => 31,
        b'8' => 127,
        b'9' => 57,
        b'?' => 127,
        b'@' => 0,
        b'\\' => 28,
        b']' => 29,
        b'^' => 30,
        b'_' => 31,
        b'a' => 1,
        b'b' => 2,
        b'c' => 3,
        b'd' => 4,
        b'e' => 5,
        b'f' => 6,
        b'g' => 7,
        b'h' => 8,
        b'j' => 10,
        b'k' => 11,
        b'l' => 12,
        b'n' => 14,
        b'o' => 15,
        b'p' => 16,
        b'q' => 17,
        b'r' => 18,
        b's' => 19,
        b't' => 20,
        b'u' => 21,
        b'v' => 22,
        b'w' => 23,
        b'x' => 24,
        b'y' => 25,
        b'z' => 26,
        b'~' => 30,

        // 'i' (0x09), 'm' (0x0D) and '[' (0x1B) are purposely NOT handled
        // here because of the fixterms specification; they are processed as
        // CSI u. https://www.leonerd.org.uk/hacks/fixterms/
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Kitty keyboard protocol
// ---------------------------------------------------------------------------

/// A single entry in the kitty keymap data (ghostty `kitty.zig`).
#[derive(Clone, Copy)]
struct KittyEntry {
    code: u32,
    final_: u8,
    modifier: bool,
}

/// The kitty functional key table. Based on
/// https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions
fn kitty_entry(k: Key) -> Option<KittyEntry> {
    fn e(code: u32, final_: u8) -> Option<KittyEntry> {
        Some(KittyEntry {
            code,
            final_,
            modifier: false,
        })
    }
    fn m(code: u32) -> Option<KittyEntry> {
        Some(KittyEntry {
            code,
            final_: b'u',
            modifier: true,
        })
    }
    match k {
        Key::Escape => e(27, b'u'),
        Key::Enter => e(13, b'u'),
        Key::Tab => e(9, b'u'),
        Key::Backspace => e(127, b'u'),
        Key::Insert => e(2, b'~'),
        Key::Delete => e(3, b'~'),
        Key::ArrowLeft => e(1, b'D'),
        Key::ArrowRight => e(1, b'C'),
        Key::ArrowUp => e(1, b'A'),
        Key::ArrowDown => e(1, b'B'),
        Key::PageUp => e(5, b'~'),
        Key::PageDown => e(6, b'~'),
        Key::Home => e(1, b'H'),
        Key::End => e(1, b'F'),
        Key::CapsLock => m(57358),
        Key::ScrollLock => e(57359, b'u'),
        Key::NumLock => m(57360),
        Key::PrintScreen => e(57361, b'u'),
        Key::Pause => e(57362, b'u'),

        Key::F1 => e(1, b'P'),
        Key::F2 => e(1, b'Q'),
        Key::F3 => e(13, b'~'),
        Key::F4 => e(1, b'S'),
        Key::F5 => e(15, b'~'),
        Key::F6 => e(17, b'~'),
        Key::F7 => e(18, b'~'),
        Key::F8 => e(19, b'~'),
        Key::F9 => e(20, b'~'),
        Key::F10 => e(21, b'~'),
        Key::F11 => e(23, b'~'),
        Key::F12 => e(24, b'~'),
        Key::F13 => e(57376, b'u'),
        Key::F14 => e(57377, b'u'),
        Key::F15 => e(57378, b'u'),
        Key::F16 => e(57379, b'u'),
        Key::F17 => e(57380, b'u'),
        Key::F18 => e(57381, b'u'),
        Key::F19 => e(57382, b'u'),
        Key::F20 => e(57383, b'u'),
        Key::F21 => e(57384, b'u'),
        Key::F22 => e(57385, b'u'),
        Key::F23 => e(57386, b'u'),
        Key::F24 => e(57387, b'u'),

        Key::Numpad0 => e(57399, b'u'),
        Key::Numpad1 => e(57400, b'u'),
        Key::Numpad2 => e(57401, b'u'),
        Key::Numpad3 => e(57402, b'u'),
        Key::Numpad4 => e(57403, b'u'),
        Key::Numpad5 => e(57404, b'u'),
        Key::Numpad6 => e(57405, b'u'),
        Key::Numpad7 => e(57406, b'u'),
        Key::Numpad8 => e(57407, b'u'),
        Key::Numpad9 => e(57408, b'u'),
        Key::NumpadDecimal => e(57409, b'u'),
        Key::NumpadDivide => e(57410, b'u'),
        Key::NumpadMultiply => e(57411, b'u'),
        Key::NumpadSubtract => e(57412, b'u'),
        Key::NumpadAdd => e(57413, b'u'),
        Key::NumpadEnter => e(57414, b'u'),
        Key::NumpadEqual => e(57415, b'u'),

        Key::ShiftLeft => m(57441),
        Key::ShiftRight => m(57447),
        Key::ControlLeft => m(57442),
        Key::ControlRight => m(57448),
        Key::MetaLeft => m(57444),
        Key::MetaRight => m(57450),
        Key::AltLeft => m(57443),
        Key::AltRight => m(57449),

        _ => None,
    }
}

/// This is the bitfields for Kitty modifiers.
#[derive(Clone, Copy, Default)]
struct KittyMods {
    shift: bool,
    alt: bool,
    ctrl: bool,
    super_: bool,
    hyper: bool,
    meta: bool,
    caps_lock: bool,
    num_lock: bool,
}

impl KittyMods {
    fn from_input(mods: KeyMods) -> KittyMods {
        KittyMods {
            shift: mods.shift,
            alt: mods.alt,
            ctrl: mods.ctrl,
            super_: mods.super_,
            hyper: false,
            meta: false,
            caps_lock: mods.caps_lock,
            num_lock: mods.num_lock,
        }
    }

    /// True if these modifiers prevent printable text. `alt_prevents_text`
    /// is true everywhere except macOS with option-as-alt disabled, which
    /// this port does not model (see the module header).
    fn prevents_text(self, alt_prevents_text: bool) -> bool {
        (self.alt && alt_prevents_text) || self.ctrl || self.super_ || self.hyper || self.meta
    }

    fn int(self) -> u16 {
        (if self.shift { 1 } else { 0 })
            | (if self.alt { 2 } else { 0 })
            | (if self.ctrl { 4 } else { 0 })
            | (if self.super_ { 8 } else { 0 })
            | (if self.hyper { 16 } else { 0 })
            | (if self.meta { 32 } else { 0 })
            | (if self.caps_lock { 64 } else { 0 })
            | (if self.num_lock { 128 } else { 0 })
    }

    /// The value sent as part of the Kitty sequence: bitmask + 1.
    fn seq_int(self) -> u16 {
        self.int() + 1
    }
}

/// Values for the event code. Kitty omits the ":1" for press but other
/// terminals include it; we include it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KittyEvent {
    None = 0,
    Press = 1,
    Repeat = 2,
    Release = 3,
}

/// A kitty key sequence:
/// `CSI unicode-key-code:alternate-key-codes ; modifiers:event-type ; text-as-codepoints u`
struct KittySequence {
    key: u32,
    final_: u8,
    mods: KittyMods,
    event: KittyEvent,
    alternates: [Option<u32>; 2],
    text: String,
}

impl KittySequence {
    fn new(key: u32, final_: u8) -> KittySequence {
        KittySequence {
            key,
            final_,
            mods: KittyMods::default(),
            event: KittyEvent::None,
            alternates: [None, None],
            text: String::new(),
        }
    }

    fn encode(&self) -> Vec<u8> {
        if self.final_ == b'u' || self.final_ == b'~' {
            self.encode_full()
        } else {
            self.encode_special()
        }
    }

    fn encode_full(&self) -> Vec<u8> {
        // Key section
        let mut out = format!("\x1b[{}", self.key);

        // Alternates
        if let Some(shifted) = self.alternates[0] {
            out.push_str(&format!(":{}", shifted));
        }
        if let Some(base) = self.alternates[1] {
            if self.alternates[0].is_none() {
                out.push_str(&format!("::{}", base));
            } else {
                out.push_str(&format!(":{}", base));
            }
        }

        // Mods and events section
        let mods = self.mods.seq_int();
        let mut emit_prior = false;
        if self.event != KittyEvent::None && self.event != KittyEvent::Press {
            out.push_str(&format!(";{}:{}", mods, self.event as u8));
            emit_prior = true;
        } else if mods > 1 {
            out.push_str(&format!(";{}", mods));
            emit_prior = true;
        }

        // Text section
        let mut count = 0usize;
        for cp in self.text.chars() {
            // Skip non-printable ASCII characters.
            if is_control(cp as u32) {
                continue;
            }
            if count == 0 {
                // We need two ";" if we didn't emit the modifier section.
                if !emit_prior {
                    out.push(';');
                }
                out.push(';');
            } else {
                out.push(':');
            }
            out.push_str(&format!("{}", cp as u32));
            count += 1;
        }

        out.push(self.final_ as char);
        out.into_bytes()
    }

    fn encode_special(&self) -> Vec<u8> {
        let mods = self.mods.seq_int();
        let final_ = self.final_ as char;
        if self.event != KittyEvent::None {
            return format!("\x1b[1;{}:{}{}", mods, self.event as u8, final_).into_bytes();
        }
        if mods > 1 {
            return format!("\x1b[1;{}{}", mods, final_).into_bytes();
        }
        format!("\x1b[{}", final_).into_bytes()
    }
}

/// Kitty keyboard protocol encoding of the key event.
fn kitty(event: &KeyEvent, opts: &KeyEncodeOptions) -> Vec<u8> {
    let flags = opts.kitty_flags;

    // This should never happen but we'll check anyway.
    if flags.0 == 0 {
        return legacy(event, opts);
    }

    // We only process "press" events unless report events is active.
    if event.action == KeyAction::Release {
        if !flags.has(KittyFlags::REPORT_EVENTS) {
            return Vec::new();
        }
        // Enter, backspace and tab do not report release events unless
        // "report all" is set.
        if !flags.has(KittyFlags::REPORT_ALL) {
            match event.key {
                Key::Enter | Key::Backspace | Key::Tab => return Vec::new(),
                _ => {}
            }
        }
    }

    let all_mods = event.mods;
    let binding_mods = event.effective_mods().binding();

    // Find the entry for this key in the kitty table, else fall back to the
    // unicode codepoint from UTF-8 (always the unshifted value).
    let entry_ = kitty_entry(event.key).or(if event.unshifted_codepoint > 0 {
        Some(KittyEntry {
            code: event.unshifted_codepoint,
            final_: b'u',
            modifier: false,
        })
    } else {
        None
    });

    // Preprocessing.
    {
        // IME confirmation still sends an enter key, so if we have enter and
        // UTF-8 text we send it directly since we assume that's what is
        // happening. Control characters are excluded because some frontends
        // deliver those as UTF-8 text.
        if !event.utf8.is_empty() && !is_control_utf8(&event.utf8) {
            match event.key {
                Key::Backspace => return Vec::new(),
                Key::Enter => return event.utf8.as_bytes().to_vec(),
                _ => {}
            }
        }

        // If we're reporting all then we always send CSI sequences.
        if !flags.has(KittyFlags::REPORT_ALL) {
            // Quote: "The only exceptions are the Enter, Tab and Backspace
            // keys which still generate the same bytes as in legacy mode;
            // this is to allow the user to type and execute commands in the
            // shell such as reset after a program that sets this mode
            // crashes without clearing it."
            if binding_mods.is_empty() {
                match event.key {
                    Key::Enter => return vec![b'\r'],
                    Key::Tab => return vec![b'\t'],
                    Key::Backspace => return vec![0x7F],
                    _ => {}
                }
            }

            // Send plain-text non-modified text directly to the terminal. We
            // don't send release events because those are specially encoded.
            // Only printable characters: the real world issue is control
            // characters.
            if !event.utf8.is_empty()
                && binding_mods.is_empty()
                && event.action != KeyAction::Release
                && !event.utf8.chars().any(|c| is_control(c as u32))
            {
                return event.utf8.as_bytes().to_vec();
            }
        }
    }

    let entry = match entry_ {
        Some(entry) => entry,
        None => {
            // No entry found. If we have UTF-8 text this is a pure text event
            // (e.g. composed/IME text), so send it as-is so programs can
            // still receive it. Release events never insert text, same as the
            // plain-text path above.
            if event.action == KeyAction::Release {
                return Vec::new();
            }
            return event.utf8.as_bytes().to_vec();
        }
    };

    // If this is just a modifier we require "report all" to send it.
    if entry.modifier && !flags.has(KittyFlags::REPORT_ALL) {
        return Vec::new();
    }

    let mut seq = KittySequence::new(entry.code, entry.final_);
    seq.mods = KittyMods::from_input(all_mods);

    if flags.has(KittyFlags::REPORT_EVENTS) {
        seq.event = match event.action {
            KeyAction::Press => KittyEvent::Press,
            KeyAction::Release => KittyEvent::Release,
            KeyAction::Repeat => KittyEvent::Repeat,
        };
    }

    if flags.has(KittyFlags::REPORT_ALTERNATES) && !is_control(seq.key) {
        let mut it = event.utf8.chars();
        if let Some(cp1) = it.next() {
            let cp1 = cp1 as u32;

            // Set the first alternate (shifted version).
            if cp1 != seq.key && seq.mods.shift {
                seq.alternates[0] = Some(cp1);
            }

            // We want to know if there are additional codepoints because the
            // logic below depends on the utf8 being a single codepoint.
            let has_cp2 = it.next().is_some();

            // Set the base layout key. We only report this if the codepoint
            // differs from our pressed key.
            if let Some(base) = key_codepoint(event.key) {
                if base != seq.key && cp1 != base && !has_cp2 {
                    seq.alternates[1] = Some(base);
                }
            }
        } else {
            // No UTF-8 so we can't report a shifted key, but we can still
            // report a base layout key.
            if let Some(base) = key_codepoint(event.key) {
                if base != seq.key {
                    seq.alternates[1] = Some(base);
                }
            }
        }
    }

    if flags.has(KittyFlags::REPORT_ASSOCIATED)
        && seq.event != KittyEvent::Release
        && !seq.mods.prevents_text(true)
    {
        seq.text = event.utf8.clone();
    }

    seq.encode()
}

// ---------------------------------------------------------------------------
// Tests (ported from ghostty `key_encode.zig`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(key: Key) -> KeyEvent {
        KeyEvent {
            action: KeyAction::Press,
            key,
            mods: KeyMods::default(),
            consumed_mods: KeyMods::default(),
            utf8: String::new(),
            unshifted_codepoint: 0,
        }
    }

    fn with_mods(mut event: KeyEvent, mods: KeyMods) -> KeyEvent {
        event.mods = mods;
        event
    }

    fn with_utf8(mut event: KeyEvent, utf8: &str) -> KeyEvent {
        event.utf8 = utf8.to_string();
        event
    }

    fn with_unshifted(mut event: KeyEvent, cp: u32) -> KeyEvent {
        event.unshifted_codepoint = cp;
        event
    }

    fn shift() -> KeyMods {
        KeyMods {
            shift: true,
            ..Default::default()
        }
    }
    fn ctrl() -> KeyMods {
        KeyMods {
            ctrl: true,
            ..Default::default()
        }
    }
    fn alt() -> KeyMods {
        KeyMods {
            alt: true,
            ..Default::default()
        }
    }
    fn super_() -> KeyMods {
        KeyMods {
            super_: true,
            ..Default::default()
        }
    }
    fn plus(a: KeyMods, b: KeyMods) -> KeyMods {
        KeyMods::from_int(a.int() | b.int())
    }

    #[track_caller]
    fn expect(event: &KeyEvent, opts: &KeyEncodeOptions, want: &str) {
        let got = encode_key(event, opts);
        assert_eq!(
            got.as_slice(),
            want.as_bytes(),
            "got {:?}, want {:?}",
            String::from_utf8_lossy(&got),
            want
        );
    }

    fn legacy_opts() -> KeyEncodeOptions {
        KeyEncodeOptions::default()
    }

    fn kitty_opts(flags: u8) -> KeyEncodeOptions {
        KeyEncodeOptions {
            kitty_flags: KittyFlags(flags),
            ..Default::default()
        }
    }

    const ALL_KITTY: u8 = KittyFlags::DISAMBIGUATE
        | KittyFlags::REPORT_EVENTS
        | KittyFlags::REPORT_ALTERNATES
        | KittyFlags::REPORT_ALL
        | KittyFlags::REPORT_ASSOCIATED;

    // -- modifier bitfields --------------------------------------------------

    #[test]
    fn csi_u_modifier_sequence_values() {
        let m = |s, a, c| CsiUMods {
            shift: s,
            alt: a,
            ctrl: c,
        };
        assert_eq!(m(false, false, false).seq_int(), 1);
        assert_eq!(m(true, false, false).seq_int(), 2);
        assert_eq!(m(false, true, false).seq_int(), 3);
        assert_eq!(m(false, false, true).seq_int(), 5);
        assert_eq!(m(true, true, false).seq_int(), 4);
        assert_eq!(m(true, false, true).seq_int(), 6);
        assert_eq!(m(false, true, true).seq_int(), 7);
        assert_eq!(m(true, true, true).seq_int(), 8);
    }

    #[test]
    fn kitty_modifier_sequence_values() {
        let m = |mods| KittyMods::from_input(mods).seq_int();
        assert_eq!(m(KeyMods::default()), 1);
        assert_eq!(m(shift()), 2);
        assert_eq!(m(alt()), 3);
        assert_eq!(m(ctrl()), 5);
        assert_eq!(m(plus(alt(), shift())), 4);
        assert_eq!(m(plus(ctrl(), shift())), 6);
        assert_eq!(m(plus(alt(), ctrl())), 7);
        assert_eq!(m(plus(plus(alt(), ctrl()), shift())), 8);
        assert_eq!(m(super_()), 9);
        assert_eq!(
            m(KeyMods {
                caps_lock: true,
                ..Default::default()
            }),
            65
        );
        assert_eq!(
            m(KeyMods {
                num_lock: true,
                ..Default::default()
            }),
            129
        );
    }

    // -- KittySequence -------------------------------------------------------

    #[test]
    fn kitty_sequence_backspace() {
        let mut seq = KittySequence::new(127, b'u');
        assert_eq!(seq.encode(), b"\x1b[127u");

        seq.event = KittyEvent::Release;
        assert_eq!(seq.encode(), b"\x1b[127;1:3u");

        let mut seq = KittySequence::new(127, b'u');
        seq.mods.shift = true;
        assert_eq!(seq.encode(), b"\x1b[127;2u");
    }

    #[test]
    fn kitty_sequence_text() {
        let mut seq = KittySequence::new(127, b'u');
        seq.text = "A".to_string();
        assert_eq!(seq.encode(), b"\x1b[127;;65u");

        seq.event = KittyEvent::Release;
        assert_eq!(seq.encode(), b"\x1b[127;1:3;65u");

        let mut seq = KittySequence::new(127, b'u');
        seq.text = "A".to_string();
        seq.mods.shift = true;
        assert_eq!(seq.encode(), b"\x1b[127;2;65u");
    }

    #[test]
    fn kitty_sequence_text_with_control_characters() {
        let mut seq = KittySequence::new(127, b'u');
        seq.text = "\n".to_string();
        assert_eq!(seq.encode(), b"\x1b[127u");

        seq.text = "A\n".to_string();
        assert_eq!(seq.encode(), b"\x1b[127;;65u");
    }

    #[test]
    fn kitty_sequence_special() {
        let mut seq = KittySequence::new(1, b'A');
        assert_eq!(seq.encode(), b"\x1b[A");

        seq.mods.shift = true;
        assert_eq!(seq.encode(), b"\x1b[1;2A");

        seq.event = KittyEvent::Release;
        assert_eq!(seq.encode(), b"\x1b[1;2:3A");
    }

    // -- kitty ---------------------------------------------------------------

    #[test]
    fn kitty_plain_text() {
        expect(
            &with_utf8(ev(Key::KeyA), "abcd"),
            &kitty_opts(KittyFlags::DISAMBIGUATE),
            "abcd",
        );
    }

    #[test]
    fn kitty_repeat_with_just_disambiguate() {
        let mut event = with_utf8(ev(Key::KeyA), "a");
        event.action = KeyAction::Repeat;
        expect(&event, &kitty_opts(KittyFlags::DISAMBIGUATE), "a");
    }

    #[test]
    fn kitty_enter_backspace_tab() {
        let opts = kitty_opts(KittyFlags::DISAMBIGUATE);
        expect(&ev(Key::Enter), &opts, "\r");
        expect(&ev(Key::Backspace), &opts, "\x7f");
        expect(&ev(Key::Tab), &opts, "\t");

        // Kitty does not support DECBKM so there should be no change.
        let decbkm = KeyEncodeOptions {
            backarrow_key_mode: true,
            ..opts
        };
        expect(&ev(Key::Backspace), &decbkm, "\x7f");

        // No release events if "report_all" is not set.
        let events = kitty_opts(KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_EVENTS);
        for key in [Key::Enter, Key::Backspace, Key::Tab] {
            let mut event = ev(key);
            event.action = KeyAction::Release;
            expect(&event, &events, "");
        }

        // Release events if "report_all" is set.
        let all = kitty_opts(
            KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_EVENTS | KittyFlags::REPORT_ALL,
        );
        for (key, want) in [
            (Key::Enter, "\x1b[13;1:3u"),
            (Key::Backspace, "\x1b[127;1:3u"),
            (Key::Tab, "\x1b[9;1:3u"),
        ] {
            let mut event = ev(key);
            event.action = KeyAction::Release;
            expect(&event, &all, want);
        }
    }

    #[test]
    fn kitty_shift_backspace_emits_csi_u() {
        expect(
            &with_mods(ev(Key::Backspace), shift()),
            &kitty_opts(KittyFlags::DISAMBIGUATE),
            "\x1b[127;2u",
        );
    }

    #[test]
    fn kitty_alt_backspace_emits_csi_u() {
        // macOS may mark Option as consumed while translating the key. With
        // no attached text, all modifiers must remain effective.
        let mut event = with_mods(ev(Key::Backspace), alt());
        event.consumed_mods = alt();
        expect(&event, &kitty_opts(KittyFlags::DISAMBIGUATE), "\x1b[127;3u");
    }

    #[test]
    fn kitty_shift_enter_emits_csi_u() {
        expect(
            &with_mods(ev(Key::Enter), shift()),
            &kitty_opts(KittyFlags::DISAMBIGUATE),
            "\x1b[13;2u",
        );
    }

    #[test]
    fn kitty_shift_tab_emits_csi_u() {
        expect(
            &with_mods(ev(Key::Tab), shift()),
            &kitty_opts(KittyFlags::DISAMBIGUATE),
            "\x1b[9;2u",
        );
        expect(
            &with_mods(ev(Key::Tab), shift()),
            &kitty_opts(KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_ALTERNATES),
            "\x1b[9;2u",
        );
    }

    #[test]
    fn kitty_enter_with_all_flags() {
        expect(&ev(Key::Enter), &kitty_opts(ALL_KITTY), "\x1b[13u");
    }

    #[test]
    fn kitty_ctrl_with_all_flags() {
        expect(
            &with_mods(ev(Key::ControlLeft), ctrl()),
            &kitty_opts(ALL_KITTY),
            "\x1b[57442;5u",
        );
    }

    #[test]
    fn kitty_ctrl_release_with_ctrl_mod_set() {
        let mut event = with_mods(ev(Key::ControlLeft), ctrl());
        event.action = KeyAction::Release;
        expect(&event, &kitty_opts(ALL_KITTY), "\x1b[57442;5:3u");
    }

    #[test]
    fn kitty_delete() {
        expect(
            &with_utf8(ev(Key::Delete), "\x7f"),
            &kitty_opts(KittyFlags::DISAMBIGUATE),
            "\x1b[3~",
        );
    }

    #[test]
    fn kitty_text_fallback_on_release() {
        for report_all in [0, KittyFlags::REPORT_ALL] {
            let mut event = with_utf8(with_mods(ev(Key::Unidentified), shift()), "!");
            event.action = KeyAction::Release;
            expect(
                &event,
                &kitty_opts(
                    KittyFlags::DISAMBIGUATE
                        | KittyFlags::REPORT_EVENTS
                        | KittyFlags::REPORT_ALTERNATES
                        | KittyFlags::REPORT_ASSOCIATED
                        | report_all,
                ),
                "",
            );
        }
    }

    #[test]
    fn kitty_text_fallback_on_repeat() {
        let mut event = with_utf8(with_mods(ev(Key::Unidentified), shift()), "!");
        event.action = KeyAction::Repeat;
        expect(
            &event,
            &kitty_opts(
                KittyFlags::DISAMBIGUATE
                    | KittyFlags::REPORT_EVENTS
                    | KittyFlags::REPORT_ALTERNATES
                    | KittyFlags::REPORT_ASSOCIATED,
            ),
            "!",
        );
    }

    #[test]
    fn kitty_composed_text_with_report_all() {
        expect(
            &with_utf8(ev(Key::Unidentified), "û"),
            &kitty_opts(ALL_KITTY),
            "û",
        );
    }

    #[test]
    fn kitty_shift_a_on_us_keyboard() {
        let event = with_unshifted(with_utf8(with_mods(ev(Key::KeyA), shift()), "A"), 97);
        expect(
            &event,
            &kitty_opts(KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_ALTERNATES),
            "\x1b[97:65;2u",
        );
    }

    #[test]
    fn kitty_matching_unshifted_codepoint() {
        // Not a valid encoding in the real world; this is a hypothetical to
        // test the logic around matching unshifted codepoints. We get an
        // alternate because the unshifted codepoint doesn't match the base
        // key.
        let event = with_unshifted(with_utf8(with_mods(ev(Key::KeyA), shift()), "A"), 65);
        expect(
            &event,
            &kitty_opts(KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_ALTERNATES),
            "\x1b[65::97;2u",
        );
    }

    #[test]
    fn kitty_report_alternates_with_caps() {
        let event = with_unshifted(
            with_utf8(
                with_mods(
                    ev(Key::KeyJ),
                    KeyMods {
                        caps_lock: true,
                        ..Default::default()
                    },
                ),
                "J",
            ),
            106,
        );
        expect(
            &event,
            &kitty_opts(
                KittyFlags::DISAMBIGUATE
                    | KittyFlags::REPORT_ALL
                    | KittyFlags::REPORT_ALTERNATES
                    | KittyFlags::REPORT_ASSOCIATED,
            ),
            "\x1b[106;65;74u",
        );
    }

    #[test]
    fn kitty_report_alternates_colon() {
        // shift+';'
        let event = with_unshifted(with_utf8(with_mods(ev(Key::Semicolon), shift()), ":"), ';' as u32);
        expect(
            &event,
            &kitty_opts(
                KittyFlags::DISAMBIGUATE
                    | KittyFlags::REPORT_ALL
                    | KittyFlags::REPORT_ALTERNATES
                    | KittyFlags::REPORT_ASSOCIATED,
            ),
            "\x1b[59:58;2;58u",
        );
    }

    #[test]
    fn kitty_report_alternates_ru_layout() {
        let flags = kitty_opts(
            KittyFlags::DISAMBIGUATE
                | KittyFlags::REPORT_ALL
                | KittyFlags::REPORT_ALTERNATES
                | KittyFlags::REPORT_ASSOCIATED,
        );

        // Unshifted
        let event = with_unshifted(with_utf8(ev(Key::Semicolon), "ч"), 1095);
        expect(&event, &flags, "\x1b[1095::59;;1095u");

        // Shifted
        let event = with_unshifted(with_utf8(with_mods(ev(Key::Semicolon), shift()), "Ч"), 1095);
        expect(&event, &flags, "\x1b[1095:1063:59;2;1063u");

        // Caps lock
        let event = with_unshifted(
            with_utf8(
                with_mods(
                    ev(Key::Semicolon),
                    KeyMods {
                        caps_lock: true,
                        ..Default::default()
                    },
                ),
                "Ч",
            ),
            1095,
        );
        expect(&event, &flags, "\x1b[1095::59;65;1063u");
    }

    #[test]
    fn kitty_report_alternates_hu_layout_release() {
        let mut event = with_unshifted(with_mods(ev(Key::BracketLeft), ctrl()), 337);
        event.action = KeyAction::Release;
        expect(&event, &kitty_opts(ALL_KITTY), "\x1b[337::91;5:3u");
    }

    #[test]
    fn kitty_up_arrow_with_utf8() {
        // macOS generates utf8 text for arrow keys.
        expect(
            &with_utf8(ev(Key::ArrowUp), "\u{1e}"),
            &kitty_opts(KittyFlags::DISAMBIGUATE),
            "\x1b[A",
        );
    }

    #[test]
    fn kitty_left_shift() {
        expect(
            &ev(Key::ShiftLeft),
            &kitty_opts(KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_ALTERNATES),
            "",
        );
        expect(
            &ev(Key::ShiftLeft),
            &kitty_opts(KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_ALL),
            "\x1b[57441u",
        );
    }

    #[test]
    fn kitty_report_associated_with_alt_text() {
        // ghostty's macos-option-as-alt = true behavior, which is what this
        // port always does: alt prevents associated text.
        let event = with_unshifted(with_utf8(with_mods(ev(Key::KeyW), alt()), "∑"), 119);
        expect(&event, &kitty_opts(ALL_KITTY), "\x1b[119;3u");

        // Without the alt modifier the text comes along.
        let event = with_unshifted(with_utf8(ev(Key::KeyW), "∑"), 119);
        expect(&event, &kitty_opts(ALL_KITTY), "\x1b[119;;8721u");
    }

    #[test]
    fn kitty_report_associated_with_modifiers() {
        let event = with_unshifted(with_utf8(with_mods(ev(Key::KeyJ), ctrl()), "j"), 106);
        expect(&event, &kitty_opts(ALL_KITTY), "\x1b[106;5u");
    }

    #[test]
    fn kitty_report_associated() {
        let event = with_unshifted(with_utf8(with_mods(ev(Key::KeyJ), shift()), "J"), 106);
        expect(
            &event,
            &kitty_opts(
                KittyFlags::DISAMBIGUATE
                    | KittyFlags::REPORT_ALL
                    | KittyFlags::REPORT_ALTERNATES
                    | KittyFlags::REPORT_ASSOCIATED,
            ),
            "\x1b[106:74;2;74u",
        );
    }

    #[test]
    fn kitty_report_associated_on_release() {
        let mut event = with_unshifted(with_utf8(with_mods(ev(Key::KeyJ), shift()), "J"), 106);
        event.action = KeyAction::Release;
        expect(&event, &kitty_opts(ALL_KITTY), "\x1b[106:74;2:3u");
    }

    #[test]
    fn kitty_alternates_omit_control_characters() {
        expect(
            &with_utf8(ev(Key::Delete), "\x7f"),
            &kitty_opts(
                KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_ALTERNATES | KittyFlags::REPORT_ALL,
            ),
            "\x1b[3~",
        );
    }

    #[test]
    fn kitty_enter_with_utf8_dead_key_state() {
        let event = with_unshifted(with_utf8(ev(Key::Enter), "A"), 0x0D);
        expect(
            &event,
            &kitty_opts(
                KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_ALTERNATES | KittyFlags::REPORT_ALL,
            ),
            "A",
        );
    }

    #[test]
    fn kitty_backspace_with_utf8_dead_key_state() {
        let event = with_unshifted(with_utf8(ev(Key::Backspace), "A"), 0x0D);
        expect(&event, &kitty_opts(ALL_KITTY), "");
    }

    #[test]
    fn kitty_keypad_number() {
        expect(
            &with_utf8(ev(Key::Numpad1), "1"),
            &kitty_opts(ALL_KITTY),
            "\x1b[57400;;49u",
        );
    }

    #[test]
    fn kitty_backspace_decbkm_with_report_all() {
        // Kitty does not support DECBKM so there should be no difference.
        expect(&ev(Key::Backspace), &kitty_opts(ALL_KITTY), "\x1b[127u");
        expect(
            &ev(Key::Backspace),
            &KeyEncodeOptions {
                backarrow_key_mode: true,
                ..kitty_opts(ALL_KITTY)
            },
            "\x1b[127u",
        );
    }

    #[test]
    fn kitty_arrow_with_mods_and_events() {
        // Special-form finals carry the event type even for press.
        let opts = kitty_opts(KittyFlags::DISAMBIGUATE | KittyFlags::REPORT_EVENTS);
        expect(&ev(Key::ArrowUp), &opts, "\x1b[1;1:1A");

        let mut event = with_mods(ev(Key::ArrowLeft), ctrl());
        event.action = KeyAction::Release;
        expect(&event, &opts, "\x1b[1;5:3D");
    }

    // -- legacy: dead key state ---------------------------------------------

    #[test]
    fn legacy_backspace_with_utf8_dead_key_state() {
        let event = with_unshifted(with_utf8(ev(Key::Backspace), "A"), 0x0D);
        expect(&event, &legacy_opts(), "");
    }

    #[test]
    fn legacy_enter_with_utf8_dead_key_state() {
        let event = with_unshifted(with_utf8(ev(Key::Enter), "A"), 0x0D);
        expect(&event, &legacy_opts(), "A");
    }

    #[test]
    fn legacy_esc_with_utf8_dead_key_state() {
        let event = with_unshifted(with_utf8(ev(Key::Escape), "A"), 0x0D);
        expect(&event, &legacy_opts(), "A");
    }

    // -- legacy: ctrl sequences ---------------------------------------------

    #[test]
    fn legacy_ctrl_c() {
        expect(
            &with_utf8(with_mods(ev(Key::KeyC), ctrl()), "c"),
            &legacy_opts(),
            "\x03",
        );
    }

    #[test]
    fn legacy_ctrl_space() {
        expect(
            &with_utf8(with_mods(ev(Key::Space), ctrl()), " "),
            &legacy_opts(),
            "\x00",
        );
    }

    #[test]
    fn legacy_ctrl_shift_minus() {
        // underscore on US
        expect(
            &with_utf8(with_mods(ev(Key::Minus), plus(ctrl(), shift())), "_"),
            &legacy_opts(),
            "\x1f",
        );
    }

    #[test]
    fn legacy_ctrl_question_mark() {
        // ctrl+shift+/ on a US layout: shift is dropped because '?' is
        // outside the letter range, so this is a plain ctrl seq of 0x7f.
        let event = with_unshifted(
            with_utf8(with_mods(ev(Key::Slash), plus(ctrl(), shift())), "?"),
            '/' as u32,
        );
        expect(&event, &legacy_opts(), "\x7f");
    }

    #[test]
    fn legacy_ctrl_alt_c() {
        expect(
            &with_utf8(with_mods(ev(Key::KeyC), plus(ctrl(), alt())), "c"),
            &legacy_opts(),
            "\x1b\x03",
        );
    }

    #[test]
    fn legacy_ctrl_table() {
        // The full Kitty-derived ctrl table.
        let cases: &[(&str, u8)] = &[
            (" ", 0),
            ("/", 31),
            ("0", 48),
            ("1", 49),
            ("2", 0),
            ("3", 27),
            ("4", 28),
            ("5", 29),
            ("6", 30),
            ("7", 31),
            ("8", 127),
            ("9", 57),
            ("?", 127),
            ("@", 0),
            ("\\", 28),
            ("]", 29),
            ("^", 30),
            ("_", 31),
            ("a", 1),
            ("b", 2),
            ("c", 3),
            ("d", 4),
            ("e", 5),
            ("f", 6),
            ("g", 7),
            ("h", 8),
            ("j", 10),
            ("k", 11),
            ("l", 12),
            ("n", 14),
            ("o", 15),
            ("p", 16),
            ("q", 17),
            ("r", 18),
            ("s", 19),
            ("t", 20),
            ("u", 21),
            ("v", 22),
            ("w", 23),
            ("x", 24),
            ("y", 25),
            ("z", 26),
            ("~", 30),
        ];
        for (utf8, want) in cases {
            let got = ctrl_seq(Key::Unidentified, utf8, 0, ctrl());
            assert_eq!(got, Some(*want), "ctrl+{:?}", utf8);
        }

        // fixterms leaves these to CSI u.
        for utf8 in ["i", "m", "["] {
            assert_eq!(ctrl_seq(Key::Unidentified, utf8, 0, ctrl()), None);
        }
    }

    #[test]
    fn ctrlseq_normal_ctrl_c() {
        assert_eq!(ctrl_seq(Key::Unidentified, "c", 'c' as u32, ctrl()), Some(3));
    }

    #[test]
    fn ctrlseq_alt_should_be_allowed() {
        assert_eq!(
            ctrl_seq(Key::Unidentified, "c", 'c' as u32, plus(alt(), ctrl())),
            Some(3)
        );
    }

    #[test]
    fn ctrlseq_no_ctrl_does_nothing() {
        assert_eq!(
            ctrl_seq(Key::Unidentified, "c", 'c' as u32, KeyMods::default()),
            None
        );
    }

    #[test]
    fn ctrlseq_shifted_non_character() {
        assert_eq!(
            ctrl_seq(Key::Unidentified, "_", '-' as u32, plus(ctrl(), shift())),
            Some(0x1F)
        );
    }

    #[test]
    fn ctrlseq_caps_ascii_letter() {
        let mods = KeyMods {
            ctrl: true,
            caps_lock: true,
            ..Default::default()
        };
        assert_eq!(ctrl_seq(Key::Unidentified, "C", 'c' as u32, mods), Some(3));
    }

    #[test]
    fn ctrlseq_shift_does_not_generate_ctrl_seq() {
        assert_eq!(ctrl_seq(Key::Unidentified, "C", 'c' as u32, shift()), None);
        assert_eq!(
            ctrl_seq(Key::Unidentified, "C", 'c' as u32, plus(shift(), ctrl())),
            None
        );
    }

    #[test]
    fn ctrlseq_russian_ctrl_c() {
        assert_eq!(ctrl_seq(Key::KeyC, "с", 0x0441, ctrl()), Some(3));
        assert_eq!(ctrl_seq(Key::KeyC, "с", 0x0441, plus(ctrl(), shift())), None);
        assert_eq!(ctrl_seq(Key::KeyC, "с", 0x0441, plus(ctrl(), alt())), Some(3));
    }

    // -- legacy: alt prefix --------------------------------------------------

    #[test]
    fn legacy_alt_c() {
        let opts = KeyEncodeOptions {
            alt_esc_prefix: true,
            ..Default::default()
        };
        expect(
            &with_utf8(with_mods(ev(Key::KeyC), alt()), "c"),
            &opts,
            "\x1bc",
        );
    }

    #[test]
    fn legacy_alt_e_only_unshifted() {
        let opts = KeyEncodeOptions {
            alt_esc_prefix: true,
            ..Default::default()
        };
        expect(
            &with_unshifted(with_mods(ev(Key::KeyE), alt()), 'e' as u32),
            &opts,
            "\x1be",
        );
    }

    #[test]
    fn legacy_alt_x_translated_text() {
        // option+x on macOS produces "≈" but we alt-prefix the unshifted byte.
        let opts = KeyEncodeOptions {
            alt_esc_prefix: true,
            ..Default::default()
        };
        let event = with_unshifted(with_utf8(with_mods(ev(Key::KeyC), alt()), "≈"), 'c' as u32);
        expect(&event, &opts, "\x1bc");
    }

    #[test]
    fn legacy_shift_alt_period() {
        let opts = KeyEncodeOptions {
            alt_esc_prefix: true,
            ..Default::default()
        };
        let event = with_unshifted(
            with_utf8(with_mods(ev(Key::Period), plus(alt(), shift())), ">"),
            '.' as u32,
        );
        expect(&event, &opts, "\x1b>");
    }

    #[test]
    fn legacy_alt_multibyte_text_is_not_prefixed() {
        let opts = KeyEncodeOptions {
            alt_esc_prefix: true,
            ..Default::default()
        };
        expect(
            &with_utf8(with_mods(ev(Key::KeyF), alt()), "ф"),
            &opts,
            "ф",
        );
    }

    #[test]
    fn legacy_alt_without_esc_prefix_mode() {
        // DECRST 1036: no ESC prefix, the text goes through as-is.
        expect(
            &with_utf8(with_mods(ev(Key::KeyC), alt()), "c"),
            &legacy_opts(),
            "c",
        );
    }

    // -- legacy: backspace ---------------------------------------------------

    #[test]
    fn legacy_ctrl_shift_backspace() {
        expect(
            &with_mods(ev(Key::Backspace), plus(ctrl(), shift())),
            &legacy_opts(),
            "\x08",
        );
    }

    #[test]
    fn legacy_backspace_decbkm() {
        let reset = legacy_opts();
        let set = KeyEncodeOptions {
            backarrow_key_mode: true,
            ..Default::default()
        };
        expect(&ev(Key::Backspace), &reset, "\x7f");
        expect(&with_mods(ev(Key::Backspace), ctrl()), &reset, "\x08");
        expect(&ev(Key::Backspace), &set, "\x08");
        expect(&with_mods(ev(Key::Backspace), ctrl()), &set, "\x7f");
    }

    #[test]
    fn legacy_backspace_with_del_utf8() {
        let event = with_unshifted(with_utf8(ev(Key::Backspace), "\x7f"), 0x08);
        expect(&event, &legacy_opts(), "\x7f");
        expect(
            &event,
            &KeyEncodeOptions {
                backarrow_key_mode: true,
                ..Default::default()
            },
            "\x08",
        );
    }

    #[test]
    fn legacy_alt_backspace() {
        expect(
            &with_mods(ev(Key::Backspace), alt()),
            &legacy_opts(),
            "\x1b\x7f",
        );
    }

    // -- legacy: modifyOtherKeys --------------------------------------------

    #[test]
    fn legacy_ctrl_shift_char_with_modify_other_state_2() {
        let opts = KeyEncodeOptions {
            modify_other_keys_state_2: true,
            ..Default::default()
        };
        expect(
            &with_utf8(with_mods(ev(Key::KeyH), plus(ctrl(), shift())), "H"),
            &opts,
            "\x1b[27;6;72~",
        );

        // Consumed mods don't change the encoding: we use the raw mods.
        let mut event = with_utf8(with_mods(ev(Key::KeyH), plus(ctrl(), shift())), "H");
        event.consumed_mods = shift();
        expect(&event, &opts, "\x1b[27;6;72~");
    }

    #[test]
    fn legacy_alt_digit_with_modify_other_state_2() {
        let opts = KeyEncodeOptions {
            modify_other_keys_state_2: true,
            ..Default::default()
        };
        expect(
            &with_utf8(with_mods(ev(Key::Digit8), alt()), "8"),
            &opts,
            "\x1b[27;3;56~",
        );
    }

    #[test]
    fn legacy_modify_other_state_2_special_keys() {
        let opts = KeyEncodeOptions {
            modify_other_keys_state_2: true,
            ..Default::default()
        };
        // The function key table has its own "set_other" entries.
        expect(&with_mods(ev(Key::Tab), shift()), &opts, "\x1b[27;2;9~");
        expect(&with_mods(ev(Key::Tab), alt()), &opts, "\x1b[27;3;9~");
        expect(&with_mods(ev(Key::Enter), alt()), &opts, "\x1b[27;3;13~");
        expect(
            &with_mods(ev(Key::Backspace), shift()),
            &opts,
            "\x1b[27;2;127~",
        );
        // ... and the "set" entries only apply when state 2 is off.
        expect(&with_mods(ev(Key::Tab), shift()), &legacy_opts(), "\x1b[Z");
        expect(&with_mods(ev(Key::Enter), alt()), &legacy_opts(), "\x1b\r");
    }

    #[test]
    fn legacy_shift_space_with_modify_other_state_2() {
        let opts = KeyEncodeOptions {
            modify_other_keys_state_2: true,
            ..Default::default()
        };
        expect(
            &with_utf8(with_mods(ev(Key::Space), shift()), " "),
            &opts,
            "\x1b[27;2;32~",
        );
        // Shift alone on a printable non-space key is not encoded.
        expect(
            &with_utf8(with_mods(ev(Key::Digit1), shift()), "!"),
            &opts,
            "!",
        );
    }

    // -- legacy: fixterms CSI u ---------------------------------------------

    #[test]
    fn legacy_fixterm_awkward_letters() {
        expect(
            &with_utf8(with_mods(ev(Key::KeyI), ctrl()), "i"),
            &legacy_opts(),
            "\x1b[105;5u",
        );
        expect(
            &with_utf8(with_mods(ev(Key::KeyM), ctrl()), "m"),
            &legacy_opts(),
            "\x1b[109;5u",
        );
        expect(
            &with_utf8(with_mods(ev(Key::BracketLeft), ctrl()), "["),
            &legacy_opts(),
            "\x1b[91;5u",
        );
        let event = with_unshifted(
            with_utf8(with_mods(ev(Key::Digit2), plus(ctrl(), shift())), "@"),
            '2' as u32,
        );
        expect(&event, &legacy_opts(), "\x1b[64;5u");
    }

    #[test]
    fn legacy_ctrl_shift_letter_ascii() {
        // Kitty behavior: ctrl+shift+letter is the unshifted letter with the
        // shift modifier present.
        let event = with_unshifted(
            with_utf8(with_mods(ev(Key::KeyM), plus(ctrl(), shift())), "M"),
            'm' as u32,
        );
        expect(&event, &legacy_opts(), "\x1b[109;6u");
    }

    #[test]
    fn legacy_hu_layout_ctrl_sends_proper_codepoint() {
        let event = with_unshifted(with_utf8(with_mods(ev(Key::BracketLeft), ctrl()), "ő"), 337);
        expect(&event, &legacy_opts(), "\x1b[337;5u");
    }

    // -- legacy: function keys ----------------------------------------------

    #[test]
    fn legacy_shift_function_key_uses_all_mods() {
        let mut event = with_mods(ev(Key::ArrowUp), shift());
        event.consumed_mods = shift();
        expect(&event, &legacy_opts(), "\x1b[1;2A");
    }

    #[test]
    fn legacy_arrows_normal_and_application() {
        let normal = legacy_opts();
        let app = KeyEncodeOptions {
            cursor_key_application: true,
            ..Default::default()
        };
        for (key, n, a) in [
            (Key::ArrowUp, "\x1b[A", "\x1bOA"),
            (Key::ArrowDown, "\x1b[B", "\x1bOB"),
            (Key::ArrowRight, "\x1b[C", "\x1bOC"),
            (Key::ArrowLeft, "\x1b[D", "\x1bOD"),
            (Key::Home, "\x1b[H", "\x1bOH"),
            (Key::End, "\x1b[F", "\x1bOF"),
        ] {
            expect(&ev(key), &normal, n);
            expect(&ev(key), &app, a);
        }
    }

    #[test]
    fn legacy_arrows_all_modifier_combinations() {
        // CSI 1;{mods}{final} for every modifier combination, in both cursor
        // key modes (pc-style entries are cursor-mode agnostic).
        let combos: [(KeyMods, u32); 15] = [
            (shift(), 2),
            (alt(), 3),
            (plus(shift(), alt()), 4),
            (ctrl(), 5),
            (plus(shift(), ctrl()), 6),
            (plus(alt(), ctrl()), 7),
            (plus(plus(shift(), alt()), ctrl()), 8),
            (super_(), 9),
            (plus(shift(), super_()), 10),
            (plus(alt(), super_()), 11),
            (plus(plus(shift(), alt()), super_()), 12),
            (plus(ctrl(), super_()), 13),
            (plus(plus(shift(), ctrl()), super_()), 14),
            (plus(plus(alt(), ctrl()), super_()), 15),
            (plus(plus(plus(shift(), alt()), ctrl()), super_()), 16),
        ];
        for (key, final_) in [
            (Key::ArrowUp, 'A'),
            (Key::ArrowDown, 'B'),
            (Key::ArrowRight, 'C'),
            (Key::ArrowLeft, 'D'),
            (Key::Home, 'H'),
            (Key::End, 'F'),
        ] {
            for (mods, code) in combos {
                let want = format!("\x1b[1;{}{}", code, final_);
                expect(&with_mods(ev(key), mods), &legacy_opts(), &want);
                expect(
                    &with_mods(ev(key), mods),
                    &KeyEncodeOptions {
                        cursor_key_application: true,
                        ..Default::default()
                    },
                    &want,
                );
            }
        }
    }

    #[test]
    fn legacy_tilde_keys_with_modifiers() {
        for (key, num) in [
            (Key::Insert, 2),
            (Key::Delete, 3),
            (Key::PageUp, 5),
            (Key::PageDown, 6),
        ] {
            expect(&ev(key), &legacy_opts(), &format!("\x1b[{}~", num));
            expect(
                &with_mods(ev(key), shift()),
                &legacy_opts(),
                &format!("\x1b[{};2~", num),
            );
            expect(
                &with_mods(ev(key), ctrl()),
                &legacy_opts(),
                &format!("\x1b[{};5~", num),
            );
            expect(
                &with_mods(ev(key), plus(ctrl(), shift())),
                &legacy_opts(),
                &format!("\x1b[{};6~", num),
            );
        }
    }

    #[test]
    fn legacy_f1_through_f5_with_ctrl() {
        expect(&with_mods(ev(Key::F1), ctrl()), &legacy_opts(), "\x1b[1;5P");
        expect(&with_mods(ev(Key::F2), ctrl()), &legacy_opts(), "\x1b[1;5Q");
        expect(&with_mods(ev(Key::F3), ctrl()), &legacy_opts(), "\x1b[13;5~");
        expect(&with_mods(ev(Key::F4), ctrl()), &legacy_opts(), "\x1b[1;5S");
        // F5 uses the new encoding.
        expect(&with_mods(ev(Key::F5), ctrl()), &legacy_opts(), "\x1b[15;5~");
    }

    #[test]
    fn legacy_function_keys_unmodified() {
        for (key, want) in [
            (Key::F1, "\x1bOP"),
            (Key::F2, "\x1bOQ"),
            (Key::F3, "\x1bOR"),
            (Key::F4, "\x1bOS"),
            (Key::F5, "\x1b[15~"),
            (Key::F6, "\x1b[17~"),
            (Key::F7, "\x1b[18~"),
            (Key::F8, "\x1b[19~"),
            (Key::F9, "\x1b[20~"),
            (Key::F10, "\x1b[21~"),
            (Key::F11, "\x1b[23~"),
            (Key::F12, "\x1b[24~"),
        ] {
            expect(&ev(key), &legacy_opts(), want);
        }
    }

    #[test]
    fn legacy_shift_tab() {
        expect(&with_mods(ev(Key::Tab), shift()), &legacy_opts(), "\x1b[Z");
        expect(&ev(Key::Tab), &legacy_opts(), "\t");
        expect(&with_mods(ev(Key::Tab), alt()), &legacy_opts(), "\x1b\t");
        expect(
            &with_mods(ev(Key::Tab), ctrl()),
            &legacy_opts(),
            "\x1b[27;5;9~",
        );
    }

    #[test]
    fn legacy_enter_and_escape() {
        expect(&ev(Key::Enter), &legacy_opts(), "\r");
        expect(&with_mods(ev(Key::Enter), alt()), &legacy_opts(), "\x1b\r");
        expect(
            &with_mods(ev(Key::Enter), shift()),
            &legacy_opts(),
            "\x1b[27;2;13~",
        );
        expect(&ev(Key::Escape), &legacy_opts(), "\x1b");
        expect(&with_mods(ev(Key::Escape), alt()), &legacy_opts(), "\x1b\x1b");
        expect(
            &with_mods(ev(Key::Escape), ctrl()),
            &legacy_opts(),
            "\x1b[27;5;27~",
        );
    }

    // -- legacy: keypad ------------------------------------------------------

    #[test]
    fn legacy_keypad_enter() {
        expect(&ev(Key::NumpadEnter), &legacy_opts(), "\r");
    }

    #[test]
    fn legacy_keypad_1() {
        expect(&with_utf8(ev(Key::Numpad1), "1"), &legacy_opts(), "1");
    }

    #[test]
    fn legacy_keypad_1_with_application_keypad() {
        let opts = KeyEncodeOptions {
            keypad_key_application: true,
            ..Default::default()
        };
        expect(&with_utf8(ev(Key::Numpad1), "1"), &opts, "\x1bOq");

        // numlock alone doesn't change it
        let mut event = with_utf8(ev(Key::Numpad1), "1");
        event.mods.num_lock = true;
        expect(&event, &opts, "\x1bOq");
    }

    #[test]
    fn legacy_keypad_1_with_application_keypad_and_numlock_ignore() {
        let opts = KeyEncodeOptions {
            keypad_key_application: true,
            ignore_keypad_with_numlock: true,
            ..Default::default()
        };
        expect(&with_utf8(ev(Key::Numpad1), "1"), &opts, "1");
    }

    #[test]
    fn legacy_keypad_application_finals() {
        let opts = KeyEncodeOptions {
            keypad_key_application: true,
            ..Default::default()
        };
        for (key, suffix) in [
            (Key::Numpad0, "p"),
            (Key::Numpad1, "q"),
            (Key::Numpad2, "r"),
            (Key::Numpad3, "s"),
            (Key::Numpad4, "t"),
            (Key::Numpad5, "u"),
            (Key::Numpad6, "v"),
            (Key::Numpad7, "w"),
            (Key::Numpad8, "x"),
            (Key::Numpad9, "y"),
            (Key::NumpadDecimal, "n"),
            (Key::NumpadDivide, "o"),
            (Key::NumpadMultiply, "j"),
            (Key::NumpadSubtract, "m"),
            (Key::NumpadAdd, "k"),
            (Key::NumpadEnter, "M"),
        ] {
            expect(&ev(key), &opts, &format!("\x1bO{}", suffix));
        }
    }

    // -- legacy: misc --------------------------------------------------------

    #[test]
    fn legacy_release_encodes_nothing() {
        let mut event = with_utf8(ev(Key::KeyA), "a");
        event.action = KeyAction::Release;
        expect(&event, &legacy_opts(), "");
    }

    #[test]
    fn legacy_repeat_encodes_like_press() {
        let mut event = with_utf8(ev(Key::KeyA), "a");
        event.action = KeyAction::Repeat;
        expect(&event, &legacy_opts(), "a");
    }

    #[test]
    fn legacy_plain_text() {
        expect(&with_utf8(ev(Key::KeyA), "a"), &legacy_opts(), "a");
        expect(
            &with_unshifted(with_utf8(with_mods(ev(Key::KeyA), shift()), "A"), 'a' as u32),
            &legacy_opts(),
            "A",
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn legacy_super_on_macos_with_text() {
        expect(
            &with_utf8(with_mods(ev(Key::KeyB), super_()), "b"),
            &legacy_opts(),
            "",
        );
        expect(
            &with_utf8(with_mods(ev(Key::KeyB), plus(super_(), shift())), "B"),
            &legacy_opts(),
            "",
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn legacy_super_off_macos_encodes_text() {
        expect(
            &with_utf8(with_mods(ev(Key::KeyB), super_()), "b"),
            &legacy_opts(),
            "b",
        );
    }
}
