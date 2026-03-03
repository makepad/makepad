use makepad_micro_serde::*;

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, Eq, PartialEq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool,
}

impl KeyModifiers {
    /// Returns true if the primary key modifier is active (pressed).
    ///
    /// The primary modifier is Logo key (Command ⌘) on macOS
    /// and the Control key on all other platforms.
    pub fn is_primary(&self) -> bool {
        #[cfg(target_vendor = "apple")]
        {
            self.logo
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            self.control
        }
    }

    pub fn any(&self) -> bool {
        self.shift || self.control || self.alt || self.logo
    }
}

bitflags::bitflags! {
    /// A `u32` bit mask of all mouse buttons that were pressed
    /// during a given mouse event.
    ///
    /// This is a bit mask because it is possible for multiple buttons
    /// to be pressed simultaneously during a given input event.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    #[doc(alias = "click")]
    pub struct MouseButton: u32 {
        /// The primary mouse button, typically the left-click button.
        #[doc(alias("left", "left-click"))]
        const PRIMARY =   1 << 0;
        /// The secondary mouse button, typically the right-click button.
        #[doc(alias("right", "right-click"))]
        const SECONDARY = 1 << 1;
        /// The middle mouse button, typically the scroll-wheel click button.
        #[doc(alias("scroll", "wheel"))]
        const MIDDLE =    1 << 2;
        /// The fourth mouse button, typically used for back navigation.
        const BACK =      1 << 3;
        /// The fifth mouse button, typically used for forward navigation.
        const FORWARD =   1 << 4;

        // Ensure that all bits are valid, such that no bits get truncated.
        const _ = !0;
    }
}

impl SerBin for MouseButton {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.bits().ser_bin(s);
    }
}

impl DeBin for MouseButton {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        Ok(MouseButton::from_bits_retain(u32::de_bin(o, d)?))
    }
}

impl SerJson for MouseButton {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        self.bits().ser_json(d, s);
    }
}

impl DeJson for MouseButton {
    fn de_json(s: &mut DeJsonState, i: &mut std::str::Chars) -> Result<Self, DeJsonErr> {
        Ok(MouseButton::from_bits_retain(u32::de_json(s, i)?))
    }
}

impl MouseButton {
    /// Returns true if the primary mouse button is pressed.
    pub fn is_primary(&self) -> bool {
        self.contains(MouseButton::PRIMARY)
    }
    /// Returns true if the secondary mouse button is pressed.
    pub fn is_secondary(&self) -> bool {
        self.contains(MouseButton::SECONDARY)
    }
    /// Returns true if the middle mouse button is pressed.
    pub fn is_middle(&self) -> bool {
        self.contains(MouseButton::MIDDLE)
    }
    /// Returns true if the back mouse button is pressed.
    pub fn is_back(&self) -> bool {
        self.contains(MouseButton::BACK)
    }
    /// Returns true if the forward mouse button is pressed.
    pub fn is_forward(&self) -> bool {
        self.contains(MouseButton::FORWARD)
    }
    /// Returns true if the `n`th button is pressed.
    ///
    /// The button values are:
    /// * n = 0: PRIMARY
    /// * n = 1: SECONDARY
    /// * n = 2: MIDDLE
    /// * n = 3: BACK
    /// * n = 4: FORWARD
    /// * n > 4: other/custom
    pub fn is_other_button(&self, n: u8) -> bool {
        self.bits() & (1 << n) != 0
    }
    /// Returns a `MouseButton` bit mask based on the raw button value: `1 << raw`.
    ///
    /// A raw button value is a number that represents a mouse button, like so:
    /// * 0: MouseButton::PRIMARY
    /// * 1: MouseButton::SECONDARY
    /// * 2: MouseButton::MIDDLE
    /// * 3: MouseButton::BACK
    /// * 4: MouseButton::FORWARD
    /// * etc.
    pub fn from_raw_button(raw: usize) -> MouseButton {
        MouseButton::from_bits_retain(1 << raw)
    }
}
