//! Adapter-level checks of engines 9..15 (noise, particle, string, modal,
//! kick, snare, hi-hat).

use super::super::OscBlock;
use super::{default_slots, Twist};
use crate::kit::core::SlotValue;

const FS_OS: f32 = 96000.0;

fn render(engine: i32, seed: u32, blocks: usize, lpg: bool, harmonics: f32) -> Vec<f32> {
    let mut slots = default_slots();
    slots[0] = SlotValue::int(engine);
    slots[1] = SlotValue::float(harmonics);
    if lpg {
        slots[5].deactivated = false;
        slots[6] = SlotValue::float(0.8);
    }
    let fm = [0.0f32; 64];
    let b = OscBlock {
        pitch: 69.0,
        drift: 0.0,
        stereo: true,
        fm: false,
        fmdepth: 0.0,
        fm_buf: &fm,
        slots: &slots,
        retrigger: true,
        character: 1,
        gate: true,
        table: None,
        extra: &[],
        audio_in: None,
        fs_os: FS_OS,
        tsr: 1.0,
    };
    let mut t = Twist::new(seed);
    t.init(&b, false);
    let mut out = Vec::with_capacity(blocks * 64);
    let mut l = [0.0; 64];
    let mut r = [0.0; 64];
    for _ in 0..blocks {
        t.process(&b, &mut l, &mut r);
        out.extend_from_slice(&l);
    }
    out
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

/// Magnitude of the Hann-windowed DFT of `x` at `f` Hz.
fn mag(x: &[f32], fs: f32, f: f64) -> f64 {
    let n = x.len();
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
        let a = 2.0 * std::f64::consts::PI * f * i as f64 / fs as f64;
        re += v as f64 * w * a.cos();
        im += v as f64 * w * a.sin();
    }
    (re * re + im * im).sqrt()
}

fn peak_hz(x: &[f32], fs: f32, lo: f32, hi: f32) -> f32 {
    let df = fs as f64 / x.len() as f64;
    let mut best = (0.0, lo as f64);
    let mut f = lo as f64;
    while f <= hi as f64 {
        let m = mag(x, fs, f);
        if m > best.0 {
            best = (m, f);
        }
        f += df / 4.0;
    }
    let mut step = df / 8.0;
    let mut c = best.1;
    for _ in 0..30 {
        let (a, m0, b) = (mag(x, fs, c - step), mag(x, fs, c), mag(x, fs, c + step));
        if a > m0 {
            c -= step;
        } else if b > m0 {
            c += step;
        } else {
            step *= 0.5;
        }
    }
    c as f32
}

/// Particles at the default density are sparse (about a dozen impulses per
/// second), so the particle model gets a 2 s window.
#[test]
fn noise_and_particles_free_running() {
    for (e, blocks) in [(9, 200), (10, 3000)] {
        let x = render(e, 5, blocks, false, 0.0);
        assert!(x.iter().all(|v| v.is_finite()), "engine {e}");
        let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= 1.01, "engine {e} peak {peak}");
        assert!(rms(&x) > 1e-3, "engine {e} rms {}", rms(&x));
    }
}

#[test]
fn struck_models_ring_and_stay_bounded() {
    for e in 11..16 {
        for lpg in [false, true] {
            let x = render(e, 9, 300, lpg, 0.0);
            assert!(x.iter().all(|v| v.is_finite()), "engine {e} lpg {lpg}");
            // The free-running string (dust exciter into the limiter) can hit
            // the 16-bit clip for a few samples; the Lanczos resampler then
            // overshoots full scale slightly.
            let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak <= 1.05, "engine {e} lpg {lpg} peak {peak}");
            assert!(rms(&x) > 1e-4, "engine {e} lpg {lpg} rms {}", rms(&x));
        }
    }
}

/// At harmonics (structure) 0.5 the string is in its dispersion range and
/// the modal resonator stretches its partials, both by design; at structure
/// 0.25 (slot value -0.5) both models are harmonic. The modal resonator then
/// sits within 2 cents of 440 Hz. The string runs 3..7 cents sharp, rising
/// with brightness: the model's own tuning error (its DC blocker's phase
/// lead is about 2 cents at 440 Hz, the damping filter's delay compensation
/// is approximate), so it gets an 8-cent bound.
#[test]
fn string_and_modal_fundamental() {
    for (e, tol) in [(11, 8.0f32), (12, 2.0)] {
        let x = render(e, 13, 400, true, -0.5);
        let tail = &x[x.len() - 8192..];
        let f = peak_hz(tail, FS_OS, 300.0, 600.0);
        let c = 1200.0 * (f / 440.0).log2();
        assert!(c.abs() < tol, "engine {e}: {f} Hz ({c} cents)");
    }
}

#[test]
fn deterministic_for_seed() {
    for e in 9..16 {
        assert_eq!(render(e, 77, 40, true, 0.3), render(e, 77, 40, true, 0.3), "engine {e}");
    }
}

