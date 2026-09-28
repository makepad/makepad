//! Algorithmic room reverb: early reflections + an 8-line feedback delay
//! network (Householder mixing, per-line damping, slow modulation), with
//! room presets that crossfade when the listener moves between zones.
//!
//! Line lengths are fixed at construction (prime-ish, spread over 30-110 ms
//! at the device rate) so a preset change never moves a read head and never
//! pitches the tail; a room differs by decay, damping, pre-delay, early
//! reflection pattern and wet level, all smoothed.

use crate::delay::{Allpass, DelayLine};
use crate::dsp::{finite, OnePole, Smooth};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RoomPreset {
    /// No reverb at all.
    Dry,
    /// Open air: a ground bounce and a faint distant slap.
    #[default]
    Outdoor,
    /// A small room or cabin.
    Room,
    /// A large hall or warehouse.
    Hall,
    /// A road/rail tunnel: long, dense, metallic low-mids.
    Tunnel,
    /// A cave: long, dark, huge pre-delay.
    Cave,
    /// A stadium or arena: big slap-back and a long diffuse tail.
    Arena,
    /// A city street between buildings: flutter slaps, short tail.
    Street,
}

impl RoomPreset {
    pub const ALL: [RoomPreset; 8] = [
        RoomPreset::Dry,
        RoomPreset::Outdoor,
        RoomPreset::Room,
        RoomPreset::Hall,
        RoomPreset::Tunnel,
        RoomPreset::Cave,
        RoomPreset::Arena,
        RoomPreset::Street,
    ];

    pub fn parse(s: &str) -> Option<RoomPreset> {
        Some(match s {
            "dry" | "none" | "off" => RoomPreset::Dry,
            "outdoor" | "outside" | "open" | "field" => RoomPreset::Outdoor,
            "room" | "cabin" | "small" | "interior" => RoomPreset::Room,
            "hall" | "warehouse" | "church" | "large" => RoomPreset::Hall,
            "tunnel" | "pipe" | "corridor" => RoomPreset::Tunnel,
            "cave" | "cavern" | "dungeon" => RoomPreset::Cave,
            "arena" | "stadium" | "hangar" => RoomPreset::Arena,
            "street" | "city" | "alley" | "canyon" => RoomPreset::Street,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            RoomPreset::Dry => "dry",
            RoomPreset::Outdoor => "outdoor",
            RoomPreset::Room => "room",
            RoomPreset::Hall => "hall",
            RoomPreset::Tunnel => "tunnel",
            RoomPreset::Cave => "cave",
            RoomPreset::Arena => "arena",
            RoomPreset::Street => "street",
        }
    }

    pub fn params(self) -> RoomParams {
        let p = |decay, damp_hz, predelay_ms, er_level, er_spacing_ms, wet| RoomParams {
            decay_s: decay,
            damp_hz,
            predelay_ms,
            er_level,
            er_spacing_ms,
            wet,
        };
        match self {
            RoomPreset::Dry => p(0.2, 8000.0, 0.0, 0.0, 5.0, 0.0),
            RoomPreset::Outdoor => p(0.35, 5000.0, 12.0, 0.25, 9.0, 0.08),
            RoomPreset::Room => p(0.55, 6500.0, 4.0, 0.55, 3.5, 0.18),
            RoomPreset::Hall => p(2.2, 5200.0, 22.0, 0.45, 9.0, 0.28),
            RoomPreset::Tunnel => p(3.2, 2600.0, 8.0, 0.8, 6.0, 0.42),
            RoomPreset::Cave => p(4.5, 1800.0, 45.0, 0.5, 17.0, 0.40),
            RoomPreset::Arena => p(2.8, 4200.0, 60.0, 0.6, 24.0, 0.30),
            RoomPreset::Street => p(0.9, 4800.0, 14.0, 0.7, 11.0, 0.16),
        }
    }
}

/// The continuous parameters a preset sets (and that a zone may override).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoomParams {
    /// RT60 in seconds.
    pub decay_s: f32,
    /// Loop damping low-pass corner.
    pub damp_hz: f32,
    pub predelay_ms: f32,
    /// Early reflection level 0..1.
    pub er_level: f32,
    /// Spacing of the early reflection pattern (room size).
    pub er_spacing_ms: f32,
    /// Wet send return level.
    pub wet: f32,
}

