//! Hotkeys — one keymap for the whole app: named commands, the chord each
//! one answers to, and the rules that decide which command a key press means.
//!
//! Before this, every widget matched its own chords: the menu bar compares
//! each entry's shortcut against every key press, the palette opens on a
//! chord its host checks by hand, the tweaker tests `Cmd+Z` itself. Each one
//! works, and none of them can be asked what is bound, what clashes, or what
//! the person changed. [`Hotkeys`] is the table they can all read.
//!
//! # The registry
//!
//! One [`Hotkeys`] lives on the `Cx` (`cx.global::<Hotkeys>()`). A host
//! registers each command once, with a label and a default chord, and asks
//! [`Hotkeys::resolve`] which command a key press means:
//!
//! ```text
//! let keys = cx.global::<Hotkeys>();
//! keys.register(live_id!(save), "Save", KeyChord::parse("Mod+S"), HotkeyScope::Global);
//! // in handle_event:
//! if let Event::KeyDown(ke) = event {
//!     if let Some(id) = Hotkeys::resolve_in(cx, ke) { ... }
//! }
//! ```
//!
//! The same defaults can be written in Splash on a [`crate::hotkey_editor`]
//! or [`crate::keyboard_map`] — the shape the menu bar already reads:
//!
//! ```text
//! hotkeys: [
//!     {id: @save label: "Save" chord: "Mod+S"}
//!     {id: @find label: "Find" chord: "Mod+F" scope: @editor}
//! ]
//! ```
//!
//! Registering an id that is already there updates its label and default and
//! keeps a chord the person chose; a chord still at the old default follows
//! the new one.
//!
//! # Chords are written once and read on every platform
//!
//! `"Mod"` is the platform's primary modifier: Command on Apple, Control
//! everywhere else. `"Cmd"` names Command where it exists and stands in for
//! Control where it does not, so a table written on one machine keeps working
//! on the other. `"Meta"`, `"Super"` and `"Win"` always name the logo key.
//! [`KeyChord::format`] prints `Cmd+Shift+P` on Apple and `Ctrl+Shift+P`
//! elsewhere; [`KeyChord::display`] is the menu form (`⇧⌘P` on Apple).
//!
//! # Which command a key press means
//!
//! The rules follow Dear ImGui's key routing:
//!
//! * A binding scoped to the focused part of the window beats a global one
//!   on the same chord. A scope is a [`LiveId`] a widget ties to its area
//!   with [`Hotkeys::set_scope_area`]; it counts as focused while the key
//!   focus is that area or is drawn inside it, and nested scopes are tried
//!   innermost first.
//! * While a text field has the key focus, chords that type or edit text —
//!   printable keys without Ctrl/Cmd, the caret and deletion keys, and the
//!   clipboard, undo and select-all chords — belong to the field and are not
//!   matched. `Mod+S` still saves while typing; `S` does not.
//! * While a hotkey editor is capturing a chord, nothing resolves — and
//!   neither does the key press that ended the capture, whichever order the
//!   editor and the host see it in.
//!
//! Two bindings on one chord in one scope are a conflict
//! ([`Hotkeys::conflicts`]); the first registered wins until one is moved. A
//! focused binding over a global one is not a conflict: that is the routing
//! doing its job.
//!
//! # Persistence
//!
//! Only what the person changed is kept, as `id=chord` lines
//! ([`Hotkeys::to_settings`] / [`Hotkeys::apply_settings`]); an empty chord
//! (`id=`) records a deliberately cleared binding. [`Hotkeys::save`] and
//! [`Hotkeys::load`] move that text through the platform's key-value store
//! (`cx.storage(namespace)`), and [`Hotkeys::handle_storage`] applies a load
//! when it arrives.
use crate::{
    makepad_draw::*,
    menu_bar::{for_each_element, obj_field, obj_string},
};

/// The two ways platforms lay out their modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPlatform {
    /// Command is the primary modifier and the modifiers print as symbols.
    Apple,
    /// Control is the primary modifier and the modifiers print as words.
    Other,
}

impl KeyPlatform {
    /// The platform this build runs on, chosen the way the menu bar and the
    /// key caps choose it: from the build target's vendor.
    pub fn current() -> Self {
        if cfg!(target_vendor = "apple") {
            KeyPlatform::Apple
        } else {
            KeyPlatform::Other
        }
    }

    /// The modifiers `Mod` stands for.
    pub fn primary(self) -> KeyModifiers {
        match self {
            KeyPlatform::Apple => KeyModifiers {
                logo: true,
                ..Default::default()
            },
            KeyPlatform::Other => KeyModifiers {
                control: true,
                ..Default::default()
            },
        }
    }

