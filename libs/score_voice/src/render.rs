//! Offline rendering: a lowered line played the way a Score player plays it
//! (each event's note_on at its frame, its release at its end), for demos,
//! tests, bounces and the pitch curve drawn over the lyrics.

use crate::choir::Choir;
use crate::lyrics::Lowered;
use crate::pitch::hz_to_midi;
use crate::singer::{Singer, SungNote};

/// Anything that sings sung notes: a solo [`Singer`] or a [`Choir`].
pub trait Sings {
    fn on(&mut self, offset: u32, note: SungNote, part: u8) -> u32;
    fn off(&mut self, offset: u32, serial: u32);
    fn add_stereo(&mut self, l: &mut [f32], r: &mut [f32]);
}

impl Sings for Singer {
    fn on(&mut self, offset: u32, note: SungNote, _part: u8) -> u32 {
        self.note_on(offset, note)
    }
    fn off(&mut self, offset: u32, serial: u32) {
        self.release(offset, serial)
    }
    fn add_stereo(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let mut mono = [0.0f32; 512];
        let mut at = 0;
        while at < n {
            let len = (n - at).min(512);
            mono[..len].fill(0.0);
            self.render(&mut mono[..len]);
            for i in 0..len {
                l[at + i] += mono[i] * std::f32::consts::FRAC_1_SQRT_2;
                r[at + i] += mono[i] * std::f32::consts::FRAC_1_SQRT_2;
            }
            at += len;
        }
    }
}

impl Sings for Choir {
    fn on(&mut self, offset: u32, note: SungNote, part: u8) -> u32 {
        self.note_on(offset, note, part)
    }
    fn off(&mut self, offset: u32, serial: u32) {
        self.release(offset, serial)
    }
    fn add_stereo(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.render_stereo(l, r)
    }
}

/// Play `line` from its first event, in host chunks of `chunk` frames, plus
/// `tail` seconds. Returns stereo.
pub fn render_line(voice: &mut dyn Sings, line: &Lowered, bpm: f64, rate: f32, chunk: usize, tail: f32) -> (Vec<f32>, Vec<f32>) {
    let origin = line.events.iter().map(|e| e.beat).fold(f64::INFINITY, f64::min);
    let origin = if origin.is_finite() { origin } else { 0.0 };
    let frame = |beat: f64| ((beat - origin) * 60.0 / bpm * rate as f64).round() as u64;
    let end = line.events.iter().map(|e| frame(e.beat + e.dur)).max().unwrap_or(0) + (tail * rate) as u64;
    // (frame, is_on, event index)
    let mut acts: Vec<(u64, bool, usize)> = Vec::new();
    for (i, e) in line.events.iter().enumerate() {
        acts.push((frame(e.beat), true, i));
        acts.push((frame(e.beat + e.dur), false, i));
    }
    // Releases before starts at the same frame (a player's order).
    acts.sort_by_key(|a| (a.0, a.1));
    let mut serials = vec![0u32; line.events.len()];
    let (mut l, mut r) = (vec![0.0f32; end as usize], vec![0.0f32; end as usize]);
    let mut next = 0;
    let mut at = 0u64;
    let chunk = chunk.max(1) as u64;
    while at < end {
        let len = chunk.min(end - at);
        while next < acts.len() && acts[next].0 < at + len {
            let (f, on, i) = acts[next];
            let e = &line.events[i];
            let offset = (f.max(at) - at) as u32;
            if on {
                let note = SungNote { note: e.note, vel: e.vel, dur: (e.dur * 60.0 / bpm) as f32, syl: line.table[e.syllable as usize] };
                serials[i] = voice.on(offset, note, e.part);
            } else {
                voice.off(offset, serials[i]);
            }
            next += 1;
        }
        voice.add_stereo(&mut l[at as usize..(at + len) as usize], &mut r[at as usize..(at + len) as usize]);
        at += len;
    }
    (l, r)
}

/// The sung pitch of part 0 over time, as (beat, MIDI) every `step` beats:
/// the curve drawn over the lyric text. Uses a silent singer, so it costs
/// no synthesis.
pub fn pitch_curve(singer: &mut Singer, line: &Lowered, bpm: f64, step: f64) -> Vec<(f64, f32)> {
    let rate = singer.rate();
    singer.set_silent(true);
    let origin = line.events.iter().map(|e| e.beat).fold(f64::INFINITY, f64::min);
    if !origin.is_finite() {
        return Vec::new();
    }
    let solo = crate::lyrics::Lowered {
        events: line.events.iter().filter(|e| e.part == 0).cloned().collect(),
        table: line.table.clone(), syllables: Vec::new(), warnings: Vec::new(),
    };
    let frames_per_step = (step * 60.0 / bpm * rate as f64).max(1.0) as usize;
    let mut curve = Vec::new();
    struct Tap<'a> {
        singer: &'a mut Singer,
        curve: &'a mut Vec<(f64, f32)>,
        frame: u64,
        every: usize,
        origin: f64,
        per_beat: f64,
    }
    impl Sings for Tap<'_> {
        fn on(&mut self, o: u32, n: SungNote, _p: u8) -> u32 {
            self.singer.note_on(o, n)
        }
        fn off(&mut self, o: u32, s: u32) {
            self.singer.release(o, s)
        }
        fn add_stereo(&mut self, l: &mut [f32], _r: &mut [f32]) {
            let mut at = 0;
            while at < l.len() {
                let len = self.every.min(l.len() - at);
                self.singer.render(&mut l[at..at + len]);
                self.frame += len as u64;
                at += len;
                let f0 = self.singer.f0();
                if f0 > 0.0 {
                    self.curve.push((self.origin + self.frame as f64 / self.per_beat, hz_to_midi(f0)));
                }
            }
        }
    }
    let mut tap = Tap { singer, curve: &mut curve, frame: 0, every: frames_per_step, origin, per_beat: rate as f64 * 60.0 / bpm };
    render_line(&mut tap, &solo, bpm, rate, frames_per_step, 0.0);
    tap.singer.set_silent(false);
    curve
}

/// A 16-bit PCM WAV of one or two channels.
pub fn wav_bytes(rate: u32, channels: &[&[f32]]) -> Vec<u8> {
    let ch = channels.len().max(1);
    let frames = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    let data = (frames * ch * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(ch as u16).to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * ch as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(ch as u16 * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        for c in channels {
            out.extend_from_slice(&((c[i].clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
    }
    out
}

/// Read a 16-bit PCM WAV: (mono mix, rate).
pub fn read_wav16(bytes: &[u8]) -> Option<(Vec<f32>, u32)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let (mut ch, mut rate, mut bits) = (0u16, 0u32, 0u16);
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = bytes.get(at + 8..(at + 8 + len).min(bytes.len()))?;
        if id == b"fmt " && body.len() >= 16 {
            ch = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            if bits != 16 || ch == 0 {
                return None;
            }
            let ch = ch as usize;
            let frames = body.len() / (2 * ch);
            let mut out = Vec::with_capacity(frames);
            for f in 0..frames {
                let mut sum = 0.0;
                for c in 0..ch {
                    let i = (f * ch + c) * 2;
                    sum += i16::from_le_bytes([body[i], body[i + 1]]) as f32 / 32768.0;
                }
                out.push(sum / ch as f32);
            }
            return Some((out, rate));
        }
        at += 8 + len + (len & 1);
    }
    None
}
