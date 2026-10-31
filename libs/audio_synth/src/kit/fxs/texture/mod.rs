//! Nimbus (effect type 22): the granular texture engine — granularizer,
//! pitch shifter / time stretcher, looping delay and spectral modes over a
//! recording buffer — and its adapter (EFFECTS-2.md section 6.8).
//!
//! The engine is a Rust port of the MIT-licensed eurorack texture
//! synthesizer by Emilie Gillet (each ported file keeps its MIT notice;
//! credit in CREDITS.md). It runs at 32 kHz on 16-bit frames in blocks of 8
//! with the hardware's memory sizes; the host signal is resampled to and from
//! 32 kHz with the shared Lanczos (a = 4) interpolator.
//!
//! The adapter (this file) is our own code.

mod atan;
mod buffer;
mod fft;
mod fx;
mod grain;
mod looping;
mod processor;
mod pvoc;
mod tables;
mod wsola;

use super::common::Line;
use super::FxCtx;
use crate::kit::core::{SlotDef, SlotUnit, SlotValue};
use crate::kit::oscs::twist::lanczos::{lanczos, LANCZOS_A};
use processor::GranularProcessor;

/// The engine's rate.
pub const ENGINE_RATE: f32 = 32000.0;
/// Frames per engine call.
const ENGINE_BLOCK: usize = 8;
/// Host block.
const HOST_BLOCK: usize = 32;
/// Host-rate output ring.
const OUT_RING: usize = 2048;
/// Input ring (host rate) and engine output ring (32 kHz), frames.
const IN_RING: usize = 256;
const ENG_RING: usize = 64;

/// The engine's own dry/wet stays at almost fully wet; the host crossfades.
const ENGINE_DRY_WET: f32 = 0.99999;

