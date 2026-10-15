//! Modifier state: keys held, lock toggles, and the masks Wayland sends.
use crate::layout::Layout;
use crate::mods::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ModState { pub depressed: u32, pub latched: u32, pub locked: u32, pub group: u32 }

pub struct Tracker {
    layout: &'static Layout,
    /// Count of held keys per real modifier bit position (shift, ctrl, alt, super).
    held: [u32; 8],
    locked: u32,
    pressed: Vec<u16>,
}
fn bit_of(sym: &str) -> Option<u32> {
    Some(match sym { "Shift_L" | "Shift_R" => SHIFT, "Control_L" | "Control_R" => CONTROL, "Alt_L" | "Alt_R" => MOD1, "Super_L" | "Super_R" => MOD4, "ISO_Level3_Shift" => MOD5, _ => return None })
}
impl Tracker {
    /// Num Lock starts on, as hosts usually have it.
    pub fn new(layout: &'static Layout) -> Self { Self { layout, held: [0; 8], locked: MOD2, pressed: Vec::new() } }
    pub fn pressed(&self) -> &[u16] { &self.pressed }
    pub fn state(&self) -> ModState {
        let mut d = 0;
        for (i, n) in self.held.iter().enumerate() { if *n > 0 { d |= 1 << i; } }
        ModState { depressed: d, latched: 0, locked: self.locked, group: 0 }
    }
    /// Feeds a key event. Returns true when the modifier state changed. Repeats are ignored.
    pub fn key(&mut self, evdev: u16, down: bool) -> bool {
        let before = self.state();
        let already = self.pressed.contains(&evdev);
        if down == already { return false; }
        if down { self.pressed.push(evdev); } else { self.pressed.retain(|&k| k != evdev); }
        let sym = self.layout.sym(evdev, 0).unwrap_or("");
        if let Some(bit) = bit_of(sym) {
            let i = bit.trailing_zeros() as usize;
            if down { self.held[i] += 1; } else { self.held[i] = self.held[i].saturating_sub(1); }
        } else if down {
            match sym { "Caps_Lock" => self.locked ^= LOCK, "Num_Lock" => self.locked ^= MOD2, _ => {} }
        }
        self.state() != before
    }
    pub fn shift(&self) -> bool { self.state().depressed & SHIFT != 0 }
    pub fn caps(&self) -> bool { self.locked & LOCK != 0 }
    pub fn numlock(&self) -> bool { self.locked & MOD2 != 0 }
    /// The text a key press types now (None while Control or Alt is held, or for non-text keys).
    pub fn text(&self, evdev: u16) -> Option<char> {
        if self.state().depressed & (CONTROL | MOD1 | MOD4) != 0 { return None; }
        self.layout.char_for(evdev, self.shift(), self.caps(), self.numlock(), self.state().depressed & MOD5 != 0)
    }
}
