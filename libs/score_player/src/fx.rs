//! FX and buses: each reads the sum of its inputs and writes one stereo
//! chunk. Parameters are four numbers whose meaning the kind names
//! ([`FxKind::params`]); a knob changes them live, smoothed per sample.

use makepad_audio_synth::delay::DelayLine;
use makepad_audio_synth::dsp::{soft_clip, Filter, FilterMode};
use makepad_audio_synth::dynamics::Limiter;
use makepad_audio_synth::reverb::{Reverb, RoomParams};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxKind {
    Lowpass,
    Highpass,
    Bandpass,
    Delay,
    Reverb,
    Drive,
    Crush,
    Chorus,
    Comp,
    Duck,
    Bus,
    Limiter,
    /// Four bands, each off / peak / low shelf / high shelf / high pass /
    /// low pass, with its frequency, gain and Q.
    Eq,
    /// Stereo width and mid/side levels, with the lows made mono below a
    /// crossover.
    Width,
    /// A DJ deck's three-band isolator: low, mid and high split at two
    /// Linkwitz-Riley crossovers, each band's gain 0 (gone) to 2, a kill and
    /// a solo per band.
    Isolator,
    /// A DJ deck's one-knob filter: the middle passes everything, left
    /// sweeps a low pass down, right a high pass up.
    DjFilter,
}

/// The most parameters an FX has (an Eq's four bands of four).
pub const MAX_FX_PARAMS: usize = 16;

/// An Eq band's shapes, by the number its `bN_type` holds.
pub const EQ_TYPES: [&str; 6] = ["off", "peak", "lowshelf", "highshelf", "highpass", "lowpass"];

/// Longest echo a Delay holds, in seconds.
pub const MAX_DELAY_SECS: f32 = 4.0;

