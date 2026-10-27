//! One sounding note of a loop: a recipe voice (synth notes, synthesized
//! drums) or a sample voice. Voices live in a fixed pool per loop; starting
//! one copies plain data into a slot and never allocates.

use crate::event::{Env, NoteOpts};
use crate::loop_engine::Sample;
use makepad_audio_dsp::grain::Grains;
use makepad_audio_synth::dsp::midi_to_hz;
use makepad_audio_synth::recipe::{Recipe, RecipeVoice};

#[derive(Clone, Copy)]
pub(crate) enum Src {
    Recipe,
    /// A sample voice: its position and step in source frames (the kit
    /// slot it plays is the voice's `slot`, `u8::MAX` for a sampler), the
    /// region it plays and, time-stretched, its two grains.
    Sample { pos: f64, env: f32, attack: f32, release: f32, play: SamplePlay, grains: Grains },
}

/// How a sample voice plays: `step` source frames per output frame (the
/// pitch; negative is backwards) over `from..to`, looping or not. With a
/// finite `time_step` it is time-stretched: the read head moves by
/// `time_step` while grains of `grain` frames play at `step`.
#[derive(Clone, Copy)]
pub(crate) struct SamplePlay {
    pub step: f64,
    pub from: f64,
    pub to: f64,
    pub looped: bool,
    pub time_step: f64,
    pub grain: f64,
    /// The sample's sustain loop (frames `from..to`), wrapped into when
    /// playing forwards past its end.
    pub sustain: Option<(f64, f64)>,
}

pub(crate) struct Voice {
    pub active: bool,
    pub note: f32,
    /// The beat this note is released at; infinite for one-shots.
    pub off_beat: f64,
    pub released: bool,
    pub gain: f32,
    /// The velocity this note started at (a glide's level is relative to it).
    pub vel: f32,
    pub pan_l: f32,
    pub pan_r: f32,
    pub serial: u32,
    /// Kit slot of a hit (for chokes), else `u8::MAX`.
    pub slot: u8,
    pub src: Src,
    pub recipe: RecipeVoice,
    /// Its handle (`play` returns it) and a glide under way.
    pub handle: u32,
    pub pitch: f32,
    pub glide: Option<Glide>,
    /// Custom envelopes on level and cutoff, and the beats since the note
    /// started (`beats_per_frame` a frame).
    pub env: Env,
    pub cutoff_env: Env,
    pub env_beats: f32,
    pub beats_per_frame: f32,
    /// The note's own pitch motion (bend, vibrato, arpeggio), and the
    /// frames it has played.
    pub moves: Option<NoteOpts>,
    pub age_frames: f32,
}

/// A voice moving to new values over `frames` frames: pitch as a ratio of
/// its note, level, cutoff (NaN: unchanged).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Glide {
    pub pitch: (f32, f32),
    pub gain: (f32, f32),
    pub cutoff: (f32, f32),
    pub frames: f32,
    pub done: f32,
}

/// Equal-power pan gains for -1..1.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let p = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (p.cos(), p.sin())
}

/// The patch with a note's overrides on every layer.
pub(crate) fn with_opts(recipe: &Recipe, opts: &NoteOpts) -> Recipe {
    let mut recipe = *recipe;
    for layer in recipe.layers.iter_mut().take(recipe.count) {
        for (key, value) in [
            ("cutoff", opts.cutoff), ("q", opts.res), ("a", opts.attack), ("d", opts.decay), ("s", opts.sustain),
            ("r", opts.release), ("drive", opts.drive), ("detune", opts.detune),
        ] {
            if value.is_finite() {
                // A cutoff on an unfiltered layer adds a low-pass: that is
                // what a cutoff on a note means.
                let _ = layer.set_num(key, value);
            }
        }
    }
    recipe
}

impl Voice {
    pub fn idle() -> Voice {
        Voice {
            active: false, note: 0.0, off_beat: f64::INFINITY, released: true, gain: 0.0, vel: 1.0, pan_l: 0.0, pan_r: 0.0, serial: 0,
            slot: u8::MAX, src: Src::Recipe, recipe: RecipeVoice::new(&Recipe::default(), 1.0, None, false, 1),
            handle: 0, pitch: 1.0, glide: None, env: Env::default(), cutoff_env: Env::default(), env_beats: 0.0, beats_per_frame: 0.0,
            moves: None, age_frames: 0.0,
        }
    }

