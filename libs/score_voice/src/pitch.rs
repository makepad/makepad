//! The sung pitch: a note target reached through a damped spring (legato
//! glides with a slight overshoot, a scoop into a fresh note), vibrato that
//! waits and then fades in (and dies away into the next note rather than
//! stopping mid-swing), and slow drift plus fast jitter. All of it is a
//! deterministic function of the notes and a seed, advanced once a block.
//!
//! In tune: vibrato swings evenly about the note, drift and jitter are
//! zero-mean and small enough that a held note's centre stays within a few
//! cents, and a glide is timed from the syllable's start to land on its
//! vowel (a vowel-initial join, which has no consonant to glide through,
//! lands within about 60 ms).

/// A deterministic generator (xorshift).
#[derive(Clone, Copy, Debug)]
pub struct Rng(pub u32);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9).wrapping_add(0x7F4A_7C15) | 1)
    }

    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// Uniform in -1..1.
    pub fn bipolar(&mut self) -> f32 {
        self.next_u32() as f32 * (2.0 / 4_294_967_296.0) - 1.0
    }

    /// Uniform in 0..1.
    pub fn unit(&mut self) -> f32 {
        self.next_u32() as f32 * (1.0 / 4_294_967_296.0)
    }
}

/// Low-passed noise of a given standard deviation.
#[derive(Clone, Copy, Debug)]
pub struct Wander {
    y: f32,
    a: f32,
    scale: f32,
}

impl Wander {
    /// `cutoff` Hz at `ctrl_rate` updates a second.
    pub fn new(cutoff: f32, std: f32, ctrl_rate: f32) -> Wander {
        let a = (-std::f32::consts::TAU * cutoff / ctrl_rate).exp();
        // One pole over uniform noise (variance 1/3): output variance is
        // (1/3)(1-a)/(1+a) per unit input.
        let scale = std / ((1.0 - a) / (1.0 + a) / 3.0).sqrt();
        Wander { y: 0.0, a, scale }
    }

    pub fn next(&mut self, rng: &mut Rng) -> f32 {
        self.y = self.a * self.y + (1.0 - self.a) * rng.bipolar() * self.scale;
        self.y
    }
}

/// How a voice sings: the expressive parameters of the pitch and timbre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Vibrato rate (Hz) and peak depth (semitones).
    pub vibrato_rate: f32,
    pub vibrato_depth: f32,
    /// Seconds after the vowel starts before vibrato begins, and its fade in.
    pub vibrato_delay: f32,
    pub vibrato_fade: f32,
    /// Slow pitch drift and fast jitter, standard deviation in cents. The
    /// drift wanders on a quarter-second scale, so a note's centre moves by
    /// about 0.6 of it: a few cents is a trained singer, 6 an unsteady one.
    pub drift_cents: f32,
    pub jitter_cents: f32,
    /// Legato glide time (seconds; wider intervals take a little longer).
    /// A glide lands by the vowel plus half this at the latest.
    pub portamento: f32,
    /// How far below a fresh note the voice starts (semitones).
    pub scoop: f32,
    /// Breathiness 0..1 (added to the profile's).
    pub breath: f32,
    /// Formant scale (1 = the profile's own vocal tract).
    pub formant_scale: f32,
    /// A fixed pitch offset (cents): a choir voice's detune.
    pub detune_cents: f32,
    /// Coarticulation: the width of a transition between phonemes (s).
    pub transition: f32,
    /// Output gain.
    pub gain: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            vibrato_rate: 5.4, vibrato_depth: 0.3, vibrato_delay: 0.22, vibrato_fade: 0.35, drift_cents: 2.5, jitter_cents: 2.5,
            portamento: 0.08, scoop: 0.3, breath: 0.0, formant_scale: 1.0, detune_cents: 0.0, transition: 0.05, gain: 1.0,
        }
    }
}

impl Style {
    /// A dead-straight voice: no vibrato, drift, jitter or scoop (tests).
    pub fn straight() -> Style {
        Style { vibrato_depth: 0.0, drift_cents: 0.0, jitter_cents: 0.0, scoop: 0.0, ..Style::default() }
    }
}

