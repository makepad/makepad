//! What the host sends the player, and what the player sends back. All of
//! it is `Copy` plain data except a new graph, which moves as a `Box` and is
//! handed back (never dropped) on the audio thread.

/// Per-note overrides of the instrument's patch (`play(n, {cutoff: 900})`).
/// NaN leaves the patch value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteOpts {
    pub cutoff: f32,
    pub res: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub drive: f32,
    pub detune: f32,
    /// Seconds from the note's start the pitch glides from (`slide`).
    pub glide_from: f32,
    /// The note's handle (`let h = play(...)`), for glides; 0 is none.
    /// On a Glide event `glide_from` is the semitones to move by.
    pub handle: u32,
    /// Sample playback: a rate multiplier (negative plays backwards), the
    /// region as fractions of the sample (`start`..`finish`) or the onset it
    /// starts at (to the next one), looping the region while held, fitting
    /// it to beats by speed (`fit`) or by time-stretch keeping the pitch
    /// (`stretch`). NaN (and `looped` false) is the plain sample.
    pub rate: f32,
    pub start: f32,
    pub finish: f32,
    pub onset: f32,
    pub fit: f32,
    pub stretch: f32,
    pub looped: bool,
    /// A custom envelope on the note's level (multiplying the instrument's
    /// own) and one on its cutoff (in Hz), as breakpoints in beats from the
    /// note's start. None when `n` is 0.
    pub env: Env,
    pub cutoff_env: Env,
    /// The pitch over the note, in semitones from its note, as breakpoints
    /// `[beats semitones]` (`bend`): a slide, a portamento, a fall.
    pub bend: Env,
    /// Vibrato (`vib: [rate depth delay]`): rate in Hz, depth in semitones
    /// either side, starting `vib_delay` beats in. Depth 0 is none.
    pub vib_rate: f32,
    pub vib_depth: f32,
    pub vib_delay: f32,
    /// An arpeggio (`arp: [0 4 7]`, `arp_step` beats each): the note steps
    /// through these semitone offsets, round and round, without restarting.
    pub arp: [f32; ARP_STEPS],
    pub arp_n: u8,
    pub arp_step: f32,
}

/// The most offsets an arpeggio steps through.
pub const ARP_STEPS: usize = 4;

impl NoteOpts {
    /// The note's pitch moves under it (a bend, a vibrato or an arpeggio).
    pub fn moves(&self) -> bool {
        self.bend.n > 0 || self.vib_depth > 0.0 || (self.arp_n > 0 && self.arp_step > 0.0)
    }

    /// Semitones from the note `beats` after it starts, `seconds` after it
    /// starts (the vibrato runs in time, the rest in beats).
    pub fn pitch_offset(&self, beats: f32, seconds: f32) -> f32 {
        let mut semis = if self.bend.n > 0 { self.bend.level(beats) } else { 0.0 };
        if self.vib_depth > 0.0 && beats >= self.vib_delay.max(0.0) {
            semis += self.vib_depth * (std::f32::consts::TAU * self.vib_rate * seconds).sin();
        }
        if self.arp_n > 0 && self.arp_step > 0.0 {
            let k = (beats / self.arp_step).max(0.0) as usize % self.arp_n as usize;
            semis += self.arp[k];
        }
        semis
    }
}

/// Breakpoints `[beats, level]`, linear between them (`curve` > 1 bends
/// each segment slow-then-fast, < 1 fast-then-slow), holding the last.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Env {
    pub pts: [[f32; 2]; ENV_POINTS],
    pub n: u8,
    pub curve: f32,
}

/// The most breakpoints an envelope keeps.
pub const ENV_POINTS: usize = 8;

impl Env {
    /// The level `beats` after the start.
    pub fn level(&self, beats: f32) -> f32 {
        let pts = &self.pts[..self.n as usize];
        let Some(first) = pts.first() else { return 1.0 };
        if beats <= first[0] {
            return first[1];
        }
        for w in pts.windows(2) {
            if beats < w[1][0] {
                let span = (w[1][0] - w[0][0]).max(1e-9);
                let mut t = ((beats - w[0][0]) / span).clamp(0.0, 1.0);
                if self.curve > 0.0 && self.curve != 1.0 {
                    t = t.powf(self.curve);
                }
                return w[0][1] + (w[1][1] - w[0][1]) * t;
            }
        }
        pts[pts.len() - 1][1]
    }
}

impl Default for NoteOpts {
    fn default() -> Self {
        NoteOpts {
            cutoff: f32::NAN, res: f32::NAN, attack: f32::NAN, decay: f32::NAN, sustain: f32::NAN,
            release: f32::NAN, drive: f32::NAN, detune: f32::NAN, glide_from: f32::NAN, handle: 0,
            rate: f32::NAN, start: f32::NAN, finish: f32::NAN, onset: f32::NAN, fit: f32::NAN, stretch: f32::NAN, looped: false, env: Env::default(), cutoff_env: Env::default(),
            bend: Env::default(), vib_rate: 0.0, vib_depth: 0.0, vib_delay: 0.0, arp: [0.0; ARP_STEPS], arp_n: 0, arp_step: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventKind {
    /// A pitched note (`play`): MIDI number, fractions allowed.
    Note,
    /// A drum voice by kit slot (`hit(@kick)`), and its General MIDI
    /// percussion note (what a modelled kit or a SoundFont plays).
    Hit(u8, u8),
    /// A playing note (`opts.handle`) glides from here over `dur` beats to
    /// `note` (a pitch), `vel` (its level) and `opts.cutoff` (NaN: that one
    /// stays).
    Glide,
    /// Let go of a held note (its handle in `opts.handle`, its note in
    /// `note`): a key released on a MIDI keyboard playing the loop.
    Release,
    /// A General MIDI program change for the notes after it (a loop on a
    /// SoundFont; imported songs).
    Program(u8),
    /// The sustain (damper) pedal to `vel` (0 up .. 1 down; between is
    /// half-pedal) for the notes sounding and after (a native piano).
    Pedal,
    /// The loop's sampler, at `note` (its root plays the sample as it is).
    Sample,
    /// A sung syllable (`sing(...)`) on a choir part (0 = the melody).
    Sung(u8, makepad_score_voice::PhonemeSeq),
    /// A neural singer's phrase (a whole `sing(...)` line) by its key
    /// (`sung::neural`): its audio starts at `beat` (its lead before the
    /// first note), at `vel`.
    Phrase(u64),
}

/// A note, hit or sample start at an absolute beat. `dur` is in beats;
/// zero or less plays the patch's own gate (one-shots).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub beat: f64,
    pub loop_key: u64,
    pub kind: EventKind,
    pub note: f32,
    pub vel: f32,
    pub pan: f32,
    pub dur: f32,
    pub opts: NoteOpts,
}
