//! Engine-level tests of the twist models 0, 1, 2, 5, 6 and 8 (our code):
//! rendered directly at the engine rate in native 12-sample blocks.

use super::chord::ChordEngine;
use super::engine::*;
use super::fm::FmEngine;
use super::stm::Random;
use super::swarm::SwarmEngine;
use super::va::VirtualAnalogEngine;
use super::waveshaping::WaveshapingEngine;
use super::wavetable::WavetableEngine;

fn run(e: &mut dyn Engine, p: EngineParameters, n: usize, seed: u32) -> (Vec<f32>, Vec<f32>) {
    let mut rng = Random::new(seed);
    e.reset();
    let mut o = Vec::with_capacity(n);
    let mut a = Vec::with_capacity(n);
    let mut out = [0.0f32; BLOCK_SIZE];
    let mut aux = [0.0f32; BLOCK_SIZE];
    while o.len() < n {
        let mut env = false;
        e.render(&p, &mut out, &mut aux, &mut env, &mut rng);
        o.extend_from_slice(&out);
        a.extend_from_slice(&aux);
    }
    (o, a)
}

/// Strongest spectral peak in [lo, hi] Hz (Hann window, golden search refine).
fn peak_hz(x: &[f32], fs: f32, lo: f32, hi: f32) -> f32 {
    let n = x.len();
    let mag = |f: f64| -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
            let a = 2.0 * std::f64::consts::PI * f * i as f64 / fs as f64;
            re += v as f64 * w * a.cos();
            im += v as f64 * w * a.sin();
        }
        re * re + im * im
    };
    let df = fs as f64 / n as f64 / 4.0;
    let (mut best, mut bf) = (0.0, lo as f64);
    let mut f = lo as f64;
    while f <= hi as f64 {
        let m = mag(f);
        if m > best {
            best = m;
            bf = f;
        }
        f += df;
    }
    let (mut a, mut b) = (bf - df, bf + df);
    for _ in 0..40 {
        let m1 = a + (b - a) * 0.382;
        let m2 = a + (b - a) * 0.618;
        if mag(m1) > mag(m2) {
            b = m2;
        } else {
            a = m1;
        }
    }
    ((a + b) * 0.5) as f32
}

fn params(harmonics: f32, timbre: f32, morph: f32) -> EngineParameters {
    EngineParameters { trigger: TRIGGER_UNPATCHED, note: 69.0, timbre, morph, harmonics, accent: 0.8 }
}

fn check_pitch(name: &str, e: &mut dyn Engine, p: EngineParameters) {
    check_pitch_at(name, e, p, 440.0);
}

fn check_pitch_at(name: &str, e: &mut dyn Engine, p: EngineParameters, want: f32) {
    let (o, _) = run(e, p, 48000, 1);
    assert!(o.iter().all(|v| v.is_finite()), "{name}: not finite");
    // Skip the start-up transient (the wavetable differentiator starts from 0).
    let peak = o[1200..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.05 && peak < 2.0, "{name}: peak {peak}");
    let f = peak_hz(&o[48000 - 16384..], SAMPLE_RATE, 300.0, 600.0);
    let cents = 1200.0 * (f / want).log2();
    assert!(cents.abs() < 2.0, "{name}: {f} Hz ({cents} cents)");
}

#[test]
fn pitched_models_play_440() {
    check_pitch("va", &mut VirtualAnalogEngine::new(), params(0.5, 0.5, 0.5));
    check_pitch("waveshaping", &mut WaveshapingEngine::new(), params(0.5, 0.5, 0.5));
    // Harmonics 0.47 = ratio 2 exactly (0.5 is the detuned 2.019 ratio,
    // whose lower sideband at 448 Hz is the strongest partial).
    check_pitch("fm", &mut FmEngine::new(), params(0.47, 0.5, 0.5));
    check_pitch("wavetable", &mut WavetableEngine::new(), params(0.0, 0.0, 0.0));
    // Octave chord, inversion 8.2 (root voice untransposed), divide-down
    // voices only. The model detunes its divide-down voices by x0.998
    // (-3.5 cents) by design.
    check_pitch_at("chord", &mut ChordEngine::new(), params(0.0, 0.41, 0.3), 440.0 * 0.998);
}

#[test]
fn swarm_renders_and_is_deterministic() {
    let (a, aa) = run(&mut SwarmEngine::new(), params(0.5, 0.5, 0.5), 24000, 9);
    let (b, _) = run(&mut SwarmEngine::new(), params(0.5, 0.5, 0.5), 24000, 9);
    assert_eq!(a, b);
    assert!(a.iter().chain(aa.iter()).all(|v| v.is_finite()));
    let rms = (a.iter().map(|v| v * v).sum::<f32>() / a.len() as f32).sqrt();
    assert!(rms > 0.01 && rms < 2.0, "swarm rms {rms}");
}

#[test]
fn models_stay_finite_over_parameter_sweep() {
    let mut engines: Vec<(&str, Box<dyn Engine>)> = vec![
        ("va", Box::new(VirtualAnalogEngine::new())),
        ("waveshaping", Box::new(WaveshapingEngine::new())),
        ("fm", Box::new(FmEngine::new())),
        ("wavetable", Box::new(WavetableEngine::new())),
        ("chord", Box::new(ChordEngine::new())),
        ("swarm", Box::new(SwarmEngine::new())),
    ];
    for (name, e) in engines.iter_mut() {
        for &h in &[0.0, 0.3, 0.7, 1.0] {
            for &t in &[0.0, 0.5, 1.0] {
                for &m in &[0.0, 0.5, 1.0] {
                    for &note in &[24.0, 69.0, 110.0] {
                        let p = EngineParameters { note, ..params(h, t, m) };
                        let (o, a) = run(e.as_mut(), p, 2400, 3);
                        let worst = o[1200..].iter().chain(a[1200..].iter()).fold(0.0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { f32::INFINITY });
                        assert!(worst < 8.0, "{name} h{h} t{t} m{m} n{note}: {worst}");
                    }
                }
            }
        }
    }
}