impl FxKind {
    /// Parameter names by index, with defaults and ranges.
    pub fn params(self) -> &'static [(&'static str, f32, f32, f32)] {
        match self {
            FxKind::Lowpass | FxKind::Highpass | FxKind::Bandpass => &[("cutoff", 1200.0, 20.0, 20000.0), ("res", 0.7, 0.3, 12.0), ("mix", 1.0, 0.0, 1.0)],
            FxKind::Delay => &[("time", 0.75, 0.0625, 8.0), ("feedback", 0.4, 0.0, 0.95), ("mix", 0.3, 0.0, 1.0), ("spread", 0.5, 0.0, 1.0)],
            FxKind::Reverb => &[("room", 0.6, 0.0, 1.0), ("mix", 0.3, 0.0, 1.0), ("damp", 6000.0, 500.0, 16000.0)],
            FxKind::Drive => &[("amount", 0.5, 0.0, 1.0), ("mix", 1.0, 0.0, 1.0), ("tone", 8000.0, 500.0, 18000.0)],
            FxKind::Crush => &[("bits", 8.0, 2.0, 16.0), ("down", 4.0, 1.0, 32.0), ("mix", 1.0, 0.0, 1.0)],
            FxKind::Chorus => &[("rate", 0.6, 0.05, 8.0), ("depth", 4.0, 0.5, 15.0), ("mix", 0.4, 0.0, 1.0)],
            FxKind::Comp => &[
                ("threshold", -18.0, -60.0, 0.0), ("ratio", 4.0, 1.0, 20.0), ("makeup", 0.0, 0.0, 24.0),
                ("attack", 5.0, 0.1, 200.0), ("release", 120.0, 10.0, 2000.0), ("knee", 6.0, 0.0, 24.0),
            ],
            FxKind::Eq => &[
                ("b1_type", 2.0, 0.0, 5.0), ("b1_freq", 100.0, 20.0, 20000.0), ("b1_gain", 0.0, -24.0, 24.0), ("b1_q", 0.7, 0.1, 18.0),
                ("b2_type", 1.0, 0.0, 5.0), ("b2_freq", 500.0, 20.0, 20000.0), ("b2_gain", 0.0, -24.0, 24.0), ("b2_q", 1.0, 0.1, 18.0),
                ("b3_type", 1.0, 0.0, 5.0), ("b3_freq", 2500.0, 20.0, 20000.0), ("b3_gain", 0.0, -24.0, 24.0), ("b3_q", 1.0, 0.1, 18.0),
                ("b4_type", 3.0, 0.0, 5.0), ("b4_freq", 8000.0, 20.0, 20000.0), ("b4_gain", 0.0, -24.0, 24.0), ("b4_q", 0.7, 0.1, 18.0),
            ],
            FxKind::Width => &[("width", 1.0, 0.0, 2.0), ("mid", 0.0, -24.0, 12.0), ("side", 0.0, -24.0, 12.0), ("bass_mono", 0.0, 0.0, 500.0)],
            FxKind::Duck => &[("depth", 8.0, 0.0, 36.0), ("release", 0.2, 0.02, 2.0)],
            FxKind::Bus => &[("gain", 1.0, 0.0, 2.0), ("pan", 0.0, -1.0, 1.0)],
            FxKind::Limiter => &[("ceiling", -0.8, -24.0, 0.0), ("gain", 1.0, 0.0, 4.0)],
            FxKind::Isolator => &[
                ("low", 1.0, 0.0, 2.0), ("mid", 1.0, 0.0, 2.0), ("high", 1.0, 0.0, 2.0),
                ("kill_low", 0.0, 0.0, 1.0), ("kill_mid", 0.0, 0.0, 1.0), ("kill_high", 0.0, 0.0, 1.0),
                ("solo_low", 0.0, 0.0, 1.0), ("solo_mid", 0.0, 0.0, 1.0), ("solo_high", 0.0, 0.0, 1.0),
                ("low_hz", 250.0, 40.0, 1000.0), ("high_hz", 2500.0, 1000.0, 12000.0),
            ],
            FxKind::DjFilter => &[("position", 0.5, 0.0, 1.0), ("res", 0.9, 0.5, 4.0)],
        }
    }

    pub fn parse(name: &str) -> Option<FxKind> {
        Some(match name {
            "Lowpass" | "Filter" => FxKind::Lowpass,
            "Highpass" => FxKind::Highpass,
            "Bandpass" => FxKind::Bandpass,
            "Delay" | "Echo" => FxKind::Delay,
            "Reverb" => FxKind::Reverb,
            "Drive" | "Distortion" => FxKind::Drive,
            "Crush" => FxKind::Crush,
            "Chorus" => FxKind::Chorus,
            "Comp" | "Compressor" => FxKind::Comp,
            "Duck" | "Sidechain" => FxKind::Duck,
            "Bus" => FxKind::Bus,
            "Limiter" => FxKind::Limiter,
            "Eq" | "EQ" => FxKind::Eq,
            "Width" | "MidSide" => FxKind::Width,
            "Isolator" => FxKind::Isolator,
            "DjFilter" => FxKind::DjFilter,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            FxKind::Lowpass => "Lowpass",
            FxKind::Highpass => "Highpass",
            FxKind::Bandpass => "Bandpass",
            FxKind::Delay => "Delay",
            FxKind::Reverb => "Reverb",
            FxKind::Drive => "Drive",
            FxKind::Crush => "Crush",
            FxKind::Chorus => "Chorus",
            FxKind::Comp => "Comp",
            FxKind::Duck => "Duck",
            FxKind::Bus => "Bus",
            FxKind::Limiter => "Limiter",
            FxKind::Eq => "Eq",
            FxKind::Width => "Width",
            FxKind::Isolator => "Isolator",
            FxKind::DjFilter => "DjFilter",
        }
    }

    /// A parameter that is a switch (0 or 1): an isolator band's kill and
    /// solo. A control binds it as a toggle.
    pub fn is_toggle(self, name: &str) -> bool {
        self == FxKind::Isolator && (name.starts_with("kill_") || name.starts_with("solo_"))
    }

    pub fn param_index(self, name: &str) -> Option<usize> {
        self.params().iter().position(|(param, ..)| *param == name)
    }

    /// Defaults, clamped values in place.
    pub fn defaults(self) -> [f32; MAX_FX_PARAMS] {
        let mut out = [0.0; MAX_FX_PARAMS];
        for (i, (_, default, ..)) in self.params().iter().enumerate() {
            out[i] = *default;
        }
        out
    }

    pub fn clamp(self, index: usize, value: f32) -> f32 {
        match self.params().get(index) {
            Some((_, default, lo, hi)) => if value.is_finite() { value.clamp(*lo, *hi) } else { *default },
            None => 0.0,
        }
    }
}

