//! The graphic equalizer's sound: ten octave bands (31 Hz to 16 kHz, the
//! ISO centres a hardware graphic EQ has) and a preamp, run in the same
//! chain as before (`audio.rs`), with its presets.
//!
//! **The sliders mean what they say.** Neighbouring peaking filters
//! overlap, so ten filters set to the sliders' values would sum to more
//! than the sliders show (all bands at +6 would give about +10). The
//! filters' own gains are therefore solved from the sliders ([`solve`]):
//! the response the ten filters give together passes through the slider
//! values at the band centres. The response the panel draws
//! ([`response_db`]) is that same sum of the ten biquads' magnitudes, not
//! a line through the slider heads.
//!
//! **No zipper noise.** The UI stores the solved gains in atomics
//! ([`GraphicEqHandle`]); the audio thread glides each band's gain toward
//! its target with a 25 ms one-pole and recomputes that band's
//! coefficients every [`STEP`] frames while it moves, so a dragged slider
//! or a preset change sweeps smoothly instead of stepping. A band at 0 dB
//! costs nothing.

use makepad_audio_route::{FrameInfo, Processor};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// The bands, their centres and names.
pub const BANDS: usize = 10;
pub const BAND_HZ: [f32; BANDS] = [31.25, 62.5, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0];
pub const BAND_NAMES: [&str; BANDS] = ["31", "63", "125", "250", "500", "1K", "2K", "4K", "8K", "16K"];
/// A slider's range either way, dB (Winamp's).
pub const RANGE_DB: f32 = 12.0;
/// The preamp fader's range either way, dB, within the output level's
/// own range (`audio::GAIN_MIN_DB`..`GAIN_MAX_DB`); 0 dB is unity.
pub const PRE_DB: f32 = 12.0;
/// The filters' own gains may go past the sliders' to reach them, this far.
const FILTER_MAX_DB: f32 = 24.0;
/// One octave per band: the peaking filters' Q (broad enough that the
/// summed curve runs smoothly between the band centres).
const Q: f32 = 1.25;
/// The rate the panel's curve and the solve are worked at.
pub const DESIGN_RATE: f32 = 48_000.0;
/// The drawn frequency axis: half an octave past the outer bands, so every
/// band's column is the same width.
pub const AXIS_LOW_HZ: f32 = 31.25 / std::f32::consts::SQRT_2;
pub const AXIS_HIGH_HZ: f32 = 16_000.0 * std::f32::consts::SQRT_2;

const MAX_CHANNELS: usize = 8;
/// Frames between coefficient updates while a band glides.
const STEP: usize = 32;
/// The glide's time constant, seconds.
const GLIDE: f32 = 0.025;

// ---- presets ----

