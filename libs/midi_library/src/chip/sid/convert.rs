//! A SID register trace as an [`Arrangement`]: each voice's gate makes its
//! notes, the frequency register their pitch frame by frame (read as an
//! arpeggio, a vibrato or a bend), the pulse width, waveform, envelope and
//! filter a SID-style patch per voice and instrument (pulse with its
//! width modulation, saw, triangle, noise, a noise click before a tone for
//! the drums, the chip's ADSR times, the filter and its sweeps). The tempo
//! is found from the notes' spacing in frames.

use super::machine::Trace;
use crate::chip::shape::breakpoints;
use crate::analyse::key_of_histogram;
use crate::arrangement::{num, Arrangement, Note, NoteFx, Part, Section, FX_POINTS};
use crate::gm::{Role, Voice};
use std::collections::HashMap;

/// The SID's attack times (s) by nibble, and its decay and release times.
const ATTACK: [f32; 16] = [0.002, 0.008, 0.016, 0.024, 0.038, 0.056, 0.068, 0.08, 0.1, 0.25, 0.5, 0.8, 1.0, 3.0, 5.0, 8.0];
const DECAY: [f32; 16] = [0.006, 0.024, 0.048, 0.072, 0.114, 0.168, 0.204, 0.24, 0.3, 0.75, 1.5, 2.4, 3.0, 9.0, 15.0, 24.0];

/// One frame of a voice while its gate is open.
#[derive(Clone, Copy, Debug)]
struct Frame {
    t: f64,
    pitch: f32,
    pw: u16,
    ctrl: u8,
    cutoff: u16,
    filtered: bool,
    volume: u8,
}

#[derive(Clone, Debug)]
struct SidNote {
    voice: usize,
    start: f64,
    end: f64,
    ad: u8,
    sr: u8,
    frames: Vec<Frame>,
}

impl SidNote {
    /// The waveform it mostly plays (the top four control bits).
    fn wave(&self) -> u8 {
        let mut count: HashMap<u8, usize> = HashMap::new();
        for f in self.frames.iter().skip(1) {
            *count.entry(f.ctrl & 0xF0).or_default() += 1;
        }
        count.into_iter().filter(|(w, _)| *w != 0).max_by_key(|(w, n)| (*n, *w)).map(|(w, _)| w).or_else(|| self.frames.first().map(|f| f.ctrl & 0xF0)).unwrap_or(0)
    }

    /// A noise click before a tone: a SID drum.
    fn click(&self) -> bool {
        self.frames.first().is_some_and(|f| f.ctrl & 0x80 != 0) && self.wave() & 0x80 == 0
    }

    fn filtered(&self) -> bool {
        self.frames.iter().filter(|f| f.filtered).count() * 2 > self.frames.len()
    }
}

fn hz_to_pitch(hz: f64) -> f32 {
    (69.0 + 12.0 * (hz / 440.0).log2()) as f32
}

/// The SID's filter cutoff register (11 bits) in Hz: the 8580's line; the
/// 6581's curve starts higher and bends.
pub fn cutoff_hz(reg: u16, mos8580: bool) -> f32 {
    let x = reg.min(2047) as f32 / 2047.0;
    if mos8580 {
        30.0 + 12_000.0 * x
    } else {
        220.0 + 17_800.0 * x.powf(1.6)
    }
}

/// How often the voices are looked at: the rate the player is called at
/// (a multispeed tune's 100 or 200 Hz), or the video frame rate for a tune
/// whose interrupts are faster than any player (sample playback).
fn frames_per_second(trace: &Trace) -> f64 {
    let video = if trace.clock > 1_000_000.0 { 59.83 } else { 50.12 };
    if trace.rate >= video * 0.8 && trace.rate <= video * 8.5 { trace.rate } else { video }
}

