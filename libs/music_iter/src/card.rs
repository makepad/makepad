//! The scorecard: a repeatable, objective reading of a rendered song, each
//! line a measurement, a 0-100 score against the targets of
//! MUSIC-AGENTS.md (mixing targets, amateur mistakes) and what to fix. The
//! same song always reads the same.
//!
//! Lines: loudness, peaks (clipping), dynamics, spectral balance, masking,
//! low-end mono, width, groove, harmony, melody, energy curve,
//! transitions, and the sung lead's pitch when the song sings.

use makepad_audio_dsp::loudness;
use crate::render::{Role, Song};
use makepad_score_live::compose::Played;

#[derive(Clone, Debug, PartialEq)]
pub struct Metric {
    pub key: String,
    /// 0..100.
    pub score: f32,
    /// The measurement, in words a mixer uses.
    pub value: String,
    /// What would raise it (empty when it is fine).
    pub fix: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub metrics: Vec<Metric>,
}

/// Each line's weight in the total.
const WEIGHTS: [(&str, f32); 13] = [
    ("loudness", 1.0),
    ("peaks", 1.0),
    ("dynamics", 0.7),
    ("balance", 1.0),
    ("masking", 1.0),
    ("low_mono", 0.7),
    ("width", 0.5),
    ("groove", 0.8),
    ("harmony", 1.0),
    ("melody", 1.0),
    ("energy", 1.2),
    ("transitions", 0.8),
    ("vocal_pitch", 0.8),
];

impl Card {
    pub fn total(&self) -> f32 {
        let (mut sum, mut weight) = (0.0, 0.0);
        for m in &self.metrics {
            let w = WEIGHTS.iter().find(|(k, _)| *k == m.key).map_or(1.0, |(_, w)| *w);
            sum += m.score * w;
            weight += w;
        }
        if weight > 0.0 { sum / weight } else { 0.0 }
    }

    pub fn get(&self, key: &str) -> Option<&Metric> {
        self.metrics.iter().find(|m| m.key == key)
    }

    /// One line: the total and every score.
    pub fn compact(&self) -> String {
        let mut out = format!("{:.0}", self.total());
        for m in &self.metrics {
            out.push_str(&format!(" · {} {:.0}", m.key, m.score));
        }
        out
    }

    /// The weakest lines with their fixes, for the model and the person.
    pub fn weakest(&self, n: usize) -> Vec<&Metric> {
        let mut v: Vec<&Metric> = self.metrics.iter().filter(|m| m.score < 85.0).collect();
        v.sort_by(|a, b| a.score.total_cmp(&b.score));
        v.truncate(n);
        v
    }

    /// Tab-separated lines: key, score, value, fix.
    pub fn to_tsv(&self) -> String {
        let mut out = format!("total\t{:.1}\t\t\n", self.total());
        for m in &self.metrics {
            out.push_str(&format!("{}\t{:.1}\t{}\t{}\n", m.key, m.score, m.value.replace('\t', " "), m.fix.replace('\t', " ")));
        }
        out
    }

    pub fn from_tsv(text: &str) -> Card {
        let metrics = text
            .lines()
            .filter_map(|line| {
                let mut f = line.split('\t');
                let key = f.next()?.to_string();
                let score = f.next()?.parse().ok()?;
                (key != "total").then(|| Metric { key, score, value: f.next().unwrap_or("").into(), fix: f.next().unwrap_or("").into() })
            })
            .collect();
        Card { metrics }
    }

    /// The text the model reads.
    pub fn report(&self) -> String {
        let mut out = format!("scorecard total {:.0}/100\n", self.total());
        for m in &self.metrics {
            out.push_str(&format!("- {} {:.0}: {}{}\n", m.key, m.score, m.value, if m.fix.is_empty() { String::new() } else { format!(" -> {}", m.fix) }));
        }
        out
    }
}

/// Keep `next` over `prev`: the total rises and no line falls by more
/// than `REGRESSION` points.
pub const REGRESSION: f32 = 8.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub delta: f32,
    pub regressions: Vec<(String, f32, f32)>,
    pub keep: bool,
}

