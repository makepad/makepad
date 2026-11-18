// Side by side: one MIDI test set through our modelled piano and through an
// installed Audio Unit instrument (macOS), at the same rate, level-matched,
// written as paired WAVs and measured with the probe's own metrics.
//
//   au_compare --list                         installed AU instruments
//   au_compare --au <name|type/sub/manu> --presets | --params
//   au_compare --au <name> --out <dir> [--au-preset <name>] [--au-param <id|name>=<value>]...
//              [--ours <preset>] [--dry] [--rate 48000] [--sets notes,scale,chords,pedal,probes]
//   au_compare --au <name> --out <dir> --script <file> [--label <name>]
//
// The test set: every key A0..C8 alone at velocities 28, 68 and 112 (the
// Salamander fixture's), two scales, two held chords and a pedalled passage.
// Single notes start from a cleared instrument (our piano is built fresh,
// the plug-in is reset), held 3.6 s and released, 4.0 s each, so every
// decay window below lies inside the hold.
//
// Outputs in --out: `<case>.A.wav` (ours), `<case>.B.wav` (the plug-in) and
// `<case>.AB.wav` (each phrase ours then the plug-in), 32-bit float (`--pcm16`: 16-bit, for listening only), one gain per
// instrument so the median key at velocity 68 is equally loud in both; and
// `report.txt`, the numbers also printed here:
//   ref        the fixture's eleven metrics, each instrument minus the recording
//   roughness  key-to-key roughness, the probe's scan definition, A0..C6
//   registers  level, centroid, early/late band decay per register
//   partials   measured f0, inharmonicity B, per-partial level and two-stage
//              decay, envelope ripple, off-line share, from the audio alone
//   probes     (--sets probes, not in the default set) dampers, una corda,
//              velocity response, re-strikes, sustain-pedal resonance, bass
//              components off the partial series, stereo image; each
//              instrument against itself, pairs at raw level
//   cpu        seconds of CPU per second of audio, pedalled passage
// A plug-in render that comes back silent (some trial builds mute keys) is
// reported and left out of every metric.
//
// --script: render a text script through both instruments, write the pair
// and print a band summary. One event per line, times in seconds:
//   <t> on <key> <velocity> | <t> off <key> | <t> pedal <0..1> | <t> soft <0|1>
//   | <t> sostenuto <0|1> | end <t>
// `analyze <key> <t0>` lines add a partials analysis of the note starting at t0.
use makepad_audio_unit_host::{self as au, ComponentDescription, MidiMessage};
use makepad_piano_model::{Piano, PianoEvent, TimedEvent, PIANO_PRESETS};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const FIXTURE: &str = include_str!("../tests/data/salamander_v3.tsv");
const BANDS: [(f64, f64); 7] =
    [(20.0, 80.0), (80.0, 200.0), (200.0, 500.0), (500.0, 1500.0), (1500.0, 4000.0), (4000.0, 10000.0), (10000.0, 20000.0)];
const VELOCITIES: [u8; 3] = [28, 68, 112];
const NOTE_SECS: f64 = 4.0;
const HOLD_SECS: f64 = 3.6;
const BLOCK: usize = 64;

#[derive(Clone, Copy, Debug)]
enum Ev {
    On(u8, u8),
    Off(u8),
    Pedal(f32),
    Soft(bool),
    Sostenuto(bool),
}

type Script = Vec<(f64, Ev)>;

struct Args(Vec<String>);

impl Args {
    fn flag(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }
    fn value(&self, name: &str) -> Option<&str> {
        self.0.iter().position(|a| a == name).and_then(|i| self.0.get(i + 1)).map(|s| s.as_str())
    }
    fn values(&self, name: &str) -> Vec<&str> {
        self.0.windows(2).filter(|w| w[0] == name).map(|w| w[1].as_str()).collect()
    }
}

fn name(key: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[key as usize % 12], key as i32 / 12 - 1)
}

// ───────────────────────── the two instruments ─────────────────────────

trait Player {
    fn label(&self) -> String;
    /// Back to silence and rest, pedals up.
    fn clear(&mut self);
    fn send(&mut self, ev: Ev, offset: u32);
    fn render(&mut self, l: &mut [f32], r: &mut [f32]);
}

struct Ours {
    preset: &'static makepad_piano_model::PianoPreset,
    rate: f32,
    dry: bool,
    piano: Piano,
    events: Vec<TimedEvent>,
}

impl Ours {
    fn build(preset: &'static makepad_piano_model::PianoPreset, rate: f32, dry: bool) -> Piano {
        let mut p = Piano::new_with_preset(rate, preset);
        if dry {
            p.set_reverb_mix(0.0);
            p.set_early_reflection_level(0.0);
            p.set_soft_clip(false);
        }
        p
    }
}

impl Player for Ours {
    fn label(&self) -> String {
        format!("ours: {}{}", self.preset.name, if self.dry { " (dry)" } else { "" })
    }
    fn clear(&mut self) {
        // Built fresh, as the probe does for every note.
        self.piano = Ours::build(self.preset, self.rate, self.dry);
        self.events.clear();
    }
    fn send(&mut self, ev: Ev, offset: u32) {
        let event = match ev {
            Ev::On(key, velocity) => PianoEvent::NoteOn { key, velocity },
            Ev::Off(key) => PianoEvent::NoteOff { key },
            Ev::Pedal(value) => PianoEvent::Sustain { value },
            Ev::Soft(on) => PianoEvent::SoftPedal { on },
            Ev::Sostenuto(on) => PianoEvent::Sostenuto { on },
        };
        self.events.push(TimedEvent { offset, event });
    }
    fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        l.fill(0.0);
        r.fill(0.0);
        self.piano.process(&self.events, l, r);
        self.events.clear();
    }
}

struct Plugin {
    unit: au::Instrument,
    preset: Option<String>,
    params: Vec<(u32, f32)>,
}

impl Plugin {
    fn configure(&mut self) {
        for &(id, value) in &self.params {
            if let Err(e) = self.unit.set_parameter(id, value) {
                eprintln!("parameter {id}: {e}");
            }
        }
    }
}

impl Player for Plugin {
    fn label(&self) -> String {
        let mut s = format!("plug-in: {}", self.unit.info().name);
        if let Some(p) = &self.preset {
            s += &format!(", preset {p}");
        }
        s
    }
    fn clear(&mut self) {
        for ev in [MidiMessage::control(0, 64, 0), MidiMessage::control(0, 66, 0), MidiMessage::control(0, 67, 0), MidiMessage::control(0, 120, 0), MidiMessage::all_notes_off(0)] {
            let _ = self.unit.midi(ev, 0);
        }
        let _ = self.unit.reset();
        // Let anything still settling inside the plug-in die away unheard.
        let n = (self.unit.sample_rate() * 0.25) as usize;
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        let _ = self.unit.render(&mut l, &mut r);
    }
    fn send(&mut self, ev: Ev, offset: u32) {
        let message = match ev {
            Ev::On(key, velocity) => MidiMessage::note_on(0, key, velocity.max(1)),
            Ev::Off(key) => MidiMessage::note_off(0, key),
            Ev::Pedal(value) => MidiMessage::control(0, 64, (value.clamp(0.0, 1.0) * 127.0).round() as u8),
            Ev::Soft(on) => MidiMessage::control(0, 67, if on { 127 } else { 0 }),
            Ev::Sostenuto(on) => MidiMessage::control(0, 66, if on { 127 } else { 0 }),
        };
        if let Err(e) = self.unit.midi(message, offset) {
            eprintln!("midi: {e}");
        }
    }
    fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        if let Err(e) = self.unit.render(l, r) {
            eprintln!("render: {e}");
            l.fill(0.0);
            r.fill(0.0);
        }
    }
}

