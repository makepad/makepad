//! One playable-instrument contract for every synth and sampler in Makepad:
//! the modelled piano, the drum kits, SoundFonts, Ironfish and the recipe
//! poly-synth all implement [`Instrument`], so a game, Stage or a score
//! player can drive any of them the same way.
//!
//! Real-time contract: `note_on`, `note_off`, `control` and `render` never
//! allocate, lock, block or do I/O. Building an instrument (loading a
//! SoundFont, calibrating the piano) is the caller's job, off the audio
//! thread; the built value is then moved to whoever renders it.

/// Continuous controls, MIDI-flavoured but in normalised units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// Sustain (damper) pedal, 0..1 (half-pedalling where supported).
    Sustain,
    /// Soft (una corda) pedal, 0 or 1.
    Soft,
    /// Sostenuto pedal, 0 or 1.
    Sostenuto,
    /// Channel volume, 0..1.
    Volume,
    /// Modulation wheel, 0..1.
    Modulation,
    /// Pitch bend in semitones (−12..12).
    PitchBend,
    /// Filter cutoff / brightness, 0..1.
    Brightness,
}

impl Control {
    pub fn parse(s: &str) -> Option<Control> {
        Some(match s {
            "sustain" | "pedal" | "damper" => Control::Sustain,
            "soft" | "una_corda" => Control::Soft,
            "sostenuto" => Control::Sostenuto,
            "volume" => Control::Volume,
            "mod" | "modulation" => Control::Modulation,
            "bend" | "pitch_bend" => Control::PitchBend,
            "brightness" | "cutoff" => Control::Brightness,
            _ => return None,
        })
    }
}

pub trait Instrument: Send {
    /// Short identity for diagnostics ("piano", "drums", "ironfish", ...).
    fn name(&self) -> &str;
    /// The rate this instance renders at.
    fn sample_rate(&self) -> f32;
    /// Start a note. `velocity` 1..127 (0 is a note-off, MIDI style).
    fn note_on(&mut self, note: u8, velocity: u8);
    fn note_off(&mut self, note: u8);
    /// A continuous control; instruments ignore what they do not have.
    fn control(&mut self, _control: Control, _value: f32) {}
    /// Release every note (and pedal) at once.
    fn all_notes_off(&mut self);
    /// Render the next `out_l.len()` frames, OVERWRITING both slices.
    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]);
    /// False when nothing is sounding (a renderer may skip the call).
    fn is_active(&self) -> bool {
        true
    }
}

impl Instrument for crate::ironfish::Ironfish {
    fn name(&self) -> &str {
        "ironfish"
    }

    fn sample_rate(&self) -> f32 {
        self.rate()
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 {
            crate::ironfish::Ironfish::note_off(self, note);
        } else {
            crate::ironfish::Ironfish::note_on(self, note, velocity);
        }
    }

    fn note_off(&mut self, note: u8) {
        crate::ironfish::Ironfish::note_off(self, note);
    }

    fn all_notes_off(&mut self) {
        crate::ironfish::Ironfish::all_notes_off(self);
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        for (l, r) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let [a, b] = self.next_frame();
            *l = a;
            *r = b;
        }
    }
}

/// General MIDI percussion numbers by name: the drum names `note_on` and
/// the drum kits answer to (`kick` 36, `snare` 38, `hat` 42, ...).
pub fn drum_note(name: &str) -> Option<u8> {
    Some(match name {
        "kick" | "bd" => 36,
        "snare" | "sd" => 38,
        "rim" | "side" | "sidestick" => 37,
        "clap" => 39,
        "hat" | "hihat" | "hh" => 42,
        "openhat" | "ohat" | "oh" => 46,
        "pedalhat" => 44,
        "tom1" | "hitom" => 50,
        "tom2" | "midtom" => 47,
        "tom3" | "lowtom" => 45,
        "floortom" => 43,
        "crash" => 49,
        "ride" => 51,
        "bell" | "ridebell" => 53,
        _ => return None,
    })
}