pub struct Pitch {
    /// Spring state (MIDI note) and velocity.
    x: f32,
    v: f32,
    target: f32,
    omega: f32,
    vib_phase: f32,
    /// The vibrato's current depth (semitones): it fades in on each note's
    /// envelope and, when a new note's envelope drops, ramps down within
    /// [`VIBRATO_RELEASE`] instead of stopping dead mid-swing.
    vib_depth: f32,
    drift: Wander,
    jitter: Wander,
    rate_wobble: Wander,
    dt: f32,
    pub rng: Rng,
}

/// Overshoot 0.6% of the interval (7 cents on an octave).
const DAMPING: f32 = 0.85;

/// How long the vibrato takes to die away at a new note (s, from full
/// depth). Cut at once, a join stepped the pitch by the swing it was on: up
/// to the full depth (30 cents) within one block. Ramped, the pitch moves no
/// faster than the vibrato's own swing, and a quick passing note is straight
/// again well before its middle.
const VIBRATO_RELEASE: f32 = 0.03;

impl Pitch {
    pub fn new(ctrl_rate: f32, style: &Style, seed: u32) -> Pitch {
        Pitch {
            x: 60.0, v: 0.0, target: 60.0, omega: 40.0, vib_phase: 0.0, vib_depth: 0.0,
            drift: Wander::new(0.6, style.drift_cents / 100.0, ctrl_rate),
            jitter: Wander::new(25.0, style.jitter_cents / 100.0, ctrl_rate),
            rate_wobble: Wander::new(0.4, 0.06, ctrl_rate),
            dt: 1.0 / ctrl_rate, rng: Rng::new(seed),
        }
    }

    /// A new note: `legato` glides from where the voice is, else it starts
    /// `scoop` below. `lead` is the time from now to its vowel (the onset
    /// consonants): the glide or scoop happens through them and lands by
    /// the vowel plus half the portamento, so the vowel is sung on pitch.
    pub fn note(&mut self, note: f32, legato: bool, lead: f32, style: &Style) {
        let from = if legato { self.x } else { note - style.scoop };
        let jump = (note - from).abs();
        let time = (style.portamento + 0.004 * jump).min(lead.max(0.0) + 0.5 * style.portamento).max(0.015);
        self.omega = 4.0 / time;
        if !legato {
            self.x = note - style.scoop;
            self.v = 0.0;
        }
        self.target = note;
    }

    /// Advance one block; `vowel_time` is seconds since this note's vowel
    /// began (vibrato waits for it). Returns the sung MIDI pitch.
    pub fn step(&mut self, vowel_time: f32, style: &Style) -> f32 {
        let dt = self.dt;
        // Semi-implicit Euler: stable for omega * dt well below one.
        self.v += (self.omega * self.omega * (self.target - self.x) - 2.0 * DAMPING * self.omega * self.v) * dt;
        self.x += self.v * dt;
        let rate = style.vibrato_rate * (1.0 + self.rate_wobble.next(&mut self.rng));
        self.vib_phase = (self.vib_phase + std::f32::consts::TAU * rate * dt) % std::f32::consts::TAU;
        let t = ((vowel_time - style.vibrato_delay) / style.vibrato_fade.max(1e-3)).clamp(0.0, 1.0);
        let depth = style.vibrato_depth * t * t * (3.0 - 2.0 * t);
        self.vib_depth = depth.max(self.vib_depth - style.vibrato_depth * dt / VIBRATO_RELEASE);
        let drift = self.drift.next(&mut self.rng);
        let jitter = self.jitter.next(&mut self.rng);
        self.x + self.vib_depth * self.vib_phase.sin() + drift + jitter + style.detune_cents / 100.0
    }

    pub fn current(&self) -> f32 {
        self.x
    }
}

pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

pub fn hz_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_lands_on_the_note_with_a_small_overshoot() {
        let style = Style::straight();
        let mut p = Pitch::new(750.0, &style, 1);
        p.note(60.0, false, 0.0, &style);
        for _ in 0..300 {
            p.step(0.0, &style);
        }
        assert!((p.current() - 60.0).abs() < 0.01);
        p.note(65.0, true, 0.1, &style);
        let mut peak = 0.0f32;
        for _ in 0..600 {
            peak = peak.max(p.step(0.0, &style));
        }
        assert!(peak > 65.0 && peak < 65.1, "overshoot to {peak}");
        assert!((p.current() - 65.0).abs() < 0.01);
    }
}