    /// Whether `m` holds this platform's primary modifier and not the other
    /// one of the Control/logo pair.
    pub fn is_primary(self, m: KeyModifiers) -> bool {
        match self {
            KeyPlatform::Apple => m.logo && !m.control,
            KeyPlatform::Other => m.control && !m.logo,
        }
    }
}

/// Canonical names, one per key: the first entry for a key is how it prints.
/// Parsing compares without case, so `"esc"` and `"ESC"` both find `Esc`.
const KEY_NAMES: &[(KeyCode, &str)] = &[
    (KeyCode::KeyA, "A"),
    (KeyCode::KeyB, "B"),
    (KeyCode::KeyC, "C"),
    (KeyCode::KeyD, "D"),
    (KeyCode::KeyE, "E"),
    (KeyCode::KeyF, "F"),
    (KeyCode::KeyG, "G"),
    (KeyCode::KeyH, "H"),
    (KeyCode::KeyI, "I"),
    (KeyCode::KeyJ, "J"),
    (KeyCode::KeyK, "K"),
    (KeyCode::KeyL, "L"),
    (KeyCode::KeyM, "M"),
    (KeyCode::KeyN, "N"),
    (KeyCode::KeyO, "O"),
    (KeyCode::KeyP, "P"),
    (KeyCode::KeyQ, "Q"),
    (KeyCode::KeyR, "R"),
    (KeyCode::KeyS, "S"),
    (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"),
    (KeyCode::KeyV, "V"),
    (KeyCode::KeyW, "W"),
    (KeyCode::KeyX, "X"),
    (KeyCode::KeyY, "Y"),
    (KeyCode::KeyZ, "Z"),
    (KeyCode::Key0, "0"),
    (KeyCode::Key1, "1"),
    (KeyCode::Key2, "2"),
    (KeyCode::Key3, "3"),
    (KeyCode::Key4, "4"),
    (KeyCode::Key5, "5"),
    (KeyCode::Key6, "6"),
    (KeyCode::Key7, "7"),
    (KeyCode::Key8, "8"),
    (KeyCode::Key9, "9"),
    (KeyCode::F1, "F1"),
    (KeyCode::F2, "F2"),
    (KeyCode::F3, "F3"),
    (KeyCode::F4, "F4"),
    (KeyCode::F5, "F5"),
    (KeyCode::F6, "F6"),
    (KeyCode::F7, "F7"),
    (KeyCode::F8, "F8"),
    (KeyCode::F9, "F9"),
    (KeyCode::F10, "F10"),
    (KeyCode::F11, "F11"),
    (KeyCode::F12, "F12"),
    (KeyCode::Escape, "Esc"),
    (KeyCode::Tab, "Tab"),
    (KeyCode::Space, "Space"),
    (KeyCode::ReturnKey, "Enter"),
    (KeyCode::Backspace, "Backspace"),
    (KeyCode::Delete, "Delete"),
    (KeyCode::Insert, "Insert"),
    (KeyCode::Home, "Home"),
    (KeyCode::End, "End"),
    (KeyCode::PageUp, "PageUp"),
    (KeyCode::PageDown, "PageDown"),
    (KeyCode::ArrowUp, "Up"),
    (KeyCode::ArrowDown, "Down"),
    (KeyCode::ArrowLeft, "Left"),
    (KeyCode::ArrowRight, "Right"),
    (KeyCode::Backtick, "`"),
    (KeyCode::Minus, "-"),
    (KeyCode::Equals, "="),
    (KeyCode::LBracket, "["),
    (KeyCode::RBracket, "]"),
    (KeyCode::Backslash, "\\"),
    (KeyCode::Semicolon, ";"),
    (KeyCode::Quote, "'"),
    (KeyCode::Comma, ","),
    (KeyCode::Period, "."),
    (KeyCode::Slash, "/"),
    (KeyCode::Capslock, "CapsLock"),
    (KeyCode::PrintScreen, "PrintScreen"),
    (KeyCode::ScrollLock, "ScrollLock"),
    (KeyCode::Pause, "Pause"),
    (KeyCode::Numlock, "NumLock"),
    (KeyCode::Numpad0, "Num0"),
    (KeyCode::Numpad1, "Num1"),
    (KeyCode::Numpad2, "Num2"),
    (KeyCode::Numpad3, "Num3"),
    (KeyCode::Numpad4, "Num4"),
    (KeyCode::Numpad5, "Num5"),
    (KeyCode::Numpad6, "Num6"),
    (KeyCode::Numpad7, "Num7"),
    (KeyCode::Numpad8, "Num8"),
    (KeyCode::Numpad9, "Num9"),
    (KeyCode::NumpadAdd, "NumAdd"),
    (KeyCode::NumpadSubtract, "NumSubtract"),
    (KeyCode::NumpadMultiply, "NumMultiply"),
    (KeyCode::NumpadDivide, "NumDivide"),
    (KeyCode::NumpadDecimal, "NumDecimal"),
    (KeyCode::NumpadEnter, "NumEnter"),
    (KeyCode::NumpadEquals, "NumEquals"),
    (KeyCode::Back, "Back"),
];

/// Other spellings people write, lowercase. `Plus` is the key that carries
/// `+` on most layouts, which is `=`; Shift stays whatever the chord says.
const KEY_ALIASES: &[(&str, KeyCode)] = &[
    ("escape", KeyCode::Escape),
    ("return", KeyCode::ReturnKey),
    ("del", KeyCode::Delete),
    ("ins", KeyCode::Insert),
    ("pgup", KeyCode::PageUp),
    ("pgdn", KeyCode::PageDown),
    ("arrowup", KeyCode::ArrowUp),
    ("arrowdown", KeyCode::ArrowDown),
    ("arrowleft", KeyCode::ArrowLeft),
    ("arrowright", KeyCode::ArrowRight),
    ("spacebar", KeyCode::Space),
    ("plus", KeyCode::Equals),
    ("+", KeyCode::Equals),
    ("equals", KeyCode::Equals),
    ("equal", KeyCode::Equals),
    ("minus", KeyCode::Minus),
    ("dash", KeyCode::Minus),
    ("comma", KeyCode::Comma),
    ("period", KeyCode::Period),
    ("dot", KeyCode::Period),
    ("slash", KeyCode::Slash),
    ("backslash", KeyCode::Backslash),
    ("semicolon", KeyCode::Semicolon),
    ("quote", KeyCode::Quote),
    ("backtick", KeyCode::Backtick),
    ("grave", KeyCode::Backtick),
    ("lbracket", KeyCode::LBracket),
    ("rbracket", KeyCode::RBracket),
    ("bracketleft", KeyCode::LBracket),
    ("bracketright", KeyCode::RBracket),
    ("caps", KeyCode::Capslock),
    ("prtsc", KeyCode::PrintScreen),
];

/// The canonical printed name of a key, or `None` for the modifier keys and
/// `Unknown`, which cannot be the key of a chord.
pub fn key_name(key: KeyCode) -> Option<&'static str> {
    KEY_NAMES
        .iter()
        .find(|(code, _)| *code == key)
        .map(|(_, name)| *name)
}