/// Render a script in `BLOCK`-frame blocks with sample-accurate events,
/// dropping the plug-in's reported latency from the front.
fn render(p: &mut dyn Player, script: &Script, secs: f64, rate: f64, latency: usize) -> (Vec<f32>, Vec<f32>) {
    let total = (secs * rate) as usize + latency;
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let mut evs: Vec<(usize, Ev)> = script.iter().map(|&(t, e)| (((t * rate).round() as usize).min(total.saturating_sub(1)), e)).collect();
    evs.sort_by_key(|e| e.0);
    let mut next = 0;
    let mut pos = 0;
    while pos < total {
        let n = BLOCK.min(total - pos);
        while next < evs.len() && evs[next].0 < pos + n {
            p.send(evs[next].1, (evs[next].0 - pos) as u32);
            next += 1;
        }
        let (bl, br) = (&mut l[pos..pos + n], &mut r[pos..pos + n]);
        p.render(bl, br);
        pos += n;
    }
    l.drain(..latency);
    r.drain(..latency);
    (l, r)
}

// ───────────────────── the probe's metric definitions ─────────────────────

fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 0..n {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut m = n >> 1;
        while m >= 1 && j & m != 0 {
            j ^= m;
            m >>= 1;
        }
        j |= m;
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f64::consts::TAU / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (ar, ai) = (re[i + k], im[i + k]);
                let (br, bi) = (re[i + k + len / 2], im[i + k + len / 2]);
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                re[i + k] = ar + tr;
                im[i + k] = ai + ti;
                re[i + k + len / 2] = ar - tr;
                im[i + k + len / 2] = ai - ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// Mean of the two channels' one-sided power spectra (periodic Hann), as
/// probe.rs, tools/acoustic.py and tests/acoustic_reference.rs define it.
fn spectrum(l: &[f32], r: &[f32], rate: f64) -> (f64, Vec<f64>) {
    let n = l.len().max(2).next_power_of_two();
    let w: Vec<f64> = (0..l.len()).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / l.len() as f64).cos()).collect();
    let norm = n as f64 * w.iter().map(|x| x * x).sum::<f64>().max(1e-30);
    let mut power = vec![0.0; n / 2 + 1];
    for ch in [l, r] {
        let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
        for (i, &v) in ch.iter().enumerate() {
            re[i] = v as f64 * w[i];
        }
        fft(&mut re, &mut im);
        for k in 0..=n / 2 {
            let one = if k == 0 || k == n / 2 { 1.0 } else { 2.0 };
            power[k] += 0.5 * one * (re[k] * re[k] + im[k] * im[k]) / norm;
        }
    }
    (rate / n as f64, power)
}

fn band(s: &(f64, Vec<f64>), lo: f64, hi: f64) -> f64 {
    let a = ((lo / s.0).ceil().max(0.0)) as usize;
    let b = ((hi / s.0).ceil() as usize).min(s.1.len());
    if a >= b {
        0.0
    } else {
        s.1[a..b].iter().sum()
    }
}

fn db(x: f64) -> f64 {
    10.0 * x.max(1e-30).log10()
}

fn energy(l: &[f32], r: &[f32]) -> f64 {
    l.iter().zip(r).map(|(&a, &b)| 0.5 * ((a as f64).powi(2) + (b as f64).powi(2))).sum()
}

fn onset(l: &[f32], r: &[f32]) -> usize {
    let frame = 48;
    let end = l.len().min(24000);
    let p: Vec<f64> = (0..end / frame).map(|i| energy(&l[i * frame..(i + 1) * frame], &r[i * frame..(i + 1) * frame])).collect();
    let peak = p.iter().copied().fold(0.0, f64::max);
    p.iter().position(|&x| x > peak * 1e-4).unwrap_or(0) * frame
}

fn sect(x: &[f32], a: f64, b: f64, rate: f64) -> &[f32] {
    &x[((a * rate).round() as usize).min(x.len())..((b * rate).round() as usize).min(x.len())]
}

fn slope(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    let (sx, sy) = (x.iter().sum::<f64>(), y.iter().sum::<f64>());
    let sxx: f64 = x.iter().map(|v| v * v).sum();
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| a * b).sum();
    (n * sxy - sx * sy) / (n * sxx - sx * sx)
}

/// The fixture's eleven metrics (index 6 is the raw 0-2 s RMS; the caller
/// turns it into dB re C4). Same definition as probe.rs.
fn fixture_metrics(l: &[f32], r: &[f32], key: u8, rate: f64) -> [f64; 11] {
    let o = onset(l, r);
    let (l, r) = (&l[o..], &r[o..]);
    let spec = |a, b| spectrum(sect(l, a, b, rate), sect(r, a, b, rate), rate);
    let mut m = [0.0; 11];
    for (i, (a, b)) in [(0.05, 0.1), (1.0, 2.0)].into_iter().enumerate() {
        let s = spec(a, b);
        let total = band(&s, 20.0, 20000.0);
        m[2 * i] = db(band(&s, 500.0, 2000.0) / total);
        m[2 * i + 1] = db(band(&s, 2000.0, 8000.0) / total);
    }
    let f0 = 440.0 * 2.0f64.powf((key as f64 - 69.0) / 12.0);
    for (i, (a, b)) in [(0.05, 0.3), (1.0, 2.0)].into_iter().enumerate() {
        let s = spec(a, b);
        let p: Vec<f64> = (1..=6).map(|n| band(&s, (n as f64 - 0.4) * f0, (n as f64 + 0.4) * f0)).collect();
        m[4 + i] = db(p[0] / p[1..].iter().copied().fold(0.0, f64::max));
    }
    let ms = energy(sect(l, 0.0, 2.0, rate), sect(r, 0.0, 2.0, rate)) / (2.0 * rate);
    m[6] = ms.sqrt();
    m[7] = energy(sect(l, 0.0, 0.005, rate), sect(r, 0.0, 0.005, rate)) / energy(sect(l, 0.0, 0.05, rate), sect(r, 0.0, 0.05, rate)).max(1e-30);
    let times: Vec<f64> = (0..17).map(|i| 0.1 + i as f64 * 0.05).collect();
    let spectra: Vec<_> = times.iter().map(|&t| spec(t, t + 0.1)).collect();
    for (i, (lo, hi)) in [(20.0, 500.0), (500.0, 2000.0), (2000.0, 8000.0)].into_iter().enumerate() {
        let p: Vec<f64> = spectra.iter().map(|s| db(band(s, lo, hi).max(ms * 1e-15))).collect();
        m[8 + i] = -slope(&times, &p);
    }
    m
}

/// Bands, total and centroid over a window (the probe's `band_line`).
fn band_line(l: &[f32], r: &[f32], a: f64, b: f64, rate: f64) -> (Vec<f64>, f64, f64) {
    let s = spectrum(sect(l, a, b, rate), sect(r, a, b, rate), rate);
    let total = band(&s, 20.0, 20000.0);
    let cols = BANDS.iter().map(|&(lo, hi)| db(band(&s, lo, hi))).collect();
    let (mut num, mut den) = (0.0, 0.0);
    for (k, &p) in s.1.iter().enumerate() {
        let f = k as f64 * s.0;
        if (20.0..20000.0).contains(&f) {
            num += f * p;
            den += p;
        }
    }
    (cols, db(total), num / den.max(1e-30))
}

fn peak(l: &[f32], r: &[f32]) -> f32 {
    l.iter().chain(r).fold(0.0f32, |m, v| m.max(v.abs()))
}

// ─────────────────── partials measured from the audio ───────────────────

