//! A sound chip's voices frame by frame (pitch, level, timbre, and the
//! moments a driver retriggered a note) as an [`Arrangement`]: notes found
//! where a voice starts sounding, is retriggered, jumps to a new pitch it
//! keeps, or restarts its software envelope; within a note, the pitch read
//! as an arpeggio, a vibrato or a bend and the level as its envelope; a
//! part per voice and timbre on the patch the chip's converter writes. The
//! game consoles' chips (NES, Game Boy, the PSG) come through here.

use super::shape::breakpoints;
use super::sid::convert::{arpeggio, grid, vibrato};
use crate::analyse::key_of_histogram;
use crate::arrangement::{Arrangement, Note, NoteFx, Part, Section, FX_POINTS};
use crate::gm::{Role, Voice};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Seconds.
    pub t: f64,
    /// A MIDI number; NaN when the voice makes no tone.
    pub pitch: f32,
    /// 0..1; 0 is silent.
    pub level: f32,
    /// The driver restarted the note on this frame.
    pub retrigger: bool,
    /// Which sound (a duty, a waveform, a noise mode, a sample).
    pub timbre: u16,
    /// A filter's cutoff (Hz) when the chip has one, else NaN.
    pub cutoff: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tone,
    /// Unpitched (the pitch is the noise's colour).
    Noise,
    /// A sample played (the key its rate).
    Sample,
}

pub struct ChipVoice {
    /// "pulse1", "triangle" ...
    pub name: String,
    pub kind: Kind,
    pub frames: Vec<Frame>,
}

pub struct Meta {
    pub title: String,
    pub artist: String,
    pub year: Option<u16>,
    pub genre: String,
    pub header: Vec<String>,
    /// Frames a second the voices were read at.
    pub rate: f64,
}

struct RawNote {
    voice: usize,
    frames: Vec<Frame>,
    end: f64,
}

fn segment(v: usize, voice: &ChipVoice, frame: f64) -> Vec<RawNote> {
    let mut out: Vec<RawNote> = Vec::new();
    let mut cur: Option<RawNote> = None;
    let fs = &voice.frames;
    for (i, f) in fs.iter().enumerate() {
        let sounding = f.level > 0.004 && (voice.kind != Kind::Tone || f.pitch.is_finite());
        if !sounding {
            if let Some(mut n) = cur.take() {
                n.end = f.t;
                out.push(n);
            }
            continue;
        }
        let start_new = match &cur {
            None => true,
            Some(n) => {
                let last = n.frames[n.frames.len() - 1];
                let first = n.frames[0];
                let base = first.pitch;
                // A pitch it keeps (two frames on), not an arpeggio's step
                // or a vibrato's swing.
                let kept = |k: usize| fs.get(k).is_some_and(|g| (g.pitch - f.pitch).abs() < 0.3 && g.level > 0.004);
                let moved = voice.kind == Kind::Tone && (f.pitch - last.pitch).abs() >= 0.5 && (f.pitch - base).abs() >= 0.5 && kept(i + 1) && kept(i + 2) && arpeggio(&n.frames.iter().map(|g| g.pitch.round() as i32).collect::<Vec<_>>(), base.round() as i32).is_none();
                let attack = f.level > last.level + 0.2;
                f.retrigger || moved || attack || f.timbre != last.timbre && voice.kind == Kind::Sample
            }
        };
        if start_new {
            if let Some(mut n) = cur.take() {
                n.end = f.t;
                out.push(n);
            }
            cur = Some(RawNote { voice: v, frames: Vec::new(), end: f.t });
        }
        cur.as_mut().unwrap().frames.push(*f);
    }
    if let Some(mut n) = cur.take() {
        n.end = n.frames.last().map_or(0.0, |f| f.t + frame);
        out.push(n);
    }
    out
}

