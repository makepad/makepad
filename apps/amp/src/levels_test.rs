//! The level through our own processing, measured: known signals (a 1 kHz
//! sine, pink noise, a hot master near full scale) run through each stage
//! of the chain a routed player and a local file play through, with the
//! RMS and peak printed per stage (`cargo test levels -- --nocapture`).
//! With the equalizer flat and the volume at unity what comes out is what
//! went in.

use super::*;
use makepad_audio_route::FrameInfo;

const RATE: f64 = 48_000.0;
const FRAMES: usize = 96_000;
const BLOCK: usize = 512;

fn db(linear: f32) -> f32 {
    20.0 * linear.max(1e-12).log10()
}

/// Stereo sine at `peak_db` dBFS.
fn sine(freq: f32, peak_db: f32) -> Vec<f32> {
    let amp = 10f32.powf(peak_db / 20.0);
    (0..FRAMES).flat_map(|i| {
        let s = amp * (std::f64::consts::TAU * freq as f64 * i as f64 / RATE).sin() as f32;
        [s, s]
    }).collect()
}

/// Stereo pink noise (Paul Kellet's filter on white noise, the two sides
/// independent) at `rms_db` dBFS RMS.
fn pink(rms_db: f32) -> Vec<f32> {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut white = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    };
    let mut b = [[0.0f32; 7]; 2];
    let mut out = Vec::with_capacity(FRAMES * 2);
    for _ in 0..FRAMES {
        for side in 0..2 {
            let w = white();
            let b = &mut b[side];
            b[0] = 0.99886 * b[0] + w * 0.0555179;
            b[1] = 0.99332 * b[1] + w * 0.0750759;
            b[2] = 0.96900 * b[2] + w * 0.1538520;
            b[3] = 0.86650 * b[3] + w * 0.3104856;
            b[4] = 0.55000 * b[4] + w * 0.5329522;
            b[5] = -0.7616 * b[5] - w * 0.0168980;
            let p = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
            b[6] = w * 0.115926;
            out.push(p);
        }
    }
    let (rms, _) = level(&out);
    let scale = 10f32.powf((rms_db - rms) / 20.0);
    out.iter_mut().for_each(|s| *s *= scale);
    out
}

/// RMS and peak, dBFS, over the second half (filters settled).
fn level(x: &[f32]) -> (f32, f32) {
    let tail = &x[x.len() / 2..];
    let rms = (tail.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / tail.len() as f64).sqrt() as f32;
    let peak = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    (db(rms), db(peak))
}

fn run(stage: &mut dyn Processor, x: &mut [f32]) {
    for block in x.chunks_mut(BLOCK * 2) {
        let info = FrameInfo { sample_rate: RATE, channels: 2, frames: block.len() / 2, host_time: 0 };
        stage.process(block, &info);
    }
}

/// Every stage of the chain on its own, then the chains as the route and
/// local playback build them; prints a line per stage and returns what the
/// route and the local chain changed (RMS dB, peak dB).
fn trace(name: &str, settings: &EqSettings, input: &[f32]) -> [(f32, f32); 2] {
    let knobs = Knobs::new();
    knobs.apply(settings);
    let (rms_in, peak_in) = level(input);
    eprintln!("-- {name}: input {rms_in:+.2} dBFS RMS, {peak_in:+.2} dBFS peak");
    let mut x = input.to_vec();
    let mut eq = GraphicEq::with_handle(&knobs.eq);
    let mut preamp = Gain::with_handle(&knobs.gain);
    let mut volume = Gain::with_handle(&knobs.volume);
    let mut limiter = makepad_audio_route::Limiter::default();
    let limits = knobs.boost.load(Ordering::Relaxed);
    let stages: [(&str, &mut dyn Processor, bool); 4] = [("graphic eq", &mut eq, true), ("preamp", &mut preamp, true), ("our volume (local)", &mut volume, true), ("safety limiter", &mut limiter, limits)];
    for (stage, processor, runs) in stages {
        if !runs {
            eprintln!("   {stage:<24} skipped (nothing boosts)");
            continue;
        }
        run(processor, &mut x);
        let (rms, peak) = level(&x);
        eprintln!("   after {stage:<18} {rms:+.2} RMS ({:+.2} dB)  {peak:+.2} peak ({:+.2} dB)", rms - rms_in, peak - peak_in);
    }
    let mut out = [(0.0, 0.0); 2];
    for (at, (chain, mut processor)) in [("route chain", knobs.route_chain()), ("local chain", knobs.local_chain())].into_iter().enumerate() {
        let mut x = input.to_vec();
        run(processor.as_mut(), &mut x);
        let (rms, peak) = level(&x);
        out[at] = (rms - rms_in, peak - peak_in);
        eprintln!("   {chain:<24} {rms:+.2} RMS ({:+.2} dB)  {peak:+.2} peak ({:+.2} dB)", rms - rms_in, peak - peak_in);
    }
    out
}

fn signals() -> [(&'static str, Vec<f32>); 3] {
    [("1 kHz sine, -12 dBFS peak", sine(1_000.0, -12.0)), ("pink noise, -12 dBFS RMS", pink(-12.0)), ("hot master, 1 kHz at -0.3 dBFS", sine(1_000.0, -0.3))]
}

/// Off, and on with every slider and the preamp at 0: the route and the
/// local chain play at the level they were handed, within ±0.1 dB, peaks
/// near full scale included.
#[test]
fn levels_flat_is_unity() {
    let on_flat = EqSettings { on: true, ..EqSettings::default() };
    for (name, input) in signals() {
        for (mode, settings) in [("off", EqSettings::default()), ("on, flat", on_flat)] {
            for (chain, (rms, peak)) in ["route", "local"].into_iter().zip(trace(&format!("{name}, {mode}"), &settings, &input)) {
                assert!(rms.abs() <= 0.1 && peak.abs() <= 0.1, "{name}, {mode}: the {chain} chain moved RMS {rms:+.2} dB, peak {peak:+.2} dB");
            }
        }
    }
}

/// The settings on this machine's disk, for the record (no assertion: they
/// boost the preamp).
#[test]
fn levels_saved_settings() {
    let saved = EqSettings::parse("on 1\nlow 0\nmid 0\nhigh 0\ngain 6\nvolume 0.98094076\n");
    for (name, input) in signals() {
        trace(&format!("{name}, saved (preamp {:+} dB, volume {})", saved.gain_db, saved.volume), &saved, &input);
    }
}

/// A boost (+6 dB on the 1 kHz slider, a 1 kHz sine at -3 dBFS) turns the
/// safety limiter on: the peaks bend under full scale instead of clipping.
#[test]
fn levels_a_boost_is_limited_not_clipped() {
    let mut bands = [0.0; crate::geq::BANDS];
    bands[5] = 6.0;
    let boosted = EqSettings { on: true, bands, ..EqSettings::default() };
    let input = sine(1_000.0, -3.0);
    let [(route_rms, _), _] = trace("1 kHz sine, -3 dBFS peak, 1 kHz slider +6 dB", &boosted, &input);
    let knobs = Knobs::new();
    knobs.apply(&boosted);
    assert!(knobs.boost.load(Ordering::Relaxed));
    let mut x = input.clone();
    run(knobs.route_chain().as_mut(), &mut x);
    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak < 1.0, "peak {peak}");
    assert!(route_rms > 1.0, "the boost still lifts it: {route_rms:+.2} dB");
}