/// The presets built in: name, preamp, bands. Their preamps sit at unity:
/// headroom is the audio path's (the limiter, AUTO when the person asks).
pub const PRESETS: [(&str, f32, [f32; BANDS]); 12] = [
    ("Flat", 0.0, [0.0; BANDS]),
    ("Rock", 0.0, [5.0, 4.0, 3.0, -1.0, -2.5, -1.0, 2.0, 4.0, 5.0, 5.5]),
    ("Pop", 0.0, [-1.0, 1.0, 3.0, 4.5, 5.0, 3.0, 1.0, -0.5, -1.0, -1.5]),
    ("Classical", 0.0, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -3.0, -4.5, -4.5, -6.0]),
    ("Vocal", 0.0, [-3.0, -3.0, -2.0, 1.0, 4.0, 4.5, 4.0, 2.0, 0.0, -1.5]),
    ("Bass Boost", 0.0, [7.0, 6.0, 4.5, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble Boost", 0.0, [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 3.0, 5.0, 6.5, 7.5]),
    ("Loudness", 0.0, [6.0, 4.5, 1.5, 0.0, -1.0, 0.0, 0.5, 2.0, 4.5, 5.5]),
    ("Dance", 0.0, [6.0, 5.0, 2.0, 0.0, 0.0, -2.5, -3.0, -3.0, 0.0, 0.0]),
    ("Jazz", 0.0, [3.0, 2.0, 1.0, 2.0, -1.5, -1.5, 0.0, 1.5, 3.0, 3.5]),
    ("Electronic", 0.0, [4.5, 4.0, 1.0, 0.0, -2.0, 2.0, 1.0, 1.5, 4.0, 5.0]),
    ("Headphones", 0.0, [3.0, 6.0, 3.0, -2.0, -1.0, 1.0, 3.0, 6.0, 7.5, 8.0]),
];

// ---- the knobs ----

/// The filters' gains the UI sets (solved from the sliders, dB), shared
/// with every chain the audio side builds.
#[derive(Clone)]
pub struct GraphicEqHandle {
    gains: Arc<[AtomicU32; BANDS]>,
}

impl GraphicEqHandle {
    pub fn new() -> Self {
        Self { gains: Arc::new(std::array::from_fn(|_| AtomicU32::new(0f32.to_bits()))) }
    }

    /// Set the filters' gains, dB.
    pub fn set(&self, gains: &[f32; BANDS]) {
        for (slot, gain) in self.gains.iter().zip(gains) {
            slot.store(gain.clamp(-FILTER_MAX_DB, FILTER_MAX_DB).to_bits(), Ordering::Relaxed);
        }
    }

    fn get(&self, band: usize) -> f32 {
        f32::from_bits(self.gains[band].load(Ordering::Relaxed))
    }
}

// ---- the processor ----

/// Ten peaking filters in series whose gains glide to the knobs.
pub struct GraphicEq {
    handle: GraphicEqHandle,
    rate: f32,
    /// Each band's gain now (gliding), dB.
    current: [f32; BANDS],
    coeffs: [Coeff; BANDS],
    state: [[State; MAX_CHANNELS]; BANDS],
}

impl GraphicEq {
    pub fn with_handle(handle: &GraphicEqHandle) -> Self {
        let mut current = [0.0; BANDS];
        for (band, value) in current.iter_mut().enumerate() {
            *value = handle.get(band);
        }
        Self { handle: handle.clone(), rate: 0.0, current, coeffs: [Coeff::UNITY; BANDS], state: [[State::default(); MAX_CHANNELS]; BANDS] }
    }
}

impl Processor for GraphicEq {
    fn process(&mut self, frames: &mut [f32], info: &FrameInfo) {
        let channels = info.channels as usize;
        if channels == 0 || channels > MAX_CHANNELS || info.sample_rate <= 0.0 {
            return;
        }
        let rate = info.sample_rate as f32;
        if rate != self.rate {
            self.rate = rate;
            self.state = [[State::default(); MAX_CHANNELS]; BANDS];
            for band in 0..BANDS {
                self.coeffs[band] = peaking(rate, BAND_HZ[band], self.current[band]);
            }
        }
        let mut target = [0.0; BANDS];
        for (band, value) in target.iter_mut().enumerate() {
            *value = self.handle.get(band);
        }
        let follow = 1.0 - (-(STEP as f32) / (rate * GLIDE)).exp();
        for chunk in frames.chunks_mut(STEP * channels) {
            // The glide: every band a step toward its knob, new coefficients
            // for the ones that moved.
            for band in 0..BANDS {
                let (now, to) = (self.current[band], target[band]);
                if now != to {
                    let next = if (to - now).abs() < 0.005 { to } else { now + (to - now) * follow };
                    self.current[band] = next;
                    self.coeffs[band] = peaking(rate, BAND_HZ[band], next);
                    if next == 0.0 {
                        self.state[band] = [State::default(); MAX_CHANNELS];
                    }
                }
            }
            for band in 0..BANDS {
                if self.current[band] == 0.0 {
                    continue;
                }
                let c = self.coeffs[band];
                let states = &mut self.state[band];
                for frame in chunk.chunks_mut(channels) {
                    for (sample, s) in frame.iter_mut().zip(states.iter_mut()) {
                        let input = *sample;
                        let output = c.b0 * input + s.z1;
                        s.z1 = flush(c.b1 * input - c.a1 * output + s.z2);
                        s.z2 = flush(c.b2 * input - c.a2 * output);
                        *sample = output;
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Coeff {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coeff {
    const UNITY: Coeff = Coeff { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };
}

#[derive(Clone, Copy, Default)]
struct State {
    z1: f32,
    z2: f32,
}

fn flush(sample: f32) -> f32 {
    if sample.abs() < 1.0e-15 { 0.0 } else { sample }
}

/// Robert Bristow-Johnson's peaking filter.
fn peaking(rate: f32, freq: f32, db: f32) -> Coeff {
    if db == 0.0 || rate <= 0.0 || freq >= rate * 0.5 {
        return Coeff::UNITY;
    }
    let a = 10f32.powf(db / 40.0);
    let w0 = std::f32::consts::TAU * freq / rate;
    let alpha = w0.sin() / (2.0 * Q);
    let cos = w0.cos();
    let a0 = 1.0 + alpha / a;
    Coeff {
        b0: (1.0 + alpha * a) / a0,
        b1: -2.0 * cos / a0,
        b2: (1.0 - alpha * a) / a0,
        a1: -2.0 * cos / a0,
        a2: (1.0 - alpha / a) / a0,
    }
}

// ---- the response ----

/// One band's filter at `db`, heard at `freq`: its magnitude in dB, exact
/// for the digital filter at [`DESIGN_RATE`].
pub fn band_db(band: usize, db: f32, freq: f32) -> f32 {
    if db == 0.0 {
        return 0.0;
    }
    let c = peaking(DESIGN_RATE, BAND_HZ[band], db);
    let w = (std::f64::consts::TAU * freq as f64 / DESIGN_RATE as f64).min(std::f64::consts::PI);
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let (b0, b1, b2, a1, a2) = (c.b0 as f64, c.b1 as f64, c.b2 as f64, c.a1 as f64, c.a2 as f64);
    let (nr, ni) = (b0 + b1 * c1 + b2 * c2, -(b1 * s1 + b2 * s2));
    let (dr, di) = (1.0 + a1 * c1 + a2 * c2, -(a1 * s1 + a2 * s2));
    let mag2 = (nr * nr + ni * ni) / (dr * dr + di * di).max(1e-30);
    (10.0 * mag2.max(1e-30).log10()) as f32
}

/// The ten filters together at `freq`, dB.
pub fn response_db(gains: &[f32; BANDS], freq: f32) -> f32 {
    (0..BANDS).map(|band| band_db(band, gains[band], freq)).sum()
}

/// A point on the drawn axis (0..1) as a frequency.
pub fn axis_hz(x: f32) -> f32 {
    AXIS_LOW_HZ * (AXIS_HIGH_HZ / AXIS_LOW_HZ).powf(x)
}

/// The filters' gains whose sum passes through `sliders` at the band
/// centres: the interaction matrix (each filter's share at every centre,
/// probed at 12 dB) inverted once, then a few corrections on the exact
/// response, since the overlap is not quite linear in the gain.
pub fn solve(sliders: &[f32; BANDS]) -> [f32; BANDS] {
    if sliders.iter().all(|s| *s == 0.0) {
        return [0.0; BANDS];
    }
    let inverse = interaction_inverse();
    let mut gains = [0.0f32; BANDS];
    for _ in 0..6 {
        let mut error = [0.0f32; BANDS];
        for at in 0..BANDS {
            error[at] = sliders[at] - response_db(&gains, BAND_HZ[at]);
        }
        let mut done = true;
        for band in 0..BANDS {
            let step: f32 = (0..BANDS).map(|at| inverse[band][at] * error[at]).sum();
            if step.abs() > 0.01 {
                done = false;
            }
            gains[band] = (gains[band] + step).clamp(-FILTER_MAX_DB, FILTER_MAX_DB);
        }
        if done {
            break;
        }
    }
    gains
}

/// The interaction matrix's inverse, worked once.
fn interaction_inverse() -> [[f32; BANDS]; BANDS] {
    static INVERSE: std::sync::OnceLock<[[f32; BANDS]; BANDS]> = std::sync::OnceLock::new();
    *INVERSE.get_or_init(|| {
        const PROBE: f32 = 12.0;
        // m[at][band]: band's share at centre `at`, per dB of its gain.
        let mut m = [[0.0f64; BANDS]; BANDS];
        for at in 0..BANDS {
            for band in 0..BANDS {
                m[at][band] = (band_db(band, PROBE, BAND_HZ[at]) / PROBE) as f64;
            }
        }
        invert(m).map(|row| row.map(|v| v as f32))
    })
}

/// Gauss-Jordan with partial pivoting; the interaction matrix is
/// diagonally dominant, so it is always invertible.
fn invert(mut m: [[f64; BANDS]; BANDS]) -> [[f64; BANDS]; BANDS] {
    let mut inv = [[0.0f64; BANDS]; BANDS];
    for (i, row) in inv.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for col in 0..BANDS {
        let pivot = (col..BANDS).max_by(|a, b| m[*a][col].abs().partial_cmp(&m[*b][col].abs()).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(col);
        m.swap(col, pivot);
        inv.swap(col, pivot);
        let p = m[col][col];
        if p.abs() < 1e-12 {
            continue;
        }
        for k in 0..BANDS {
            m[col][k] /= p;
            inv[col][k] /= p;
        }
        for row in 0..BANDS {
            if row != col {
                let f = m[row][col];
                if f != 0.0 {
                    for k in 0..BANDS {
                        m[row][k] -= f * m[col][k];
                        inv[row][k] -= f * inv[col][k];
                    }
                }
            }
        }
    }
    inv
}

/// The preamp that keeps the loudest point of the curve at 0 dB (AUTO),
/// in half-dB steps.
pub fn auto_preamp(sliders: &[f32; BANDS]) -> f32 {
    let gains = solve(sliders);
    let peak = (0..96).map(|i| response_db(&gains, axis_hz(i as f32 / 95.0))).fold(0.0f32, f32::max);
    (-(peak * 2.0).ceil() / 2.0).clamp(-PRE_DB, 0.0)
}

// ---- the person's presets ----

/// Presets the person saved, and which preset the panel shows, kept in
/// `<data>/eq_presets.txt`: `current <name>`, then one `preset` line per
/// preset (`name|preamp|b0 … b9`).
#[derive(Clone, Debug, Default)]
pub struct UserPresets {
    pub list: Vec<(String, f32, [f32; BANDS])>,
    pub current: String,
}

fn presets_path() -> Option<std::path::PathBuf> {
    Some(crate::worker::data_dir()?.join("eq_presets.txt"))
}

impl UserPresets {
    pub fn load() -> Self {
        let mut out = Self { current: "Flat".into(), ..Default::default() };
        let Some(text) = presets_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return out };
        for line in text.lines().take(512) {
            if let Some(name) = line.strip_prefix("current ") {
                out.current = name.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("preset ") {
                let mut parts = rest.split('|');
                let (Some(name), Some(pre), Some(bands)) = (parts.next(), parts.next(), parts.next()) else { continue };
                let Ok(pre) = pre.trim().parse::<f32>() else { continue };
                let values: Vec<f32> = bands.split_whitespace().filter_map(|v| v.parse().ok()).filter(|v: &f32| v.is_finite()).collect();
                if values.len() != BANDS || name.trim().is_empty() {
                    continue;
                }
                let mut b = [0.0; BANDS];
                for (slot, v) in b.iter_mut().zip(values) {
                    *slot = v.clamp(-RANGE_DB, RANGE_DB);
                }
                out.list.push((name.trim().to_string(), pre.clamp(-PRE_DB, PRE_DB), b));
            }
        }
        out
    }

    pub fn save(&self) {
        let mut text = format!("current {}\n", self.current);
        for (name, pre, bands) in &self.list {
            let values: Vec<String> = bands.iter().map(|v| format!("{v}")).collect();
            text.push_str(&format!("preset {}|{}|{}\n", name.replace('|', "/"), pre, values.join(" ")));
        }
        if let Some(path) = presets_path() {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
            let _ = std::fs::write(path, text);
        }
    }

    /// Every preset by name: the built-in ones, then the person's.
    pub fn names(&self) -> Vec<String> {
        PRESETS.iter().map(|(n, _, _)| n.to_string()).chain(self.list.iter().map(|(n, _, _)| n.clone())).collect()
    }

    pub fn find(&self, name: &str) -> Option<(f32, [f32; BANDS])> {
        PRESETS.iter().find(|(n, _, _)| *n == name).map(|(_, p, b)| (*p, *b)).or_else(|| self.list.iter().find(|(n, _, _)| n == name).map(|(_, p, b)| (*p, *b)))
    }

    /// Keep `bands` and `pre` under the preset shown if it is the
    /// person's, else under a new name; the name it went under.
    pub fn store(&mut self, pre: f32, bands: [f32; BANDS]) -> String {
        if let Some(entry) = self.list.iter_mut().find(|(n, _, _)| *n == self.current) {
            entry.1 = pre;
            entry.2 = bands;
            return self.current.clone();
        }
        let mut at = self.list.len() + 1;
        let name = loop {
            let name = format!("User {at}");
            if !self.list.iter().any(|(n, _, _)| *n == name) {
                break name;
            }
            at += 1;
        };
        self.list.push((name.clone(), pre, bands));
        self.current = name.clone();
        name
    }
}