/// The voices' notes from the writes.
fn notes(trace: &Trace) -> Vec<SidNote> {
    let voices = trace.chips * 3;
    let mut regs = vec![[0u8; 25]; trace.chips];
    let mut open: Vec<Option<SidNote>> = vec![None; voices];
    let mut out = Vec::new();
    let frame = 1.0 / frames_per_second(trace);
    let mut next_frame = frame * 0.9;
    let snapshot = |regs: &Vec<[u8; 25]>, open: &mut Vec<Option<SidNote>>, t: f64, clock: f64| {
        for (v, slot) in open.iter_mut().enumerate() {
            let Some(n) = slot.as_mut() else { continue };
            let r = &regs[v / 3];
            let base = (v % 3) * 7;
            let freq = r[base] as u16 | (r[base + 1] as u16) << 8;
            if freq == 0 {
                continue;
            }
            let hz = freq as f64 * clock / 16_777_216.0;
            // The envelope the note plays with: players often set it just
            // after the gate, in the same call.
            if n.frames.is_empty() {
                n.ad = r[base + 5];
                n.sr = r[base + 6];
            }
            n.frames.push(Frame {
                t,
                pitch: hz_to_pitch(hz),
                pw: (r[base + 2] as u16 | ((r[base + 3] & 15) as u16) << 8),
                ctrl: r[base + 4],
                cutoff: (r[0x15] & 7) as u16 | (r[0x16] as u16) << 3,
                filtered: r[0x17] & (1 << (v % 3)) != 0,
                volume: r[0x18] & 15,
            });
        }
    };
    for w in &trace.writes {
        while w.t >= next_frame {
            snapshot(&regs, &mut open, next_frame, trace.clock);
            next_frame += frame;
        }
        let chip = w.chip as usize;
        let reg = w.reg as usize;
        let old = regs[chip][reg];
        regs[chip][reg] = w.value;
        if reg % 7 == 4 && reg < 21 {
            let v = chip * 3 + reg / 7;
            let (was, now) = (old & 1 != 0, w.value & 1 != 0);
            if was && !now {
                if let Some(mut n) = open[v].take() {
                    n.end = w.t;
                    out.push(n);
                }
            } else if !was && now {
                if let Some(mut n) = open[v].take() {
                    n.end = w.t;
                    out.push(n);
                }
                let base = (v % 3) * 7;
                open[v] = Some(SidNote { voice: v, start: w.t, end: w.t, ad: regs[chip][base + 5], sr: regs[chip][base + 6], frames: Vec::new() });
            }
        }
    }
    for slot in open.iter_mut() {
        if let Some(mut n) = slot.take() {
            n.end = trace.seconds;
            out.push(n);
        }
    }
    out.retain(|n| !n.frames.is_empty() && n.end > n.start && n.frames.iter().any(|f| f.ctrl & 0xF0 != 0 && f.ctrl & 0x08 == 0));
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// The step of the grid the notes start on, in frames (a sixteenth).
fn grid_frames(notes: &[SidNote], frame: f64) -> usize {
    grid(&notes.iter().map(|n| (n.voice, n.start)).collect::<Vec<_>>(), frame)
}

/// The grid step (frames) of notes starting at (voice, seconds).
pub(crate) fn grid(starts: &[(usize, f64)], frame: f64) -> usize {
    let mut by_voice: HashMap<usize, Vec<f64>> = HashMap::new();
    for &(v, t) in starts {
        by_voice.entry(v).or_default().push(t);
    }
    let mut iois: Vec<usize> = Vec::new();
    for starts in by_voice.values() {
        for w in starts.windows(2) {
            let d = ((w[1] - w[0]) / frame).round() as usize;
            if (1..=96).contains(&d) {
                iois.push(d);
            }
        }
    }
    if iois.is_empty() {
        return 6;
    }
    // The unit most intervals are whole multiples of (preferring the
    // larger when several fit).
    let mut best = (0.0f64, 6usize);
    for u in 2..=24usize {
        let fit = iois.iter().filter(|&&d| d % u == 0).count() as f64 / iois.len() as f64;
        let score = fit + u as f64 * 0.004;
        if fit > 0.75 && score > best.0 {
            best = (score, u);
        }
    }
    best.1
}

/// The arpeggio a note's pitch steps through (offsets from `key`, a step
/// a frame), if it is one.
pub(crate) fn arpeggio(steps: &[i32], key: i32) -> Option<Vec<f32>> {
    if steps.len() < 4 {
        return None;
    }
    let changes = steps.windows(2).filter(|w| w[0] != w[1]).count();
    if changes * 2 < steps.len() {
        return None;
    }
    for period in 2..=4usize {
        let same = (period..steps.len()).filter(|&k| steps[k] == steps[k - period]).count();
        if same * 10 >= (steps.len() - period) * 8 {
            let figure: Vec<f32> = steps[..period].iter().map(|&s| (s - key) as f32).collect();
            if figure.iter().all(|o| o.abs() <= 24.0) {
                return Some(figure);
            }
        }
    }
    None
}

/// A vibrato at the end of a pitch line (after any slide into it):
/// (rate Hz, depth semitones, the frame it starts at, its centre).
pub(crate) fn vibrato(pitch: &[f32], frame: f64) -> Option<(f32, f32, usize, f32)> {
    if pitch.len() < 8 {
        return None;
    }
    // The earliest start from which the rest swings about one centre.
    for start in 0..pitch.len().saturating_sub(6) {
        let tail = &pitch[start..];
        let lo = tail.iter().cloned().fold(f32::MAX, f32::min);
        let hi = tail.iter().cloned().fold(f32::MIN, f32::max);
        let depth = (hi - lo) / 2.0;
        if !(0.04..2.5).contains(&depth) {
            continue;
        }
        let centre = (hi + lo) / 2.0;
        let crossings = tail.windows(2).filter(|w| (w[0] - centre).signum() != (w[1] - centre).signum()).count();
        if crossings >= 4 {
            let rate = crossings as f64 / 2.0 / (tail.len() as f64 * frame);
            return Some((rate as f32, depth, start, centre));
        }
    }
    None
}

/// A part's patch: the SID voice as a Score synth.
fn patch(n: &SidNote, pwm: (f32, f32), mos8580: bool) -> String {
    let wave = n.wave();
    let (a, d) = (ATTACK[(n.ad >> 4) as usize], DECAY[(n.ad & 15) as usize]);
    let (s, r) = ((n.sr >> 4) as f32 / 15.0, DECAY[(n.sr & 15) as usize]);
    let width = n.frames.iter().find(|f| f.ctrl & 0x40 != 0).map_or(0.5, |f| (f.pw.min(4095) as f32 / 4096.0).clamp(0.05, 0.95));
    let osc = if wave & 0x80 != 0 {
        "noise: @white".to_string()
    } else if wave & 0x40 != 0 {
        let pwm_text = if pwm.1 > 0.005 { format!(" pwm: {} pwm_depth: {}", num(pwm.0 as f64, 2), num(pwm.1 as f64, 3)) } else { String::new() };
        // Pulse combined with saw or triangle: the narrow, thin pulse the
        // combination sounds like.
        let width = if wave & 0x30 != 0 { (width * 0.5).max(0.05) } else { width };
        format!("osc: @pulse width: {}{pwm_text}", num(width as f64, 3))
    } else if wave & 0x20 != 0 {
        "osc: @saw".to_string()
    } else {
        "osc: @tri".to_string()
    };
    let filter = if n.filtered() {
        let f = n.frames.iter().find(|f| f.filtered).copied().unwrap_or(n.frames[0]);
        format!(" filter: @lp cutoff: {} res: 2", num(cutoff_hz(f.cutoff, mos8580) as f64, 0))
    } else {
        String::new()
    };
    let env = format!("attack: {} decay: {} sustain: {} release: {}", num(a as f64, 3), num(d as f64, 3), num(s as f64, 3), num(r as f64, 3));
    if n.click() {
        // A drum: a frame of noise, then the tone (its pitch drop is the
        // note's bend).
        let click = n.frames.iter().take_while(|f| f.ctrl & 0x80 != 0).count().max(1) as f64 * 0.02;
        return format!("Synth{{layers: [{{noise: @white attack: 0.001 decay: {} sustain: 0 release: 0.01 gain: 0.45}} {{{osc} {env} delay: {} gain: 0.45{filter}}}]}}", num(click, 3), num(click, 3));
    }
    format!("Synth{{{osc} {env} gain: 0.45{filter}}}")
}

pub struct Meta {
    pub title: String,
    pub artist: String,
    pub year: Option<u16>,
    pub mos8580: bool,
    pub header: Vec<String>,
}

/// The trace as an arrangement, and the second of the tune its beat 0 is
/// (the first note's place in its sixteenth).
pub fn convert(trace: &Trace, meta: &Meta) -> Result<(Arrangement, f64), String> {
    let notes = notes(trace);
    if notes.is_empty() {
        return Err("the tune plays no notes".into());
    }
    let frame = 1.0 / frames_per_second(trace);
    let mut unit = grid_frames(&notes, frame);
    // A sixteenth at a tempo a score plays at.
    let bpm_of = |u: usize| 60.0 / (4.0 * u as f64 * frame);
    while bpm_of(unit) > 190.0 {
        unit *= 2;
    }
    while bpm_of(unit) < 70.0 && unit % 2 == 0 {
        unit /= 2;
    }
    let bpm = bpm_of(unit);
    let beat_of = |t: f64| t * bpm / 60.0;
    // The music starts on the grid: the first note's place in its
    // sixteenth (not the silence before it, which is kept whole).
    let first = notes.iter().map(|n| n.start).fold(f64::MAX, f64::min);
    let sixteenth = unit as f64 * frame;
    let t0 = first - (first / sixteenth).floor() * sixteenth;
    let frame_beats = beat_of(frame) as f32;
    // Parts: a voice and its instrument (waveform, click, envelope).
    let mut groups: Vec<((usize, u8, bool, u8, u8), Vec<&SidNote>)> = Vec::new();
    for n in &notes {
        let key = (n.voice, n.wave(), n.click(), n.ad, n.sr);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(n),
            None => groups.push((key, vec![n])),
        }
    }
    // Rare instruments join their voice's commonest one with the same wave.
    groups.sort_by_key(|(k, v)| (k.0, std::cmp::Reverse(v.len())));
    let mut per_voice: HashMap<usize, usize> = HashMap::new();
    for n in &notes {
        *per_voice.entry(n.voice).or_default() += 1;
    }
    let mut merged: Vec<((usize, u8, bool, u8, u8), Vec<&SidNote>)> = Vec::new();
    for (k, v) in groups {
        // An instrument with a tenth of its voice's notes keeps its part.
        let small = v.len() < 8 || v.len() * 10 < per_voice[&k.0];
        match merged.iter_mut().find(|(m, _)| m.0 == k.0 && m.1 == k.1 && m.2 == k.2 && (small || (m.3 == k.3 && m.4 == k.4))) {
            Some((_, into)) => into.extend(v),
            None => merged.push((k, v)),
        }
    }
    let mut parts = Vec::new();
    let mut taken: Vec<String> = Vec::new();
    let mut hist = [0f64; 12];
    for ((voice, wave, click, _, _), group) in &merged {
        let mean = group.iter().filter_map(|n| n.frames.first()).map(|f| f.pitch as f64).sum::<f64>() / group.len() as f64;
        let (role, base) = if *click || wave & 0x80 != 0 {
            (Role::Drums, "drums")
        } else if mean < 50.0 {
            (Role::Bass, "bass")
        } else if group.iter().any(|n| n.frames.len() > 3 && arpeggio(&n.frames.iter().map(|f| f.pitch.round() as i32).collect::<Vec<_>>(), n.frames[0].pitch.round() as i32).is_some()) {
            (Role::Keys, "arp")
        } else {
            (Role::Lead, "lead")
        };
        let mut name = format!("v{}_{base}", voice + 1);
        let mut k = 2;
        while taken.contains(&name) {
            name = format!("v{}_{base}_{k}", voice + 1);
            k += 1;
        }
        taken.push(name.clone());
        // The pulse width's sweep: its swing and how fast it turns.
        let mut swings = Vec::new();
        for n in group.iter().filter(|n| n.frames.len() >= 6) {
            let pws: Vec<f32> = n.frames.iter().map(|f| f.pw as f32 / 4096.0).collect();
            let lo = pws.iter().cloned().fold(f32::MAX, f32::min);
            let hi = pws.iter().cloned().fold(f32::MIN, f32::max);
            if hi - lo > 0.01 {
                let secs = n.frames.len() as f32 * frame as f32;
                swings.push(((0.5 / secs).clamp(0.1, 8.0), ((hi - lo) / 2.0).min(0.45)));
            }
        }
        swings.sort_by(|a, b| a.1.total_cmp(&b.1));
        let pwm = swings.get(swings.len() / 2).copied().unwrap_or((0.0, 0.0));
        let model = group.iter().max_by_key(|n| n.frames.len()).unwrap();
        let node = patch(model, pwm, meta.mos8580);
        let mut out = Vec::new();
        for n in group {
            let pitches: Vec<f32> = n.frames.iter().map(|f| f.pitch).collect();
            let steps: Vec<i32> = pitches.iter().map(|p| p.round() as i32).collect();
            // A drum's pitch is where its tone starts.
            let key_pitch = if *click { n.frames.iter().find(|f| f.ctrl & 0x80 == 0).map_or(pitches[0], |f| f.pitch) } else { pitches[0] };
            let key = key_pitch.round();
            let mut fx = NoteFx::default();
            if let Some(figure) = arpeggio(&steps, key as i32) {
                for (i, o) in figure.iter().enumerate().take(4) {
                    fx.arp[i] = *o;
                }
                fx.arp_n = figure.len().min(4) as u8;
                fx.arp_step = (frame_beats * 1e5).round() / 1e5;
            } else {
                // A vibrato (after any slide into it) and the line it
                // swings about.
                let vib = vibrato(&pitches, frame);
                let mut line: Vec<(f32, f32)> = n.frames.iter().map(|f| (frame_beats * ((f.t - n.start) / frame).round() as f32, f.pitch - key)).collect();
                if let Some((rate, depth, from, centre)) = vib {
                    fx.vib = ((rate * 100.0).round() / 100.0, (depth * 1000.0).round() / 1000.0, ((from as f32 * frame_beats) * 1e4).round() / 1e4);
                    for p in line.iter_mut().skip(from) {
                        p.1 = centre - key;
                    }
                }
                for (i, &(b, v)) in breakpoints(&line, 0.06, 0.0, FX_POINTS).iter().enumerate() {
                    fx.bend[i] = ((b * 1e4).round() / 1e4, (v * 1000.0).round() / 1000.0);
                    fx.bend_n = i as u8 + 1;
                }
            }
            if n.filtered() {
                let line: Vec<(f32, f32)> = n.frames.iter().map(|f| ((beat_of(f.t - n.start)) as f32, cutoff_hz(f.cutoff, meta.mos8580))).collect();
                let first = line[0].1;
                if line.iter().any(|p| (p.1 - first).abs() > first * 0.05) {
                    for (i, &(b, v)) in breakpoints(&line, first * 0.04, first, FX_POINTS).iter().enumerate() {
                        fx.cutoff[i] = ((b * 1e4).round() / 1e4, v.round());
                        fx.cutoff_n = i as u8 + 1;
                    }
                }
            }
            // The master volume's moves (fades).
            let vols: Vec<(f32, f32)> = n.frames.iter().map(|f| ((beat_of(f.t - n.start)) as f32, f.volume as f32 / 15.0)).collect();
            let peak = vols.iter().map(|v| v.1).fold(0.0f32, f32::max).max(1e-3);
            let rel: Vec<(f32, f32)> = vols.iter().map(|&(b, v)| (b, v / peak)).collect();
            for (i, &(b, v)) in breakpoints(&rel, 0.05, 1.0, FX_POINTS).iter().enumerate() {
                fx.level[i] = ((b * 1e4).round() / 1e4, (v * 1000.0).round() / 1000.0);
                fx.level_n = i as u8 + 1;
            }
            // On the frame grid from the first note.
            let on = ((n.start - t0) / frame).round();
            let off = ((n.end - t0) / frame).round().max(on + 1.0);
            let beat = on * frame_beats as f64;
            let dur = (off - on) * frame_beats as f64;
            if role != Role::Drums {
                hist[(key as i32).rem_euclid(12) as usize] += dur;
            }
            out.push(Note { beat: (beat * 1e4).round() / 1e4, key, dur: (dur * 1e4).round() / 1e4, vel: ((peak as f64) * 0.9 * 100.0).round() / 100.0, fx: (!fx.is_plain()).then_some(fx) });
        }
        out.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        let wave_name = match wave {
            w if w & 0x80 != 0 => "noise",
            w if w & 0x40 != 0 && w & 0x30 != 0 => "pulse combined",
            w if w & 0x40 != 0 => "pulse",
            w if w & 0x20 != 0 => "saw",
            _ => "triangle",
        };
        parts.push(Part {
            name,
            role,
            melody: false,
            about: format!("SID voice {}{}, {wave_name}{}{}", voice % 3 + 1, if trace.chips > 1 { format!(" of chip {}", voice / 3 + 1) } else { String::new() }, if *click { " with a noise click" } else { "" }, if model.filtered() { ", filtered" } else { "" }),
            voice: Voice::Node(node),
            notes: out,
            pedal: Vec::new(),
            programs: Vec::new(),
            amp: 0.6,
            pan: [0.0, -0.3, 0.3][voice % 3],
        });
    }
    // The tune: the lead with the most notes.
    if let Some(i) = parts.iter().enumerate().filter(|(_, p)| p.role == Role::Lead).max_by_key(|(_, p)| p.notes.len()).map(|(i, _)| i) {
        parts[i].melody = true;
    }
    let end = parts.iter().flat_map(|p| p.notes.iter()).map(|n| n.beat + n.dur).fold(0.0, f64::max);
    let per_bar = 4.0;
    let bar_count = (end / per_bar).ceil().max(1.0) as usize;
    let bars: Vec<f64> = (0..=bar_count).map(|b| b as f64 * per_bar).collect();
    // Sections of eight bars (the form is found again by the arranger).
    let mut sections = Vec::new();
    let mut from = 0;
    while from < bar_count {
        let to = (from + 8).min(bar_count);
        sections.push(Section { name: format!("part_{}", sections.len() + 1), kind: "part".into(), letter: (b'A' + (sections.len() % 26) as u8) as char, from, to });
        from = to;
    }
    let mut header = meta.header.clone();
    header.push(format!(
        "C64 SID tune ({}{}): {} voices, played {:.0} times a second; {} notes; a sixteenth is {} frames.",
        if meta.mos8580 { "8580" } else { "6581" },
        if trace.chips > 1 { format!(", {} chips", trace.chips) } else { String::new() },
        trace.chips * 3,
        frames_per_second(trace),
        notes.len(),
        unit
    ));
    header.push("Each part is a SID voice playing one instrument, on a SID-style synth (pulse with its width modulation, saw, triangle, noise, the chip's envelope times and filter); a note's options are what the player did to it frame by frame: arp (the chip arpeggio), vib, bend (slides, drum pitch drops), cutoff_env (filter sweeps), env (volume).".into());
    Ok((Arrangement {
        title: meta.title.clone(),
        artist: meta.artist.clone(),
        year: meta.year,
        genre: "C64 SID".into(),
        key: key_of_histogram(&hist),
        bpm: (bpm * 1000.0).round() / 1000.0,
        sig: (4, 4),
        bars,
        parts,
        sections,
        tempo: Vec::new(),
        notes: header,
        rights: None,
        limiter_gain: 1.2,
        excerpt: false,
    }, t0))
}