pub enum FxState {
    Filter { l: Filter, r: Filter, cutoff: f32, res: f32 },
    Delay { l: DelayLine, r: DelayLine, time: f32 },
    Reverb { room: Box<Reverb>, send: Box<[f32]> },
    Drive { lp_l: f32, lp_r: f32 },
    Crush { hold_l: f32, hold_r: f32, count: f32 },
    Chorus { l: DelayLine, r: DelayLine, phase: f32 },
    /// The detector's level (dB) and the gain reduction it applies now.
    Comp { env_db: f32, reduction_db: f32 },
    Eq { bands: [Biquad; 4], set: [[f32; 4]; 4] },
    Width { side_hp: Biquad, crossover: f32 },
    Duck { level: f32 },
    Bus { gain: f32 },
    Limiter { limiter: Limiter, ceiling: f32 },
    /// Low split (two low passes, two high passes) at `low_hz`, the upper
    /// part split again at `high_hz`, and each band's gain as it ramps.
    Isolator { split: [[Biquad; 2]; 4], corners: [f32; 2], gains: [f32; 3] },
    /// The two sides' filters, what they were designed for, and the wet
    /// amount as it ramps (0 in the middle).
    DjFilter { lp: Biquad, hp: Biquad, set: [f32; 2], wet: f32, high: bool },
}

/// The DJ filter's dead middle: within this of 0.5 it passes everything.
pub const DJ_FILTER_DEADZONE: f32 = 0.02;
/// Where the low pass sweeps (fully left: its lowest) and the high pass.
pub const DJ_LP_HZ: (f32, f32) = (20000.0, 150.0);
pub const DJ_HP_HZ: (f32, f32) = (20.0, 6000.0);

/// Which side a DJ filter knob is on and its corner: `None` in the middle,
/// `Some((false, hz))` a low pass, `Some((true, hz))` a high pass, each
/// swept on a log scale from the middle to its end.
pub fn dj_filter_corner(position: f32) -> Option<(bool, f32)> {
    let centre = 0.5;
    if !position.is_finite() || (position - centre).abs() <= DJ_FILTER_DEADZONE {
        return None;
    }
    let sweep = |(from, to): (f32, f32), t: f32| from * (to / from).powf(t.clamp(0.0, 1.0));
    let t = ((position - centre).abs() - DJ_FILTER_DEADZONE) / (centre - DJ_FILTER_DEADZONE);
    if position < centre {
        Some((false, sweep(DJ_LP_HZ, t)))
    } else {
        Some((true, sweep(DJ_HP_HZ, t)))
    }
}

/// An isolator band's gain: 0 when killed or when another band is soloed.
pub fn isolator_gains(p: &[f32; MAX_FX_PARAMS]) -> [f32; 3] {
    let solo = [p[6], p[7], p[8]].map(|v| v > 0.5);
    let any_solo = solo.iter().any(|on| *on);
    std::array::from_fn(|band| {
        let killed = p[3 + band] > 0.5 || (any_solo && !solo[band]);
        if killed { 0.0 } else { p[band].max(0.0) }
    })
}

fn room_of(room: f32, damp: f32) -> RoomParams {
    RoomParams { decay_s: 0.3 + room * room * 8.0, damp_hz: damp, predelay_ms: 8.0 + room * 20.0, er_level: 0.35, er_spacing_ms: 4.0 + room * 10.0, wet: 1.0 }
}