/// The voices as an arrangement; `patch(voice, timbre)` writes a part's
/// instrument node. Returns the second the Score's beat 0 is.
/// `patch(voice, timbre)` gives a part's instrument node and whether it
/// plays notes (a pitched sample) or hits (a drum sample, a noise).
pub fn convert(voices: &[ChipVoice], meta: &Meta, patch: &dyn Fn(&ChipVoice, u16) -> (String, bool)) -> Result<(Arrangement, f64), String> {
    let frame = 1.0 / meta.rate.max(1.0);
    let notes: Vec<RawNote> = voices.iter().enumerate().flat_map(|(v, voice)| segment(v, voice, frame)).collect();
    // Velocities against the loudest moment of the song.
    let loudest = voices.iter().flat_map(|v| v.frames.iter()).map(|f| f.level).fold(0.0f32, f32::max).max(1e-3);
    if notes.is_empty() {
        return Err("the tune plays no notes".into());
    }
    let mut unit = grid(&notes.iter().map(|n| (n.voice, n.frames[0].t)).collect::<Vec<_>>(), frame);
    let bpm_of = |u: usize| 60.0 / (4.0 * u as f64 * frame);
    while bpm_of(unit) > 190.0 {
        unit *= 2;
    }
    while bpm_of(unit) < 70.0 && unit % 2 == 0 {
        unit /= 2;
    }
    let bpm = bpm_of(unit);
    let frame_beats = (frame * bpm / 60.0) as f32;
    let first = notes.iter().map(|n| n.frames[0].t).fold(f64::MAX, f64::min);
    let sixteenth = unit as f64 * frame;
    let t0 = first - (first / sixteenth).floor() * sixteenth;
    // Parts: a voice and timbre each.
    let mut groups: Vec<((usize, u16), Vec<&RawNote>)> = Vec::new();
    for n in &notes {
        let timbre = {
            let mut c: HashMap<u16, usize> = HashMap::new();
            for f in &n.frames {
                *c.entry(f.timbre).or_default() += 1;
            }
            c.into_iter().max_by_key(|(t, k)| (*k, *t)).map_or(0, |(t, _)| t)
        };
        match groups.iter_mut().find(|(k, _)| *k == (n.voice, timbre)) {
            Some((_, v)) => v.push(n),
            None => groups.push(((n.voice, timbre), vec![n])),
        }
    }
    // A timbre a voice seldom plays joins the voice's commonest one.
    groups.sort_by_key(|((v, _), g)| (*v, std::cmp::Reverse(g.len())));
    let mut per_voice: HashMap<usize, usize> = HashMap::new();
    for ((v, _), g) in &groups {
        *per_voice.entry(*v).or_default() += g.len();
    }
    let mut merged: Vec<((usize, u16), Vec<&RawNote>)> = Vec::new();
    for ((v, t), g) in groups {
        let small = g.len() < 8 || g.len() * 10 < per_voice[&v];
        let pitched_alike = voices[v].kind != Kind::Sample;
        match merged.iter_mut().find(|((mv, _), _)| *mv == v).filter(|_| small && pitched_alike) {
            Some((_, into)) => into.extend(g),
            None => merged.push(((v, t), g)),
        }
    }
    let mut groups = merged;
    for (_, g) in groups.iter_mut() {
        g.sort_by(|a, b| a.frames[0].t.total_cmp(&b.frames[0].t));
    }
    groups.sort_by_key(|((v, t), _)| (*v, *t));
    let mut parts = Vec::new();
    let mut hist = [0f64; 12];
    let mut taken: Vec<String> = Vec::new();
    for ((v, timbre), group) in &groups {
        let voice = &voices[*v];
        let mean = group.iter().filter_map(|n| n.frames.first()).map(|f| f.pitch as f64).filter(|p| p.is_finite()).sum::<f64>() / group.len().max(1) as f64;
        let (node, pitched) = patch(voice, *timbre);
        let (role, base) = match voice.kind {
            Kind::Noise => (Role::Drums, "drums"),
            _ if !pitched => (Role::Drums, "drums"),
            _ if mean < 50.0 => (Role::Bass, "bass"),
            _ => (Role::Lead, "lead"),
        };
        let mut name = format!("{}_{base}", voice.name);
        let mut k = 2;
        while taken.contains(&name) {
            name = format!("{}_{base}_{k}", voice.name);
            k += 1;
        }
        taken.push(name.clone());
        let mut out = Vec::new();
        for n in group {
            let pitches: Vec<f32> = n.frames.iter().map(|f| if f.pitch.is_finite() { f.pitch } else { 60.0 }).collect();
            let steps: Vec<i32> = pitches.iter().map(|p| p.round() as i32).collect();
            let key = pitches[0].round();
            let mut fx = NoteFx::default();
            if voice.kind != Kind::Noise {
                if let Some(figure) = arpeggio(&steps, key as i32) {
                    for (i, o) in figure.iter().enumerate().take(4) {
                        fx.arp[i] = *o;
                    }
                    fx.arp_n = figure.len().min(4) as u8;
                    fx.arp_step = (frame_beats * 1e5).round() / 1e5;
                } else {
                    let vib = vibrato(&pitches, frame);
                    let mut line: Vec<(f32, f32)> = n.frames.iter().map(|f| (frame_beats * ((f.t - n.frames[0].t) / frame).round() as f32, if f.pitch.is_finite() { f.pitch - key } else { 0.0 })).collect();
                    if let Some((rate, depth, from, centre)) = vib {
                        fx.vib = ((rate * 100.0).round() / 100.0, (depth * 1000.0).round() / 1000.0, ((from as f32 * frame_beats) * 1e4).round() / 1e4);
                        for p in line.iter_mut().skip(from) {
                            p.1 = centre - key;
                        }
                    }
                    for (i, &(b, val)) in breakpoints(&line, 0.06, 0.0, FX_POINTS).iter().enumerate() {
                        fx.bend[i] = ((b * 1e4).round() / 1e4, (val * 1000.0).round() / 1000.0);
                        fx.bend_n = i as u8 + 1;
                    }
                }
            }
            let peak = n.frames.iter().map(|f| f.level).fold(0.0f32, f32::max).max(1e-3);
            let levels: Vec<(f32, f32)> = n.frames.iter().map(|f| (frame_beats * ((f.t - n.frames[0].t) / frame).round() as f32, f.level / peak)).collect();
            for (i, &(b, val)) in breakpoints(&levels, 0.04, 1.0, FX_POINTS).iter().enumerate() {
                fx.level[i] = ((b * 1e4).round() / 1e4, (val * 1000.0).round() / 1000.0);
                fx.level_n = i as u8 + 1;
            }
            let cutoffs: Vec<(f32, f32)> = n.frames.iter().filter(|f| f.cutoff.is_finite()).map(|f| (frame_beats * ((f.t - n.frames[0].t) / frame).round() as f32, f.cutoff)).collect();
            if let Some(&(_, c0)) = cutoffs.first() {
                let mut bp = breakpoints(&cutoffs, c0 * 0.04, c0, FX_POINTS);
                if bp.is_empty() {
                    bp.push((0.0, c0));
                }
                for (i, &(b, val)) in bp.iter().enumerate() {
                    fx.cutoff[i] = ((b * 1e4).round() / 1e4, val.round());
                    fx.cutoff_n = i as u8 + 1;
                }
            }
            let on = ((n.frames[0].t - t0) / frame).round();
            let off = ((n.end - t0) / frame).round().max(on + 1.0);
            let dur = (off - on) * frame_beats as f64;
            if role != Role::Drums {
                hist[(key as i32).rem_euclid(12) as usize] += dur;
            }
            out.push(Note { beat: on * frame_beats as f64, key, dur: (dur * 1e4).round() / 1e4, vel: ((peak / loudest) as f64 * 100.0).round().max(1.0) / 100.0, fx: (!fx.is_plain()).then_some(fx) });
        }
        out.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        parts.push(Part {
            name,
            role,
            melody: false,
            about: format!("{} ({}), sound {timbre}", voice.name, match voice.kind { Kind::Tone => "tone", Kind::Noise => "noise", Kind::Sample => "samples" }),
            voice: Voice::Node(node),
            notes: out,
            pedal: Vec::new(),
            programs: Vec::new(),
            amp: 0.6,
            pan: 0.0,
        });
    }
    if let Some(i) = parts.iter().enumerate().filter(|(_, p)| p.role == Role::Lead).max_by_key(|(_, p)| p.notes.len()).map(|(i, _)| i) {
        parts[i].melody = true;
    }
    let end = parts.iter().flat_map(|p| p.notes.iter()).map(|n| n.beat + n.dur).fold(0.0, f64::max);
    let bar_count = (end / 4.0).ceil().max(1.0) as usize;
    let bars: Vec<f64> = (0..=bar_count).map(|b| b as f64 * 4.0).collect();
    let mut sections = Vec::new();
    let mut from = 0;
    while from < bar_count {
        let to = (from + 8).min(bar_count);
        sections.push(Section { name: format!("part_{}", sections.len() + 1), kind: "part".into(), letter: (b'A' + (sections.len() % 26) as u8) as char, from, to });
        from = to;
    }
    let mut header = meta.header.clone();
    header.push(format!("{}: {} voices read {:.0} times a second; a sixteenth is {unit} frames. A note's options are what the driver did to it frame by frame: arp, vib, bend, env (its volume envelope), cutoff_env.", meta.genre, voices.len(), meta.rate));
    Ok((
        Arrangement {
            title: meta.title.clone(),
            artist: meta.artist.clone(),
            year: meta.year,
            genre: meta.genre.clone(),
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
        },
        t0,
    ))
}