pub const MODE_NAMES: [&str; 4] = ["Granularizer", "Pitch Shifter", "Looping Delay", "Spectral Madness"];
pub const QUALITY_NAMES: [&str; 4] =
    ["32k 16-bit Stereo", "32k 16-bit Mono", "16k 8-bit Stereo", "16k 8-bit Mono"];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        SlotDef { about: "playback mode", ..SlotDef::choice("Mode", &MODE_NAMES, 0.0) },
        SlotDef { about: "buffer rate / resolution / channels", ..SlotDef::choice("Quality", &QUALITY_NAMES, 0.0) },
        SlotDef { about: "where in the recording grains are read", ..SlotDef::float("Position", 0.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "grain / window / loop size (label depends on the mode)", ..SlotDef::float("Size", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "transposition", ..SlotDef::float("Pitch", -48.0, 48.0, 0.0, SlotUnit::Semis) },
        SlotDef { about: "grain density (label depends on the mode)", ..SlotDef::float("Density", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "grain shape / filter (label depends on the mode)", ..SlotDef::float("Texture", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "stereo spread of grains (granularizer)", ..SlotDef::float("Spread", 0.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "freeze the recording (> 0.5 = on)", ..SlotDef::float("Freeze", 0.0, 1.0, 0.0, SlotUnit::None) },
        SlotDef { about: "output fed back into the recording", ..SlotDef::float("Feedback", 0.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "reverb amount", ..SlotDef::float("Reverb", 0.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "dry/wet", ..SlotDef::float("Mix", 0.0, 1.0, 0.5, SlotUnit::Percent) },
    ]
}

/// Display name of slot 3 / 5 / 6 for a mode (UI only).
pub fn slot_label(mode: i32, slot: usize) -> &'static str {
    let names: [&str; 3] = match mode {
        1 | 2 => ["Size", "Diffusion", "Filter"],
        3 => ["Warp", "Smear", "Texture"],
        _ => ["Size", "Density", "Texture"],
    };
    match slot {
        3 => names[0],
        5 => names[1],
        6 => names[2],
        _ => "",
    }
}

/// No migrations.
pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

#[inline]
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

pub struct Texture {
    processor: Box<GranularProcessor>,
    sr: f32,
    // Host input ring and the 32 kHz read position into it.
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    in_written: i64,
    in_pos: f64,
    // One engine block being filled.
    block_in: [[i16; 2]; ENGINE_BLOCK],
    block_fill: usize,
    // Engine output (32 kHz) and the host-rate read position into it.
    eng_l: Vec<f32>,
    eng_r: Vec<f32>,
    eng_written: i64,
    eng_pos: f64,
    // Host-rate output ring.
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    out_written: usize,
    out_read: usize,
    started: bool,
    start_threshold: usize,
    /// The host crossfade: fully wet at (re)init, then per block
    /// 0.25 Mix + 0.75 previous, ramped across the block.
    mix: Line,
}

impl Texture {
    pub fn new(sr: f32) -> Texture {
        Texture::with_seed(sr, 0x21)
    }

    /// Back to the start state (allocates: called when a patch is installed).
    pub fn reset(&mut self) {
        *self = Texture::new(self.sr);
    }

    /// `seed` seeds the engine's random generator (grain scheduling, spectral
    /// randomisation).
    pub fn with_seed(sr: f32, seed: u32) -> Texture {
        let ratio_steps = (sr / ENGINE_RATE).ceil() as i64;
        let start_threshold = HOST_BLOCK * (1 + (ratio_steps - 2).max(0) as usize) + 8;
        Texture {
            processor: GranularProcessor::new(seed),
            sr,
            in_l: vec![0.0; IN_RING],
            in_r: vec![0.0; IN_RING],
            in_written: 0,
            in_pos: 0.0,
            block_in: [[0; 2]; ENGINE_BLOCK],
            block_fill: 0,
            eng_l: vec![0.0; ENG_RING],
            eng_r: vec![0.0; ENG_RING],
            eng_written: 0,
            eng_pos: 0.0,
            out_l: vec![0.0; OUT_RING],
            out_r: vec![0.0; OUT_RING],
            out_written: 0,
            out_read: 0,
            started: false,
            start_threshold,
            mix: {
                let mut m = Line::new();
                m.snap(1.0);
                m
            },
        }
    }

    /// Frames the output must have produced before the wet signal is read.
    pub fn latency_threshold(&self) -> usize {
        self.start_threshold
    }

    fn set_parameters(&mut self, p: &[SlotValue; 12]) {
        let proc_ = &mut self.processor;
        proc_.set_playback_mode(p[0].i().clamp(0, 3));
        proc_.set_quality(p[1].i().clamp(0, 3));
        let freeze = p[8].v > 0.5;
        {
            let q = proc_.parameters_mut();
            q.position = clamp01(p[2].v);
            q.size = clamp01(p[3].v);
            q.pitch = p[4].v.clamp(-48.0, 48.0);
            q.density = clamp01((p[5].v + 1.0) * 0.5);
            q.texture = clamp01((p[6].v + 1.0) * 0.5);
            q.stereo_spread = clamp01(p[7].v);
            q.feedback = clamp01(p[9].v);
            q.reverb = clamp01(p[10].v);
            q.dry_wet = ENGINE_DRY_WET;
            q.trigger = false;
            q.gate = freeze;
        }
        proc_.set_freeze(freeze);
        proc_.set_silence(false);
    }

    fn run_engine_block(&mut self, p: &[SlotValue; 12]) {
        let mut out = [[0i16; 2]; ENGINE_BLOCK];
        self.set_parameters(p);
        self.processor.prepare();
        self.processor.process(&self.block_in, &mut out, ENGINE_BLOCK);
        for f in out.iter() {
            let w = (self.eng_written as usize) % ENG_RING;
            self.eng_l[w] = f[0] as f32 / 32767.0;
            self.eng_r[w] = f[1] as f32 / 32767.0;
            self.eng_written += 1;
        }
        // Engine output -> host rate, into the output ring.
        let step = (ENGINE_RATE / self.sr) as f64;
        while (self.eng_pos.floor() as i64) + LANCZOS_A < self.eng_written {
            let w = self.out_written % OUT_RING;
            self.out_l[w] = lanczos(&self.eng_l, self.eng_pos);
            self.out_r[w] = lanczos(&self.eng_r, self.eng_pos);
            self.out_written += 1;
            self.eng_pos += step;
        }
    }

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        if ctx.sr != self.sr && ctx.sr > 0.0 {
            // A rate change re-times the resamplers (no reallocation needed).
            self.sr = ctx.sr;
        }
        // Host input -> ring.
        for i in 0..HOST_BLOCK {
            let w = (self.in_written as usize) % IN_RING;
            self.in_l[w] = l[i];
            self.in_r[w] = r[i];
            self.in_written += 1;
        }
        // Ring -> 32 kHz frames -> 8-frame engine blocks (leftovers kept).
        let step = (self.sr / ENGINE_RATE) as f64;
        while (self.in_pos.floor() as i64) + LANCZOS_A < self.in_written {
            let a = lanczos(&self.in_l, self.in_pos).clamp(-1.0, 1.0);
            let b = lanczos(&self.in_r, self.in_pos).clamp(-1.0, 1.0);
            self.in_pos += step;
            self.block_in[self.block_fill] = [(a * 32767.0) as i16, (b * 32767.0) as i16];
            self.block_fill += 1;
            if self.block_fill == ENGINE_BLOCK {
                self.block_fill = 0;
                self.run_engine_block(p);
            }
        }
        if !self.started && self.out_written > self.start_threshold {
            self.started = true;
        }
        self.mix.set_target_smoothed(clamp01(p[11].v));
        for i in 0..HOST_BLOCK {
            let (wl, wr) = if self.started {
                let k = self.out_read % OUT_RING;
                self.out_read += 1;
                (self.out_l[k], self.out_r[k])
            } else {
                (0.0, 0.0)
            };
            let m = self.mix.at(i);
            l[i] = l[i] * (1.0 - m) + wl * m;
            r[i] = r[i] * (1.0 - m) + wr * m;
        }
        // Keep the counters bounded (exact: multiples of the ring sizes).
        if self.in_written > (1 << 40) {
            let s = (self.in_written / IN_RING as i64) * IN_RING as i64;
            self.in_written -= s;
            self.in_pos -= s as f64;
            let e = (self.eng_written / ENG_RING as i64) * ENG_RING as i64;
            self.eng_written -= e;
            self.eng_pos -= e as f64;
        }
    }

    /// The reference has no control-only hook for this effect: nothing
    /// happens (not even the mix smoothing advances).
    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn ctx() -> FxCtx<'static> {
        FxCtx::new(SR)
    }

    fn params(mode: i32) -> [SlotValue; 12] {
        let d = slot_defs();
        let mut p = [SlotValue::default(); 12];
        for i in 0..12 {
            p[i] = d[i].default_value();
        }
        p[0] = SlotValue::int(mode);
        p[11] = SlotValue::float(1.0);
        p
    }

    /// Renders `blocks` host blocks of a 440 Hz sine (silence after `input_blocks`).
    fn render(n: &mut Texture, p: &[SlotValue; 12], blocks: usize, input_blocks: usize, phase: &mut usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(blocks * 64);
        let c = ctx();
        for b in 0..blocks {
            let mut l = [0.0f32; 32];
            let mut r = [0.0f32; 32];
            if b < input_blocks {
                for i in 0..32 {
                    let s = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * *phase as f32 / SR).sin();
                    l[i] = s;
                    r[i] = s;
                    *phase += 1;
                }
            }
            n.process(p, &c, &mut l, &mut r);
            for i in 0..32 {
                out.push(l[i]);
                out.push(r[i]);
            }
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn every_mode_renders_finite_sane_levels() {
        for mode in 0..4 {
            for quality in 0..4 {
                let mut p = params(mode);
                p[1] = SlotValue::int(quality);
                p[5] = SlotValue::float(0.6); // some grain density
                p[3] = SlotValue::float(0.5);
                let mut n = Texture::new(SR);
                let mut ph = 0;
                let x = render(&mut n, &p, 600, 600, &mut ph);
                assert!(x.iter().all(|v| v.is_finite()), "mode {mode} q {quality} NaN");
                let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!(peak <= 1.25, "mode {mode} q {quality} peak {peak}");
                let tail = rms(&x[x.len() / 2..]);
                assert!(tail > 1e-3, "mode {mode} q {quality} silent (rms {tail})");
            }
        }
    }

    #[test]
    fn feedback_and_reverb_at_max_do_not_blow_up() {
        for mode in 0..4 {
            let mut p = params(mode);
            p[9] = SlotValue::float(1.0);
            p[10] = SlotValue::float(1.0);
            p[5] = SlotValue::float(1.0);
            let mut n = Texture::new(SR);
            let mut ph = 0;
            let x = render(&mut n, &p, 1500, 1500, &mut ph);
            assert!(x.iter().all(|v| v.is_finite()), "mode {mode}");
            let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak <= 1.25, "mode {mode} peak {peak}");
        }
    }

    #[test]
    fn freeze_holds_output_after_input_stops() {
        for mode in [0, 2, 3] {
            let mut p = params(mode);
            p[5] = SlotValue::float(0.6);
            p[3] = SlotValue::float(0.5);
            let mut n = Texture::new(SR);
            let mut ph = 0;
            render(&mut n, &p, 600, 600, &mut ph);
            p[8] = SlotValue::float(1.0);
            let x = render(&mut n, &p, 600, 0, &mut ph);
            let late = rms(&x[x.len() / 2..]);
            assert!(late > 1e-3, "mode {mode}: frozen output decayed (rms {late})");
            // Without freeze the same silence lets the output die away.
            let mut p2 = params(mode);
            p2[5] = p[5];
            p2[3] = p[3];
            let mut n2 = Texture::new(SR);
            let mut ph2 = 0;
            render(&mut n2, &p2, 600, 600, &mut ph2);
            let y = render(&mut n2, &p2, 2000, 0, &mut ph2);
            let late2 = rms(&y[y.len() - 2000..]);
            assert!(late2 < late, "mode {mode}: freeze {late} vs no freeze {late2}");
        }
    }

    /// Strongest frequency in [lo, hi] Hz of the left channel (Hann window).
    fn peak_hz(x: &[f32], lo: f32, hi: f32) -> f32 {
        let l: Vec<f32> = x.iter().step_by(2).copied().collect();
        let n = l.len();
        let mag = |f: f32| -> f32 {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in l.iter().enumerate() {
                let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
                let a = 2.0 * std::f64::consts::PI * f as f64 * i as f64 / SR as f64;
                re += v as f64 * w * a.cos();
                im += v as f64 * w * a.sin();
            }
            (re * re + im * im).sqrt() as f32
        };
        let mut best = (0.0, lo);
        let mut f = lo;
        while f <= hi {
            let m = mag(f);
            if m > best.0 {
                best = (m, f);
            }
            f += 0.5;
        }
        best.1
    }

    #[test]
    fn unshifted_modes_keep_the_input_pitch() {
        for mode in 0..3 {
            let mut p = params(mode);
            p[5] = SlotValue::float(0.6);
            p[3] = SlotValue::float(0.5);
            let mut n = Texture::new(SR);
            let mut ph = 0;
            let x = render(&mut n, &p, 900, 900, &mut ph);
            let f = peak_hz(&x[x.len() - 2 * 8192..], 400.0, 480.0);
            let cents = 1200.0 * (f / 440.0).log2();
            assert!(cents.abs() < 5.0, "mode {mode}: {f} Hz");
        }
    }

    #[test]
    fn mix_starts_wet_and_smooths_per_block() {
        let mut p = params(0);
        p[11] = SlotValue::float(0.0);
        let mut n = Texture::new(SR);
        let c = ctx();
        // Control-only blocks do nothing at all.
        for _ in 0..10 {
            n.process_only_control(&p, &c);
        }
        assert_eq!((n.mix.cur, n.mix.target), (1.0, 1.0));
        let mut want = 1.0f32;
        for blk in 0..4 {
            let prev = want;
            want *= 0.75;
            let mut l = [0.5f32; 32];
            let mut r = [0.5f32; 32];
            n.process(&p, &c, &mut l, &mut r);
            assert_eq!((n.mix.cur, n.mix.target), (prev, want));
            if blk == 0 {
                // Wet is still silent in the first block: only the dry share.
                assert!(!n.started);
                for k in 0..32 {
                    let m = prev + (want - prev) * (k + 1) as f32 / 32.0;
                    assert!((l[k] - 0.5 * (1.0 - m)).abs() < 1e-6, "{k} {} {}", l[k], 0.5 * (1.0 - m));
                }
            }
        }
        // A reset goes back to fully wet at once.
        n.reset();
        assert_eq!((n.mix.cur, n.mix.target), (1.0, 1.0));
    }

    #[test]
    fn size_is_a_plain_clamp() {
        let mut p = params(0);
        let mut n = Texture::new(SR);
        for (v, want) in [(-0.5f32, 0.0f32), (0.3, 0.3), (1.5, 1.0)] {
            p[3] = SlotValue::float(v);
            p[5] = SlotValue::float(-0.5);
            n.set_parameters(&p);
            let q = n.processor.parameters_mut();
            assert_eq!(q.size, want);
            assert_eq!(q.density, 0.25, "density keeps (v + 1) / 2");
        }
    }

    #[test]
    fn mix_zero_is_dry_exactly() {
        let mut p = params(0);
        p[11] = SlotValue::float(0.0);
        let mut n = Texture::new(SR);
        let c = ctx();
        // The crossfade starts fully wet and decays by 0.75 per block; it
        // reaches exactly 0 once the target underflows.
        for _ in 0..600 {
            let (mut l, mut r) = ([0.0f32; 32], [0.0f32; 32]);
            n.process(&p, &c, &mut l, &mut r);
        }
        for b in 0..100 {
            let mut l = [0.0f32; 32];
            let mut r = [0.0f32; 32];
            for i in 0..32 {
                l[i] = ((b * 32 + i) as f32 * 0.01).sin();
                r[i] = -l[i];
            }
            let (dl, dr) = (l, r);
            n.process(&p, &c, &mut l, &mut r);
            assert_eq!(l, dl);
            assert_eq!(r, dr);
        }
    }

    #[test]
    fn deterministic_for_seed() {
        for mode in 0..4 {
            let mut p = params(mode);
            p[5] = SlotValue::float(0.8);
            p[7] = SlotValue::float(1.0);
            let mut a = Texture::with_seed(SR, 7);
            let mut b = Texture::with_seed(SR, 7);
            let (mut pa, mut pb) = (0, 0);
            assert_eq!(render(&mut a, &p, 300, 300, &mut pa), render(&mut b, &p, 300, 300, &mut pb), "mode {mode}");
        }
    }

    #[test]
    fn wet_is_silent_until_the_latency_threshold() {
        assert_eq!(Texture::new(48000.0).latency_threshold(), 40);
        assert_eq!(Texture::new(96000.0).latency_threshold(), 72);
        assert_eq!(Texture::new(44100.0).latency_threshold(), 40);
        // Fully wet: the first block (before 40 frames were produced) is silent.
        let p = params(2);
        let mut n = Texture::new(SR);
        let c = ctx();
        let mut l = [0.5f32; 32];
        let mut r = [0.5f32; 32];
        n.process(&p, &c, &mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|&v| v == 0.0));
        assert!(!n.started);
        for _ in 0..4 {
            let mut l = [0.5f32; 32];
            let mut r = [0.5f32; 32];
            n.process(&p, &c, &mut l, &mut r);
        }
        assert!(n.started);
    }
}
