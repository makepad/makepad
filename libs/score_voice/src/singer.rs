//! One singing voice, realtime-safe: a note arrives with its syllable (a
//! [`PhonemeSeq`]); the singer lays the syllable out in time (onset
//! consonants, the vowel on the note, the offglide and coda at the end),
//! morphs between phoneme shapes with formant warping (the formants glide
//! from one phoneme's positions to the next, coarticulation), fires stop
//! bursts, and sings the pitch with glides and vibrato through a [`Voicer`].
//!
//! Timing: `note_on` / `release` take a frame offset from the start of the
//! next `render` call; they act at the first internal block boundary at or
//! after that frame, so a render is identical whatever chunk sizes the host
//! renders in. `render` ADDS into its buffer. Nothing here allocates or
//! locks after `Singer::new`.

use crate::bands::*;
use crate::phoneme::{self, Class, Ph};
use crate::pitch::{midi_to_hz, Pitch, Style};
use crate::profile::VoiceProfile;
use crate::voicer::{Frame, Voicer, BLOCK};
use std::sync::Arc;

pub const MAX_PHONES: usize = 14;

/// The phonemes of one sung syllable (or one note of a melisma).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhonemeSeq {
    pub ph: [Ph; MAX_PHONES],
    pub len: u8,
    pub flags: u8,
}

impl PhonemeSeq {
    /// Joins the previous note without a new attack (slurred).
    pub const LEGATO: u8 = 1;
    /// A melisma's later note: the vowel continues, no onset.
    pub const CONT: u8 = 2;

    pub fn new(phones: &[Ph], flags: u8) -> PhonemeSeq {
        let mut ph = [phoneme::SIL; MAX_PHONES];
        let len = phones.len().min(MAX_PHONES);
        ph[..len].copy_from_slice(&phones[..len]);
        PhonemeSeq { ph, len: len as u8, flags }
    }

    /// From IPA (Kokoro's notation accepted).
    pub fn ipa(pron: &str) -> PhonemeSeq {
        PhonemeSeq::new(&phoneme::parse(pron), 0)
    }

    pub fn phones(&self) -> &[Ph] {
        &self.ph[..self.len as usize]
    }

    pub fn nucleus(&self) -> Option<usize> {
        self.phones().iter().position(|p| phoneme::is_vowel(*p))
    }

    pub fn with_flags(mut self, flags: u8) -> PhonemeSeq {
        self.flags = flags;
        self
    }
}