/// The strongest bin's frequency in [lo, hi), refined by a parabola through
/// its neighbours in dB.
fn peak_in(s: &(f64, Vec<f64>), lo: f64, hi: f64) -> Option<(f64, f64)> {
    let a = ((lo / s.0).ceil().max(1.0)) as usize;
    let b = ((hi / s.0).ceil() as usize).min(s.1.len() - 1);
    if a >= b {
        return None;
    }
    let k = (a..b).max_by(|&i, &j| s.1[i].total_cmp(&s.1[j]))?;
    let (y0, y1, y2) = (db(s.1[k - 1]), db(s.1[k]), db(s.1[k + 1]));
    let d = 0.5 * (y0 - y2) / (y0 - 2.0 * y1 + y2).min(-1e-12);
    Some(((k as f64 + d.clamp(-0.5, 0.5)) * s.0, s.1[k - 1] + s.1[k] + s.1[k + 1]))
}

struct Partials {
    f0: f64,
    b: f64,
    /// (n, Hz, level dB 0.1-0.6 s, level dB 1-2 s, early dB/s, late dB/s, ripple dB)
    rows: Vec<(usize, f64, f64, f64, f64, f64, f64)>,
    off_line_db: f64,
}

/// f0 and B fitted to the measured partial peaks (f_n^2 / n^2 = f0^2 + f0^2 B n^2),
/// then each partial's level, two-stage decay and envelope ripple (the
/// RMS residual of its 0.1-2 s dB envelope about a two-segment fit: beating
/// and swells show up here).
fn measure_partials(l: &[f32], r: &[f32], key: u8, rate: f64) -> Partials {
    let o = onset(l, r);
    let (l, r) = (&l[o..], &r[o..]);
    let nominal = 440.0 * 2.0f64.powf((key as f64 - 69.0) / 12.0);
    let s = spectrum(sect(l, 0.05, 1.05, rate), sect(r, 0.05, 1.05, rate), rate);
    // Coarse search first: the f0 (within 30 cents) and B whose partial
    // series holds the most energy. Starting the regression from B = 0 locks
    // onto the wrong peaks in the bass, where partial 24 sits 5-10 % sharp.
    let (mut f0, mut b, mut best) = (nominal, 0.0f64, 0.0f64);
    for ci in -12..=12 {
        let fc = nominal * 2.0f64.powf(ci as f64 * 2.5 / 1200.0);
        let half = (0.1 * fc).min(10.0);
        for bi in 0..=40 {
            let bc = if bi == 0 { 0.0 } else { 1e-6 * 10f64.powf(bi as f64 * 3.5 / 40.0) };
            let mut e = 0.0;
            for n in 1..=40 {
                let nf = n as f64;
                let f = nf * fc * (1.0 + bc * nf * nf).sqrt();
                if f > 12000.0 {
                    break;
                }
                e += band(&s, f - half, f + half);
            }
            if e > best {
                (best, f0, b) = (e, fc, bc);
            }
        }
    }
    for _ in 0..3 {
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        for n in 1..=24usize {
            let nf = n as f64;
            let guess = nf * f0 * (1.0 + b * nf * nf).sqrt();
            if guess > 0.45 * rate || guess > 12000.0 {
                break;
            }
            let half = (0.25 * f0).min(25.0);
            if let Some((f, p)) = peak_in(&s, guess - half, guess + half) {
                if db(p) > db(band(&s, 20.0, 20000.0)) - 70.0 {
                    xs.push(nf * nf);
                    ys.push((f / nf).powi(2));
                }
            }
        }
        if xs.len() < 3 {
            break;
        }
        let k = slope(&xs, &ys);
        let c = ys.iter().sum::<f64>() / ys.len() as f64 - k * xs.iter().sum::<f64>() / xs.len() as f64;
        if c > 0.0 {
            f0 = c.sqrt();
            b = (k / c).max(0.0);
        }
    }
    let times: Vec<f64> = (0..70).map(|i| 0.05 + 0.05 * i as f64).filter(|&t| t + 0.2 <= l.len() as f64 / rate).collect();
    let spectra: Vec<_> = times.iter().map(|&t| spectrum(sect(l, t, t + 0.2, rate), sect(r, t, t + 0.2, rate), rate)).collect();
    let lvl = spectrum(sect(l, 0.1, 0.6, rate), sect(r, 0.1, 0.6, rate), rate);
    let late = spectrum(sect(l, 1.0, 2.0, rate), sect(r, 1.0, 2.0, rate), rate);
    let half = (0.3 * f0).min(12.0);
    let mut rows = Vec::new();
    for n in [1usize, 2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 20, 24, 32] {
        let nf = n as f64;
        let f = nf * f0 * (1.0 + b * nf * nf).sqrt();
        if f > 12000.0 {
            break;
        }
        let series: Vec<f64> = spectra.iter().map(|sp| db(band(sp, f - half, f + half))).collect();
        let pick = |a: f64, z: f64| -> (Vec<f64>, Vec<f64>) {
            times.iter().zip(&series).filter(|(&t, _)| t >= a && t <= z).map(|(&t, &y)| (t, y)).unzip()
        };
        let (te, ye) = pick(0.1, 1.0);
        // Late windows end before the note-off at HOLD_SECS.
        let (tl, yl) = pick(1.5, HOLD_SECS - 0.25);
        let (early, late_slope) = (if te.len() > 2 { -slope(&te, &ye) } else { 0.0 }, if tl.len() > 2 { -slope(&tl, &yl) } else { 0.0 });
        let (tr, yr) = pick(0.1, 2.0);
        let ripple = if tr.len() > 4 {
            let mid = tr.len() / 2;
            let fit = |t: &[f64], y: &[f64]| -> f64 {
                let s = slope(t, y);
                let c = y.iter().sum::<f64>() / y.len() as f64 - s * t.iter().sum::<f64>() / t.len() as f64;
                t.iter().zip(y).map(|(&x, &v)| (v - (c + s * x)).powi(2)).sum::<f64>()
            };
            ((fit(&tr[..mid], &yr[..mid]) + fit(&tr[mid..], &yr[mid..])) / tr.len() as f64).sqrt()
        } else {
            0.0
        };
        rows.push((n, f, db(band(&lvl, f - half, f + half)), db(band(&late, f - half, f + half)), early, late_slope, ripple));
    }
    let s = spectrum(sect(l, 0.3, 1.3, rate), sect(r, 0.3, 1.3, rate), rate);
    let total = band(&s, 20.0, 12000.0);
    let mut lines = 0.0;
    for n in 1..=200usize {
        let nf = n as f64;
        let f = nf * f0 * (1.0 + b * nf * nf).sqrt();
        if f >= 12000.0 {
            break;
        }
        lines += band(&s, f - half, f + half);
    }
    Partials { f0, b, rows, off_line_db: db(((total - lines) / total.max(1e-30)).max(1e-12)) }
}

fn partials_text(label: &str, p: &Partials, key: u8) -> String {
    let nominal = 440.0 * 2.0f64.powf((key as f64 - 69.0) / 12.0);
    let mut s = format!(
        "  {label}: f0 {:.2} Hz ({:+.1} cents) B {:.2e} off-line {:.1} dB\n   ",
        p.f0,
        1200.0 * (p.f0 / nominal).log2(),
        p.b,
        p.off_line_db
    );
    for &(n, f, a, b, e, lt, rp) in &p.rows {
        let _ = write!(s, " {n}:{f:.0}Hz {a:.1}>{b:.1} {e:.1}/{lt:.1} r{rp:.1}");
    }
    s.push('\n');
    s
}

// ───────────────────────────── behaviour probes ─────────────────────────────

/// Mean-square level in dB over [a, b) seconds.
fn level_db(x: &(Vec<f32>, Vec<f32>), a: f64, b: f64, rate: f64) -> f64 {
    let (l, r) = (sect(&x.0, a, b, rate), sect(&x.1, a, b, rate));
    db(energy(l, r) / l.len().max(1) as f64)
}