/// The key a name means, in any case and under any of its spellings.
pub fn key_from_name(name: &str) -> Option<KeyCode> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    KEY_NAMES
        .iter()
        .find(|(_, n)| n.to_ascii_lowercase() == lower)
        .map(|(code, _)| *code)
        .or_else(|| {
            KEY_ALIASES
                .iter()
                .find(|(n, _)| *n == lower)
                .map(|(_, code)| *code)
        })
}

/// Whether a key is one of the four modifier keys.
pub fn is_modifier_key(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::Control | KeyCode::Alt | KeyCode::Shift | KeyCode::Logo
    )
}

/// Split a chord on `+`. A trailing `++` (or a lone `+`) is the plus key.
fn split_chord(text: &str) -> Vec<String> {
    let text = text.trim();
    let (body, plus_key) = if text == "+" {
        ("", true)
    } else if let Some(body) = text.strip_suffix("++") {
        (body, true)
    } else {
        (text, false)
    };
    let mut out: Vec<String> = body
        .split('+')
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .collect();
    if plus_key {
        out.push("+".to_string());
    }
    out
}

/// A key and the modifiers held with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyChord {
    pub key: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub fn new(key: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { key, modifiers }
    }

    /// A key with no modifiers.
    pub fn plain(key: KeyCode) -> Self {
        Self::new(key, KeyModifiers::default())
    }

    /// A key with this platform's primary modifier (`Mod+key`).
    pub fn primary(key: KeyCode) -> Self {
        Self::new(key, KeyPlatform::current().primary())
    }

    /// The chord a key press makes, or `None` for a press of a modifier key
    /// alone or of a key the platform could not name.
    pub fn from_key_event(ke: &KeyEvent) -> Option<Self> {
        if is_modifier_key(ke.key_code) || ke.key_code.is_unknown() {
            return None;
        }
        key_name(ke.key_code)?;
        Some(Self::new(ke.key_code, ke.modifiers))
    }

    /// Parse `"Mod+Shift+P"`, `"Cmd+S"`, `"Ctrl+Alt+Delete"`, `"F5"`,
    /// `"Ctrl++"` for this build's platform.
    pub fn parse(text: &str) -> Option<Self> {
        Self::parse_for(text, KeyPlatform::current())
    }

    /// Parse for a named platform. Modifiers may come in any order; exactly
    /// one non-modifier key is required.
    pub fn parse_for(text: &str, platform: KeyPlatform) -> Option<Self> {
        let mut modifiers = KeyModifiers::default();
        let mut key = None;
        for token in split_chord(text) {
            let lower = token.to_ascii_lowercase();
            match lower.as_str() {
                "mod" | "primary" | "cmdorctrl" | "commandorcontrol" => {
                    let primary = platform.primary();
                    modifiers.control |= primary.control;
                    modifiers.logo |= primary.logo;
                }
                "ctrl" | "control" | "ctl" => modifiers.control = true,
                "alt" | "option" | "opt" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "cmd" | "command" => match platform {
                    KeyPlatform::Apple => modifiers.logo = true,
                    KeyPlatform::Other => modifiers.control = true,
                },
                "meta" | "super" | "win" | "windows" | "logo" => modifiers.logo = true,
                _ => {
                    if key.is_some() {
                        return None;
                    }
                    key = Some(key_from_name(&token)?);
                }
            }
        }
        Some(Self::new(key?, modifiers))
    }

    /// The chord as text for this build's platform: `Cmd+Shift+P` on Apple,
    /// `Ctrl+Shift+P` elsewhere. Parses back to the same chord.
    pub fn format(&self) -> String {
        self.format_for(KeyPlatform::current())
    }

    pub fn format_for(&self, platform: KeyPlatform) -> String {
        let m = self.modifiers;
        let mut parts: Vec<&str> = Vec::new();
        match platform {
            KeyPlatform::Apple => {
                if m.logo {
                    parts.push("Cmd");
                }
                if m.control {
                    parts.push("Ctrl");
                }
                if m.alt {
                    parts.push("Opt");
                }
            }
            KeyPlatform::Other => {
                if m.control {
                    parts.push("Ctrl");
                }
                if m.logo {
                    parts.push("Super");
                }
                if m.alt {
                    parts.push("Alt");
                }
            }
        }
        if m.shift {
            parts.push("Shift");
        }
        parts.push(key_name(self.key).unwrap_or("?"));
        parts.join("+")
    }

    /// The same chord on every platform: `Ctrl`, `Alt`, `Shift` and `Meta`
    /// (the logo key) spelled out. This is what the settings text stores, so
    /// a file read on another platform means the same keys.
    pub fn format_canonical(&self) -> String {
        let m = self.modifiers;
        let mut parts: Vec<&str> = Vec::new();
        if m.control {
            parts.push("Ctrl");
        }
        if m.logo {
            parts.push("Meta");
        }
        if m.alt {
            parts.push("Alt");
        }
        if m.shift {
            parts.push("Shift");
        }
        parts.push(key_name(self.key).unwrap_or("?"));
        parts.join("+")
    }

    /// The menu form: `⌃⌥⇧⌘P` on Apple (modifier symbols in the order Apple
    /// menus use, no separators, arrows as arrows and every other key by its
    /// name), the [`Self::format`] text elsewhere.
    pub fn display(&self) -> String {
        self.display_for(KeyPlatform::current())
    }

    pub fn display_for(&self, platform: KeyPlatform) -> String {
        match platform {
            KeyPlatform::Other => self.format_for(platform),
            KeyPlatform::Apple => {
                let m = self.modifiers;
                let mut out = String::new();
                if m.control {
                    out.push('\u{2303}');
                }
                if m.alt {
                    out.push('\u{2325}');
                }
                if m.shift {
                    out.push('\u{21e7}');
                }
                if m.logo {
                    out.push('\u{2318}');
                }
                let key = match self.key {
                    KeyCode::ArrowUp => "\u{2191}",
                    KeyCode::ArrowDown => "\u{2193}",
                    KeyCode::ArrowLeft => "\u{2190}",
                    KeyCode::ArrowRight => "\u{2192}",
                    other => key_name(other).unwrap_or("?"),
                };
                out.push_str(key);
                out
            }
        }
    }

    /// The chord in the words [`crate::kbd::KbdGroup`] reads, so a chord can
    /// be drawn as key caps in the platform's own style.
    pub fn to_kbd_shortcut(&self) -> String {
        let m = self.modifiers;
        let mut parts: Vec<&str> = Vec::new();
        if m.control {
            parts.push("ctrl");
        }
        if m.logo {
            parts.push("meta");
        }
        if m.alt {
            parts.push("alt");
        }
        if m.shift {
            parts.push("shift");
        }
        let key = match self.key {
            KeyCode::ArrowUp => "up",
            KeyCode::ArrowDown => "down",
            KeyCode::ArrowLeft => "left",
            KeyCode::ArrowRight => "right",
            other => key_name(other).unwrap_or("?"),
        };
        parts.push(key);
        parts.join("+")
    }

    /// Does this key press make this chord? The modifiers must match
    /// exactly, so `Mod+N` does not fire while `Mod+Shift+N` is pressed. In
    /// a browser the primary modifier may arrive as either Control or the
    /// logo key, and either one is taken for it.
    pub fn matches(&self, ke: &KeyEvent) -> bool {
        if ke.key_code != self.key {
            return false;
        }
        let a = self.modifiers;
        let b = ke.modifiers;
        if a.shift != b.shift || a.alt != b.alt {
            return false;
        }
        if cfg!(target_arch = "wasm32") && (a.control != a.logo) {
            return (b.control || b.logo) && !(b.control && b.logo);
        }
        a.control == b.control && a.logo == b.logo
    }

    /// Whether a text field that has the key focus owns this chord: it types
    /// a character, moves the caret, deletes, or is one of the clipboard,
    /// undo and select-all chords every text field answers to.
    pub fn belongs_to_text(&self, platform: KeyPlatform) -> bool {
        let m = self.modifiers;
        if matches!(
            self.key,
            KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::ArrowLeft
                | KeyCode::ArrowRight
                | KeyCode::ArrowUp
                | KeyCode::ArrowDown
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::ReturnKey
                | KeyCode::NumpadEnter
        ) {
            return true;
        }
        if platform.is_primary(m)
            && !m.alt
            && matches!(
                self.key,
                KeyCode::KeyA | KeyCode::KeyC | KeyCode::KeyV | KeyCode::KeyX | KeyCode::KeyZ | KeyCode::KeyY
            )
        {
            return true;
        }
        if self.key == KeyCode::Tab || self.key.to_char(false).is_none() {
            return false;
        }
        if m.logo {
            return false;
        }
        match platform {
            // Option with a key types a character of its own.
            KeyPlatform::Apple => !m.control,
            // Ctrl+Alt is AltGr, which types; Alt alone is a menu key.
            KeyPlatform::Other => {
                if m.control {
                    m.alt
                } else {
                    !m.alt
                }
            }
        }
    }
}

