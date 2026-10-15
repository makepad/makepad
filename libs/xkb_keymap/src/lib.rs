//! XKB keymap producer for Wayland hosts. A typed layout table (evdev code -> keysyms) is the
//! single source of truth: it emits the `xkb_keymap` text clients compile, and answers
//! "which character does this key produce" for the host's own text input.
pub mod layout;
pub mod text;
pub mod tracker;

pub use layout::{by_name, Layout, Level, DE, DVORAK, FR, GB, US};
pub use tracker::{ModState, Tracker};

/// Modifier masks as clients see them in `wl_keyboard.modifiers` (real modifier indices).
pub mod mods {
    pub const SHIFT: u32 = 1 << 0;
    pub const LOCK: u32 = 1 << 1;
    pub const CONTROL: u32 = 1 << 2;
    pub const MOD1: u32 = 1 << 3; // Alt
    pub const MOD2: u32 = 1 << 4; // NumLock
    pub const MOD4: u32 = 1 << 6; // Super
    pub const MOD5: u32 = 1 << 7; // AltGr (Level3)
}