/// Interchannel correlation and left-minus-right level over a window.
fn stereo(x: &(Vec<f32>, Vec<f32>), a: f64, b: f64, rate: f64) -> (f64, f64) {
    let (l, r) = (sect(&x.0, a, b, rate), sect(&x.1, a, b, rate));
    let (mut lr, mut ll, mut rr) = (0.0, 0.0, 0.0);
    for (&p, &q) in l.iter().zip(r) {
        let (p, q) = (p as f64, q as f64);
        lr += p * q;
        ll += p * p;
        rr += q * q;
    }
    (lr / (ll * rr).sqrt().max(1e-30), db(ll / rr.max(1e-30)))
}

/// Behaviour probes, each rendered through both instruments with the same
/// script: dampers, una corda, velocity response, re-strikes, sustain-pedal
/// resonance, bass components off the partial series, and the stereo image.
/// Every number compares an instrument with itself, so no level match is
/// needed; the pairs are written at their raw levels.
fn probes(ours: &mut Ours, plugin: &mut Plugin, rate: f64, latency: usize, out: &Path, report: &mut String) {
    let mut both = |script: &Script, secs: f64| -> [(Vec<f32>, Vec<f32>); 2] {
        ours.clear();
        plugin.clear();
        plugin.configure();
        [render(ours, script, secs, rate, 0), render(plugin, script, secs, rate, latency)]
    };
    let mut pairs = Vec::new();
    let side = ["A", "B"];

    let _ = writeln!(report, "\n== probe dampers v80, key let go at 0.5 s: dB re the held note at +0.1 +0.25 +0.5 +1.0 s | dB/s over the first 0.25 s");
    for key in [36u8, 48, 60, 72, 81] {
        let held = both(&vec![(0.0, Ev::On(key, 80)), (3.0, Ev::Off(key))], 2.0);
        let [a, b] = both(&vec![(0.0, Ev::On(key, 80)), (0.5, Ev::Off(key))], 2.0);
        for (s, x) in [&a, &b].into_iter().enumerate() {
            let rel: Vec<String> = [0.1, 0.25, 0.5, 1.0]
                .iter()
                .map(|&t| format!("{:+6.1}", level_db(x, 0.475 + t, 0.525 + t, rate) - level_db(&held[s], 0.475 + t, 0.525 + t, rate)))
                .collect();
            let times: Vec<f64> = (0..12).map(|i| 0.5 + 0.02 * i as f64).collect();
            let lv: Vec<f64> = times.iter().map(|&t| level_db(x, t, t + 0.02, rate)).collect();
            let _ = writeln!(report, "{:>4} {}: {} | {:6.1} dB/s", name(key), side[s], rel.join(" "), -slope(&times, &lv));
        }
        pairs.push(Pair { case: format!("probe_damper_{}", name(key)), a, b });
    }

    let _ = writeln!(report, "\n== probe una corda v80, soft minus normal: level 0.05-0.5 s dB | centroid 0.02-0.5 s Hz | partials 1-6 dB | early decay p1-4 dB/s normal>soft");
    for key in [36u8, 60, 72] {
        let normal = both(&vec![(0.0, Ev::On(key, 80)), (2.5, Ev::Off(key))], 2.6);
        let [a, b] = both(&vec![(0.0, Ev::Soft(true)), (0.0, Ev::On(key, 80)), (2.5, Ev::Off(key))], 2.6);
        for (s, x) in [&a, &b].into_iter().enumerate() {
            let n = &normal[s];
            let dl = level_db(x, 0.05, 0.5, rate) - level_db(n, 0.05, 0.5, rate);
            let dc = band_line(&x.0, &x.1, 0.02, 0.5, rate).2 - band_line(&n.0, &n.1, 0.02, 0.5, rate).2;
            let (pn, ps) = (measure_partials(&n.0, &n.1, key, rate), measure_partials(&x.0, &x.1, key, rate));
            let dp: Vec<String> = pn.rows.iter().zip(&ps.rows).take(6).map(|(p, q)| format!("{:+5.1}", q.2 - p.2)).collect();
            let dd: Vec<String> = pn.rows.iter().zip(&ps.rows).take(4).map(|(p, q)| format!("{:.1}>{:.1}", p.4, q.4)).collect();
            let _ = writeln!(report, "{:>4} {}: {dl:+5.1} | {dc:+6.0} | {} | {}", name(key), side[s], dp.join(" "), dd.join(" "));
        }
        pairs.push(Pair { case: format!("probe_soft_{}", name(key)), a, b });
    }

    const VELS: [u8; 10] = [8, 16, 24, 32, 48, 64, 80, 96, 112, 127];
    let _ = writeln!(report, "\n== probe velocity {VELS:?}: level 0-0.5 s dB re v64 | centroid 0.02-0.3 s Hz");
    for key in [36u8, 60, 81] {
        let renders: Vec<[(Vec<f32>, Vec<f32>); 2]> = VELS.iter().map(|&v| both(&vec![(0.0, Ev::On(key, v)), (1.0, Ev::Off(key))], 1.2)).collect();
        for s in 0..2 {
            let reference = level_db(&renders[5][s], 0.0, 0.5, rate);
            let lv: Vec<String> = renders.iter().map(|x| format!("{:+5.1}", level_db(&x[s], 0.0, 0.5, rate) - reference)).collect();
            let cen: Vec<String> = renders.iter().map(|x| format!("{:5.0}", band_line(&x[s].0, &x[s].1, 0.02, 0.3, rate).2)).collect();
            let _ = writeln!(report, "{:>4} {}: {} | {}", name(key), side[s], lv.join(" "), cen.join(" "));
        }
    }

    let _ = writeln!(report, "\n== probe re-strike v80 under the pedal (strikes at 0 and 1.5 s): first 0-0.3 s dB | ring 1.4-1.5 s re first | second 1.5-1.8 s re first | centroids first, second Hz");
    for key in [48u8, 72] {
        let script = vec![(0.0, Ev::Pedal(1.0)), (0.0, Ev::On(key, 80)), (0.3, Ev::Off(key)), (1.5, Ev::On(key, 80)), (1.8, Ev::Off(key)), (3.2, Ev::Pedal(0.0))];
        let [a, b] = both(&script, 3.5);
        for (s, x) in [&a, &b].into_iter().enumerate() {
            let first = level_db(x, 0.0, 0.3, rate);
            let (c1, c2) = (band_line(&x.0, &x.1, 0.02, 0.3, rate).2, band_line(&x.0, &x.1, 1.52, 1.8, rate).2);
            let _ = writeln!(
                report,
                "{:>4} {}: {first:6.1} | {:+5.1} | {:+5.1} | {c1:5.0} {c2:5.0}",
                name(key),
                side[s],
                level_db(x, 1.4, 1.5, rate) - first,
                level_db(x, 1.5, 1.8, rate) - first
            );
        }
        pairs.push(Pair { case: format!("probe_restrike_{}", name(key)), a, b });
    }

    let _ = writeln!(report, "\n== probe sustain-pedal resonance v90: pedal down, key let go after 0.2 s, minus the key held without pedal: dB at 0.02-0.3 s, 1-2 s, 2-3 s | off-line share held, pedal dB");
    for key in [36u8, 48, 60, 72] {
        let held = both(&vec![(0.0, Ev::On(key, 90)), (3.4, Ev::Off(key))], 3.5);
        let t0 = 0.3;
        let [a, b] = both(&vec![(0.0, Ev::Pedal(1.0)), (t0, Ev::On(key, 90)), (t0 + 0.2, Ev::Off(key))], t0 + 3.2);
        for (s, x) in [&a, &b].into_iter().enumerate() {
            let h = &held[s];
            let d: Vec<String> = [(0.02, 0.3), (1.0, 2.0), (2.0, 3.0)]
                .iter()
                .map(|&(p, q)| format!("{:+5.1}", level_db(x, t0 + p, t0 + q, rate) - level_db(h, p, q, rate)))
                .collect();
            let from = (t0 * rate) as usize;
            let (oh, op) = (measure_partials(&h.0, &h.1, key, rate).off_line_db, measure_partials(&x.0[from..], &x.1[from..], key, rate).off_line_db);
            let _ = writeln!(report, "{:>4} {}: {} | {oh:6.1} {op:6.1}", name(key), side[s], d.join(" "));
        }
        pairs.push(Pair { case: format!("probe_pedal_{}", name(key)), a, b });
    }

    let _ = writeln!(report, "\n== probe bass off the partial series, v28 > v112: off-line share dB | fitted B | partials within 30 dB of the strongest (0.1-0.6 s)");
    for key in [21u8, 24, 29, 36] {
        let renders: Vec<[(Vec<f32>, Vec<f32>); 2]> = [28u8, 112].iter().map(|&v| both(&vec![(0.0, Ev::On(key, v)), (2.2, Ev::Off(key))], 2.3)).collect();
        for s in 0..2 {
            let m: Vec<Partials> = renders.iter().map(|x| measure_partials(&x[s].0, &x[s].1, key, rate)).collect();
            let count = |p: &Partials| {
                let top = p.rows.iter().map(|r| r.2).fold(f64::MIN, f64::max);
                p.rows.iter().filter(|r| r.2 > top - 30.0).count()
            };
            let _ = writeln!(
                report,
                "{:>4} {}: {:6.1} > {:6.1} | {:.2e} > {:.2e} | {} > {} of {}",
                name(key),
                side[s],
                m[0].off_line_db,
                m[1].off_line_db,
                m[0].b,
                m[1].b,
                count(&m[0]),
                count(&m[1]),
                m[1].rows.len()
            );
        }
    }

    let _ = writeln!(report, "\n== probe stereo v68: interchannel correlation and L-R dB, 0-0.05 s and 0.1-1.0 s");
    for key in [21u8, 33, 45, 57, 69, 81, 93, 105] {
        let x = both(&vec![(0.0, Ev::On(key, 68)), (1.2, Ev::Off(key))], 1.3);
        for s in 0..2 {
            let ((c0, d0), (c1, d1)) = (stereo(&x[s], 0.0, 0.05, rate), stereo(&x[s], 0.1, 1.0, rate));
            let _ = writeln!(report, "{:>4} {}: {c0:+5.2} {d0:+5.1} | {c1:+5.2} {d1:+5.1}", name(key), side[s]);
        }
    }
    write_pairs(out, &pairs, 1.0, 1.0, rate as u32);
}