impl FxState {
    pub fn new(kind: FxKind, rate: f32) -> FxState {
        let max = (MAX_DELAY_SECS * rate) as usize + 8;
        match kind {
            FxKind::Lowpass | FxKind::Highpass | FxKind::Bandpass => {
                let mode = match kind { FxKind::Highpass => FilterMode::Highpass, FxKind::Bandpass => FilterMode::Bandpass, _ => FilterMode::Lowpass };
                FxState::Filter { l: Filter::new(mode), r: Filter::new(mode), cutoff: f32::NAN, res: f32::NAN }
            }
            FxKind::Delay => FxState::Delay { l: DelayLine::new(max), r: DelayLine::new(max), time: f32::NAN },
            FxKind::Reverb => {
                let mut room = Box::new(Reverb::new(rate));
                room.snap_room(room_of(0.6, 6000.0));
                FxState::Reverb { room, send: vec![0.0; crate::CHUNK].into_boxed_slice() }
            }
            FxKind::Drive => FxState::Drive { lp_l: 0.0, lp_r: 0.0 },
            FxKind::Crush => FxState::Crush { hold_l: 0.0, hold_r: 0.0, count: 0.0 },
            FxKind::Chorus => FxState::Chorus { l: DelayLine::new((0.05 * rate) as usize + 8), r: DelayLine::new((0.05 * rate) as usize + 8), phase: 0.0 },
            FxKind::Comp => FxState::Comp { env_db: -120.0, reduction_db: 0.0 },
            FxKind::Eq => FxState::Eq { bands: [Biquad::default(); 4], set: [[f32::NAN; 4]; 4] },
            FxKind::Width => FxState::Width { side_hp: Biquad::default(), crossover: f32::NAN },
            FxKind::Duck => FxState::Duck { level: 0.0 },
            FxKind::Bus => FxState::Bus { gain: 1.0 },
            FxKind::Limiter => FxState::Limiter { limiter: Limiter::new(rate, -0.8), ceiling: -0.8 },
            FxKind::Isolator => FxState::Isolator { split: [[Biquad::default(); 2]; 4], corners: [f32::NAN; 2], gains: [1.0; 3] },
            FxKind::DjFilter => FxState::DjFilter { lp: Biquad::default(), hp: Biquad::default(), set: [f32::NAN; 2], wet: 0.0, high: false },
        }
    }

    /// Process `l`/`r` in place. `side` is the key level of a Duck (0..1),
    /// `seconds_per_beat` places tempo-synced echoes.
    /// Gain reduction now, dB (≤ 0): a Comp's, else 0.
    pub fn reduction_db(&self) -> f32 {
        match self {
            FxState::Comp { reduction_db, .. } => *reduction_db,
            _ => 0.0,
        }
    }