pub fn compare(prev: &Card, next: &Card) -> Verdict {
    let delta = next.total() - prev.total();
    let regressions: Vec<(String, f32, f32)> = prev
        .metrics
        .iter()
        .filter_map(|a| next.get(&a.key).filter(|b| a.score - b.score > REGRESSION).map(|b| (a.key.clone(), a.score, b.score)))
        .collect();
    Verdict { delta, keep: delta > 0.25 && regressions.is_empty(), regressions }
}

fn clamp(v: f32) -> f32 {
    v.clamp(0.0, 100.0)
}

/// A score of 100 inside `lo..hi`, falling by `slope` per unit outside.
fn band(v: f32, lo: f32, hi: f32, slope: f32) -> f32 {
    clamp(100.0 - slope * if v < lo { lo - v } else if v > hi { v - hi } else { 0.0 })
}

fn db(x: f64) -> f32 {
    (10.0 * x.max(1e-20).log10()) as f32
}

// ------------------------------------------------------------- filters

#[derive(Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn new(kind: u8, hz: f32, q: f32, rate: f32) -> Biquad {
        let w = std::f32::consts::TAU * hz / rate;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b = match kind {
            0 => [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0],   // low-pass
            1 => [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], // high-pass
            _ => [alpha, 0.0, -alpha],                          // band-pass
        };
        Biquad { b: [b[0] / a0, b[1] / a0, b[2] / a0], a: [-2.0 * c / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// `x` through a chain, as energy per frame of `hop` samples.
fn frames(x: &[f32], chain: &[Biquad], hop: usize) -> Vec<f64> {
    let mut chain = chain.to_vec();
    let mut out = Vec::with_capacity(x.len() / hop + 1);
    let mut acc = 0.0f64;
    for (i, v) in x.iter().enumerate() {
        let mut y = *v;
        for f in chain.iter_mut() {
            y = f.run(y);
        }
        acc += (y * y) as f64;
        if (i + 1) % hop == 0 {
            out.push(acc / hop as f64);
            acc = 0.0;
        }
    }
    out
}

fn energy(x: &[f32], chain: &[Biquad]) -> f64 {
    let f = frames(x, chain, 4800);
    f.iter().sum::<f64>() / f.len().max(1) as f64
}

// ------------------------------------------------------------- the card

pub struct Opts {
    /// Integrated loudness to aim at (LUFS).
    pub target_lufs: f32,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { target_lufs: -10.0 }
    }
}

pub fn score(song: &Song, opts: &Opts) -> Card {
    let mix = &song.mix;
    let rate = mix.rate;
    let (l, r) = (&mix.left[..], &mix.right[..]);
    let mid: Vec<f32> = l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)).collect();
    let side: Vec<f32> = l.iter().zip(r).map(|(a, b)| 0.5 * (a - b)).collect();
    let mut metrics = Vec::new();
    let mut push = |key: &str, score: f32, value: String, fix: &str| metrics.push(Metric { key: key.into(), score: clamp(score), value, fix: if score < 85.0 { fix.to_string() } else { String::new() } });

    // Loudness, peaks, dynamics.
    let loud = loudness::measure(l, r, rate as f64);
    let lufs = loud.lufs as f32;
    push("loudness", band(lufs, opts.target_lufs - 1.0, opts.target_lufs + 1.0, 15.0), format!("{lufs:.1} LUFS integrated (aim {:.0})", opts.target_lufs), if lufs < opts.target_lufs { "raise the master limiter's gain, or add the master chain" } else { "lower the limiter's gain" });
    let overs = l.iter().chain(r).filter(|v| v.abs() >= 0.999).count();
    let tp = loud.true_peak as f32;
    push("peaks", 100.0 - 40.0 * (tp + 1.0).max(0.0) - (overs as f32).min(50.0), format!("true peak {tp:.1} dBTP, {overs} samples at full scale"), "keep the true peak under -1 dBTP: lower the limiter's gain or ceiling");
    let plr = tp - lufs;
    let lra = loud.lra as f32;
    push("dynamics", 0.5 * band(plr, 8.0, 12.0, 12.0) + 0.5 * band(lra, 3.0, 9.0, 10.0), format!("peak-to-loudness {plr:.1} dB, loudness range {lra:.1} LU"), if lra < 3.0 { "more contrast between sections (breakdowns quieter, drops fuller)" } else if plr < 8.0 { "less limiting: the mix is squashed" } else { "control the level jumps between sections" });

    // Spectral balance: octave bands against a modern mix's tilt.
    const CENTRES: [f32; 10] = [31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
    const REFERENCE: [f32; 10] = [-6.0, 0.0, -1.5, -4.0, -6.0, -8.0, -10.0, -12.0, -14.5, -21.0];
    let bands: Vec<f32> = CENTRES.iter().map(|hz| db(energy(&mid, &[Biquad::new(2, *hz, 1.41, rate)]))).collect();
    let top = bands.iter().cloned().fold(f32::MIN, f32::max);
    let devs: Vec<f32> = bands.iter().zip(REFERENCE).map(|(b, r)| b - top - r).collect();
    let err = devs.iter().map(|d| (d.abs() - 2.0).max(0.0)).sum::<f32>() / devs.len() as f32;
    let names = ["31", "63", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];
    let worst = devs.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, d)| (names[i], *d)).unwrap_or(("", 0.0));
    let shape: Vec<String> = bands.iter().zip(names).map(|(b, n)| format!("{n}:{:.0}", b - top)).collect();
    let fix = format!("{} Hz is {:+.0} dB against a modern mix's tilt: {}", worst.0, worst.1, if worst.1 > 0.0 { "cut it (Eq, or a lower part / high-pass)" } else { "add energy there (a brighter sound, hats, air, or a part in that register)" });
    // A dark top (4k-16k well under the tilt) is the commonest fault: it
    // counts twice.
    let dark = devs[7..].iter().map(|d| (-d - 3.0).max(0.0)).sum::<f32>() / 3.0;
    push("balance", 100.0 - 8.0 * err - 4.0 * dark, shape.join(" "), &fix);

    // Masking: kick against bass in the low end, the vocal against the rest
    // in its presence band.
    let hop = (0.05 * rate) as usize;
    let low = |x: &[f32]| frames(x, &[Biquad::new(1, 40.0, 0.71, rate), Biquad::new(0, 160.0, 0.71, rate)], hop);
    let presence = |x: &[f32]| frames(x, &[Biquad::new(1, 1000.0, 0.71, rate), Biquad::new(0, 4000.0, 0.71, rate)], hop);
    let mono = |s: &crate::render::Stem| -> Vec<f32> { s.left.iter().zip(&s.right).map(|(a, b)| 0.5 * (a + b)).collect() };
    let mut mask_scores = Vec::new();
    let mut mask_notes = Vec::new();
    let mut mask_fix = String::new();
    if let (Some(d), Some(b)) = (song.stem(Role::Drums), song.stem(Role::Bass)) {
        let (d, b) = (low(&mono(d)), low(&mono(b)));
        let floor = d.iter().chain(&b).cloned().fold(0.0, f64::max) * 1e-3;
        let both = d.iter().zip(&b).filter(|(x, y)| **x > floor && **y > floor).count();
        let clash = d.iter().zip(&b).filter(|(x, y)| **x > floor && **y > floor && (db(**x) - db(**y)).abs() < 6.0).count();
        let frac = clash as f32 / d.len().max(1) as f32;
        mask_scores.push(100.0 - 2.0 * (100.0 * frac - 10.0).max(0.0));
        mask_notes.push(format!("kick and bass collide in {:.0}% of the song ({:.0}% share the low end)", 100.0 * frac, 100.0 * both as f32 / d.len().max(1) as f32));
        mask_fix = "duck the bass under the kick (Duck 6-10 dB) or move bass notes off the kick".into();
    }
    if let Some(v) = song.stem(Role::Vocals) {
        let vocal = presence(&mono(v));
        let rest: Vec<f32> = song.stems.iter().filter(|s| s.role != Role::Vocals).fold(vec![0.0f32; mid.len()], |mut acc, s| {
            for (a, (x, y)) in acc.iter_mut().zip(s.left.iter().zip(&s.right)) {
                *a += 0.5 * (x + y);
            }
            acc
        });
        let rest = presence(&rest);
        let floor = vocal.iter().cloned().fold(0.0, f64::max) * 1e-2;
        let mut ratios: Vec<f32> = vocal.iter().zip(&rest).filter(|(v, _)| **v > floor).map(|(v, r)| db(*v) - db(*r)).collect();
        ratios.sort_by(f32::total_cmp);
        if let Some(m) = ratios.get(ratios.len() / 2) {
            mask_scores.push(70.0 + 6.0 * m);
            mask_notes.push(format!("the vocal sits {m:+.1} dB over the rest at 1-4 kHz"));
            if *m < 3.0 {
                mask_fix = "carve 1-4 kHz out of pads and leads under the vocal, or lift the vocal".into();
            }
        }
    }
    if !mask_scores.is_empty() {
        push("masking", mask_scores.iter().sum::<f32>() / mask_scores.len() as f32, mask_notes.join("; "), &mask_fix);
    }

    // Stereo: the low end mono, the rest wide but not hollow.
    let lp = [Biquad::new(0, 150.0, 0.71, rate), Biquad::new(0, 150.0, 0.71, rate)];
    let hp = [Biquad::new(1, 150.0, 0.71, rate), Biquad::new(1, 150.0, 0.71, rate)];
    let low_ratio = db(energy(&side, &lp)) - db(energy(&mid, &lp));
    push("low_mono", 100.0 - 5.0 * (low_ratio + 20.0).max(0.0), format!("side {low_ratio:.0} dB under mid below 150 Hz"), "keep bass and kick mono: Width bass_mono 120-150 on the master, no spread on sub or bass");
    let width_ratio = db(energy(&side, &hp)) - db(energy(&mid, &hp));
    push("width", band(width_ratio, -14.0, -5.0, 5.0), format!("side {width_ratio:.0} dB against mid above 150 Hz"), if width_ratio < -14.0 { "widen pads, hats and returns (Width, pan, chorus)" } else { "too wide: narrow the leads and the vocal, centre the core" });

    // Groove: a grid the beat detector locks onto, at the written tempo,
    // and drums that breathe (velocity varies).
    let env = makepad_beat::onset::Envelopes::from_mono(&mid, rate as f64);
    let beat = makepad_beat::offline::analyze_envelopes(&env, mid.len() as f64 / rate as f64, makepad_beat::OfflineOptions::default());
    let drums: Vec<&Played> = mix.played.iter().filter(|p| song.roles.iter().any(|(n, r)| *r == Role::Drums && *n == p.loop_name)).collect();
    if !drums.is_empty() {
        let found = beat.grid.bpm;
        let ratio = [0.5, 1.0, 2.0].iter().map(|k| (found * k / mix.bpm - 1.0).abs()).fold(f64::MAX, f64::min) as f32;
        let conf = beat.grid.confidence;
        let mean = drums.iter().map(|p| p.vel).sum::<f32>() / drums.len() as f32;
        let sd = (drums.iter().map(|p| (p.vel - mean).powi(2)).sum::<f32>() / drums.len() as f32).sqrt();
        let offbeat = drums.iter().filter(|p| ((p.beat * 2.0).fract() - 0.5).abs() < 0.26 || (p.beat * 4.0).fract().abs() > 0.01).count() as f32 / drums.len() as f32;
        let s = 40.0 * (conf / 0.6).min(1.0) + 30.0 * (1.0 - ratio * 20.0).max(0.0) + 20.0 * (sd / 0.12).min(1.0) + 10.0 * (offbeat / 0.25).min(1.0);
        push("groove", s, format!("grid {found:.1} bpm (written {:.1}), confidence {conf:.2}; drum velocity spread {sd:.2}; {:.0}% of hits off the beat", mix.bpm, 100.0 * offbeat), if sd < 0.12 { "vary drum velocities: accents, ghost notes 0.2-0.35" } else { "a steadier, clearer drum grid (kick on the beat, hats with an accent pattern)" });
    }

    // Harmony: the notes against the song's key, clashes and low thirds.
    let pitched: Vec<&Played> = mix.played.iter().filter(|p| !p.hit && song.roles.iter().any(|(n, r)| *r != Role::Drums && *n == p.loop_name)).collect();
    if pitched.len() >= 8 {
        let (key_name, in_key, clash, low_thirds) = harmony(&pitched);
        let s = band(in_key, 0.9, 1.0, 300.0) - 400.0 * clash - 200.0 * low_thirds;
        push("harmony", s, format!("{key_name}: {:.0}% of the note time in key; {:.1}% semitone clashes; {:.1}% thirds below C3", 100.0 * in_key, 100.0 * clash, 100.0 * low_thirds), if low_thirds > 0.01 { "only roots and fifths below C3" } else if clash > 0.01 { "voice chords so no two held notes sit a semitone apart" } else { "keep to the key (borrowed chords are fine, stray notes are not)" });
    }

    // Melody: the lead's motifs repeat, with variation, in a singable range.
    if let Some((name, notes)) = lead_line(song) {
        let (rep, range, pcs) = melody(&notes);
        let s = 0.5 * band(rep, 0.35, 0.75, 200.0) + 0.3 * band(range as f32, 5.0, 19.0, 8.0) + 0.2 * band(pcs as f32, 4.0, 9.0, 15.0);
        push("melody", s, format!("lead `{name}`: {:.0}% of its 4-note shapes repeat, range {range} semitones, {pcs} pitch classes", 100.0 * rep), if rep < 0.35 { "a 1-2 bar motif repeated with variation: the lead wanders" } else if rep > 0.75 { "vary the hook every 4th bar: it repeats too exactly" } else { "keep the lead within a tenth or so" });
    }

    // The arrangement: loudness per section.
    let kl = loudness::k_weighted(l, rate as f64);
    let kr = loudness::k_weighted(r, rate as f64);
    let mut spans = Vec::new();
    for (i, (start, name)) in mix.sections.iter().enumerate() {
        let end = mix.sections.get(i + 1).map_or(mix.seconds(), |(b, _)| mix.seconds_at(*b));
        let a = mix.seconds_at(*start);
        if end - a >= 1.5 {
            spans.push((name.clone(), a, end, loudness::span_lufs(&kl, &kr, rate as f64, (a * rate as f64) as usize, (end * rate as f64) as usize) as f32));
        }
    }
    if spans.len() >= 2 {
        let peak = spans.iter().cloned().fold(None::<(String, f64, f64, f32)>, |m, s| if m.as_ref().is_none_or(|m| s.3 > m.3) { Some(s) } else { m }).unwrap();
        let low = spans.iter().map(|s| s.3).fold(f32::MAX, f32::min);
        let contrast = peak.3 - low;
        let late = (peak.1 / mix.seconds()) as f32;
        let first = spans[0].3;
        let last = spans[spans.len() - 1].3;
        let s = 0.5 * clamp(contrast / 6.0 * 100.0) + 0.2 * if late >= 0.2 { 100.0 } else { 30.0 } + 0.15 * clamp((peak.3 - first) / 3.0 * 100.0) + 0.15 * clamp((peak.3 - last) / 2.0 * 100.0);
        let curve: Vec<String> = spans.iter().map(|s| format!("{} {:.0}", s.0, s.3)).collect();
        push("energy", s, format!("{} (LUFS per section; contrast {contrast:.1} LU, peak in {})", curve.join(" · "), peak.0), if contrast < 6.0 { "more contrast: thin the intro and breakdowns (no kick, no sub), make the peak fuller" } else { "start smaller and end lower than the peak" });
        // Transitions: something changes at every boundary, a fill or a
        // level step leads into it.
        let bar = mix.beats_per_bar;
        let mut points = Vec::new();
        for w in mix.sections.windows(2) {
            let (b0, b1) = (w[0].0, w[1].0);
            let into = drums.iter().filter(|p| p.beat >= b1 - bar && p.beat < b1).count() as f32;
            let before = drums.iter().filter(|p| p.beat >= b0 && p.beat < b1 - bar).count() as f32 / (((b1 - b0) / bar - 1.0).max(1.0) as f32);
            let fill = into > before * 1.25 || (before > 0.0 && into < before * 0.5);
            let la = spans.iter().find(|s| s.0 == w[0].1).map(|s| s.3);
            let lb = spans.iter().find(|s| s.0 == w[1].1).map(|s| s.3);
            let step = matches!((la, lb), (Some(a), Some(b)) if (a - b).abs() >= 1.5);
            let changed = song.doc.sections.iter().find(|s| s.name == w[0].1).map(|s| &s.loops) != song.doc.sections.iter().find(|s| s.name == w[1].1).map(|s| &s.loops);
            points.push(0.5 * (fill as u8 as f32) + 0.25 * (step as u8 as f32) + 0.25 * (changed as u8 as f32));
        }
        if !points.is_empty() {
            let s = 100.0 * points.iter().sum::<f32>() / points.len() as f32;
            push("transitions", s, format!("{} boundaries, {:.0}% marked (a fill or drop-out into it, a level step, a change of parts)", points.len(), s), "lead into each section: a fill, a riser, a 1-beat drop-out, a crash on the downbeat");
        }
    }

    // The sung lead's pitch.
    if let Some((name, dry)) = &song.lead_vocal {
        let mut notes: Vec<&Played> = mix.played.iter().filter(|p| &p.loop_name == name && !p.hit).collect();
        notes.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        let targets: Vec<makepad_score_voice::tuning::TargetNote> = notes
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let end = (p.beat + p.dur as f64).min(notes.get(i + 1).map_or(f64::INFINITY, |n| n.beat));
                makepad_score_voice::tuning::TargetNote { start: mix.seconds_at(p.beat) as f32, end: mix.seconds_at(end) as f32, note: p.note }
            })
            .filter(|t| t.end - t.start >= 0.15)
            .collect();
        let reports = makepad_score_voice::tuning::report(dry, rate, &targets, 30.0);
        let mut cents: Vec<f32> = reports.iter().map(|r| r.centre.abs()).filter(|c| c.is_finite()).collect();
        cents.sort_by(f32::total_cmp);
        if let Some(med) = cents.get(cents.len() / 2) {
            let within = cents.iter().filter(|c| **c <= 25.0).count() as f32 / cents.len() as f32;
            push("vocal_pitch", 0.5 * (100.0 - 2.5 * (med - 8.0).max(0.0)) + 0.5 * 100.0 * within, format!("`{name}`: median {med:.0} cents off, {:.0}% of {} notes within 25 cents", 100.0 * within, cents.len()), "shorter notes and smaller leaps sing in tune; keep the line in the singer's range");
        }
    }
    Card { metrics }
}

