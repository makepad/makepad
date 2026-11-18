//! The waveguide grand (`makepad-piano-waveguide`) as an [`Instrument`]: the
//! second modelled piano, listed beside the modal one's presets.

use crate::instrument::{Control, Instrument};
use makepad_piano_waveguide::{PianoEvent, TimedEvent, WaveguidePiano, PRESET_NAME};

const MAX_PENDING: usize = 256;

pub struct WaveguideInstrument {
    piano: WaveguidePiano,
    events: [TimedEvent; MAX_PENDING],
    count: usize,
}

impl WaveguideInstrument {
    /// The preset name that selects this instrument.
    pub const PRESET: &'static str = PRESET_NAME;

    /// Slow (designs 88 keys and allocates): never on the audio thread.
    pub fn new(rate: f32) -> Self {
        WaveguideInstrument {
            piano: WaveguidePiano::new(rate),
            events: [TimedEvent { offset: 0, event: PianoEvent::AllSoundOff }; MAX_PENDING],
            count: 0,
        }
    }

    fn push(&mut self, event: PianoEvent) {
        // A full queue drops the newest event: 256 events inside one audio
        // block is a stuck producer, not music.
        if self.count < MAX_PENDING {
            self.events[self.count] = TimedEvent { offset: 0, event };
            self.count += 1;
        }
    }
}

impl Instrument for WaveguideInstrument {
    fn name(&self) -> &str {
        "piano"
    }

    fn sample_rate(&self) -> f32 {
        self.piano.sample_rate()
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        self.push(PianoEvent::NoteOn { key: note.min(127), velocity: velocity.min(127) });
    }

    fn note_off(&mut self, note: u8) {
        self.push(PianoEvent::NoteOff { key: note.min(127) });
    }

    fn control(&mut self, control: Control, value: f32) {
        let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        match control {
            Control::Sustain => self.push(PianoEvent::Sustain { value }),
            Control::Soft => self.push(PianoEvent::SoftPedal { on: value >= 0.5 }),
            Control::Sostenuto => self.push(PianoEvent::Sostenuto { on: value >= 0.5 }),
            Control::Volume => self.piano.set_master_gain(value * 2.0),
            _ => {}
        }
    }

    fn all_notes_off(&mut self) {
        // Released, not cut: the dampers fall and the board rings out.
        self.push(PianoEvent::Sustain { value: 0.0 });
        for key in 21..=108u8 {
            self.push(PianoEvent::NoteOff { key });
        }
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len().min(out_r.len());
        if len == 0 {
            return;
        }
        self.piano.process(&self.events[..self.count], &mut out_l[..len], &mut out_r[..len]);
        self.count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instruments::{build, parse_kind, PianoInstrument};

    fn rms(inst: &mut dyn Instrument, frames: usize) -> f32 {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            inst.render(cl, cr);
        }
        assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()));
        (l.iter().chain(r.iter()).map(|x| x * x).sum::<f32>() / (2 * frames) as f32).sqrt()
    }

    #[test]
    fn the_waveguide_grand_is_a_piano_preset_that_plays_and_damps() {
        assert!(PianoInstrument::preset_names().any(|p| p == WaveguideInstrument::PRESET), "listed beside the modal presets");
        assert!(PianoInstrument::has_preset("waveguide grand"), "found whatever the case");
        let kind = parse_kind("piano", Some(WaveguideInstrument::PRESET)).unwrap();
        let rate = 48_000.0;
        let mut held = build(&kind, rate).unwrap();
        assert_eq!((held.name(), held.sample_rate()), ("piano", rate));
        held.control(Control::Sustain, 1.0);
        held.note_on(60, 100);
        let struck = rms(held.as_mut(), 9_600);
        assert!(struck > 0.01 && struck < 0.5, "a forte middle C at a usable level: {struck}");
        held.note_off(60);
        rms(held.as_mut(), 14_400);
        let pedalled = rms(held.as_mut(), 9_600);

        let mut released = build(&kind, rate).unwrap();
        released.note_on(60, 100);
        rms(released.as_mut(), 9_600);
        released.note_off(60);
        rms(released.as_mut(), 14_400);
        let damped = rms(released.as_mut(), 9_600);
        assert!(pedalled > 10.0 * damped, "the pedal holds what a release damps: {pedalled} vs {damped}");

        held.all_notes_off();
        rms(held.as_mut(), 24_000);
        assert!(rms(held.as_mut(), 9_600) < 0.05 * pedalled, "all_notes_off lets the dampers fall");
    }
}