    /// `key`: the sidechain signal a Comp listens to instead of its input.
    #[allow(clippy::too_many_arguments)]
    pub fn process(&mut self, kind: FxKind, p: &[f32; MAX_FX_PARAMS], l: &mut [f32], r: &mut [f32], rate: f32, seconds_per_beat: f32, side: f32, key: Option<(&[f32], &[f32])>) {
        let n = l.len();
        match self {
            FxState::Filter { l: fl, r: fr, cutoff, res } => {
                if *cutoff != p[0] || *res != p[1] {
                    *cutoff = p[0];
                    *res = p[1];
                    fl.set(p[0], p[1], rate);
                    fr.set(p[0], p[1], rate);
                }
                let mix = p[2];
                for i in 0..n {
                    l[i] += (fl.process(l[i]) - l[i]) * mix;
                    r[i] += (fr.process(r[i]) - r[i]) * mix;
                }
            }
            FxState::Delay { l: dl, r: dr, time } => {
                let target = (p[0] * seconds_per_beat * rate).clamp(2.0, (MAX_DELAY_SECS * rate) - 2.0);
                if !time.is_finite() {
                    *time = target;
                }
                let (feedback, mix, spread) = (p[1], p[2], p[3]);
                for i in 0..n {
                    *time += (target - *time) * 0.001;
                    let a = dl.read(*time);
                    let b = dr.read(*time);
                    let (x, y) = (l[i], r[i]);
                    // Spread crosses the feedback: 1 is a full ping-pong.
                    let into_l = x + (a * (1.0 - spread) + b * spread) * feedback;
                    let into_r = y + (b * (1.0 - spread) + a * spread) * feedback;
                    dl.push(soft_clip(into_l));
                    dr.push(soft_clip(into_r));
                    l[i] = x + a * mix;
                    r[i] = y + b * mix;
                }
            }
            FxState::Reverb { room, send } => {
                let want = room_of(p[0], p[2]);
                if room.room() != want {
                    room.set_room(want);
                }
                let mix = p[1];
                for i in 0..n {
                    send[i] = (l[i] + r[i]) * 0.5 * mix;
                }
                room.process(&send[..n], l, r);
            }
            FxState::Drive { lp_l, lp_r } => {
                let gain = 1.0 + p[0] * 24.0;
                let mix = p[1];
                let k = 1.0 - (-std::f32::consts::TAU * p[2] / rate).exp();
                let norm = 1.0 / soft_clip(gain).max(0.1);
                for i in 0..n {
                    let a = soft_clip(l[i] * gain) * norm;
                    let b = soft_clip(r[i] * gain) * norm;
                    *lp_l += (a - *lp_l) * k;
                    *lp_r += (b - *lp_r) * k;
                    l[i] += (*lp_l - l[i]) * mix;
                    r[i] += (*lp_r - r[i]) * mix;
                }
            }
            FxState::Crush { hold_l, hold_r, count } => {
                let levels = 2f32.powf(p[0].round().max(1.0) - 1.0);
                let down = p[1].max(1.0);
                let mix = p[2];
                for i in 0..n {
                    *count += 1.0;
                    if *count >= down {
                        *count -= down;
                        *hold_l = (l[i] * levels).round() / levels;
                        *hold_r = (r[i] * levels).round() / levels;
                    }
                    l[i] += (*hold_l - l[i]) * mix;
                    r[i] += (*hold_r - r[i]) * mix;
                }
            }
            FxState::Chorus { l: dl, r: dr, phase } => {
                let depth = p[1] * 0.001 * rate;
                let base = 0.012 * rate;
                let mix = p[2];
                for i in 0..n {
                    *phase += p[0] / rate;
                    if *phase >= 1.0 {
                        *phase -= 1.0;
                    }
                    let s = (*phase * std::f32::consts::TAU).sin();
                    let c = (*phase * std::f32::consts::TAU).cos();
                    dl.push(l[i]);
                    dr.push(r[i]);
                    let a = dl.read(base + depth * (0.5 + 0.5 * s));
                    let b = dr.read(base + depth * (0.5 + 0.5 * c));
                    l[i] += (a - l[i]) * mix * 0.5;
                    r[i] += (b - r[i]) * mix * 0.5;
                }
            }
            FxState::Comp { env_db, reduction_db } => {
                let (threshold, ratio, knee) = (p[0], p[1].max(1.0), p[5].max(0.0));
                let makeup = makepad_audio_synth::dsp::db_to_gain(p[2]);
                let attack = 1.0 - (-1.0 / (p[3].max(0.1) * 0.001 * rate)).exp();
                let release = 1.0 - (-1.0 / (p[4].max(1.0) * 0.001 * rate)).exp();
                let slope = 1.0 - 1.0 / ratio;
                for i in 0..n {
                    let (kl, kr) = key.map_or((l[i], r[i]), |(a, b)| (a.get(i).copied().unwrap_or(0.0), b.get(i).copied().unwrap_or(0.0)));
                    let level = 20.0 * kl.abs().max(kr.abs()).max(1e-6).log10();
                    let c = if level > *env_db { attack } else { release };
                    *env_db += (level - *env_db) * c;
                    let over = *env_db - threshold;
                    let k = knee * 0.5;
                    let gr = if over <= -k { 0.0 } else if over >= k || k <= 0.0 { over * slope } else { slope * (over + k) * (over + k) / (4.0 * k) };
                    *reduction_db = -gr;
                    let g = makepad_audio_synth::dsp::db_to_gain(-gr) * makeup;
                    l[i] *= g;
                    r[i] *= g;
                }
            }
            FxState::Eq { bands, set } => {
                for b in 0..4 {
                    let band = [p[b * 4], p[b * 4 + 1], p[b * 4 + 2], p[b * 4 + 3]];
                    if band != set[b] {
                        set[b] = band;
                        bands[b].design(band, rate);
                    }
                }
                for i in 0..n {
                    let (mut a, mut c) = (l[i], r[i]);
                    for (b, band) in bands.iter_mut().enumerate() {
                        if set[b][0].round() as i32 != 0 {
                            (a, c) = band.process(a, c);
                        }
                    }
                    l[i] = a;
                    r[i] = c;
                }
            }
            FxState::Width { side_hp, crossover } => {
                let (width, mid, side_gain) = (p[0].max(0.0), makepad_audio_synth::dsp::db_to_gain(p[1]), makepad_audio_synth::dsp::db_to_gain(p[2]));
                if *crossover != p[3] {
                    *crossover = p[3];
                    side_hp.design([4.0, p[3].max(10.0), 0.0, std::f32::consts::FRAC_1_SQRT_2], rate);
                }
                let mono_bass = p[3] > 0.0;
                for i in 0..n {
                    let m = (l[i] + r[i]) * 0.5;
                    let mut s = (l[i] - r[i]) * 0.5;
                    // Bass mono: the side through a high pass at the crossover.
                    if mono_bass {
                        s = side_hp.process(s, 0.0).0;
                    }
                    let (m, s) = (m * mid, s * width * side_gain);
                    l[i] = m + s;
                    r[i] = m - s;
                }
            }
            FxState::Duck { level } => {
                let attack = 1.0 - (-1.0 / (0.004 * rate)).exp();
                let release = 1.0 - (-1.0 / (p[1].max(0.01) * rate)).exp();
                // The key level is one number per chunk: the ramp makes it
                // a pump, not a gate.
                let target = side.clamp(0.0, 1.0);
                for i in 0..n {
                    let c = if target > *level { attack } else { release };
                    *level += (target - *level) * c;
                    let g = makepad_audio_synth::dsp::db_to_gain(-p[0] * *level);
                    l[i] *= g;
                    r[i] *= g;
                }
            }
            FxState::Limiter { limiter, ceiling } => {
                if *ceiling != p[0] {
                    *ceiling = p[0];
                    limiter.ceiling = makepad_audio_synth::dsp::db_to_gain(p[0]);
                }
                for i in 0..n {
                    let (a, b) = limiter.process(l[i] * p[1], r[i] * p[1]);
                    l[i] = a;
                    r[i] = b;
                }
            }
            FxState::Isolator { split, corners, gains } => {
                let (low_hz, high_hz) = (p[9], p[10].max(p[9] * 1.5));
                if corners[0] != low_hz || corners[1] != high_hz {
                    *corners = [low_hz, high_hz];
                    let q = std::f32::consts::FRAC_1_SQRT_2;
                    // Two Butterworth sections each: a Linkwitz-Riley split.
                    for (stage, (kind, hz)) in [(5.0, low_hz), (4.0, low_hz), (5.0, high_hz), (4.0, high_hz)].into_iter().enumerate() {
                        for section in split[stage].iter_mut() {
                            section.design([kind, hz, 0.0, q], rate);
                        }
                    }
                }
                let target = isolator_gains(p);
                let k = 1.0 - (-1.0 / (0.005 * rate)).exp();
                for i in 0..n {
                    let x = (l[i], r[i]);
                    let cascade = |pair: &mut [Biquad; 2], (a, b): (f32, f32)| {
                        let (a, b) = pair[0].process(a, b);
                        pair[1].process(a, b)
                    };
                    let [low_lp, low_hp, high_lp, high_hp] = split;
                    let low = cascade(low_lp, x);
                    let upper = cascade(low_hp, x);
                    let mid = cascade(high_lp, upper);
                    let high = cascade(high_hp, upper);
                    for (gain, target) in gains.iter_mut().zip(target) {
                        *gain += (target - *gain) * k;
                    }
                    l[i] = low.0 * gains[0] + mid.0 * gains[1] + high.0 * gains[2];
                    r[i] = low.1 * gains[0] + mid.1 * gains[1] + high.1 * gains[2];
                }
            }
            FxState::DjFilter { lp, hp, set, wet, high } => {
                let corner = dj_filter_corner(p[0]);
                if [p[0], p[1]] != *set {
                    *set = [p[0], p[1]];
                    if let Some((is_high, hz)) = corner {
                        let band = [if is_high { 4.0 } else { 5.0 }, hz, 0.0, p[1]];
                        if is_high { hp.design(band, rate) } else { lp.design(band, rate) }
                        if is_high != *high && *wet < 1e-3 {
                            *high = is_high;
                        }
                    }
                }
                // Crossing the middle: the old side fades out before the
                // new one comes in.
                let target = match corner {
                    Some((is_high, _)) if is_high == *high => 1.0,
                    _ => 0.0,
                };
                if let Some((is_high, _)) = corner {
                    if is_high != *high && *wet < 1e-3 {
                        *high = is_high;
                    }
                }
                let k = 1.0 - (-1.0 / (0.01 * rate)).exp();
                for i in 0..n {
                    *wet += (target - *wet) * k;
                    let (a, b) = if *high { hp.process(l[i], r[i]) } else { lp.process(l[i], r[i]) };
                    l[i] += (a - l[i]) * *wet;
                    r[i] += (b - r[i]) * *wet;
                }
            }
            FxState::Bus { gain } => {
                let (pl, pr) = crate::voice::pan_gains(p[1]);
                let (pl, pr) = (pl * std::f32::consts::SQRT_2, pr * std::f32::consts::SQRT_2);
                for i in 0..n {
                    *gain += (p[0] - *gain) * 0.002;
                    l[i] *= *gain * pl.min(1.0);
                    r[i] *= *gain * pr.min(1.0);
                }
            }
        }
        let _ = kind;
    }
}

