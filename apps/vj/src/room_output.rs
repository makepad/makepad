//! Where the programme goes: the system default, as it always has, or one
//! output the operator pinned by name.
//!
//! Worked out from the device list alone, so every case is a test and none
//! of them needs a rig. The page in `main.rs` hands the result to the
//! platform and says what happened in the log.

use makepad_widgets::*;

/// How the pin fared on a resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomPin {
    /// No pin: the room follows the system default.
    Follow,
    /// The pinned output is here and the room is on it.
    Held,
    /// The pinned output is not here, or has failed: the room is on the
    /// default until it comes back.
    Missing,
    /// The pin names the phones device, which the room may not take: the
    /// room is on the default.
    Refused,
}

/// The two output slots as the rig wants them: the room first, the phones
/// second. The order is law, because device callback slots are positional
/// and captured at thread spawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomRequest {
    pub main: Option<AudioDeviceId>,
    pub phones: Option<AudioDeviceId>,
    pub pin: RoomPin,
}

impl RoomRequest {
    /// The devices to open, in slot order. The phones never ride without
    /// the room.
    pub fn devices(&self) -> Vec<AudioDeviceId> {
        let mut out = Vec::new();
        if let Some(main) = self.main {
            out.push(main);
            if let Some(phones) = self.phones {
                out.push(phones);
            }
        }
        out
    }
}

/// Build the request. `default` is the system default output as the
/// platform's fallback chain resolves it; `pin` is the pinned output name
/// and `phones` the phones device name, both matched exactly.
///
/// The pin is refused when it names the phones device: the cue may not
/// become the room, the way the phones picker refuses the room. Absent, the
/// room follows the default, which is what every install did before the
/// pin existed.
pub fn build_room_request(
    outputs: &[AudioDeviceDesc],
    default: Option<AudioDeviceId>,
    pin: Option<&str>,
    phones: Option<&str>,
) -> RoomRequest {
    let by_name = |name: &str| {
        outputs
            .iter()
            .find(|desc| !desc.has_failed && desc.name == name)
            .map(|desc| desc.device_id)
    };
    let (main, pin) = match pin {
        None => (default, RoomPin::Follow),
        Some(name) if phones == Some(name) => (default, RoomPin::Refused),
        Some(name) => match by_name(name) {
            Some(id) => (Some(id), RoomPin::Held),
            None => (default, RoomPin::Missing),
        },
    };
    // Resolved by EXACT name, never through a no-match fallback to the
    // default: that is the one device the cue must never land on. The
    // room offered as phones would silently keep its slot-0 binding, so
    // it counts as unconfigured instead.
    let phones = phones.and_then(by_name).filter(|id| Some(*id) != main);
    RoomRequest { main, phones, pin }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> AudioDeviceId {
        AudioDeviceId(LiveId(n))
    }

    fn desc(name: &str, n: u64, is_default: bool, has_failed: bool) -> AudioDeviceDesc {
        AudioDeviceDesc {
            device_id: id(n),
            device_type: AudioDeviceType::Output,
            is_default,
            has_failed,
            channel_count: 2,
            name: name.to_string(),
        }
    }

    /// Speakers are the default, an interface and a headset beside them.
    fn rig() -> Vec<AudioDeviceDesc> {
        vec![
            desc("Speakers", 1, true, false),
            desc("Interface", 2, false, false),
            desc("Headset", 3, false, false),
        ]
    }

    #[test]
    fn no_pin_is_the_request_the_rig_always_made() {
        let request = build_room_request(&rig(), Some(id(1)), None, Some("Headset"));
        assert_eq!(request.pin, RoomPin::Follow);
        assert_eq!(request.devices(), vec![id(1), id(3)]);
        let alone = build_room_request(&rig(), Some(id(1)), None, None);
        assert_eq!(alone.devices(), vec![id(1)], "no phones, no second slot");
    }

    #[test]
    fn a_pinned_output_that_is_here_takes_the_room() {
        let request = build_room_request(&rig(), Some(id(1)), Some("Interface"), Some("Headset"));
        assert_eq!(request.pin, RoomPin::Held);
        assert_eq!(request.devices(), vec![id(2), id(3)]);
    }

    /// The case the pin exists for: the OS default has become the phones
    /// device. Following it would drop the cue; the pin keeps both.
    #[test]
    fn a_default_that_became_the_phones_no_longer_swallows_them() {
        let followed = build_room_request(&rig(), Some(id(3)), None, Some("Headset"));
        assert_eq!(followed.devices(), vec![id(3)], "today: the cue is gone");
        let pinned = build_room_request(&rig(), Some(id(3)), Some("Interface"), Some("Headset"));
        assert_eq!(pinned.devices(), vec![id(2), id(3)], "pinned: room and cue both stand");
    }

    #[test]
    fn a_pinned_output_that_is_not_here_falls_back_to_the_default_and_says_so() {
        let request = build_room_request(&rig(), Some(id(1)), Some("Studio Monitors"), Some("Headset"));
        assert_eq!(request.pin, RoomPin::Missing);
        assert_eq!(request.devices(), vec![id(1), id(3)]);
    }

    #[test]
    fn a_pinned_output_that_has_failed_counts_as_missing() {
        let mut outputs = rig();
        outputs[1].has_failed = true;
        let request = build_room_request(&outputs, Some(id(1)), Some("Interface"), None);
        assert_eq!(request.pin, RoomPin::Missing);
        assert_eq!(request.main, Some(id(1)));
    }

    #[test]
    fn a_pin_on_the_phones_device_is_refused_and_the_phones_stay() {
        let request = build_room_request(&rig(), Some(id(1)), Some("Headset"), Some("Headset"));
        assert_eq!(request.pin, RoomPin::Refused);
        assert_eq!(request.devices(), vec![id(1), id(3)]);
    }

    #[test]
    fn phones_on_the_room_device_are_dropped_not_doubled() {
        let request = build_room_request(&rig(), Some(id(1)), None, Some("Speakers"));
        assert_eq!(request.devices(), vec![id(1)]);
        let pinned = build_room_request(&rig(), Some(id(1)), Some("Interface"), Some("Interface"));
        assert_eq!(pinned.pin, RoomPin::Refused, "the same name is refused before it gets that far");
        assert_eq!(pinned.devices(), vec![id(1), id(2)]);
    }

    #[test]
    fn a_failed_phones_device_is_not_requested() {
        let mut outputs = rig();
        outputs[2].has_failed = true;
        let request = build_room_request(&outputs, Some(id(1)), None, Some("Headset"));
        assert_eq!(request.devices(), vec![id(1)]);
    }

    #[test]
    fn without_a_room_nothing_is_requested() {
        let request = build_room_request(&rig(), None, Some("Studio Monitors"), Some("Headset"));
        assert_eq!(request.devices(), Vec::<AudioDeviceId>::new(), "the phones never ride alone");
        let empty = build_room_request(&[], None, None, None);
        assert_eq!(empty.pin, RoomPin::Follow);
        assert!(empty.devices().is_empty());
    }
}
