//! A played module as an [`Arrangement`]: a part per instrument on a
//! sampler that plays the module's own sample (its loop kept), every note
//! with its slides, vibrato, arpeggio, volume and pan as the Score's note
//! options; the order list as sections (a letter per pattern), the speed
//! and tempo changes as the tempo map.

use super::replay::{self, Played};
use super::{Kind, Module};
use crate::chip::shape::{self, Point};
use crate::analyse::key_of_histogram;
use crate::arrangement::{Arrangement, Note, Part, Section};
use crate::gm::{Role, Voice};
use std::sync::Arc;

/// A sample the Score plays, by the library name its `Sampler` names.
#[derive(Clone)]
pub struct SampleAsset {
    pub asset: makepad_score_live::AssetRef,
    pub sample: Arc<makepad_score_player::Sample>,
    /// What it is, for the credit and the Resources list.
    pub name: String,
}

pub struct Converted {
    pub arrangement: Arrangement,
    pub samples: Vec<SampleAsset>,
}

/// A stable short hash of the file (the samples' library names).
pub fn hash(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3));
    format!("{:08x}", (h >> 32) as u32 ^ h as u32)
}

/// What a part is called and does, from its instrument's and sample's
/// names, then from what it plays.
fn role_of(names: &str, notes: &[&replay::Note], sample: &super::Sample) -> (Role, &'static str) {
    let lower = names.to_lowercase();
    // Words without their numbers ("BD1", "snare02"); a long key matches
    // inside a word too ("SuperHyperBass").
    let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).map(|w| w.trim_end_matches(|c: char| c.is_ascii_digit())).filter(|w| !w.is_empty()).collect();
    let has = |keys: &[&str]| words.iter().any(|w| keys.iter().any(|k| *w == *k || (k.len() >= 4 && w.contains(k))));
    if has(&["kick", "bd", "bassdrum", "kik", "bdrum"]) || lower.contains("bass drum") {
        return (Role::Drums, "kick");
    }
    if has(&["snare", "sd", "snr", "snar"]) {
        return (Role::Drums, "snare");
    }
    if has(&["hat", "hh", "hihat", "hiha", "chh", "ohh"]) {
        return (Role::Drums, "hat");
    }
    if has(&["crash", "ride", "cymbal", "cym", "splash"]) {
        return (Role::Drums, "cymbal");
    }
    if has(&["tom", "toms"]) {
        return (Role::Drums, "tom");
    }
    if has(&["clap"]) {
        return (Role::Drums, "clap");
    }
    if has(&["perc", "drum", "drums", "conga", "bongo", "shaker", "tamb", "cowbell", "rim", "break", "loop"]) {
        return (Role::Drums, "perc");
    }
    if has(&["bass", "bas", "sub"]) {
        return (Role::Bass, "bass");
    }
    if has(&["pad", "choir", "aah", "ooh", "vox", "atmo", "strings", "string", "str", "violin", "cello"]) {
        return (Role::Pads, "pad");
    }
    if has(&["piano", "pno", "organ", "chord", "chords", "keys", "rhodes", "epiano", "harp", "bell", "bells"]) {
        return (Role::Keys, "keys");
    }
    if has(&["guitar", "gtr", "guit", "dist"]) {
        return (Role::Guitar, "guitar");
    }
    if has(&["brass", "horn", "trumpet", "trp", "sax", "flute", "trombone"]) {
        return (Role::Brass, "brass");
    }
    if has(&["lead", "solo", "melody", "arp", "sqr", "square", "saw", "pulse", "synth"]) {
        return (Role::Lead, "lead");
    }
    // By what it plays: a short one-shot on few keys is a drum.
    let mut keys: Vec<i32> = notes.iter().map(|n| n.key.round() as i32).collect();
    keys.sort_unstable();
    keys.dedup();
    let seconds = if sample.rate > 0.0 { sample.left.len() as f64 / sample.rate } else { 1.0 };
    if sample.looped.is_none() && seconds < 0.4 && keys.len() <= 3 {
        return (Role::Drums, "perc");
    }
    let mean = notes.iter().map(|n| n.key as f64).sum::<f64>() / notes.len().max(1) as f64;
    if mean < 50.0 {
        return (Role::Bass, "bass");
    }
    let overlapping = notes.windows(2).filter(|w| w[1].beat < w[0].end - 1e-6).count();
    if overlapping * 3 > notes.len() {
        return (Role::Keys, "keys");
    }
    (Role::Lead, "lead")
}

