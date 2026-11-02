//! Deriving the hybrid table and proving it did what it claims: used by the
//! `generate_hybrid_targets` example and by the tests in `hybrid.rs`.

use super::*;
use makepad_piano_model::{Instrument, PianoEvent, TimedEvent};

pub(super) const FS: f32 = 48_000.0;

pub(super) fn render<I: Instrument>(p: &mut I, key: u8, vel: u8, secs: f64) -> Vec<f32> {
    let total = (secs * FS as f64) as usize;
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let (mut pos, mut started) = (0usize, false);
    while pos < total {
        let n = 512.min(total - pos);
        let ev = [TimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity: vel } }];
        let events: &[TimedEvent] = if !started { &ev } else { &[] };
        started = true;
        p.process(events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect()
}

fn dft_mag(x: &[f32], f: f64) -> f64 {
    let n = x.len();
    let (mut re, mut im, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
    let w0 = std::f64::consts::TAU * f / FS as f64;
    for (k, &v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / n as f64).cos();
        wsum += w;
        let ph = w0 * k as f64;
        re += w * v as f64 * ph.cos();
        im -= w * v as f64 * ph.sin();
    }
    2.0 * (re * re + im * im).sqrt() / wsum
}

fn peak_near(x: &[f32], guess: f64, half: f64) -> (f64, f64) {
    let step = (half / 30.0).max(0.05);
    let (mut best_f, mut best_m) = (guess, -1.0f64);
    let mut f = guess - half;
    while f <= guess + half {
        if f > 5.0 {
            let m = dft_mag(x, f);
            if m > best_m {
                best_m = m;
                best_f = f;
            }
        }
        f += step;
    }
    (best_f, best_m)
}

fn decay_sigma(x: &[f32], f: f64, t0: f64, t1: f64) -> f64 {
    let win = (0.080 * FS as f64) as usize;
    let hop = win / 2;
    let (mut ts, mut ms) = (Vec::new(), Vec::new());
    let mut a = (t0 * FS as f64) as usize;
    let end = ((t1 * FS as f64) as usize).min(x.len());
    while a + win <= end {
        let m = dft_mag(&x[a..a + win], f);
        if m > 1e-12 {
            ts.push((a + win / 2) as f64 / FS as f64);
            ms.push(m.ln());
        }
        a += hop;
    }
    if ts.len() < 3 {
        return 0.0;
    }
    let n = ts.len() as f64;
    let (sx, sy): (f64, f64) = (ts.iter().sum(), ms.iter().sum());
    let sxx: f64 = ts.iter().map(|v| v * v).sum();
    let sxy: f64 = ts.iter().zip(&ms).map(|(a, b)| a * b).sum();
    let d = n * sxx - sx * sx;
    if d.abs() < 1e-12 {
        return 0.0;
    }
    -((n * sxy - sx * sy) / d)
}

pub(super) struct Ladder {
    pub t100: Vec<f64>,
    /// Read by the `generate_hybrid_targets` example, which shares this file; the tests read only `t100`.
    #[allow(dead_code)]
    pub sigma: Vec<f64>,
}

pub(super) fn measure(x: &[f32], f0: f64, n_partials: usize, want_sigma: bool) -> Ladder {
    let win_s = (4.0 / f0).clamp(0.02, 0.15);
    let win = (win_s * FS as f64) as usize;
    let slice = |t0: f64| -> &[f32] {
        let a = ((t0 * FS as f64) as usize).min(x.len().saturating_sub(win));
        &x[a..(a + win).min(x.len())]
    };
    let onset = slice(0.5 * win_s);
    let mut freqs = Vec::new();
    for n in 1..=n_partials {
        let half = (0.45 * f0).max(6.0);
        freqs.push(peak_near(onset, n as f64 * f0, half).0);
    }
    let s = slice(0.1);
    let raw: Vec<f64> = freqs.iter().map(|&f| dft_mag(s, f)).collect();
    let m = raw.iter().cloned().fold(1e-30, f64::max);
    let t100: Vec<f64> = raw.iter().map(|&x| 20.0 * (x / m).max(1e-9).log10()).collect();
    let sigma = if want_sigma {
        freqs.iter().map(|&f| decay_sigma(x, f, 0.1, 1.0)).collect()
    } else {
        vec![0.0; freqs.len()]
    };
    Ladder { t100, sigma }
}

pub(super) fn n_partials_for(f0: f64) -> usize {
    ((0.42 * FS as f64 / f0) as usize).clamp(3, PARTIALS)
}

