//! Akai APC40 MkII performance mapping.
//!
//! This module is deliberately UI- and platform-free: raw three-byte MIDI
//! messages become semantic performance actions, and semantic pad states
//! become delta-compressed LED messages. The app adapter only selects the
//! matching Makepad MIDI ports and dispatches actions to its existing cue,
//! deck, and SFX engines.

pub const PAD_COUNT: usize = 40;
pub const PAGE_SIZE: usize = PAD_COUNT;

pub const NOTE_PAN: u8 = 0x57;
pub const NOTE_SENDS: u8 = 0x58;
// NOTE 0x59 (USER) is automate lighting POWER. Do not steal it for a VJ surface.
pub const NOTE_PLAY: u8 = 0x5b;
pub const NOTE_STOP: u8 = 0x5c;
pub const NOTE_UP: u8 = 0x5e;
pub const NOTE_DOWN: u8 = 0x5f;
pub const NOTE_RIGHT: u8 = 0x60;
pub const NOTE_LEFT: u8 = 0x61;
pub const CC_MASTER: u8 = 0x0e;
pub const CC_CROSSFADER: u8 = 0x0f;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApcSurface {
    #[default]
    Video,
    Music,
    Sfx,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ApcAction {
    Pad {
        surface: ApcSurface,
        pad: usize,
        index: usize,
        pressed: bool,
    },
    Surface(ApcSurface),
    VideoPlayPause,
    VideoStop,
    Master(f32),
    Crossfader(f32),
    BankChanged,
}

#[derive(Default)]
pub struct Apc40State {
    pub surface: ApcSurface,
    pub bank: usize,
}

impl Apc40State {
    pub fn decode(&mut self, data: [u8; 3]) -> Option<ApcAction> {
        let status = data[0] >> 4;
        let is_note = status == 0x8 || status == 0x9;
        let pressed = status == 0x9 && data[2] != 0;
        if is_note {
            let note = data[1];
            if (note as usize) < PAD_COUNT {
                return Some(ApcAction::Pad {
                    surface: self.surface,
                    pad: note as usize,
                    index: self.bank.saturating_add(note as usize),
                    pressed,
                });
            }
            if !pressed {
                return None;
            }
            return match note {
                NOTE_PAN => {
                    self.surface = ApcSurface::Video;
                    self.bank = 0;
                    Some(ApcAction::Surface(self.surface))
                }
                NOTE_SENDS => {
                    self.surface = ApcSurface::Music;
                    self.bank = 0;
                    Some(ApcAction::Surface(self.surface))
                }
                NOTE_PLAY => Some(ApcAction::VideoPlayPause),
                NOTE_STOP => Some(ApcAction::VideoStop),
                NOTE_UP => {
                    self.bank = self.bank.saturating_sub(PAGE_SIZE);
                    Some(ApcAction::BankChanged)
                }
                NOTE_DOWN => {
                    self.bank = self.bank.saturating_add(PAGE_SIZE);
                    Some(ApcAction::BankChanged)
                }
                NOTE_LEFT => {
                    self.bank = self.bank.saturating_sub(8);
                    Some(ApcAction::BankChanged)
                }
                NOTE_RIGHT => {
                    self.bank = self.bank.saturating_add(8);
                    Some(ApcAction::BankChanged)
                }
                _ => None,
            };
        }
        if status == 0xb {
            let value = data[2] as f32 / 127.0;
            return match data[1] {
                CC_MASTER => Some(ApcAction::Master(value)),
                CC_CROSSFADER => Some(ApcAction::Crossfader(value)),
                _ => None,
            };
        }
        None
    }

    pub fn clamp_bank(&mut self, item_count: usize) {
        if item_count == 0 {
            self.bank = 0;
            return;
        }
        let last_page = ((item_count - 1) / PAGE_SIZE) * PAGE_SIZE;
        self.bank = self.bank.min(last_page);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PadLed {
    #[default]
    Off,
    Ready,
    Queued,
    Live,
    Failed,
}

impl PadLed {
    fn message(self, note: u8) -> [u8; 3] {
        // APC40 MkII palette velocities: blue 41, amber 126, green 21,
        // red 5. Live uses the device's gentle 1/8 pulse LED type.
        let (channel, velocity) = match self {
            Self::Off => (0, 0),
            Self::Ready => (0, 41),
            Self::Queued => (0, 126),
            Self::Live => (8, 21),
            Self::Failed => (0, 5),
        };
        [0x90 | channel, note, velocity]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedFrame {
    pub pads: [PadLed; PAD_COUNT],
    pub surface: ApcSurface,
    pub video_playing: bool,
}

impl Default for LedFrame {
    fn default() -> Self {
        Self {
            pads: [PadLed::Off; PAD_COUNT],
            surface: ApcSurface::Video,
            video_playing: false,
        }
    }
}

#[derive(Default)]
pub struct LedDiff {
    last: Option<LedFrame>,
}

impl LedDiff {
    pub fn update(&mut self, next: LedFrame) -> Vec<[u8; 3]> {
        let mut out = Vec::new();
        for (index, state) in next.pads.iter().copied().enumerate() {
            if self.last.as_ref().is_none_or(|last| last.pads[index] != state) {
                out.push(state.message(index as u8));
            }
        }
        for (surface, note) in [
            (ApcSurface::Video, NOTE_PAN),
            (ApcSurface::Music, NOTE_SENDS),
        ] {
            let changed = self.last.as_ref().is_none_or(|last| last.surface != next.surface);
            if changed {
                out.push([0x90, note, if next.surface == surface { 127 } else { 0 }]);
            }
        }
        if self
            .last
            .as_ref()
            .is_none_or(|last| last.video_playing != next.video_playing)
        {
            out.push([0x90, NOTE_PLAY, if next.video_playing { 127 } else { 0 }]);
        }
        self.last = Some(next);
        out
    }

    pub fn invalidate(&mut self) {
        self.last = None;
    }
}

pub fn is_apc40_port(name: &str) -> bool {
    let compact: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    // The original APC40 uses a different channel/note grid map. Never
    // silently attach this MkII protocol adapter to that device.
    compact.contains("apc40mkii") || compact.contains("apc40mk2")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_press_release_and_banking_are_exact() {
        let mut state = Apc40State::default();
        assert_eq!(
            state.decode([0x90, 39, 127]),
            Some(ApcAction::Pad {
                surface: ApcSurface::Video,
                pad: 39,
                index: 39,
                pressed: true,
            })
        );
        assert_eq!(state.decode([0x90, NOTE_DOWN, 127]), Some(ApcAction::BankChanged));
        assert_eq!(state.bank, 40);
        assert_eq!(
            state.decode([0x80, 0, 127]),
            Some(ApcAction::Pad {
                surface: ApcSurface::Video,
                pad: 0,
                index: 40,
                pressed: false,
            })
        );
        state.clamp_bank(41);
        assert_eq!(state.bank, 40);
        state.clamp_bank(40);
        assert_eq!(state.bank, 0);
    }

    #[test]
    fn mode_and_continuous_controls_decode() {
        let mut state = Apc40State::default();
        assert_eq!(
            state.decode([0x90, NOTE_SENDS, 127]),
            Some(ApcAction::Surface(ApcSurface::Music))
        );
        assert_eq!(state.surface, ApcSurface::Music);
        assert_eq!(state.decode([0xb0, CC_CROSSFADER, 127]), Some(ApcAction::Crossfader(1.0)));
        assert_eq!(state.decode([0xb0, CC_MASTER, 0]), Some(ApcAction::Master(0.0)));
    }

    #[test]
    fn pad_action_captures_surface_before_later_batch_messages() {
        let mut state = Apc40State::default();
        let pad = state.decode([0x90, 7, 127]);
        assert_eq!(
            state.decode([0x90, NOTE_SENDS, 127]),
            Some(ApcAction::Surface(ApcSurface::Music))
        );
        assert_eq!(
            pad,
            Some(ApcAction::Pad {
                surface: ApcSurface::Video,
                pad: 7,
                index: 7,
                pressed: true,
            })
        );
    }

    #[test]
    fn led_output_is_delta_compressed_and_live_pulses() {
        let mut diff = LedDiff::default();
        let mut frame = LedFrame::default();
        frame.pads[3] = PadLed::Ready;
        frame.pads[4] = PadLed::Live;
        let first = diff.update(frame.clone());
        assert!(first.contains(&[0x90, 3, 41]));
        assert!(first.contains(&[0x98, 4, 21]));
        assert!(diff.update(frame.clone()).is_empty());
        frame.pads[3] = PadLed::Failed;
        assert_eq!(diff.update(frame), vec![[0x90, 3, 5]]);
    }

    #[test]
    fn port_match_is_specific_but_tolerates_spacing() {
        assert!(is_apc40_port("Akai APC40 mkII"));
        assert!(is_apc40_port("APC 40 mk2 Control"));
        assert!(!is_apc40_port("Akai APC40"));
        assert!(!is_apc40_port("Launchpad Pro"));
    }
}