/// The genre a module's format and tracker say.
pub fn genre(m: &Module) -> &'static str {
    match m.kind {
        Kind::Mod if m.channels == 4 => "Amiga MOD",
        Kind::Mod => "PC MOD",
        Kind::S3m => "Scream Tracker S3M",
        Kind::Xm => "FastTracker XM",
        Kind::It => "Impulse Tracker IT",
    }
}

/// The module as an arrangement. `title` and `artist` override what the
/// file says (an index's credits); `notes` are header lines (licence,
/// credit) put first.
pub fn convert(m: &Module, played: &Played, file_hash: &str, title: &str, artist: &str, notes: Vec<String>) -> Converted {
    // Samples: those the notes play.
    let mut used: Vec<usize> = played.notes.iter().map(|n| n.sample).collect();
    used.sort_unstable();
    used.dedup();
    let mut samples = Vec::new();
    let mut asset_of = std::collections::HashMap::new();
    for &si in &used {
        let s = &m.samples[si];
        let left: Arc<[f32]> = s.left.clone().into();
        let right: Option<Arc<[f32]>> = s.right.clone().map(Into::into);
        let mut sample = makepad_score_player::Sample::new(left, right, s.rate as f32);
        if let Some(lp) = s.sustain.or(s.looped) {
            sample = sample.with_loop(lp.from as u32, lp.to as u32);
        }
        let asset = makepad_score_live::AssetRef { id: format!("trk-{file_hash}-s{}", si + 1), rev: "1".into() };
        asset_of.insert(si, asset.clone());
        samples.push(SampleAsset { asset, sample: Arc::new(sample), name: s.name.clone() });
    }
    // Parts: an instrument and sample pair each (an XM or IT instrument
    // over several samples is a part per sample: a sampler plays one).
    let mut groups: Vec<((usize, usize), Vec<&replay::Note>)> = Vec::new();
    for n in &played.notes {
        match groups.iter_mut().find(|(k, _)| *k == (n.instrument, n.sample)) {
            Some((_, v)) => v.push(n),
            None => groups.push(((n.instrument, n.sample), vec![n])),
        }
    }
    groups.sort_by_key(|((i, s), _)| (*i, *s));
    let tick_beats = 1.0 / played.ticks_per_beat.max(1.0);
    let mut parts: Vec<Part> = Vec::new();
    let mut taken: Vec<String> = Vec::new();
    let mut hist = [0f64; 12];
    for ((inst, si), notes) in &groups {
        let instrument = &m.instruments[*inst];
        let sample = &m.samples[*si];
        let names = format!("{} {}", instrument.name, sample.name);
        let (role, base) = role_of(&names, notes, sample);
        let mut name = base.to_string();
        let mut k = 2;
        while taken.contains(&name) {
            name = format!("{base}_{k}");
            k += 1;
        }
        taken.push(name.clone());
        let pan = notes.iter().filter_map(|n| n.ticks.first()).map(|t| t.pan).sum::<f32>() / notes.len().max(1) as f32;
        let pan = (pan * 100.0).round() / 100.0;
        let mut out: Vec<Note> = Vec::new();
        for n in notes {
            // A note is cut where its arpeggio changes: the Score's
            // arpeggio is one figure a note.
            let mut cuts = vec![0usize];
            for i in 1..n.ticks.len() {
                if n.ticks[i].arp_param != n.ticks[i - 1].arp_param && (n.ticks[i].beat - n.ticks[cuts[cuts.len() - 1]].beat) >= tick_beats * 2.0 - 1e-9 {
                    cuts.push(i);
                }
            }
            for (j, &from) in cuts.iter().enumerate() {
                let to = cuts.get(j + 1).copied().unwrap_or(n.ticks.len());
                let ticks = &n.ticks[from..to];
                let Some(first) = ticks.first() else { continue };
                let start = first.beat;
                let end = n.ticks.get(to).map_or(n.end, |t| t.beat).max(start + 1e-3);
                let key = if j == 0 { n.key } else { first.pitch };
                let key = if (key - key.round()).abs() < 0.02 { key.round() } else { (key * 100.0).round() / 100.0 };
                let points: Vec<Point> = ticks.iter().map(|t| Point { beat: (t.beat - start) as f32, pitch: t.pitch, level: t.level, pan: t.pan }).collect();
                let (vel, mut fx) = shape::shape(key, &points, pan);
                if vel < 0.004 {
                    continue;
                }
                if let Some((hz, depth)) = n.vibrato {
                    if let Some(t) = ticks.iter().find(|t| t.vibrato.abs() > 1e-4) {
                        fx.vib = ((hz * 100.0).round() / 100.0, (depth * 1000.0).round() / 1000.0, ((t.beat - start) as f32 * 1e4).round() / 1e4);
                    }
                }
                if first.arp_param != 0 {
                    fx.arp = [0.0, (first.arp_param >> 4) as f32, (first.arp_param & 15) as f32, 0.0];
                    fx.arp_n = 3;
                    let step = ticks.get(1).map_or(tick_beats, |t| t.beat - first.beat);
                    fx.arp_step = (step as f32 * 1e5).round() / 1e5;
                }
                if j == 0 && n.offset > 0 && !sample.left.is_empty() {
                    fx.start = ((n.offset as f32 / sample.left.len() as f32).min(0.999) * 1e5).round() / 1e5;
                }
                let dur = ((end - start) * 1e4).round() / 1e4;
                if role != Role::Drums {
                    hist[(key.round() as i32).rem_euclid(12) as usize] += dur;
                }
                out.push(Note { beat: (start * 1e4).round() / 1e4, key, dur, vel, fx: (!fx.is_plain()).then_some(fx) });
            }
        }
        out.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        let asset = &asset_of[si];
        let looped = sample.sustain.or(sample.looped).map_or(String::new(), |l| format!(", looped {}-{}", l.from, l.to));
        let node = format!("Sampler{{src: asset(\"{}\", \"{}\") root: 60 attack: 0 release: 0.01}}", asset.id, asset.rev);
        let label = |s: &str| if s.trim().is_empty() { String::new() } else { format!(" \"{}\"", s.trim().replace('"', "'")) };
        parts.push(Part {
            name,
            role,
            melody: false,
            voice: Voice::Node(node),
            about: format!("instrument {}{}, sample {}{} ({} frames at {} Hz{looped})", inst + 1, label(&instrument.name), si + 1, label(&sample.name), sample.left.len(), sample.rate.round()),
            notes: out,
            pedal: Vec::new(),
            programs: Vec::new(),
            amp: 0.6,
            pan: if m.kind == Kind::Mod { (pan * 0.7 * 100.0).round() as f64 / 100.0 } else { pan as f64 },
        });
    }
    // Amiga panning is hard left and right; a cover keeps 70% of it.
    if m.kind == Kind::Mod {
        for p in parts.iter_mut() {
            for n in p.notes.iter_mut() {
                if let Some(fx) = n.fx.as_mut() {
                    fx.pan *= 0.7;
                }
            }
        }
    }
    parts.retain(|p| !p.notes.is_empty());
    // The tune: the busiest lead, else the highest pitched part.
    let mean = |p: &Part| p.notes.iter().map(|n| n.key as f64).sum::<f64>() / p.notes.len().max(1) as f64;
    let melody = parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.role == Role::Lead)
        .max_by_key(|(_, p)| p.notes.len())
        .or_else(|| parts.iter().enumerate().filter(|(_, p)| p.role != Role::Drums && p.role != Role::Bass).max_by(|a, b| mean(a.1).total_cmp(&mean(b.1))))
        .map(|(i, _)| i);
    if let Some(i) = melody {
        parts[i].melody = true;
    }
    // Bars: four beats, a new bar at each order (a pattern broken early
    // makes a short bar).
    let per_bar = 4.0;
    let mut bars: Vec<f64> = Vec::new();
    let mut sections: Vec<Section> = Vec::new();
    let mut letters: Vec<u16> = Vec::new();
    for (i, &(start, order, pattern)) in played.orders.iter().enumerate() {
        let end = played.orders.get(i + 1).map_or(played.end, |o| o.0);
        if end <= start + 1e-9 {
            continue;
        }
        let from = bars.len();
        let mut b = start;
        while b < end - 1e-9 {
            bars.push((b * 1e6).round() / 1e6);
            b += per_bar;
        }
        let letter_index = match letters.iter().position(|&p| p == pattern) {
            Some(k) => k,
            None => {
                letters.push(pattern);
                letters.len() - 1
            }
        };
        const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let letter = LETTERS[letter_index % LETTERS.len()] as char;
        sections.push(Section { name: format!("order_{order}"), kind: format!("pattern {pattern}"), letter, from, to: bars.len() });
    }
    bars.push((played.end * 1e6).round() / 1e6);
    // An order shorter than a bar (a pattern broken off at once) joins
    // the section before it (or after it, at the start).
    let len = |s: &Section, bars: &[f64]| bars[s.to.min(bars.len() - 1)] - bars[s.from];
    let mut i = 0;
    while i < sections.len() && sections.len() > 1 {
        if len(&sections[i], &bars) < per_bar - 1e-6 {
            let gone = sections.remove(i);
            if i > 0 {
                sections[i - 1].to = gone.to;
            } else {
                sections[0].from = gone.from;
            }
            continue;
        }
        i += 1;
    }
    if sections.is_empty() {
        sections.push(Section { name: "song".into(), kind: "song".into(), letter: 'A', from: 0, to: bars.len().saturating_sub(1) });
    }
    let bpm = played.tempo.first().map_or(125.0, |t| t.1);
    // A tempo change on the last row (a slow speed that ends the song)
    // would hold the last note for ages; a score's tempo is 20..400.
    let last_row = played.end - 1.0 / played.rows_per_beat as f64 - 1e-9;
    let tempo: Vec<(f64, f64)> = played.tempo.iter().skip(1).filter(|t| t.0 < last_row).map(|&(b, t)| ((b * 1e4).round() / 1e4, (t.clamp(20.0, 400.0) * 1000.0).round() / 1000.0)).collect();
    let mut header = notes;
    header.push(format!(
        "{} module{}: {} channels, {} patterns, {} instruments; {} orders played, {:.0} s{}.",
        genre(m),
        if m.tracker.trim().is_empty() { String::new() } else { format!(" ({})", m.tracker.trim()) },
        m.channels,
        m.patterns.len(),
        m.instruments.len(),
        played.orders.len(),
        played.seconds,
        if played.rows_per_beat == 4 { String::new() } else { format!(", {} rows a beat", played.rows_per_beat) }
    ));
    header.push("Each part is one of the module's samples on a sampler (its loop kept); a note's options are the tracker's effects: bend (slides, portamento), vib, arp, env (volume slides, envelopes), pan, start (sample offset).".into());
    let message: Vec<String> = m.message.lines().map(str::trim).filter(|l| !l.is_empty()).take(12).map(str::to_string).collect();
    if !message.is_empty() {
        header.push(format!("From the module: {}", message.join(" / ")));
    }
    let title = if title.trim().is_empty() { m.title.trim().to_string() } else { title.trim().to_string() };
    Converted {
        arrangement: Arrangement {
            title: if title.is_empty() { "Untitled module".into() } else { title },
            artist: artist.trim().to_string(),
            year: None,
            genre: genre(m).into(),
            key: key_of_histogram(&hist),
            bpm: (bpm * 1000.0).round() / 1000.0,
            sig: (4, 4),
            bars,
            parts,
            sections,
            tempo,
            notes: header,
            rights: None,
            limiter_gain: 1.2,
            excerpt: false,
        },
        samples,
    }
}

/// Register the samples with the engine for renders in this process (a
/// Stage document gets them from the library instead).
pub fn register(samples: &[SampleAsset]) {
    for s in samples {
        makepad_score_live::render::register_sample(&s.asset, s.sample.clone());
    }
}