impl std::fmt::Display for KeyChord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.format())
    }
}

/// Where a binding applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HotkeyScope {
    /// Anywhere in the window.
    #[default]
    Global,
    /// Only while the key focus is inside the scope with this id (see
    /// [`Hotkeys::set_scope_area`]); beats a global binding on the same
    /// chord.
    Focused(LiveId),
}

/// One named command and its binding.
#[derive(Clone, Debug, PartialEq)]
pub struct Hotkey {
    pub id: LiveId,
    pub label: String,
    /// The chord it ships with; what Reset goes back to.
    pub default: Option<KeyChord>,
    /// The chord it answers to now; `None` is unbound.
    pub chord: Option<KeyChord>,
    pub scope: HotkeyScope,
}

impl Hotkey {
    /// Whether the person moved it away from its default.
    pub fn is_customized(&self) -> bool {
        self.chord != self.default
    }
}

/// The app's keymap. See the module documentation.
#[derive(Default)]
pub struct Hotkeys {
    hotkeys: Vec<Hotkey>,
    /// Focus scopes and the areas they cover.
    scope_areas: Vec<(LiveId, Area)>,
    /// Undo groups: every rebinding in a group, with the chord it replaced.
    history: Vec<Vec<(LiveId, Option<KeyChord>)>>,
    capturing: bool,
    /// The key press that ended a capture, as (key, time): the same event
    /// reaching a host after the editor must not run what it just bound.
    swallowed: Option<(KeyCode, f64)>,
    revision: u64,
    pending_load: Option<StorageRequestId>,
}