const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
const NOTE: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// The key (by profile correlation), the share of note time in it (minor
/// counts its raised sixth and seventh), the share of overlapping time two
/// notes spend a semitone apart in the middle register, and the share in
/// thirds below C3.
fn harmony(notes: &[&Played]) -> (String, f32, f32, f32) {
    let mut hist = [0.0f32; 12];
    for p in notes {
        hist[(p.note.round() as i32).rem_euclid(12) as usize] += p.dur.max(0.05) * p.vel.max(0.1);
    }
    let corr = |profile: &[f32; 12], root: usize| {
        let mh = hist.iter().sum::<f32>() / 12.0;
        let mp = profile.iter().sum::<f32>() / 12.0;
        let (mut n, mut a, mut b) = (0.0, 0.0, 0.0);
        for i in 0..12 {
            let (x, y) = (hist[(i + root) % 12] - mh, profile[i] - mp);
            n += x * y;
            a += x * x;
            b += y * y;
        }
        n / (a * b).sqrt().max(1e-9)
    };
    let mut best = (f32::MIN, 0, false);
    for root in 0..12 {
        for minor in [false, true] {
            let c = corr(if minor { &MINOR } else { &MAJOR }, root);
            if c > best.0 {
                best = (c, root, minor);
            }
        }
    }
    let (_, root, minor) = best;
    let scale: &[usize] = if minor { &[0, 2, 3, 5, 7, 8, 9, 10, 11] } else { &[0, 2, 4, 5, 7, 9, 11] };
    let total = hist.iter().sum::<f32>().max(1e-9);
    let in_key = scale.iter().map(|d| hist[(root + d) % 12]).sum::<f32>() / total;
    // Overlaps, by a sweep over notes sorted by start.
    let mut sorted: Vec<&&Played> = notes.iter().collect();
    sorted.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    let (mut clash, mut thirds, mut time) = (0.0f64, 0.0f64, 0.0f64);
    for (i, a) in sorted.iter().enumerate() {
        let a_end = a.beat + a.dur as f64;
        time += a.dur as f64;
        for b in sorted[i + 1..].iter().take_while(|b| b.beat < a_end) {
            if b.loop_name == a.loop_name && (b.beat - a.beat).abs() > 1e-6 {
                continue;
            }
            let overlap = (a_end.min(b.beat + b.dur as f64) - b.beat).max(0.0);
            if overlap < 0.25 {
                continue;
            }
            let iv = (a.note - b.note).abs().round() as i32;
            let low = a.note.min(b.note);
            if iv == 1 && low >= 48.0 && low < 84.0 {
                clash += overlap;
            }
            if (iv == 3 || iv == 4) && a.note.max(b.note) < 48.0 {
                thirds += overlap;
            }
        }
    }
    let name = format!("{} {}", NOTE[root], if minor { "minor" } else { "major" });
    (name, in_key, (clash / time.max(1e-9)) as f32, (thirds / time.max(1e-9)) as f32)
}

