//! A person at this machine: the last input event the HAL's own device readers took from a
//! device with a known origin. A WM asks this (`makepad-wm activity`) so devhub can tell a
//! person using the box from work running on it.
//!
//! Only device reports count: a physical device (any bus but the kernel's virtual one) or a
//! Desk Link viewer's devices (the uinput devices the WM makes for a viewer's input; only the
//! fleet admin's viewers get input). Events an app or the remote endpoint injects, other
//! virtual devices (uinput tools), accelerometers and a gamepad's sticks never do, and nothing
//! about what runs (windows, animation, load) is looked at.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

/// Where a counted input came from.
#[makepad_relax_api::relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// a physical keyboard, mouse, touchpad, touchscreen or gamepad
    Device,
    /// the user's Desk Link viewer (its uinput devices on this desktop)
    DeskLink,
}

/// What the input was.
#[makepad_relax_api::relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Key,
    Pointer,
    Gamepad,
}

/// The last counted input: how many so far (this process), when (`crate::clock_ns`), from
/// where, what.
#[makepad_relax_api::relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserInput {
    pub seq: u64,
    pub at_ns: u64,
    pub origin: Origin,
    pub what: What,
}

static SEQ: AtomicU64 = AtomicU64::new(0);
/// Desk Link viewers connected now (a person may be watching with no input for a while).
static VIEWERS: AtomicU64 = AtomicU64::new(0);

/// The WM says how many Desk Link viewers are connected.
#[makepad_relax_api::relax_api]
pub fn set_viewers(n: usize) {
    VIEWERS.store(n as u64, std::sync::atomic::Ordering::Relaxed);
}

/// Desk Link viewers connected now.
#[makepad_relax_api::relax_api]
pub fn viewers() -> u64 {
    VIEWERS.load(std::sync::atomic::Ordering::Relaxed)
}

static AT_NS: AtomicU64 = AtomicU64::new(0);
/// origin * 4 + what (0: none yet)
static KIND: AtomicU8 = AtomicU8::new(0);

/// The tests that note and read the one process-wide record take turns.
#[cfg(test)]
pub(crate) static TEST_TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The kernel's virtual bus (uinput devices).
const BUS_VIRTUAL: u16 = 0x06;

/// The origin of an evdev device by its id (`EVIOCGID`): None for a device whose input never
/// counts (a virtual device that is not Desk Link's).
pub fn origin_of(bustype: u16, vendor: u16) -> Option<Origin> {
    match (bustype, vendor) {
        (BUS_VIRTUAL, crate::direct::DESK_LINK_INPUT_VENDOR) => Some(Origin::DeskLink),
        (BUS_VIRTUAL, _) => None,
        _ => Some(Origin::Device),
    }
}

/// A device reader took a report of real input.
pub fn note(origin: Origin, what: What) {
    let o = match origin {
        Origin::Device => 1,
        Origin::DeskLink => 2,
    };
    let w = match what {
        What::Key => 1,
        What::Pointer => 2,
        What::Gamepad => 3,
    };
    AT_NS.store(crate::clock_ns(), Ordering::Release);
    KIND.store(o * 4 + w, Ordering::Release);
    SEQ.fetch_add(1, Ordering::AcqRel);
}

/// The last counted input; None before the first.
#[makepad_relax_api::relax_api]
pub fn last() -> Option<UserInput> {
    let seq = SEQ.load(Ordering::Acquire);
    let kind = KIND.load(Ordering::Acquire);
    if seq == 0 || kind == 0 {
        return None;
    }
    let origin = if kind / 4 == 2 { Origin::DeskLink } else { Origin::Device };
    let what = match kind % 4 {
        1 => What::Key,
        3 => What::Gamepad,
        _ => What::Pointer,
    };
    Some(UserInput { seq, at_ns: AT_NS.load(Ordering::Acquire), origin, what })
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Device => "device",
            Origin::DeskLink => "desk link",
        }
    }
}

impl What {
    pub fn as_str(self) -> &'static str {
        match self {
            What::Key => "key",
            What::Pointer => "pointer",
            What::Gamepad => "gamepad",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_physical_and_desk_link_devices_count() {
        let _turn = TEST_TURN.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(origin_of(0x03, 0x046d), Some(Origin::Device), "a USB mouse");
        assert_eq!(origin_of(0x11, 0x0001), Some(Origin::Device), "a PS/2 keyboard");
        assert_eq!(origin_of(0x19, 0x0000), Some(Origin::Device), "the power button");
        assert_eq!(origin_of(BUS_VIRTUAL, crate::direct::DESK_LINK_INPUT_VENDOR), Some(Origin::DeskLink));
        assert_eq!(origin_of(BUS_VIRTUAL, 0x1234), None, "another uinput tool's device");
        note(Origin::DeskLink, What::Key);
        let l = last().unwrap();
        assert_eq!((l.origin, l.what), (Origin::DeskLink, What::Key));
        note(Origin::Device, What::Gamepad);
        let m = last().unwrap();
        assert!(m.seq > l.seq && m.at_ns >= l.at_ns);
        assert_eq!((m.origin, m.what), (Origin::Device, What::Gamepad));
    }
}
