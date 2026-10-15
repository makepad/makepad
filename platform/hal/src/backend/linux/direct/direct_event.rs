//! The evdev modifier state; input events go straight into the HAL EventQueue.
use crate::types::input;

#[derive(Clone, Copy, Debug, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Modifiers {
    pub fn bits(self) -> u32 {
        u32::from(self.shift) * input::MOD_SHIFT
            | u32::from(self.control) * input::MOD_CONTROL
            | u32::from(self.alt) * input::MOD_ALT
            | u32::from(self.logo) * input::MOD_LOGO
    }
}