// ─────────────────────────────── the test set ───────────────────────────────

fn note_script(key: u8, vel: u8) -> Script {
    vec![(0.0, Ev::On(key, vel)), (HOLD_SECS, Ev::Off(key))]
}

fn phrases() -> Vec<(&'static str, Script, f64)> {
    let mut out = Vec::new();
    let c_major = [0u8, 2, 4, 5, 7, 9, 11];
    let scale = |base: u8, vel: u8| -> Script {
        let mut keys: Vec<u8> = (0..2).flat_map(|o| c_major.iter().map(move |&s| base + 12 * o + s)).collect();
        keys.push(base + 24);
        let down: Vec<u8> = keys.iter().rev().skip(1).copied().collect();
        keys.extend(down);
        let mut sc = Vec::new();
        for (i, &k) in keys.iter().enumerate() {
            let t = 0.25 * i as f64;
            sc.push((t, Ev::On(k, vel)));
            sc.push((t + 0.24, Ev::Off(k)));
        }
        sc
    };
    out.push(("scale_c3_c5_v68", scale(48, 68), 9.0));
    out.push(("scale_c1_c3_v68", scale(24, 68), 9.0));
    let chord = |keys: &[u8], vel: u8| -> Script {
        let mut sc: Script = keys.iter().map(|&k| (0.0, Ev::On(k, vel))).collect();
        sc.extend(keys.iter().map(|&k| (4.0, Ev::Off(k))));
        sc
    };
    out.push(("chord_low_c2_g2_c3_e3_v90", chord(&[36, 43, 48, 52], 90), 5.5));
    out.push(("chord_mid_c4_e4_g4_c5_v68", chord(&[60, 64, 67, 72], 68), 5.5));
    // Arpeggios over C, F, G7, C with a pedal change on each chord.
    let mut ped: Script = Vec::new();
    let chords: [[u8; 6]; 4] = [[36, 43, 48, 52, 55, 60], [29, 41, 45, 48, 53, 57], [31, 43, 47, 50, 53, 59], [36, 43, 48, 52, 55, 64]];
    for (c, ch) in chords.iter().enumerate() {
        let t0 = 2.0 * c as f64;
        if c > 0 {
            ped.push((t0 - 0.02, Ev::Pedal(0.0)));
        }
        ped.push((t0 + 0.08, Ev::Pedal(1.0)));
        for (i, &k) in ch.iter().enumerate() {
            let t = t0 + 0.25 * i as f64;
            ped.push((t, Ev::On(k, 56 + 8 * (i as u8 % 3))));
            ped.push((t + 0.22, Ev::Off(k)));
        }
    }
    ped.push((8.5, Ev::Pedal(0.0)));
    out.push(("pedalled_arpeggios_v56_72", ped, 10.5));
    out
}

/// A text script (see the header). Returns the script, its length and the
/// notes to analyse.
fn parse_script(text: &str) -> (Script, f64, Vec<(u8, f64)>) {
    let (mut script, mut end, mut analyze) = (Vec::new(), 0.0f64, Vec::new());
    for line in text.lines() {
        let f: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
        if f.is_empty() {
            continue;
        }
        let num = |i: usize| -> f64 { f.get(i).and_then(|v| v.parse().ok()).unwrap_or_else(|| panic!("bad script line: {line}")) };
        match (f[0], f.get(1).copied()) {
            ("end", _) => end = num(1),
            ("analyze", _) => analyze.push((num(1) as u8, num(2))),
            (_, Some("on")) => script.push((num(0), Ev::On(num(2) as u8, num(3) as u8))),
            (_, Some("off")) => script.push((num(0), Ev::Off(num(2) as u8))),
            (_, Some("pedal")) => script.push((num(0), Ev::Pedal(num(2) as f32))),
            (_, Some("soft")) => script.push((num(0), Ev::Soft(num(2) >= 0.5))),
            (_, Some("sostenuto")) => script.push((num(0), Ev::Sostenuto(num(2) >= 0.5))),
            _ => panic!("bad script line: {line}"),
        }
        end = end.max(script.last().map_or(0.0, |e| e.0 + 1.0));
    }
    (script, end, analyze)
}

// ─────────────────────────────── output ───────────────────────────────