const LINES: usize = 8;
/// Line lengths in ms at any rate (mutually prime once converted).
const LINE_MS: [f32; LINES] = [29.7, 37.1, 41.1, 43.7, 53.3, 61.9, 71.3, 83.9];
const ER_TAPS: usize = 8;
const ER_PATTERN: [(f32, f32, f32); ER_TAPS] = [
    // (spacing multiple, gain, pan -1..1)
    (1.0, 0.9, -0.6),
    (1.7, 0.75, 0.7),
    (2.3, 0.6, -0.3),
    (3.1, 0.55, 0.4),
    (3.9, 0.45, -0.8),
    (4.6, 0.38, 0.9),
    (5.7, 0.3, -0.1),
    (6.8, 0.25, 0.2),
];

/// Stereo reverb, mono send in.
pub struct Reverb {
    rate: f32,
    predelay: DelayLine,
    diffusers: [Allpass; 4],
    lines: [DelayLine; LINES],
    line_len: [f32; LINES],
    damp: [OnePole; LINES],
    lfo: f32,
    decay: Smooth,
    damp_hz: Smooth,
    predelay_ms: Smooth,
    er_level: Smooth,
    er_spacing: Smooth,
    wet: Smooth,
    coef: f32,
    target: RoomParams,
}

impl Reverb {
    pub fn new(rate: f32) -> Self {
        let rate = finite(rate, 48_000.0).clamp(8_000.0, 384_000.0);
        let ms = |m: f32| (m * 0.001 * rate) as usize;
        let lines = std::array::from_fn(|i| DelayLine::new(ms(LINE_MS[i] * 1.1) + 8));
        let line_len = std::array::from_fn(|i| LINE_MS[i] * 0.001 * rate);
        let p = RoomPreset::Outdoor.params();
        Reverb {
            rate,
            // Pre-delay + the widest early reflection pattern.
            predelay: DelayLine::new(ms(80.0 + 30.0 * 7.0) + 8),
            diffusers: [
                Allpass::new(ms(4.77), 0.7),
                Allpass::new(ms(3.59), 0.7),
                Allpass::new(ms(12.7), 0.6),
                Allpass::new(ms(9.31), 0.6),
            ],
            lines,
            line_len,
            damp: [OnePole::new(p.damp_hz, rate); LINES],
            lfo: 0.0,
            decay: Smooth::new(p.decay_s),
            damp_hz: Smooth::new(p.damp_hz),
            predelay_ms: Smooth::new(p.predelay_ms),
            er_level: Smooth::new(p.er_level),
            er_spacing: Smooth::new(p.er_spacing_ms),
            wet: Smooth::new(p.wet),
            coef: Smooth::coef(0.8, rate / 64.0),
            target: p,
        }
    }

    pub fn set_room(&mut self, p: RoomParams) {
        self.target = p;
        self.decay.target = finite(p.decay_s, 1.0).clamp(0.05, 20.0);
        self.damp_hz.target = finite(p.damp_hz, 5000.0).clamp(200.0, 18_000.0);
        self.predelay_ms.target = finite(p.predelay_ms, 10.0).clamp(0.0, 80.0);
        self.er_level.target = finite(p.er_level, 0.3).clamp(0.0, 1.0);
        self.er_spacing.target = finite(p.er_spacing_ms, 8.0).clamp(1.0, 30.0);
        self.wet.target = finite(p.wet, 0.2).clamp(0.0, 1.0);
    }

    /// Jump to a room without the crossfade (a fresh scene, offline renders).
    pub fn snap_room(&mut self, p: RoomParams) {
        self.set_room(p);
        for s in [
            &mut self.decay,
            &mut self.damp_hz,
            &mut self.predelay_ms,
            &mut self.er_level,
            &mut self.er_spacing,
            &mut self.wet,
        ] {
            s.value = s.target;
        }
    }

    pub fn room(&self) -> RoomParams {
        self.target
    }