/// One stereo biquad (RBJ's cookbook), for an Eq band.
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z_l: [f32; 2],
    z_r: [f32; 2],
}

impl Biquad {
    /// Coefficients for a band `[type, freq, gain dB, q]` (see [`EQ_TYPES`]).
    pub fn coefficients(band: [f32; 4], rate: f32) -> ([f32; 3], [f32; 2]) {
        let (kind, freq, gain, q) = (band[0].round() as i32, band[1].clamp(10.0, rate * 0.45), band[2], band[3].max(0.05));
        let w = std::f32::consts::TAU * freq / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a_gain = 10f32.powf(gain / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            1 => (1.0 + alpha * a_gain, -2.0 * cos, 1.0 - alpha * a_gain, 1.0 + alpha / a_gain, -2.0 * cos, 1.0 - alpha / a_gain),
            2 => {
                let s = 2.0 * a_gain.sqrt() * alpha;
                (a_gain * ((a_gain + 1.0) - (a_gain - 1.0) * cos + s), 2.0 * a_gain * ((a_gain - 1.0) - (a_gain + 1.0) * cos), a_gain * ((a_gain + 1.0) - (a_gain - 1.0) * cos - s),
                 (a_gain + 1.0) + (a_gain - 1.0) * cos + s, -2.0 * ((a_gain - 1.0) + (a_gain + 1.0) * cos), (a_gain + 1.0) + (a_gain - 1.0) * cos - s)
            }
            3 => {
                let s = 2.0 * a_gain.sqrt() * alpha;
                (a_gain * ((a_gain + 1.0) + (a_gain - 1.0) * cos + s), -2.0 * a_gain * ((a_gain - 1.0) + (a_gain + 1.0) * cos), a_gain * ((a_gain + 1.0) + (a_gain - 1.0) * cos - s),
                 (a_gain + 1.0) - (a_gain - 1.0) * cos + s, 2.0 * ((a_gain - 1.0) - (a_gain + 1.0) * cos), (a_gain + 1.0) - (a_gain - 1.0) * cos - s)
            }
            4 => ((1.0 + cos) * 0.5, -(1.0 + cos), (1.0 + cos) * 0.5, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            5 => ((1.0 - cos) * 0.5, 1.0 - cos, (1.0 - cos) * 0.5, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            _ => (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
        };
        ([b0 / a0, b1 / a0, b2 / a0], [a1 / a0, a2 / a0])
    }

    pub fn design(&mut self, band: [f32; 4], rate: f32) {
        (self.b, self.a) = Self::coefficients(band, rate);
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let tick = |z: &mut [f32; 2], x: f32, b: &[f32; 3], a: &[f32; 2]| {
            let y = b[0] * x + z[0];
            z[0] = b[1] * x - a[0] * y + z[1];
            z[1] = b[2] * x - a[1] * y;
            y
        };
        (tick(&mut self.z_l, l, &self.b, &self.a), tick(&mut self.z_r, r, &self.b, &self.a))
    }
}

/// An Eq's response at `freq` Hz, in dB (for its curve): the sum of its
/// bands' magnitudes.
pub fn eq_response_db(p: &[f32], freq: f32, rate: f32) -> f32 {
    let w = std::f32::consts::TAU * freq / rate;
    let mut db = 0.0;
    for band in p.chunks_exact(4).take(4) {
        if band[0].round() as i32 == 0 {
            continue;
        }
        let (b, a) = Biquad::coefficients([band[0], band[1], band[2], band[3]], rate);
        // |H(e^jw)|
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let (nr, ni) = (b[0] + b[1] * c1 + b[2] * c2, -(b[1] * s1 + b[2] * s2));
        let (dr, di) = (1.0 + a[0] * c1 + a[1] * c2, -(a[0] * s1 + a[1] * s2));
        let mag = ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-20)).sqrt();
        db += 20.0 * mag.max(1e-9).log10();
    }
    db
}

#[cfg(test)]
mod deck_tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    /// The RMS of `kind` with `params` over a sine at `hz`, after it settles.
    fn rms_through(kind: FxKind, params: &[(&str, f32)], hz: f32) -> f32 {
        let mut p = kind.defaults();
        for (name, value) in params {
            p[kind.param_index(name).unwrap()] = *value;
        }
        let mut state = FxState::new(kind, RATE);
        let mut sum = 0.0;
        let mut count = 0;
        let mut t = 0usize;
        for chunk in 0..200 {
            let mut l: Vec<f32> = (0..256).map(|i| (std::f32::consts::TAU * hz * (t + i) as f32 / RATE).sin()).collect();
            let mut r = l.clone();
            t += 256;
            state.process(kind, &p, &mut l, &mut r, RATE, 0.5, 0.0, None);
            if chunk >= 100 {
                sum += l.iter().map(|v| v * v).sum::<f32>();
                count += l.len();
            }
        }
        (sum / count as f32).sqrt()
    }