/// The voices played dry frame by frame on simple models of the chips'
/// waveforms (pulses by duty, the triangle, squares, an FM-ish sine,
/// noise coloured by its cutoff, samples by `sample(timbre)` at their
/// root): the original a cover is held against.
pub fn render(voices: &[ChipVoice], seconds: f64, rate: f32, sample: &dyn Fn(u16) -> Option<(std::sync::Arc<makepad_score_player::Sample>, f32)>) -> (Vec<f32>, Vec<f32>) {
    let n = (seconds * rate as f64) as usize;
    let mut out = vec![0.0f32; n];
    for v in voices {
        let mut phase = 0.0f64;
        let mut pos = 0.0f64;
        let mut lfsr: u32 = 0x7FFF;
        let mut lp = 0.0f32;
        for (i, f) in v.frames.iter().enumerate() {
            let from = (f.t * rate as f64) as usize;
            let to = v.frames.get(i + 1).map_or(n, |g| (g.t * rate as f64) as usize).min(n);
            if f.retrigger {
                pos = 0.0;
            }
            if f.level <= 0.0 || from >= to {
                continue;
            }
            let hz = 440.0 * 2f64.powf((f.pitch as f64 - 69.0) / 12.0);
            let step = hz / rate as f64;
            let smp = if v.kind == Kind::Sample { sample(f.timbre) } else { None };
            for x in out.iter_mut().take(to).skip(from) {
                let y = match v.kind {
                    Kind::Noise => {
                        phase += f.cutoff.max(100.0) as f64 / rate as f64;
                        while phase >= 1.0 {
                            phase -= 1.0;
                            let bit = (lfsr ^ (lfsr >> 1)) & 1;
                            lfsr = (lfsr >> 1) | (bit << 14);
                        }
                        let raw = if lfsr & 1 == 0 { 0.5 } else { -0.5 };
                        lp += (raw - lp) * 0.5;
                        lp
                    }
                    Kind::Sample => match &smp {
                        Some((s, root)) => {
                            let speed = 2f64.powf((f.pitch - root) as f64 / 12.0) * s.rate as f64 / rate as f64;
                            if let Some((a, b)) = s.sustain {
                                if pos >= b as f64 {
                                    pos = a as f64 + (pos - b as f64) % ((b - a).max(1) as f64);
                                }
                            }
                            let k = pos as usize;
                            pos += speed;
                            s.left.get(k).copied().unwrap_or(0.0) * 0.5
                        }
                        None => 0.0,
                    },
                    Kind::Tone => {
                        phase = (phase + step).fract();
                        let p = phase as f32;
                        if v.name.starts_with("triangle") {
                            (4.0 * (p - 0.5).abs() - 1.0) * 0.5
                        } else if v.name.starts_with("pulse") {
                            let duty = [0.125, 0.25, 0.5, 0.75][f.timbre as usize & 3];
                            if p < duty { 0.4 } else { -0.4 }
                        } else if v.name.starts_with("fm") || v.name.starts_with("opm") {
                            ((p * std::f32::consts::TAU).sin() + 0.5 * (2.0 * p * std::f32::consts::TAU).sin()) * 0.3
                        } else if p < 0.5 {
                            0.35
                        } else {
                            -0.35
                        }
                    }
                };
                *x += y * f.level;
            }
        }
    }
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    out.iter_mut().for_each(|v| *v *= 0.9 / peak);
    (out.clone(), out)
}