    pub fn clear(&mut self) {
        self.predelay.clear();
        self.diffusers.iter_mut().for_each(Allpass::clear);
        self.lines.iter_mut().for_each(DelayLine::clear);
    }

    /// Process a block: `send` is the mono send bus; the wet stereo result
    /// is ADDED to `out_l`/`out_r`.
    pub fn process(&mut self, send: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = send.len().min(out_l.len()).min(out_r.len());
        let mut i = 0;
        while i < n {
            // Control rate: every 64 samples.
            let decay = self.decay.next(self.coef);
            let damp_hz = self.damp_hz.next(self.coef);
            let predelay = self.predelay_ms.next(self.coef) * 0.001 * self.rate;
            let er_level = self.er_level.next(self.coef);
            let er_spacing = self.er_spacing.next(self.coef) * 0.001 * self.rate;
            let wet = self.wet.next(self.coef);
            let mut fb = [0.0f32; LINES];
            for k in 0..LINES {
                // Gain per pass for the requested RT60.
                fb[k] = 10f32.powf(-3.0 * self.line_len[k] / (decay * self.rate));
                self.damp[k].set(damp_hz * (1.0 - 0.04 * k as f32), self.rate);
            }
            let end = (i + 64).min(n);
            for j in i..end {
                let x = send[j];
                self.predelay.push(x);
                let mut er_l = 0.0;
                let mut er_r = 0.0;
                for (mult, g, pan) in ER_PATTERN {
                    let s = self.predelay.read(predelay + er_spacing * mult + 1.0) * g;
                    er_l += s * (1.0 - pan) * 0.5;
                    er_r += s * (1.0 + pan) * 0.5;
                }
                let mut d = self.predelay.read(predelay + 1.0);
                for ap in self.diffusers.iter_mut() {
                    d = ap.process(d);
                }
                // Slow modulation of two lines de-rings the tail.
                self.lfo += 0.7 / self.rate;
                if self.lfo >= 1.0 {
                    self.lfo -= 1.0;
                }
                let m = (self.lfo * crate::dsp::TAU).sin() * 6.0;
                let mut outs = [0.0f32; LINES];
                for k in 0..LINES {
                    let wobble = if k == 1 { m } else if k == 5 { -m } else { 0.0 };
                    outs[k] = self.lines[k].read(self.line_len[k] + wobble);
                }
                // Householder: y = x - 2/N * sum(x).
                let sum: f32 = outs.iter().sum::<f32>() * (2.0 / LINES as f32);
                for k in 0..LINES {
                    let v = self.damp[k].lp(outs[k] - sum) * fb[k];
                    let input = if k % 2 == 0 { d } else { -d };
                    self.lines[k].push(v + input * 0.9);
                }
                let tail_l = outs[0] - outs[2] + outs[4] - outs[6];
                let tail_r = outs[1] - outs[3] + outs[5] - outs[7];
                out_l[j] += (tail_l * 0.7 + er_l * er_level) * wet;
                out_r[j] += (tail_r * 0.7 + er_r * er_level) * wet;
            }
            i = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energy_after(preset: RoomPreset, from_s: f32) -> f32 {
        let rate = 48_000.0;
        let mut r = Reverb::new(rate);
        r.snap_room(preset.params());
        let mut send = vec![0.0; 96_000];
        send[0] = 1.0;
        let (mut l, mut rr) = (vec![0.0; 96_000], vec![0.0; 96_000]);
        r.process(&send, &mut l, &mut rr);
        let start = (from_s * rate) as usize;
        l[start..].iter().chain(rr[start..].iter()).map(|x| x * x).sum()
    }

    #[test]
    fn a_cave_rings_longer_than_a_room_and_stays_finite() {
        let cave = energy_after(RoomPreset::Cave, 0.8);
        let room = energy_after(RoomPreset::Room, 0.8);
        assert!(cave.is_finite() && room.is_finite());
        assert!(cave > room * 20.0, "cave {cave} room {room}");
    }

    #[test]
    fn dry_is_silent() {
        assert!(energy_after(RoomPreset::Dry, 0.0) < 1.0e-6);
    }
}