/// Renders are written as 32-bit float, so that partials 80 dB and more
/// below a note's strongest stay above the file's own floor (a 16-bit file
/// flattens their decay); `--pcm16` writes 16-bit, for listening only.
static FLOAT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn write_wav(path: &Path, l: &[f32], r: &[f32], gain: f32, rate: u32) {
    let float = FLOAT.load(std::sync::atomic::Ordering::Relaxed);
    let width: u16 = if float { 4 } else { 2 };
    let frames = l.len().min(r.len());
    let mut bytes = Vec::with_capacity(44 + frames * 2 * width as usize);
    let data = (frames * 2 * width as usize) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&(if float { 3u16 } else { 1 }).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2 * width as u32).to_le_bytes());
    bytes.extend_from_slice(&(2 * width).to_le_bytes());
    bytes.extend_from_slice(&(8 * width).to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        for v in [l[i], r[i]] {
            if float {
                bytes.extend_from_slice(&(v * gain).to_le_bytes());
            } else {
                let s = (v * gain).clamp(-1.0, 1.0) * 32767.0;
                bytes.extend_from_slice(&(s.round() as i16).to_le_bytes());
            }
        }
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

struct Pair {
    case: String,
    a: (Vec<f32>, Vec<f32>),
    b: (Vec<f32>, Vec<f32>),
}

fn write_pairs(out: &Path, pairs: &[Pair], ga: f32, gb: f32, rate: u32) {
    for p in pairs {
        write_wav(&out.join(format!("{}.A.wav", p.case)), &p.a.0, &p.a.1, ga, rate);
        write_wav(&out.join(format!("{}.B.wav", p.case)), &p.b.0, &p.b.1, gb, rate);
    }
}

/// Concatenate segments, A then B for each, into one listening file.
fn write_ab(path: &Path, segments: &[(&Pair, usize, usize)], ga: f32, gb: f32, rate: u32) {
    let gap = (rate as usize) / 2;
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for &(p, a, z) in segments {
        for (side, g) in [(&p.a, ga), (&p.b, gb)] {
            let z = z.min(side.0.len());
            l.extend(side.0[a..z].iter().map(|x| x * g));
            r.extend(side.1[a..z].iter().map(|x| x * g));
            l.extend(std::iter::repeat(0.0).take(gap));
            r.extend(std::iter::repeat(0.0).take(gap));
        }
    }
    write_wav(path, &l, &r, 1.0, rate);
}

// ─────────────────────────────── main ───────────────────────────────

fn find_plugin(spec: &str) -> au::ComponentInfo {
    let all = au::instruments();
    if let Some(desc) = ComponentDescription::parse(spec) {
        if let Some(info) = all.iter().find(|i| i.description == desc) {
            return info.clone();
        }
    }
    let lower = spec.to_lowercase();
    all.iter()
        .find(|i| i.name.to_lowercase().contains(&lower))
        .cloned()
        .unwrap_or_else(|| panic!("no installed AU instrument matches '{spec}' (try --list)"))
}

fn main() {
    let args = Args(std::env::args().skip(1).collect());
    if args.flag("--list") {
        for info in au::instruments() {
            println!("{}  {}  (version {:#x})", info.description.code(), info.name, info.version);
        }
        return;
    }
    let spec = args.value("--au").expect("--au <name or type/sub/manu> (see --list)");
    let rate: f64 = args.value("--rate").map_or(48000.0, |v| v.parse().expect("--rate"));
    let info = find_plugin(spec);
    let t = std::time::Instant::now();
    let mut unit = au::Instrument::new(&info.description, rate, 1024).unwrap_or_else(|e| panic!("load {}: {e}", info.name));
    eprintln!("loaded {} in {:.0} ms", info.name, t.elapsed().as_secs_f64() * 1000.0);
    let _ = unit.set_offline(true);
    if args.flag("--presets") {
        for (n, name) in unit.factory_presets() {
            println!("{n}\t{name}");
        }
        return;
    }
    let mut preset = None;
    if let Some(want) = args.value("--au-preset") {
        let presets = unit.factory_presets();
        let lower = want.to_lowercase();
        let (n, name) = presets
            .iter()
            .find(|(_, n)| n.to_lowercase() == lower)
            .or_else(|| presets.iter().find(|(_, n)| n.to_lowercase().contains(&lower)))
            .unwrap_or_else(|| panic!("no factory preset matches '{want}' (try --presets)"));
        unit.set_factory_preset(*n).unwrap_or_else(|e| panic!("preset {name}: {e}"));
        preset = Some(name.clone());
    }
    let params = unit.parameters();
    let mut set = Vec::new();
    for item in args.values("--au-param") {
        let (key, value) = item.split_once('=').expect("--au-param <id|name>=<value>");
        let p = params
            .iter()
            .find(|p| p.id.to_string() == key || p.name.eq_ignore_ascii_case(key))
            .unwrap_or_else(|| panic!("no parameter '{key}' (try --params)"));
        set.push((p.id, value.parse::<f32>().expect("parameter value")));
    }
    if args.flag("--params") {
        for p in &params {
            let v = unit.parameter(p.id).unwrap_or(f32::NAN);
            let text = unit.parameter_text(p.id, v).unwrap_or_default();
            println!("{}\t{}\t{v}\t[{} .. {}] default {}\t{text}", p.id, p.name, p.min, p.max, p.default);
        }
        return;
    }
    let latency = (unit.latency_seconds() * rate).round() as usize;
    let mut plugin = Plugin { unit, preset, params: set };
    plugin.configure();

    let preset_name = args.value("--ours");
    let ours_preset: &'static makepad_piano_model::PianoPreset = match preset_name {
        Some(n) => PIANO_PRESETS.iter().find(|p| p.name.eq_ignore_ascii_case(n)).unwrap_or_else(|| panic!("no preset {n}")),
        None => PIANO_PRESETS.iter().find(|p| p.is_default).unwrap(),
    };
    let dry = args.flag("--dry");
    FLOAT.store(!args.flag("--pcm16"), std::sync::atomic::Ordering::Relaxed);
    let mut ours = Ours { preset: ours_preset, rate: rate as f32, dry, piano: Ours::build(ours_preset, rate as f32, dry), events: Vec::new() };
    let out = PathBuf::from(args.value("--out").expect("--out <dir>"));
    std::fs::create_dir_all(&out).expect("create --out");
    let mut report = String::new();
    let _ = writeln!(report, "A = {}\nB = {}\nrate {rate} Hz, plug-in latency {latency} frames", ours.label(), plugin.label());

    if let Some(path) = args.value("--script") {
        let text = std::fs::read_to_string(path).expect("read --script");
        let (script, secs, analyze) = parse_script(&text);
        let label = args.value("--label").unwrap_or("script");
        ours.clear();
        plugin.clear();
        plugin.configure();
        let a = render(&mut ours, &script, secs, rate, 0);
        let b = render(&mut plugin, &script, secs, rate, latency);
        for (side, (l, r)) in [("A", &a), ("B", &b)] {
            let (cols, total, cen) = band_line(l, r, 0.0, secs, rate);
            let cols: Vec<String> = cols.iter().map(|x| format!("{x:6.1}")).collect();
            let _ = writeln!(report, "{label} {side}: total {total:6.1} | {} | cen {cen:5.0} | peak {:.3}", cols.join(" "), peak(l, r));
        }
        for (key, t0) in analyze {
            let _ = writeln!(report, "-- {} at {t0} s", name(key));
            for (side, (l, r)) in [("A", &a), ("B", &b)] {
                let from = ((t0 * rate) as usize).min(l.len());
                report += &partials_text(side, &measure_partials(&l[from..], &r[from..], key, rate), key);
            }
        }
        let pair = Pair { case: label.to_string(), a, b };
        write_pairs(&out, std::slice::from_ref(&pair), 1.0, 1.0, rate as u32);
        print!("{report}");
        std::fs::write(out.join(format!("{label}.report.txt")), report).expect("write report");
        return;
    }

    let sets: Vec<&str> = args.value("--sets").map_or(vec!["notes", "scale", "chords", "pedal"], |s| s.split(',').collect());
    let keys: Vec<u8> = (21..=108).collect();
    // Single notes: (velocity, key) -> pair.
    let mut notes: Vec<(u8, u8, Pair, bool)> = Vec::new();
    if sets.contains(&"notes") {
        for &vel in &VELOCITIES {
            for &key in &keys {
                let script = note_script(key, vel);
                ours.clear();
                plugin.clear();
                plugin.configure();
                let a = render(&mut ours, &script, NOTE_SECS, rate, 0);
                let b = render(&mut plugin, &script, NOTE_SECS, rate, latency);
                notes.push((vel, key, Pair { case: format!("note_{}_v{vel}", name(key)), a, b }, false));
            }
            eprintln!("notes at v{vel} rendered");
        }
        // A plug-in note far quieter than its neighbours came back muted.
        for &vel in &VELOCITIES {
            let idx: Vec<usize> = (0..notes.len()).filter(|&i| notes[i].0 == vel).collect();
            let lv: Vec<f64> = idx.iter().map(|&i| db(energy(&notes[i].2.b.0, &notes[i].2.b.1))).collect();
            for (j, &i) in idx.iter().enumerate() {
                let mut around: Vec<f64> = (j.saturating_sub(4)..(j + 5).min(lv.len())).filter(|&k| k != j).map(|k| lv[k]).collect();
                around.sort_by(f64::total_cmp);
                let median = around[around.len() / 2];
                notes[i].3 = peak(&notes[i].2.b.0, &notes[i].2.b.1) < 1e-5 || lv[j] < median - 30.0;
            }
        }
    }
    let muted: Vec<String> = notes.iter().filter(|n| n.3).map(|n| format!("{}v{}", name(n.1), n.0)).collect();
    let _ = writeln!(report, "plug-in renders silent (left out of every metric): {}", if muted.is_empty() { "none".to_string() } else { muted.join(" ") });

    // One gain per instrument: the median key at v68, 0-2 s RMS, at -24 dBFS.
    let level = |side: fn(&Pair) -> &(Vec<f32>, Vec<f32>)| -> f64 {
        let mut v: Vec<f64> = notes
            .iter()
            .filter(|n| n.0 == 68 && !n.3)
            .map(|n| {
                let (l, r) = side(&n.2);
                db(energy(sect(l, 0.0, 2.0, rate), sect(r, 0.0, 2.0, rate)) / (2.0 * rate))
            })
            .collect();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(-24.0)
    };
    let (la, lb) = (level(|p| &p.a), level(|p| &p.b));
    let mut ga = 10f64.powf((-24.0 - la) / 20.0) as f32;
    let mut gb = 10f64.powf((-24.0 - lb) / 20.0) as f32;

    let mut phrase_pairs = Vec::new();
    for (case, script, secs) in phrases() {
        let wanted = (case.starts_with("scale") && sets.contains(&"scale"))
            || (case.starts_with("chord") && sets.contains(&"chords"))
            || (case.starts_with("pedal") && sets.contains(&"pedal"));
        if !wanted {
            continue;
        }
        ours.clear();
        plugin.clear();
        plugin.configure();
        let a = render(&mut ours, &script, secs, rate, 0);
        let b = render(&mut plugin, &script, secs, rate, latency);
        phrase_pairs.push(Pair { case: case.to_string(), a, b });
    }
    let max_a = notes.iter().map(|n| &n.2).chain(&phrase_pairs).map(|p| peak(&p.a.0, &p.a.1)).fold(0.0f32, f32::max) * ga;
    let max_b = notes.iter().filter(|n| !n.3).map(|n| &n.2).chain(&phrase_pairs).map(|p| peak(&p.b.0, &p.b.1)).fold(0.0f32, f32::max) * gb;
    let worst = max_a.max(max_b);
    if worst > 0.97 {
        ga *= 0.97 / worst;
        gb *= 0.97 / worst;
    }
    let _ = writeln!(
        report,
        "level match: median key at v68 is {la:.1} dBFS (A) and {lb:.1} dBFS (B) raw; gains {:+.1} dB (A), {:+.1} dB (B)",
        20.0 * (ga as f64).log10(),
        20.0 * (gb as f64).log10()
    );

    // Listening files: every third key per velocity, A then B; the phrases.
    if !notes.is_empty() {
        for &vel in &VELOCITIES {
            let picked: Vec<&Pair> = notes.iter().filter(|n| n.0 == vel && (n.1 - 21) % 3 == 0 && !n.3).map(|n| &n.2).collect();
            let segs: Vec<(&Pair, usize, usize)> = picked.iter().map(|p| (*p, 0, (NOTE_SECS * rate) as usize)).collect();
            write_ab(&out.join(format!("keys_every3_v{vel}.AB.wav")), &segs, ga, gb, rate as u32);
            let join = |side: fn(&Pair) -> &(Vec<f32>, Vec<f32>)| -> (Vec<f32>, Vec<f32>) {
                let (mut l, mut r) = (Vec::new(), Vec::new());
                for p in &picked {
                    l.extend_from_slice(&side(p).0);
                    r.extend_from_slice(&side(p).1);
                }
                (l, r)
            };
            let (a, b) = (join(|p| &p.a), join(|p| &p.b));
            write_pairs(&out, &[Pair { case: format!("keys_every3_v{vel}"), a, b }], ga, gb, rate as u32);
        }
    }
    write_pairs(&out, &phrase_pairs, ga, gb, rate as u32);
    for p in &phrase_pairs {
        write_ab(&out.join(format!("{}.AB.wav", p.case)), &[(p, 0, usize::MAX)], ga, gb, rate as u32);
    }

    let get = |vel: u8, key: u8| notes.iter().find(|n| n.0 == vel && n.1 == key && !n.3).map(|n| &n.2);

    // ref: the fixture's metrics, each side minus the recording.
    if !notes.is_empty() {
        let _ = writeln!(report, "\n== ref: minus the Salamander recording (emid ehigh lmid lhigh p1c_e p1c_l reg onset dlow dmid dhigh); A row then B row");
        let rows: Vec<(u8, u8, [f64; 11])> = FIXTURE
            .lines()
            .filter(|l| !l.starts_with('#'))
            .skip(1)
            .map(|line| {
                let f: Vec<&str> = line.split('\t').collect();
                (f[0].parse().unwrap(), f[1].parse().unwrap(), std::array::from_fn(|i| f[10 + i].parse().unwrap()))
            })
            .collect();
        let mut sum = [[[0.0f64; 11]; 2]; 2];
        let mut cnt = [[0.0f64; 2]; 2];
        for (key, vel, reference) in &rows {
            let (Some(p), Some(c4)) = (get(*vel, *key), get(*vel, 60)) else {
                let _ = writeln!(report, "{:>3} v{vel:<3} (left out: a side is missing)", name(*key));
                continue;
            };
            let sides: [(fn(&Pair) -> &(Vec<f32>, Vec<f32>), &str); 2] = [(|p| &p.a, "A"), (|p| &p.b, "B")];
            for (side, (pick, label)) in sides.into_iter().enumerate() {
                let (l, r) = pick(p);
                let (cl, cr) = pick(c4);
                let m = fixture_metrics(l, r, *key, rate);
                let c = fixture_metrics(cl, cr, 60, rate);
                let mut d = [0.0; 11];
                for k in 0..11 {
                    let v = if k == 6 { 20.0 * (m[6] / c[6]).log10() } else { m[k] };
                    d[k] = v - reference[k];
                }
                let g = usize::from(*key > 48);
                cnt[side][g] += 1.0;
                for k in 0..11 {
                    sum[side][g][k] += d[k].abs();
                }
                let cols: Vec<String> = d.iter().enumerate().map(|(k, x)| if k == 7 { format!("{x:+6.3}") } else { format!("{x:+6.1}") }).collect();
                let _ = writeln!(report, "{:>3} v{vel:<3} {label} {}", name(*key), cols.join(" "));
            }
        }
        for (g, group) in ["A0..C3", "C#3..C7"].iter().enumerate() {
            for (side, label) in ["A", "B"].iter().enumerate() {
                let s: Vec<String> = (0..11).map(|k| format!("{:.2}", sum[side][g][k] / cnt[side][g].max(1.0))).collect();
                let _ = writeln!(report, "mean|err| {group} {label}: {}", s.join(" "));
            }
        }

        // roughness: the probe's scan columns and its definition, A0..C6.
        let _ = writeln!(report, "\n== roughness: key against the mean of its neighbours, RMS (max) dB, keys A0..C6");
        for &vel in &VELOCITIES {
            for (label, pick) in [("A", (|p: &Pair| &p.a) as fn(&Pair) -> &(Vec<f32>, Vec<f32>)), ("B", |p: &Pair| &p.b)] {
                let mut cols: Vec<Vec<Option<f64>>> = vec![Vec::new(); 8];
                for key in 21u8..=84 {
                    let row = get(vel, key).map(|p| {
                        let (l, r) = pick(p);
                        let o = onset(l, r);
                        let (l, r) = (&l[o..], &r[o..]);
                        let rms = db(energy(sect(l, 0.0, 2.0, rate), sect(r, 0.0, 2.0, rate)) / (2.0 * rate));
                        let s = spectrum(sect(l, 0.02, 2.0, rate), sect(r, 0.02, 2.0, rate), rate);
                        let (_, early, _) = band_line(l, r, 0.02, 0.5, rate);
                        let (_, late, _) = band_line(l, r, 1.0, 2.0, rate);
                        let b: Vec<f64> = BANDS[..6].iter().map(|&(lo, hi)| db(band(&s, lo, hi))).collect();
                        [rms, late - early, b[0], b[1], b[2], b[3], b[4], b[5]]
                    });
                    for (c, col) in cols.iter_mut().enumerate() {
                        col.push(row.map(|r| r[c]));
                    }
                }
                let rough = |c: &[Option<f64>], a: usize, z: usize| -> (f64, f64) {
                    let d: Vec<f64> = (a.max(1)..z.min(c.len() - 1))
                        .filter_map(|i| Some(c[i]? - 0.5 * (c[i - 1]? + c[i + 1]?)))
                        .collect();
                    if d.is_empty() {
                        return (0.0, 0.0);
                    }
                    ((d.iter().map(|x| x * x).sum::<f64>() / d.len() as f64).sqrt(), d.iter().fold(0.0f64, |m, x| m.max(x.abs())))
                };
                for (reg, a, z) in [("A0..B2", 0usize, 27usize), ("C3..B4", 27, 51), ("C5..C6", 51, 64)] {
                    let (r0, m0) = rough(&cols[0], a, z);
                    let (r1, m1) = rough(&cols[1], a, z);
                    let (r3, m3) = rough(&cols[3], a, z);
                    let (r4, m4) = rough(&cols[4], a, z);
                    let (r5, m5) = rough(&cols[5], a, z);
                    let _ = writeln!(report, "v{vel:<3} {label} {reg}: rms {r0:.2} ({m0:.1}) | late-early {r1:.2} ({m1:.1}) | 80-200 {r3:.2} ({m3:.1}) | 200-500 {r4:.2} ({m4:.1}) | 500-1.5k {r5:.2} ({m5:.1})");
                }
            }
        }

        // registers: the shape of the instrument across the keyboard.
        let _ = writeln!(report, "\n== registers v68: rms dB re C4 | centroid 0.02-0.5 s > 1-3.5 s | late-early dB | decay dB/s low/mid/high (0.1-1 s) | attack 0-30/0-500 ms");
        for key in (21u8..=108).step_by(6) {
            let mut line = format!("{:>4}", name(key));
            for (label, pick) in [("A", (|p: &Pair| &p.a) as fn(&Pair) -> &(Vec<f32>, Vec<f32>)), ("B", |p: &Pair| &p.b)] {
                let (Some(p), Some(c4)) = (get(68, key), get(68, 60)) else {
                    line += &format!("  {label} (silent)");
                    continue;
                };
                let (l, r) = pick(p);
                let (cl, cr) = pick(c4);
                let o = onset(l, r);
                let (l, r) = (&l[o..], &r[o..]);
                let oc = onset(cl, cr);
                let rel = db(energy(sect(l, 0.0, 2.0, rate), sect(r, 0.0, 2.0, rate)) / energy(sect(&cl[oc..], 0.0, 2.0, rate), sect(&cr[oc..], 0.0, 2.0, rate)).max(1e-30));
                let (_, early, ce) = band_line(l, r, 0.02, 0.5, rate);
                let (_, late, cl2) = band_line(l, r, 1.0, 3.5, rate);
                let m = fixture_metrics(l, r, key, rate);
                let att = energy(sect(l, 0.0, 0.03, rate), sect(r, 0.0, 0.03, rate)) / energy(sect(l, 0.0, 0.5, rate), sect(r, 0.0, 0.5, rate)).max(1e-30);
                line += &format!(
                    "  {label} {rel:+5.1} | {ce:5.0}>{cl2:5.0} | {:+5.1} | {:5.1} {:5.1} {:5.1} | {att:.2}",
                    late - early,
                    m[8],
                    m[9],
                    m[10]
                );
            }
            let _ = writeln!(report, "{line}");
        }

        // partials, from the audio alone.
        let _ = writeln!(report, "\n== partials v68: n:Hz level 0.1-0.6 s > 1-2 s, early/late dB/s, r = envelope ripple dB (beats, swells)");
        for key in [21u8, 24, 28, 33, 36, 43, 48, 55, 60, 67, 72, 84, 96] {
            let _ = writeln!(report, "-- {}", name(key));
            for (label, pick) in [("A", (|p: &Pair| &p.a) as fn(&Pair) -> &(Vec<f32>, Vec<f32>)), ("B", |p: &Pair| &p.b)] {
                if let Some(p) = get(68, key) {
                    let (l, r) = pick(p);
                    report += &partials_text(label, &measure_partials(l, r, key, rate), key);
                }
            }
        }
    }

    if !phrase_pairs.is_empty() {
        let _ = writeln!(report, "\n== phrases: total dB | bands (20-80 80-200 200-500 500-1.5k 1.5-4k 4-10k 10-20k) | centroid, after the level match");
        for p in &phrase_pairs {
            for (label, (l, r), g) in [("A", &p.a, ga), ("B", &p.b, gb)] {
                let (cols, total, cen) = band_line(l, r, 0.0, l.len() as f64 / rate, rate);
                let gdb = 20.0 * (g as f64).log10();
                let cols: Vec<String> = cols.iter().map(|x| format!("{:6.1}", x + gdb)).collect();
                let _ = writeln!(report, "{:<28} {label}: {:6.1} | {} | {cen:5.0}", p.case, total + gdb, cols.join(" "));
            }
        }
    }

    if sets.contains(&"probes") {
        probes(&mut ours, &mut plugin, rate, latency, &out, &mut report);
    }

    // cpu: the probe's pedalled 48-note passage, rendered 10 s, best of 3.
    let mut cpu = vec![(0.0, Ev::Pedal(1.0))];
    for i in 0..48u32 {
        let key = 24 + ((i * 29) % 72) as u8;
        cpu.push((0.125 * i as f64, Ev::On(key, 60 + ((i * 13) % 50) as u8)));
    }
    let mut best = [f64::MAX; 2];
    for _ in 0..3 {
        for (side, p) in [&mut ours as &mut dyn Player, &mut plugin].into_iter().enumerate() {
            p.clear();
            let t = std::time::Instant::now();
            std::hint::black_box(render(p, &cpu, 10.0, rate, 0));
            best[side] = best[side].min(t.elapsed().as_secs_f64() / 10.0);
        }
    }
    let _ = writeln!(report, "\n== cpu: pedalled 48-note passage, s of CPU per s of audio, best of 3: A {:.4} ({:.1}x real time), B {:.4} ({:.1}x)", best[0], 1.0 / best[0], best[1], 1.0 / best[1]);
    let _ = writeln!(report, "   (B is wall time of the render calls; a plug-in may compute on threads of its own)");

    print!("{report}");
    std::fs::write(out.join("report.txt"), report).expect("write report");
}