/// The lead: the sung lead, else the highest mostly-monophonic part with
/// enough notes.
fn lead_line(song: &Song) -> Option<(String, Vec<f32>)> {
    let notes_of = |name: &str| -> Vec<&Played> {
        let mut v: Vec<&Played> = song.mix.played.iter().filter(|p| p.loop_name == name && !p.hit).collect();
        v.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        v
    };
    let pick = |name: &str| {
        let v = notes_of(name);
        // One note per onset (the top of a chord).
        let mut line: Vec<(f64, f32)> = Vec::new();
        for p in v {
            match line.last_mut() {
                Some(last) if (last.0 - p.beat).abs() < 1e-6 => last.1 = last.1.max(p.note),
                _ => line.push((p.beat, p.note)),
            }
        }
        line.into_iter().map(|(_, n)| n).collect::<Vec<f32>>()
    };
    if let Some((name, _)) = &song.lead_vocal {
        return Some((name.clone(), pick(name)));
    }
    song.roles
        .iter()
        .filter(|(_, r)| *r == Role::Other || *r == Role::Vocals)
        .filter_map(|(name, _)| {
            let all = notes_of(name);
            let onsets = pick(name);
            let poly = all.len() as f32 / onsets.len().max(1) as f32;
            (onsets.len() >= 16 && poly <= 1.3).then(|| (name.clone(), onsets.iter().sum::<f32>() / onsets.len() as f32, onsets))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(name, _, notes)| (name, notes))
}

/// Repetition (the share of 4-note interval shapes heard before), the
/// range in semitones and the number of pitch classes.
fn melody(notes: &[f32]) -> (f32, i32, usize) {
    let iv: Vec<i32> = notes.windows(2).map(|w| (w[1] - w[0]).round() as i32).collect();
    let grams: Vec<&[i32]> = iv.windows(3).collect();
    let mut seen = std::collections::HashSet::new();
    let repeated = grams.iter().filter(|g| !seen.insert(**g)).count();
    let rep = repeated as f32 / grams.len().max(1) as f32;
    let lo = notes.iter().cloned().fold(f32::MAX, f32::min);
    let hi = notes.iter().cloned().fold(f32::MIN, f32::max);
    let pcs: std::collections::HashSet<i32> = notes.iter().map(|n| (n.round() as i32).rem_euclid(12)).collect();
    (rep, (hi - lo).round() as i32, pcs.len())
}