    #[test]
    fn the_isolator_at_unity_passes_every_band_and_a_kill_takes_one_away() {
        let unity = std::f32::consts::FRAC_1_SQRT_2;
        for hz in [60.0, 800.0, 8000.0] {
            let rms = rms_through(FxKind::Isolator, &[], hz);
            assert!((rms - unity).abs() < 0.03, "{hz} Hz at unity: {rms}");
        }
        assert!(rms_through(FxKind::Isolator, &[("kill_low", 1.0)], 60.0) < 0.05);
        assert!(rms_through(FxKind::Isolator, &[("kill_low", 1.0)], 8000.0) > 0.65, "the other bands stay");
        assert!(rms_through(FxKind::Isolator, &[("solo_high", 1.0)], 800.0) < 0.08, "a solo takes the others away");
        assert!(rms_through(FxKind::Isolator, &[("high", 2.0)], 8000.0) > 1.3);
        assert!(rms_through(FxKind::Isolator, &[("mid", 0.0)], 800.0) < 0.1);
    }

    #[test]
    fn the_dj_filter_passes_in_the_middle_and_sweeps_either_side() {
        assert_eq!(dj_filter_corner(0.5), None);
        assert_eq!(dj_filter_corner(0.5 + DJ_FILTER_DEADZONE * 0.5), None);
        assert_eq!(dj_filter_corner(0.0), Some((false, DJ_LP_HZ.1)));
        let (high, hz) = dj_filter_corner(1.0).unwrap();
        assert!(high && (hz - DJ_HP_HZ.1).abs() < 1.0);
        let (_, quarter) = dj_filter_corner(0.25).unwrap();
        assert!(quarter < DJ_LP_HZ.0 && quarter > DJ_LP_HZ.1);
        let unity = std::f32::consts::FRAC_1_SQRT_2;
        assert!((rms_through(FxKind::DjFilter, &[], 5000.0) - unity).abs() < 0.01, "the middle passes");
        assert!(rms_through(FxKind::DjFilter, &[("position", 0.0)], 5000.0) < 0.05, "fully left cuts the highs");
        assert!(rms_through(FxKind::DjFilter, &[("position", 0.0)], 60.0) > 0.6, "and keeps the lows");
        assert!(rms_through(FxKind::DjFilter, &[("position", 1.0)], 60.0) < 0.05, "fully right cuts the lows");
    }
}
