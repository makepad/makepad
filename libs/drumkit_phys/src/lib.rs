// makepad-drumkit-phys — the parked physically modelled acoustic drum kit.
//
// No samples, no impulse responses, no dependencies. Every voice is a modal
// synthesis of the actual mechanics:
// - kick, snare, toms: a circular membrane (Bessel modes with air loading),
//   its second head coupled through the shell's air column (the doublet
//   that gives a drum its long note and the kick its 400-cent drop), tension
//   modulation from the modal strain energy (the velocity-dependent glide),
//   shell and cavity modes, struck by a Hunt-Crossley felt beater / stick
//   whose force pulse shortens with velocity (membrane.rs, contact.rs);
// - snare wires: impacts gated by the snare-side head's own displacement,
//   coloured by the head and the wire formants (rattle.rs);
// - cymbals: dense statistically placed plate modes with a frequency-
//   dependent damping law and a cubic energy cascade low -> mid -> high that
//   makes a crash bloom after the strike and a soft hit not (cymbal.rs);
//   hi-hat closed/open/pedal as contact-damped, free, and clapped states of
//   the same plate with a chatter detector;
// - hand clap: a flam of 3-5 noise bursts and a tail through a body-formant
//   bank (rattle.rs).
// Measured against the Salamander Drumkit (see design.rs and the report).
//
// Real-time contract: DrumKit::process never allocates, locks, blocks, does
// IO or panics; all state is preallocated at new(). Output is bit-identical
// for any block-size decomposition of the same trigger stream: every control
// decision happens on a per-voice absolute 32-sample grid. Polyphony is
// fixed at 16 voices; the oldest is stolen. Any sample rate 44.1-96 kHz.
// Deterministic: per-hit variation (strike-point jitter, noise streams)
// comes from a generator seeded by (voice, trigger serial).

mod contact;
mod cymbal;
mod design;
mod membrane;
mod modal;
mod rattle;
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
mod simd;
mod util;
mod voice;

use voice::{Protos, Voice};

const POLYPHONY: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrumVoice {
    Kick,
    Snare,
    SideStick,
    HiHatClosed,
    HiHatOpen,
    HiHatPedal,
    TomHigh,
    TomMid,
    TomLow,
    TomFloor,
    Ride,
    RideBell,
    Crash,
    Clap,
}

impl DrumVoice {
    pub const ALL: [Self; 14] = [
        Self::Kick,
        Self::Snare,
        Self::SideStick,
        Self::HiHatClosed,
        Self::HiHatOpen,
        Self::HiHatPedal,
        Self::TomHigh,
        Self::TomMid,
        Self::TomLow,
        Self::TomFloor,
        Self::Ride,
        Self::RideBell,
        Self::Crash,
        Self::Clap,
    ];

    pub const fn gm_note(self) -> u8 {
        match self {
            Self::Kick => 36,
            Self::Snare => 38,
            Self::SideStick => 37,
            Self::HiHatClosed => 42,
            Self::HiHatOpen => 46,
            Self::HiHatPedal => 44,
            Self::TomHigh => 50,
            Self::TomMid => 48,
            Self::TomLow => 45,
            Self::TomFloor => 41,
            Self::Ride => 51,
            Self::RideBell => 53,
            Self::Crash => 49,
            Self::Clap => 39,
        }
    }

    pub(crate) const fn index(self) -> u32 {
        match self {
            Self::Kick => 0,
            Self::Snare => 1,
            Self::SideStick => 2,
            Self::HiHatClosed => 3,
            Self::HiHatOpen => 4,
            Self::HiHatPedal => 5,
            Self::TomHigh => 6,
            Self::TomMid => 7,
            Self::TomLow => 8,
            Self::TomFloor => 9,
            Self::Ride => 10,
            Self::RideBell => 11,
            Self::Crash => 12,
            Self::Clap => 13,
        }
    }
}

impl From<DrumVoice> for u8 {
    fn from(voice: DrumVoice) -> Self {
        voice.gm_note()
    }
}

impl TryFrom<u8> for DrumVoice {
    type Error = ();

    fn try_from(note: u8) -> Result<Self, Self::Error> {
        match note {
            36 => Ok(Self::Kick),
            38 => Ok(Self::Snare),
            37 => Ok(Self::SideStick),
            42 => Ok(Self::HiHatClosed),
            46 => Ok(Self::HiHatOpen),
            44 => Ok(Self::HiHatPedal),
            50 => Ok(Self::TomHigh),
            48 => Ok(Self::TomMid),
            45 => Ok(Self::TomLow),
            41 => Ok(Self::TomFloor),
            51 => Ok(Self::Ride),
            53 => Ok(Self::RideBell),
            49 => Ok(Self::Crash),
            39 => Ok(Self::Clap),
            _ => Err(()),
        }
    }
}

pub struct DrumKit {
    voices: Box<[Voice; POLYPHONY]>,
    protos: Box<Protos>,
    sample_rate: f32,
    serial: u64,
}

impl DrumKit {
    /// Builds every instrument prototype (Bessel zeros, coupled-head
    /// eigenproblems, plate mode placement) for `sample_rate`. Not
    /// real-time: a few milliseconds.
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() { sample_rate.clamp(8000.0, 384_000.0) } else { 48_000.0 };
        Self { voices: Box::new([Voice::idle(); POLYPHONY]), protos: Box::new(Protos::build(sample_rate)), sample_rate, serial: 0 }
    }

    /// Starts a hit at the beginning of the next `process` block. Steals the
    /// oldest voice when all 16 are sounding. Allocation-free.
    pub fn trigger(&mut self, voice: DrumVoice, velocity: f32) {
        let velocity = if velocity.is_finite() { velocity.clamp(0.0, 1.0) } else { 0.0 };
        if velocity <= 0.0 {
            return;
        }
        self.serial = self.serial.wrapping_add(1);
        let slot = self.voices.iter().position(|v| !v.active).unwrap_or_else(|| {
            let mut best = 0;
            for (i, v) in self.voices.iter().enumerate() {
                if v.serial < self.voices[best].serial {
                    best = i;
                }
            }
            best
        });
        self.voices[slot].start(voice, velocity, self.serial, &self.protos, self.sample_rate);
    }

    /// Adds the kit into `out`. Never allocates, locks, panics or does IO.
    pub fn process(&mut self, out: &mut [[f32; 2]]) {
        for v in self.voices.iter_mut() {
            if v.active {
                v.render(out);
            }
        }
    }

    pub fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.active = false;
        }
    }

    pub fn active(&self) -> bool {
        self.voices.iter().any(|v| v.active)
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gm_notes_round_trip() {
        for voice in DrumVoice::ALL {
            assert_eq!(DrumVoice::try_from(u8::from(voice)), Ok(voice));
        }
        assert!(DrumVoice::try_from(35).is_err());
        assert_eq!(DrumVoice::Clap.gm_note(), 39);
    }

    #[test]
    fn process_uses_fixed_voice_storage_and_steals_oldest() {
        let mut kit = DrumKit::new(48000.0);
        for index in 0..(POLYPHONY * 3) {
            kit.trigger(DrumVoice::ALL[index % DrumVoice::ALL.len()], 1.0);
        }
        assert_eq!(kit.voices.len(), POLYPHONY);
        let oldest = kit.voices.iter().map(|v| v.serial).min().unwrap();
        assert_eq!(oldest, (POLYPHONY * 2 + 1) as u64);
        let mut block = [[0.0; 2]; 128];
        kit.process(&mut block);
        assert!(block.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
        kit.all_off();
        assert!(!kit.active());
    }
}