/// A note to sing: MIDI pitch, velocity 0..1, length in seconds (the time to
/// the next syllable's first sound; 0 or less = until released).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SungNote {
    pub note: f32,
    pub vel: f32,
    pub dur: f32,
    pub syl: PhonemeSeq,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum NoiseSrc {
    None,
    /// The phoneme's own noise bands, offset (dB).
    Own(Ph, f32),
    /// Breath through the shape of a vowel (h, aspiration), offset (dB).
    Aspirate(Ph, f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Seg {
    /// Start (s): from the syllable's start, or from the tail's for a tail
    /// segment.
    t: f32,
    shape: Ph,
    harm: f32,
    noise: NoiseSrc,
    /// Noise is pulsed with the glottal cycle (voiced sounds).
    pulsed: bool,
    burst: bool,
    /// Width of the transition into this segment (s), centred on `t`.
    w: f32,
    tail: bool,
}

const MAX_SEGS: usize = 40;

#[derive(Clone, Copy, Debug)]
struct Plan {
    segs: [Seg; MAX_SEGS],
    n: usize,
    /// Where the vowel starts (s).
    vowel_t: f32,
    tail_len: f32,
    /// Where the tail starts when the note's length is known.
    tail_t0: f32,
}

const EMPTY_SEG: Seg = Seg { t: 0.0, shape: phoneme::SIL, harm: 0.0, noise: NoiseSrc::None, pulsed: false, burst: false, w: 0.0, tail: false };

/// Onset and tail lengths (s) of a syllable at a note length, after
/// squeezing: consonants take at most 70% of the note. The lowering uses
/// this to put the vowel on the beat.
pub fn syllable_timing(seq: &PhonemeSeq, dur: f32) -> (f32, f32) {
    let (onset, tail) = raw_timing(seq);
    let scale = if dur > 0.0 && onset + tail > dur * 0.7 { dur * 0.7 / (onset + tail) } else { 1.0 };
    (onset * scale, tail * scale)
}

fn consonant_len(ph: Ph, coda: bool) -> f32 {
    let info = phoneme::info(ph);
    let base = if coda { info.coda } else { info.onset };
    match info.class {
        Class::Stop => base + if info.voiced { 0.012 } else if coda { 0.03 } else { 0.045 },
        Class::Affricate => base + if coda { 0.06 } else { 0.07 },
        _ => base,
    }
}

fn raw_timing(seq: &PhonemeSeq) -> (f32, f32) {
    let phones = seq.phones();
    let Some(nuc) = seq.nucleus() else {
        return (phones.iter().map(|p| consonant_len(*p, false)).sum(), 0.0);
    };
    let onset = if seq.flags & PhonemeSeq::CONT != 0 { 0.0 } else { phones[..nuc].iter().map(|p| consonant_len(*p, false)).sum() };
    let mut tail = 0.0;
    for p in &phones[nuc + 1..] {
        tail += if phoneme::is_vowel(*p) { 0.1 } else { consonant_len(*p, true) };
    }
    (onset, tail)
}

struct Builder {
    plan: Plan,
    t: f32,
    tail: bool,
    scale: f32,
    nucleus: Ph,
}

impl Builder {
    fn push(&mut self, len: f32, seg: Seg) {
        if self.plan.n < MAX_SEGS {
            self.plan.segs[self.plan.n] = Seg { t: self.t, tail: self.tail, ..seg };
            self.plan.n += 1;
        }
        self.t += len * self.scale;
    }

    fn consonant(&mut self, ph: Ph, coda: bool) {
        let info = phoneme::info(ph);
        let base = (if coda { info.coda } else { info.onset }).max(0.0);
        let s = |shape: Ph, harm: f32, noise: NoiseSrc, w: f32| Seg { shape, harm, noise, w, ..EMPTY_SEG };
        let nucleus = self.nucleus;
        match info.class {
            Class::Stop | Class::Affricate => {
                let closure = if info.voiced { s(phoneme::VOICEBAR, 1.0, NoiseSrc::None, 0.015) } else { s(ph, 0.0, NoiseSrc::None, 0.015) };
                self.push(base, closure);
                if info.class == Class::Affricate {
                    let fric = phoneme::frication_of(ph);
                    let harm = if info.voiced { 1.0 } else { 0.0 };
                    self.push(if coda { 0.06 } else { 0.07 }, Seg { burst: true, ..s(ph, harm, NoiseSrc::Own(fric, 0.0), 0.004) });
                } else if info.voiced {
                    self.push(0.012, Seg { burst: true, ..s(ph, 1.0, NoiseSrc::None, 0.004) });
                } else {
                    self.push(0.012, Seg { burst: true, ..s(ph, 0.0, NoiseSrc::None, 0.004) });
                    let asp = if coda { -24.0 } else { -16.0 };
                    self.push(if coda { 0.018 } else { 0.033 }, s(ph, 0.0, NoiseSrc::Aspirate(nucleus, asp), 0.01));
                }
            }
            Class::Fricative => {
                let harm = if info.voiced { 1.0 } else { 0.0 };
                self.push(base, s(ph, harm, NoiseSrc::Own(ph, 0.0), 0.03));
            }
            Class::Aspirate => self.push(base, s(ph, 0.0, NoiseSrc::Aspirate(nucleus, -12.0), 0.03)),
            Class::Glottal | Class::Silence => self.push(base, s(phoneme::SIL, 0.0, NoiseSrc::None, 0.01)),
            Class::Flap => self.push(base, Seg { pulsed: true, ..s(ph, 1.0, NoiseSrc::Own(ph, 0.0), 0.012) }),
            Class::Nasal | Class::Liquid | Class::Glide | Class::VoiceBar | Class::Vowel => {
                self.push(base, Seg { pulsed: true, ..s(ph, 1.0, NoiseSrc::Own(ph, 0.0), 0.045) })
            }
        }
    }
}

fn plan(seq: &PhonemeSeq, dur: f32, transition: f32) -> Plan {
    let phones = seq.phones();
    let (onset, tail) = raw_timing(seq);
    let (s_onset, s_tail) = syllable_timing(seq, dur);
    let scale = if onset + tail > 0.0 { (s_onset + s_tail) / (onset + tail) } else { 1.0 };
    let nuc = seq.nucleus();
    let nucleus = nuc.map_or(phoneme::by_ipa("ə").unwrap(), |i| phones[i]);
    let mut b = Builder {
        plan: Plan { segs: [EMPTY_SEG; MAX_SEGS], n: 0, vowel_t: 0.0, tail_len: 0.0, tail_t0: f32::INFINITY },
        t: 0.0, tail: false, scale, nucleus,
    };
    let Some(nuc) = nuc else {
        // No vowel: the consonants alone (a hum on "mm", a hiss on "sh").
        for p in phones {
            b.consonant(*p, false);
        }
        if b.plan.n > 0 {
            // The last consonant holds.
            b.plan.segs[b.plan.n - 1].w = b.plan.segs[b.plan.n - 1].w.max(0.02);
        }
        return b.plan;
    };
    if seq.flags & PhonemeSeq::CONT == 0 {
        for p in &phones[..nuc] {
            b.consonant(*p, false);
        }
    }
    b.plan.vowel_t = b.t;
    b.scale = 1.0;
    b.push(0.0, Seg { shape: nucleus, harm: 1.0, noise: NoiseSrc::Own(nucleus, 0.0), pulsed: true, w: transition, ..EMPTY_SEG });
    // Tail: offglide vowels and coda consonants, timed from the tail start.
    b.t = 0.0;
    b.tail = true;
    b.scale = scale;
    for p in &phones[nuc + 1..] {
        if phoneme::is_vowel(*p) {
            b.push(0.1, Seg { shape: *p, harm: 1.0, noise: NoiseSrc::Own(*p, 0.0), pulsed: true, w: 0.08, ..EMPTY_SEG });
        } else {
            b.consonant(*p, true);
        }
    }
    b.plan.tail_len = s_tail;
    if dur > 0.0 {
        b.plan.tail_t0 = dur - s_tail;
    }
    b.plan
}

/// A shape ready to morph: a frame plus the formants it is anchored at.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mix {
    frame: Frame,
    formants: [f32; 4],
}

impl Mix {
    const SILENT: Mix = Mix { frame: Frame::SILENT, formants: [500.0, 1500.0, 2500.0, 3400.0] };
}

#[derive(Clone, Copy, Debug)]
enum Cmd {
    On(SungNote, u32),
    Off(u32),
    OffAll,
}

const QUEUE: usize = 32;

pub struct Singer {
    profile: Arc<VoiceProfile>,
    pub style: Style,
    voicer: Voicer,
    pitch: Pitch,
    rate: f32,
    dt: f32,
    grid: [f32; ENV_POINTS],
    plan: Plan,
    /// Seconds since the syllable started.
    t: f32,
    active: bool,
    released: bool,
    vel: f32,
    note_dur: f32,
    /// Where the morph starts from at the syllable's start, and over how long.
    from: Mix,
    join_w: f32,
    last: Mix,
    seg: usize,
    amp: f32,
    burst_env: f32,
    burst_ph: Ph,
    serial: u32,
    next_serial: u32,
    queue: [(u64, Cmd); QUEUE],
    queued: usize,
    /// Frames handed to the host so far; the next block's first frame.
    pos: u64,
    next_block: u64,
    buf: [f32; BLOCK],
    buf_at: usize,
    f0: f32,
    silent: bool,
}

impl Singer {
    pub fn new(profile: Arc<VoiceProfile>, rate: f32, style: Style, seed: u32) -> Singer {
        let ctrl = rate / BLOCK as f32;
        Singer {
            pitch: Pitch::new(ctrl, &style, seed), voicer: Voicer::new(rate, seed), profile, style, rate, dt: 1.0 / ctrl,
            grid: env_grid(), plan: plan(&PhonemeSeq::new(&[], 0), 0.0, 0.05), t: 0.0, active: false, released: true, vel: 0.8,
            note_dur: 0.0, from: Mix::SILENT, join_w: 0.0, last: Mix::SILENT, seg: 0, amp: 0.0, burst_env: 0.0, burst_ph: 0, serial: 0,
            next_serial: 1, queue: [(0, Cmd::OffAll); QUEUE], queued: 0, pos: 0, next_block: 0, buf: [0.0; BLOCK], buf_at: BLOCK, f0: 0.0,
            silent: false,
        }
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// Swap the profile; the old one is handed back so the caller drops it
    /// off the audio thread.
    pub fn set_profile(&mut self, profile: Arc<VoiceProfile>) -> Arc<VoiceProfile> {
        std::mem::replace(&mut self.profile, profile)
    }

    pub fn profile(&self) -> &Arc<VoiceProfile> {
        &self.profile
    }

    /// Advance pitch and plan but render nothing (pitch curves for display).
    pub fn set_silent(&mut self, silent: bool) {
        self.silent = silent;
    }

    /// Start a note `offset` frames into the next render. Returns its serial
    /// (for `release`). A note arriving while the voice still sounds joins it
    /// (a glide and a morph, never a cut), so a stolen syllable cannot click.
    pub fn note_on(&mut self, offset: u32, note: SungNote) -> u32 {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        self.enqueue(offset, Cmd::On(note, serial));
        serial
    }

    /// Release the note `serial` (a later note's serial wins: a stale
    /// release is ignored).
    pub fn release(&mut self, offset: u32, serial: u32) {
        self.enqueue(offset, Cmd::Off(serial));
    }

    pub fn release_all(&mut self, offset: u32) {
        self.enqueue(offset, Cmd::OffAll);
    }

    fn enqueue(&mut self, offset: u32, cmd: Cmd) {
        if self.queued < QUEUE {
            self.queue[self.queued] = (self.pos + offset as u64, cmd);
            self.queued += 1;
        }
    }

    /// Sounding (or still fading).
    pub fn is_active(&self) -> bool {
        self.active || self.amp > 0.0 || self.queued > 0
    }

    /// The pitch sung in the last block (Hz, 0 when idle).
    pub fn f0(&self) -> f32 {
        self.f0
    }

    /// Current serial (the note sounding).
    pub fn serial(&self) -> u32 {
        self.serial
    }

    /// ADD `out.len()` frames of this voice into `out`.
    pub fn render(&mut self, out: &mut [f32]) {
        let mut i = 0;
        while i < out.len() {
            if self.buf_at == BLOCK {
                self.block();
                self.buf_at = 0;
            }
            let n = (BLOCK - self.buf_at).min(out.len() - i);
            for (o, s) in out[i..i + n].iter_mut().zip(&self.buf[self.buf_at..self.buf_at + n]) {
                *o += *s;
            }
            self.buf_at += n;
            self.pos += n as u64;
            i += n;
        }
    }

    fn take_events(&mut self) {
        let block = self.next_block;
        let mut i = 0;
        while i < self.queued {
            let (frame, cmd) = self.queue[i];
            // Acts at the first block boundary at or after its frame.
            let at = frame.div_ceil(BLOCK as u64) * BLOCK as u64;
            if at <= block {
                // Keep order: shift the rest down.
                self.queue.copy_within(i + 1..self.queued, i);
                self.queued -= 1;
                self.apply(cmd);
            } else {
                i += 1;
            }
        }
    }

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::On(note, serial) => self.start(note, serial),
            Cmd::Off(serial) => {
                if serial == self.serial {
                    self.release_now();
                }
            }
            Cmd::OffAll => self.release_now(),
        }
    }

    fn release_now(&mut self) {
        if !self.active || self.released {
            return;
        }
        self.released = true;
        // The tail (offglide, coda) starts now unless it already has.
        if self.plan.tail_t0 > self.t {
            self.plan.tail_t0 = self.t.max(self.plan.vowel_t);
        }
    }

    fn start(&mut self, note: SungNote, serial: u32) {
        let legato = self.amp > 0.05 || note.syl.flags & PhonemeSeq::LEGATO != 0 && self.amp > 0.001;
        self.plan = plan(&note.syl, note.dur, self.style.transition);
        self.from = if legato || self.amp > 0.001 {
            self.last
        } else {
            // From silence: the first shape, with no sound.
            let mut first = self.target(0);
            first.frame.harm = 0.0;
            first.frame.noise = [0.0; NOISE_BANDS];
            first.frame.pulsed = [0.0; NOISE_BANDS];
            first
        };
        self.join_w = if note.syl.flags & PhonemeSeq::LEGATO != 0 { self.style.transition * 1.4 } else if legato { 0.03 } else { 0.012 };
        self.pitch.note(note.note, legato, self.plan.vowel_t, &self.style);
        self.t = 0.0;
        self.seg = usize::MAX;
        self.active = true;
        self.released = false;
        self.vel = note.vel.clamp(0.0, 1.0);
        self.note_dur = note.dur;
        self.serial = serial;
    }

    fn seg_start(&self, i: usize) -> f32 {
        let seg = &self.plan.segs[i];
        if seg.tail { self.plan.tail_t0 + seg.t } else { seg.t }
    }

    fn breath(&self) -> f32 {
        (self.profile.breath + self.style.breath + (1.0 - self.vel) * 0.25).clamp(0.0, 1.0)
    }

    /// The sound of segment `i` alone.
    fn target(&self, i: usize) -> Mix {
        let seg = if self.plan.n == 0 { EMPTY_SEG } else { self.plan.segs[i.min(self.plan.n - 1)] };
        let shape = self.profile.shape(seg.shape);
        let mut frame = Frame { env: shape.env, harm: seg.harm, noise: [0.0; NOISE_BANDS], pulsed: [0.0; NOISE_BANDS] };
        let bands = match seg.noise {
            NoiseSrc::None => [FLOOR_DB; NOISE_BANDS],
            NoiseSrc::Own(ph, off) => self.profile.shape(ph).noise.map(|v| v + off),
            NoiseSrc::Aspirate(v, off) => {
                let env = &self.profile.shape(v).env;
                std::array::from_fn(|b| env_at(env, NOISE_CENTERS[b]) + off)
            }
        };
        let lin = bands.map(db_to_lin);
        if seg.pulsed { frame.pulsed = lin } else { frame.noise = lin }
        if seg.harm > 0.0 {
            // Breathiness: turbulence above a cutoff that falls from 6.5 kHz
            // to 1.5 kHz as the voice gets breathier.
            let breath = self.breath();
            let cut = 6500.0 - 5000.0 * breath;
            for b in 0..NOISE_BANDS {
                let fc = NOISE_CENTERS[b];
                let edge = if fc >= cut { 0.0 } else { -12.0 * (cut / fc).log2() };
                frame.noise[b] += db_to_lin(env_at(&frame.env, fc) - 40.0 + 28.0 * breath + edge) * seg.harm;
            }
        }
        Mix { frame, formants: shape.formants }
    }

    fn morph(&self, a: &Mix, b: &Mix, x: f32) -> Mix {
        if x <= 0.0 {
            return *a;
        }
        if x >= 1.0 {
            return *b;
        }
        // Formant anchors glide geometrically; every other frequency is
        // placed between its neighbouring anchors, and each side's envelope
        // is read at the matching place in its own spectrum.
        let top = ENV_HI_HZ;
        let aa = [0.0, a.formants[0], a.formants[1], a.formants[2], a.formants[3], top];
        let bb = [0.0, b.formants[0], b.formants[1], b.formants[2], b.formants[3], top];
        let mut cc = [0.0f32; 6];
        for j in 0..6 {
            cc[j] = if j == 0 || j == 5 { aa[j] } else { aa[j].powf(1.0 - x) * bb[j].powf(x) };
        }
        let mut out = Mix { frame: Frame::SILENT, formants: [cc[1], cc[2], cc[3], cc[4]] };
        let mut j = 0;
        for (g, f) in self.grid.iter().enumerate() {
            while j < 4 && *f >= cc[j + 1] {
                j += 1;
            }
            let span = (cc[j + 1] - cc[j]).max(1e-3);
            let u = ((f - cc[j]) / span).clamp(0.0, 1.0);
            let fa = aa[j] + u * (aa[j + 1] - aa[j]);
            let fb = bb[j] + u * (bb[j + 1] - bb[j]);
            out.frame.env[g] = (1.0 - x) * env_at(&a.frame.env, fa) + x * env_at(&b.frame.env, fb);
        }
        out.frame.harm = a.frame.harm + (b.frame.harm - a.frame.harm) * x;
        for k in 0..NOISE_BANDS {
            out.frame.noise[k] = a.frame.noise[k] + (b.frame.noise[k] - a.frame.noise[k]) * x;
            out.frame.pulsed[k] = a.frame.pulsed[k] + (b.frame.pulsed[k] - a.frame.pulsed[k]) * x;
        }
        out
    }

    fn current_mix(&mut self) -> Mix {
        let n = self.plan.n;
        if n == 0 {
            return Mix::SILENT;
        }
        let t = self.t;
        let mut i = 0;
        for k in 1..n {
            if self.seg_start(k) <= t {
                i = k;
            }
        }
        if i != self.seg {
            // Entering a burst segment fires its transient.
            if self.plan.segs[i].burst {
                self.burst_env = 1.0;
                self.burst_ph = self.plan.segs[i].shape;
            }
            self.seg = i;
        }
        let smooth = |x: f32| {
            let x = x.clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        let here = self.target(i);
        let start = self.seg_start(i);
        let w = self.plan.segs[i].w.max(1e-3);
        let mut mix = if i == 0 {
            if self.join_w > 0.0 && t < self.join_w { self.morph(&self.from, &here, smooth(t / self.join_w)) } else { here }
        } else if t < start + w * 0.5 {
            let prev = self.target(i - 1);
            self.morph(&prev, &here, smooth(0.5 + (t - start) / w))
        } else {
            here
        };
        if i + 1 < n {
            let next_start = self.seg_start(i + 1);
            let wn = self.plan.segs[i + 1].w.max(1e-3);
            if next_start.is_finite() && t > next_start - wn * 0.5 {
                let next = self.target(i + 1);
                mix = self.morph(&mix, &next, smooth((t - (next_start - wn * 0.5)) / wn));
            }
        }
        mix
    }

    fn block(&mut self) {
        self.take_events();
        self.next_block += BLOCK as u64;
        if !self.active && self.amp == 0.0 {
            self.buf = [0.0; BLOCK];
            self.f0 = 0.0;
            return;
        }
        // A note of known length releases itself at its end.
        if self.active && !self.released && self.note_dur > 0.0 && self.t >= self.note_dur {
            self.release_now();
        }
        let mut mix = self.current_mix();
        // Loudness and glottal tension from velocity: a louder note is
        // brighter (a flatter source).
        let vel = self.vel;
        let tension = 0.25 + 0.65 * vel;
        let tilt = (tension - 0.6) * 5.0;
        for (e, f) in mix.frame.env.iter_mut().zip(&self.grid) {
            if *f > 500.0 {
                *e += tilt * (f / 500.0).log2();
            }
        }
        if self.burst_env > 1e-3 {
            let burst = &self.profile.shape(self.burst_ph).burst;
            for b in 0..NOISE_BANDS {
                mix.frame.noise[b] += db_to_lin(burst[b]) * self.burst_env;
            }
            self.burst_env *= (-self.dt / 0.006).exp();
        } else {
            self.burst_env = 0.0;
        }
        // The envelope: held while the note sounds, released after the tail.
        let tail_end = self.plan.tail_t0 + self.plan.tail_len;
        let sounding = self.active && !(self.released && self.t >= tail_end);
        let target = if sounding { 1.0 } else { 0.0 };
        let tau = if sounding { 0.008 } else { 0.06 };
        self.amp += (target - self.amp) * (1.0 - (-self.dt / tau).exp());
        if !sounding && self.amp < 1e-4 {
            self.amp = 0.0;
            self.active = false;
        }
        let vowel_time = (self.t - self.plan.vowel_t).max(0.0);
        let note = self.pitch.step(vowel_time, &self.style);
        self.f0 = midi_to_hz(note);
        let gain = self.style.gain * self.amp * db_to_lin(-18.0 * (1.0 - vel));
        self.last = mix;
        if self.silent {
            self.buf = [0.0; BLOCK];
        } else {
            self.voicer.render_block(&mut self.buf, self.f0, &mix.frame, self.style.formant_scale, gain, self.profile.f0_ref);
        }
        self.t += self.dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_puts_onsets_before_the_vowel_and_the_coda_at_the_end() {
        let seq = PhonemeSeq::ipa("stɑp");
        let p = plan(&seq, 1.0, 0.05);
        let (onset, tail) = syllable_timing(&seq, 1.0);
        assert!((p.vowel_t - onset).abs() < 1e-5, "{} {}", p.vowel_t, onset);
        assert!((p.tail_t0 + tail - 1.0).abs() < 1e-5);
        // s, t closure, burst, aspiration, vowel, p closure, burst, aspiration.
        assert_eq!(p.n, 8);
        assert!(p.segs[2].burst && p.segs[6].burst);
        // Squeezed into a short note, consonants take 70% of it.
        let (o, t) = syllable_timing(&seq, 0.2);
        assert!((o + t - 0.14).abs() < 1e-5);
    }
}