    /// A synth note (`note` a MIDI number, fractions allowed).
    pub fn start_note(&mut self, recipe: &Recipe, note: f32, held: bool, seed: u32) {
        self.recipe = RecipeVoice::new(recipe, 1.0, Some(midi_to_hz(note)), held, seed);
        self.src = Src::Recipe;
        self.slot = u8::MAX;
        self.start_common(note);
    }

    /// A synthesized drum voice at `pitch` × as written.
    pub fn start_hit(&mut self, recipe: &Recipe, pitch: f32, slot: u8, seed: u32) {
        self.recipe = RecipeVoice::new(recipe, pitch, None, false, seed);
        self.src = Src::Recipe;
        self.slot = slot;
        self.start_common(60.0);
    }

    pub fn start_sample(&mut self, play: SamplePlay, kit: u8, attack: f32, release: f32, note: f32) {
        let backwards = if play.time_step.is_finite() { play.time_step < 0.0 } else { play.step < 0.0 };
        let pos = if backwards { play.to - 1.0 } else { play.from };
        let grains = Grains::new(play.grain, pos, if play.step < 0.0 { -1.0 } else { 1.0 });
        self.src = Src::Sample { pos, env: if attack > 0.0 { 0.0 } else { 1.0 }, attack, release, play, grains };
        self.slot = kit;
        self.start_common(note);
    }

    fn start_common(&mut self, note: f32) {
        self.pitch = 1.0;
        self.glide = None;
        self.env = Env::default();
        self.cutoff_env = Env::default();
        self.env_beats = 0.0;
        self.moves = None;
        self.age_frames = 0.0;
        self.handle = 0;
        self.active = true;
        self.released = false;
        self.note = note;
        self.off_beat = f64::INFINITY;
    }