/// The most undo groups kept.
const HISTORY_LIMIT: usize = 64;

/// The key the settings text is stored under.
const STORAGE_KEY: &str = "hotkeys";

impl Hotkeys {
    /// Bumped on every change, so a widget drawing the keymap can tell
    /// whether what it drew is still current.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Add a command, or update the label, default and scope of one already
    /// registered. A chord the person chose is kept; a chord still at the
    /// old default follows the new default.
    pub fn register(
        &mut self,
        id: LiveId,
        label: &str,
        default: Option<KeyChord>,
        scope: HotkeyScope,
    ) {
        if let Some(hotkey) = self.hotkeys.iter_mut().find(|h| h.id == id) {
            if hotkey.label == label && hotkey.default == default && hotkey.scope == scope {
                return;
            }
            if !hotkey.is_customized() {
                hotkey.chord = default;
            }
            hotkey.label = label.to_string();
            hotkey.default = default;
            hotkey.scope = scope;
        } else {
            self.hotkeys.push(Hotkey {
                id,
                label: label.to_string(),
                default,
                chord: default,
                scope,
            });
        }
        self.touch();
    }

    /// [`Self::register`] with the default written as text (`"Mod+S"`); an
    /// empty or unreadable chord registers the command unbound.
    pub fn register_str(&mut self, id: LiveId, label: &str, chord: &str, scope: HotkeyScope) {
        self.register(id, label, KeyChord::parse(chord), scope);
    }

    /// Register every `{id: @x label: "..." chord: "..." scope: @y}` object
    /// of a Splash array. Objects without an id are skipped; `scope` is
    /// optional and leaves the command global.
    pub fn register_script(vm: &mut ScriptVm, value: ScriptValue) {
        let mut defs: Vec<(LiveId, String, String, Option<LiveId>)> = Vec::new();
        for_each_element(vm, value, &mut |vm, item| {
            let Some(object) = item.as_object() else {
                return;
            };
            let Some(id) = obj_field(vm, object, id!(id)).as_id() else {
                return;
            };
            let label = obj_string(vm, object, id!(label)).unwrap_or_default();
            let chord = obj_string(vm, object, id!(chord)).unwrap_or_default();
            let scope = obj_field(vm, object, id!(scope))
                .as_id()
                .filter(|scope| !scope.is_empty());
            defs.push((id, label, chord, scope));
        });
        if defs.is_empty() {
            return;
        }
        let hotkeys = vm.cx_mut().global::<Hotkeys>();
        for (id, label, chord, scope) in defs {
            let scope = match scope {
                Some(scope) => HotkeyScope::Focused(scope),
                None => HotkeyScope::Global,
            };
            let label = if label.is_empty() {
                id.to_string()
            } else {
                label
            };
            hotkeys.register_str(id, &label, &chord, scope);
        }
    }

    pub fn unregister(&mut self, id: LiveId) {
        let before = self.hotkeys.len();
        self.hotkeys.retain(|h| h.id != id);
        if self.hotkeys.len() != before {
            self.touch();
        }
    }

    pub fn get(&self, id: LiveId) -> Option<&Hotkey> {
        self.hotkeys.iter().find(|h| h.id == id)
    }

    /// Every command, in the order it was registered.
    pub fn iter(&self) -> impl Iterator<Item = &Hotkey> {
        self.hotkeys.iter()
    }

    pub fn len(&self) -> usize {
        self.hotkeys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hotkeys.is_empty()
    }

    /// The chord a command answers to now.
    pub fn bindings_for(&self, id: LiveId) -> Option<KeyChord> {
        self.get(id).and_then(|h| h.chord)
    }

    /// Every command bound to a chord on this key, whatever its modifiers.
    pub fn bindings_on_key(&self, key: KeyCode) -> Vec<&Hotkey> {
        self.hotkeys
            .iter()
            .filter(|h| h.chord.is_some_and(|c| c.key == key))
            .collect()
    }

    /// The command's chord as a menu prints it, or an empty string when it
    /// is unbound or unknown.
    pub fn display(&self, id: LiveId) -> String {
        self.bindings_for(id)
            .map(|chord| chord.display())
            .unwrap_or_default()
    }

    /// Bind a command to a chord, or unbind it with `None`, keeping the
    /// chord it had for [`Self::undo`]. False when the id is unknown or the
    /// chord is already the one it has.
    pub fn rebind(&mut self, id: LiveId, chord: Option<KeyChord>) -> bool {
        let Some(hotkey) = self.hotkeys.iter_mut().find(|h| h.id == id) else {
            return false;
        };
        if hotkey.chord == chord {
            return false;
        }
        let previous = hotkey.chord;
        hotkey.chord = chord;
        self.push_history(vec![(id, previous)]);
        self.touch();
        true
    }

    /// Put one command back on its default. False when it already was.
    pub fn reset(&mut self, id: LiveId) -> bool {
        let Some(default) = self.get(id).map(|h| h.default) else {
            return false;
        };
        self.rebind(id, default)
    }

    /// Put every command back on its default, as one undo step. Answers the
    /// ids that changed.
    pub fn reset_all(&mut self) -> Vec<LiveId> {
        let mut group = Vec::new();
        for hotkey in self.hotkeys.iter_mut() {
            if hotkey.is_customized() {
                group.push((hotkey.id, hotkey.chord));
                hotkey.chord = hotkey.default;
            }
        }
        let changed: Vec<LiveId> = group.iter().map(|(id, _)| *id).collect();
        if !group.is_empty() {
            self.push_history(group);
            self.touch();
        }
        changed
    }