    pub fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        self.off_beat = f64::INFINITY;
        if let Src::Recipe = self.src {
            self.recipe.release();
        }
    }

    /// Start gliding to `note` (NaN: stay) or `by` semitones from where it
    /// is, to level `amp` (a velocity; NaN: stay), to `cutoff`, over `frames`.
    pub fn glide_to(&mut self, note: f32, by: f32, amp: f32, cutoff: f32, frames: f32) {
        let cutoff_now = self.recipe.recipe.layers.first().map_or(f32::NAN, |l| l.cutoff);
        let pitch_to = if note.is_finite() {
            2f32.powf((note - self.note) / 12.0)
        } else if by.is_finite() {
            self.pitch * 2f32.powf(by / 12.0)
        } else {
            self.pitch
        };
        let gain_to = if amp.is_finite() && self.vel > 0.0 { self.gain * amp.max(0.0) / self.vel } else { self.gain };
        if amp.is_finite() && self.vel > 0.0 {
            self.vel = amp.max(0.0);
        }
        self.glide = Some(Glide {
            pitch: (self.pitch, pitch_to),
            gain: (self.gain, gain_to),
            cutoff: (cutoff_now, if cutoff.is_finite() { cutoff } else { cutoff_now }),
            frames: frames.max(1.0),
            done: 0.0,
        });
    }

    /// The note's pitch motion, when it has one.
    pub fn set_moves(&mut self, opts: &NoteOpts) {
        self.moves = opts.moves().then_some(*opts);
    }

    fn set_cutoff(&mut self, c: f32) {
        let count = self.recipe.recipe.count;
        for layer in self.recipe.recipe.layers.iter_mut().take(count) {
            layer.cutoff = c;
        }
    }

    /// ADD this voice into `l`/`r`; gliding or under a custom envelope, in
    /// steps of 32 frames.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32], scratch: &mut [f32], rate: f32, sample: Option<&Sample>) {
        let (enveloped, filtered) = (self.env.n > 0, self.cutoff_env.n > 0);
        if self.glide.is_none() && !enveloped && !filtered && self.moves.is_none() {
            return self.render_block(l, r, scratch, rate, sample);
        }
        let n = l.len();
        let mut at = 0;
        while at < n {
            let k = (n - at).min(32);
            if let Some(mut glide) = self.glide {
                let t = ((glide.done + k as f32 * 0.5) / glide.frames).clamp(0.0, 1.0);
                let lerp = |a: (f32, f32)| a.0 + (a.1 - a.0) * t;
                self.pitch = lerp(glide.pitch);
                self.gain = lerp(glide.gain);
                if glide.cutoff.1.is_finite() {
                    self.set_cutoff(lerp(glide.cutoff));
                }
                glide.done += k as f32;
                self.glide = if glide.done >= glide.frames {
                    self.pitch = glide.pitch.1;
                    self.gain = glide.gain.1;
                    None
                } else {
                    Some(glide)
                };
            }
            let mid = self.env_beats + k as f32 * 0.5 * self.beats_per_frame;
            if filtered {
                self.set_cutoff(self.cutoff_env.level(mid).clamp(20.0, 20_000.0));
            }
            let level = if enveloped { self.env.level(mid).max(0.0) } else { 1.0 };
            let keep = self.gain;
            let keep_pitch = self.pitch;
            self.gain *= level;
            if let Some(moves) = &self.moves {
                let seconds = (self.age_frames + k as f32 * 0.5) / rate.max(1.0);
                self.pitch *= 2f32.powf(moves.pitch_offset(mid, seconds) / 12.0);
            }
            self.render_block(&mut l[at..at + k], &mut r[at..at + k], scratch, rate, sample);
            self.gain = keep;
            self.pitch = keep_pitch;
            self.age_frames += k as f32;
            self.env_beats += k as f32 * self.beats_per_frame;
            at += k;
        }
    }

    fn render_block(&mut self, l: &mut [f32], r: &mut [f32], scratch: &mut [f32], rate: f32, sample: Option<&Sample>) {
        let n = l.len();
        let scratch = &mut scratch[..n];
        scratch.fill(0.0);
        let pitch = self.pitch as f64;
        match &mut self.src {
            Src::Recipe => {
                self.recipe.render(scratch, rate, self.pitch);
                if self.recipe.finished() {
                    self.active = false;
                }
            }
            Src::Sample { pos, env, attack, release, play, grains } => {
                let Some(sample) = sample.filter(|sample| sample.len() > 1) else {
                    self.active = false;
                    return;
                };
                let last = (sample.len() - 1) as f64;
                let up = if *attack > 0.0 { 1.0 / (*attack * rate) } else { 1.0 };
                let down = 1.0 / (release.max(0.002) * rate);
                let right = sample.right.as_deref();
                let read = |ch: &[f32], x: f64| -> f32 {
                    let x = x.clamp(0.0, last);
                    let at = (x as usize).min(ch.len().saturating_sub(2));
                    let frac = (x - at as f64) as f32;
                    ch[at] + (ch[at + 1] - ch[at]) * frac
                };
                let (from, to) = (play.from.max(0.0), play.to.min(last + 1.0));
                let span = (to - from).max(1.0);
                let step = play.step * pitch;
                let stretched = play.time_step.is_finite();
                let mut stereo_r = [0.0f32; crate::CHUNK];
                let sustain = play.sustain.filter(|_| !play.looped && step > 0.0 && !stretched);
                for i in 0..n {
                    if let Some((a, b)) = sustain {
                        if *pos >= b {
                            *pos = a + (*pos - b).rem_euclid((b - a).max(1.0));
                        }
                    }
                    if *pos < from || *pos >= to {
                        if !play.looped {
                            self.active = false;
                            break;
                        }
                        *pos = from + (*pos - from).rem_euclid(span);
                    }
                    if *env <= 0.0 && self.released {
                        self.active = false;
                        break;
                    }
                    *env = if self.released { (*env - down).max(0.0) } else { (*env + up).min(1.0) };
                    let (a, b) = if stretched {
                        let (mut a, mut b) = (0.0, 0.0);
                        for (x, w) in grains.next(*pos, step) {
                            let x = x.clamp(from, to - 1.0);
                            a += read(&sample.left, x) * w;
                            if let Some(right) = right {
                                b += read(right, x) * w;
                            }
                        }
                        *pos += play.time_step;
                        (a, b)
                    } else {
                        let a = read(&sample.left, *pos);
                        let b = right.map_or(0.0, |right| read(right, *pos));
                        *pos += step;
                        (a, b)
                    };
                    scratch[i] = a * *env;
                    stereo_r[i] = b * *env;
                }
                if right.is_some() {
                    for i in 0..n {
                        l[i] += scratch[i] * self.gain * self.pan_l;
                        r[i] += stereo_r[i] * self.gain * self.pan_r;
                    }
                    return;
                }
            }
        }
        for i in 0..n {
            let x = scratch[i] * self.gain;
            l[i] += x * self.pan_l;
            r[i] += x * self.pan_r;
        }
    }
}