    fn push_history(&mut self, group: Vec<(LiveId, Option<KeyChord>)>) {
        self.history.push(group);
        if self.history.len() > HISTORY_LIMIT {
            self.history.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// Take back the last rebinding, reset or reset-all. Answers the ids
    /// whose chords changed, with the chord each has now.
    pub fn undo(&mut self) -> Vec<(LiveId, Option<KeyChord>)> {
        let Some(group) = self.history.pop() else {
            return Vec::new();
        };
        let mut restored = Vec::new();
        for (id, chord) in group.into_iter().rev() {
            if let Some(hotkey) = self.hotkeys.iter_mut().find(|h| h.id == id) {
                hotkey.chord = chord;
                restored.push((id, chord));
            }
        }
        if !restored.is_empty() {
            self.touch();
        }
        restored
    }

    /// Every pair of commands bound to the same chord in the same scope, the
    /// earlier-registered first.
    pub fn conflicts(&self) -> Vec<(LiveId, LiveId)> {
        let mut out = Vec::new();
        for (i, a) in self.hotkeys.iter().enumerate() {
            let Some(chord) = a.chord else {
                continue;
            };
            for b in &self.hotkeys[i + 1..] {
                if b.chord == Some(chord) && b.scope == a.scope {
                    out.push((a.id, b.id));
                }
            }
        }
        out
    }

    /// The commands a chord would clash with if `id` were bound to it.
    pub fn conflicts_with(&self, id: LiveId, chord: KeyChord) -> Vec<LiveId> {
        let scope = self.get(id).map(|h| h.scope).unwrap_or_default();
        self.hotkeys
            .iter()
            .filter(|h| h.id != id && h.chord == Some(chord) && h.scope == scope)
            .map(|h| h.id)
            .collect()
    }

    /// Tie a focus scope to the area it covers. Call it where the scope's
    /// widget draws, so the area stays current.
    pub fn set_scope_area(&mut self, scope: LiveId, area: Area) {
        if let Some(entry) = self.scope_areas.iter_mut().find(|(id, _)| *id == scope) {
            entry.1 = area;
        } else {
            self.scope_areas.push((scope, area));
        }
    }

    pub fn clear_scope_area(&mut self, scope: LiveId) {
        self.scope_areas.retain(|(id, _)| *id != scope);
    }

    /// The focus scopes the key focus is in, innermost first: a scope whose
    /// own area has the focus, then the scopes whose areas contain the
    /// middle of the focused area, smallest first.
    pub fn focused_scopes(&self, cx: &Cx) -> Vec<LiveId> {
        let focus = cx.key_focus();
        if focus.is_empty() || !focus.is_valid(cx) {
            return Vec::new();
        }
        let rect = focus.rect(cx);
        let middle = rect.pos + rect.size * 0.5;
        let mut hits: Vec<(f64, LiveId)> = Vec::new();
        for (scope, area) in &self.scope_areas {
            if area.is_empty() || !area.is_valid(cx) {
                continue;
            }
            if *area == focus {
                hits.push((-1.0, *scope));
                continue;
            }
            let scope_rect = area.rect(cx);
            if scope_rect.contains(middle) {
                hits.push((scope_rect.size.x * scope_rect.size.y, *scope));
            }
        }
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut out: Vec<LiveId> = Vec::new();
        for (_, scope) in hits {
            if !out.contains(&scope) {
                out.push(scope);
            }
        }
        out
    }

    /// Whether the key focus is on a text field. A text field asks the
    /// platform for the text keyboard on the same area it takes the key
    /// focus with, so the focus sitting on the area the keyboard was last
    /// raised for means the caret is there.
    pub fn text_has_focus(cx: &Cx) -> bool {
        let focus = cx.key_focus();
        if focus.is_empty() || !focus.is_valid(cx) {
            return false;
        }
        let ime = cx.get_ime_area_rect();
        if ime.size.x <= 0.0 && ime.size.y <= 0.0 {
            return false;
        }
        focus.rect(cx) == ime
    }

    /// Tell the registry a hotkey editor is capturing a chord, so the key
    /// the person presses to bind is not also run as a command.
    pub fn set_capturing(&mut self, capturing: bool) {
        self.capturing = capturing;
    }

    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    /// End a capture that this key press finished. The press is not
    /// resolved, even by a host that sees it after the editor did.
    pub fn end_capture_with(&mut self, ke: &KeyEvent) {
        self.capturing = false;
        self.swallowed = Some((ke.key_code, ke.time));
    }

    /// Which command a key press means, by the routing rules in the module
    /// documentation, reading the focus from `cx`.
    pub fn resolve(&self, cx: &Cx, ke: &KeyEvent) -> Option<LiveId> {
        if self.capturing || self.hotkeys.is_empty() {
            return None;
        }
        let scopes = self.focused_scopes(cx);
        self.resolve_with(ke, &scopes, Self::text_has_focus(cx))
    }

    /// [`Self::resolve`] on the registry held by `cx`; `None` when the app
    /// never made one.
    pub fn resolve_in(cx: &Cx, ke: &KeyEvent) -> Option<LiveId> {
        cx.get_global_ref::<Hotkeys>()?.resolve(cx, ke)
    }

    /// The routing on its own, for a host that knows its focus better than
    /// the registry can tell: `scopes` innermost first, and whether a text
    /// field has the key focus.
    pub fn resolve_with(
        &self,
        ke: &KeyEvent,
        scopes: &[LiveId],
        text_focus: bool,
    ) -> Option<LiveId> {
        if self.capturing || self.swallowed == Some((ke.key_code, ke.time)) {
            return None;
        }
        let chord = KeyChord::from_key_event(ke)?;
        if text_focus && chord.belongs_to_text(KeyPlatform::current()) {
            return None;
        }
        let mut best: Option<(usize, LiveId)> = None;
        for hotkey in &self.hotkeys {
            let Some(bound) = hotkey.chord else {
                continue;
            };
            if !bound.matches(ke) {
                continue;
            }
            // Rank: a focused scope by its depth, global after all of them.
            let rank = match hotkey.scope {
                HotkeyScope::Global => scopes.len(),
                HotkeyScope::Focused(scope) => match scopes.iter().position(|s| *s == scope) {
                    Some(depth) => depth,
                    None => continue,
                },
            };
            if best.is_none_or(|(r, _)| rank < r) {
                best = Some((rank, hotkey.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The person's changes as `id=chord` lines, in registration order.
    /// Commands still on their default are left out; an unbound one is
    /// written `id=`.
    pub fn to_settings(&self) -> String {
        let mut out = String::new();
        for hotkey in &self.hotkeys {
            if !hotkey.is_customized() {
                continue;
            }
            let chord = hotkey
                .chord
                .map(|c| c.format_canonical())
                .unwrap_or_default();
            out.push_str(&format!("{}={}\n", hotkey.id, chord));
        }
        out
    }

    /// Apply `id=chord` lines. Lines for commands not registered, and lines
    /// that do not read, are skipped; commands the text does not mention go
    /// back to their defaults. Not an undo step. Answers how many lines
    /// applied.
    pub fn apply_settings(&mut self, text: &str) -> usize {
        let mut chosen: Vec<(LiveId, Option<KeyChord>)> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, chord)) = line.split_once('=') else {
                continue;
            };
            let Some(id) = self.id_named(key.trim()) else {
                continue;
            };
            let chord = chord.trim();
            let chord = if chord.is_empty() {
                None
            } else {
                match KeyChord::parse(chord) {
                    Some(chord) => Some(chord),
                    None => continue,
                }
            };
            chosen.push((id, chord));
        }
        for hotkey in self.hotkeys.iter_mut() {
            hotkey.chord = match chosen.iter().find(|(id, _)| *id == hotkey.id) {
                Some((_, chord)) => *chord,
                None => hotkey.default,
            };
        }
        self.touch();
        chosen.len()
    }

    /// A registered id from the text a settings line names it by.
    fn id_named(&self, name: &str) -> Option<LiveId> {
        let hashed = LiveId::from_str(name);
        let hex = u64::from_str_radix(name, 16).ok();
        self.hotkeys
            .iter()
            .map(|h| h.id)
            .find(|id| *id == hashed || Some(id.0) == hex || id.to_string() == name)
    }

    /// Write the person's changes to `storage` under the key `hotkeys`.
    pub fn save(&self, cx: &mut Cx, storage: &StorageHandle) -> StorageRequestId {
        storage.set(cx, STORAGE_KEY, self.to_settings().into_bytes())
    }

    /// Ask `storage` for the saved changes. They apply when the answer
    /// arrives, through [`Self::handle_storage`]; register the defaults
    /// before that.
    pub fn load(&mut self, cx: &mut Cx, storage: &StorageHandle) -> StorageRequestId {
        let request = storage.get(cx, STORAGE_KEY);
        self.pending_load = Some(request);
        request
    }

    /// Apply a pending [`Self::load`] when its answer is in `event`. True
    /// when it applied; a missing value leaves the defaults standing.
    pub fn handle_storage(&mut self, event: &Event) -> bool {
        let Event::Storage(responses) = event else {
            return false;
        };
        let Some(pending) = self.pending_load else {
            return false;
        };
        for response in responses.iter() {
            if response.request_id != pending {
                continue;
            }
            self.pending_load = None;
            if let Ok(StorageResult::Value(Some(bytes))) = &response.result {
                if let Ok(text) = std::str::from_utf8(bytes) {
                    self.apply_settings(text);
                    return true;
                }
            }
            return false;
        }
        false
    }
}
