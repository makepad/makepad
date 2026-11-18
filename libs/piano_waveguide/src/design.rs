//! Design of one string loop: the dispersion allpass that gives the loop the
//! stiff string's partial series, the loss filter, and the delay that puts
//! the first partial exactly on pitch. Everything here runs at construction
//! in f64; the loop's phase is evaluated numerically, so the pitch does not
//! rest on approximations of the filters.

use std::f64::consts::TAU;

pub const MAX_SECTIONS: usize = 16;

/// The filters of a loop, shared by the strings of a key.
#[derive(Clone, Debug, Default)]
pub struct LoopFilters {
    /// Biquad allpass sections (c2 + c1 z^-1 + z^-2) / (1 + c1 z^-1 + c2 z^-2).
    pub sections: usize,
    pub c1: [f64; MAX_SECTIONS],
    pub c2: [f64; MAX_SECTIONS],
    /// Loss filter g0 / (1 - a z^-1).
    pub loss_g0: f64,
    pub loss_a: f64,
}

/// Delay line length and tuning allpass coefficient of one string.
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    pub delay: usize,
    pub eta: f64,
}

/// Angular frequency (rad/sample) of the (continuous) partial `n` of a stiff
/// string whose first partial is `f1`.
fn omega_of(n: f64, f1: f64, b: f64, fs: f64) -> f64 {
    TAU * f1 / (1.0 + b).sqrt() * n * (1.0 + b * n * n).sqrt() / fs
}

fn bisect(mut lo: f64, mut hi: f64, f: impl Fn(f64) -> f64) -> f64 {
    // f is increasing, f(lo) <= 0 <= f(hi)
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if f(mid) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Allpass order (in biquads) that carries the stretch up to partial `n`:
/// the phase the loop must add beyond a pure delay, in turns.
fn sections_needed(n: f64, b: f64) -> f64 {
    let x = b * n * n;
    n * x / (1.0 + 2.0 * x)
}

/// Dispersion allpass after Abel and Smith: the wanted phase is cut into
/// bands of one turn each, and every band gets one pole pair at its centre
/// whose bandwidth matches the band.
pub fn design_dispersion(f1: f64, b: f64, fs: f64, max_sections: usize, f_limit: f64) -> LoopFilters {
    let mut out = LoopFilters::default();
    let w_limit = TAU * f_limit.min(0.42 * fs) / fs;
    let n_limit = bisect(0.0, 4000.0, |n| omega_of(n, f1, b, fs) - w_limit);
    let wanted = sections_needed(n_limit, b);
    let sections = (wanted.ceil() as usize + 2).clamp(2, max_sections.min(MAX_SECTIONS));
    let nb = sections as f64;
    // The band ends where `sections` turns are used up (or at the limit,
    // when rounding asked for slightly more than the limit holds).
    let n_max = if sections_needed(n_limit, b) <= nb { n_limit } else { bisect(0.0, n_limit, |n| sections_needed(n, b) - nb) };
    let w_max = omega_of(n_max, f1, b, fs);
    let pure_delay = TAU * (n_max - nb) / w_max;
    let phase = |n: f64| TAU * n - pure_delay * omega_of(n, f1, b, fs);
    let at_turns = |turns: f64| bisect(0.0, n_max, |n| phase(n) - TAU * turns);
    const BETA: f64 = 0.85;
    for k in 0..sections {
        let lo = omega_of(at_turns(k as f64), f1, b, fs);
        let hi = omega_of(at_turns(k as f64 + 1.0), f1, b, fs);
        let centre = omega_of(at_turns(k as f64 + 0.5), f1, b, fs);
        let delta = 0.5 * (hi - lo);
        let eta = (1.0 - BETA * delta.cos()) / (1.0 - BETA);
        let rho = eta - (eta * eta - 1.0).max(0.0).sqrt();
        out.c1[k] = -2.0 * rho * centre.cos();
        out.c2[k] = rho * rho;
    }
    out.sections = sections;
    out
}

fn section_lag(c1: f64, c2: f64, w: f64) -> f64 {
    let (s1, k1) = w.sin_cos();
    let (s2, k2) = (2.0 * w).sin_cos();
    2.0 * w + 2.0 * (-c1 * s1 - c2 * s2).atan2(1.0 + c1 * k1 + c2 * k2)
}

/// Solve the dense system `a x = b` in place (Gauss with partial pivoting);
/// `a` is n x n, row-major. A singular pivot leaves that unknown at zero.
fn solve(a: &mut [f64], b: &mut [f64]) {
    let n = b.len();
    for col in 0..n {
        let mut pivot = col;
        for row in col + 1..n {
            if a[row * n + col].abs() > a[pivot * n + col].abs() {
                pivot = row;
            }
        }
        if a[pivot * n + col].abs() < 1e-300 {
            continue;
        }
        if pivot != col {
            for k in 0..n {
                a.swap(col * n + k, pivot * n + k);
            }
            b.swap(col, pivot);
        }
        for row in col + 1..n {
            let m = a[row * n + col] / a[col * n + col];
            if m != 0.0 {
                for k in col..n {
                    a[row * n + k] -= m * a[col * n + k];
                }
                b[row] -= m * b[col];
            }
        }
    }
    for col in (0..n).rev() {
        let d = a[col * n + col];
        if d.abs() < 1e-300 {
            b[col] = 0.0;
            continue;
        }
        let mut x = b[col];
        for k in col + 1..n {
            x -= a[col * n + k] * b[k];
        }
        b[col] = x / d;
    }
}

/// Move the allpass poles so the loop's resonances land on the stiff
/// string's partials below `f_fit`: least squares on the loop phase at each
/// partial (Levenberg-Marquardt), with the loop tuned to partial 1 at every
/// step. `filters` holds the starting design and the loss filter. Returns
/// the RMS error in cents over the fitted partials, before and after.
pub fn refine_dispersion(filters: &mut LoopFilters, f1: f64, b: f64, fs: f64, f_fit: f64) -> (f64, f64) {
    let sections = filters.sections;
    let ws: Vec<f64> = (1..=72).map(|n| TAU * string_partial(f1, b, n) / fs).take_while(|&w| w < TAU * f_fit.min(0.45 * fs) / fs).collect();
    let count = ws.len();
    if sections == 0 || count < 2 {
        return (0.0, 0.0);
    }
    let fixed: Vec<f64> = ws.iter().map(|&w| (filters.loss_a * w.sin()).atan2(1.0 - filters.loss_a * w.cos())).collect();
    // params: [theta_0, rho_0, theta_1, rho_1, ...]
    let mut params: Vec<f64> = (0..sections)
        .flat_map(|k| {
            let rho = filters.c2[k].max(0.0).sqrt();
            let theta = if rho > 1e-9 { (-filters.c1[k] / (2.0 * rho)).clamp(-1.0, 1.0).acos() } else { 1.0 };
            [theta, rho]
        })
        .collect();
    let coefs = |theta: f64, rho: f64| (-2.0 * rho * theta.cos(), rho * rho);
    let lags_of = |theta: f64, rho: f64, out: &mut [f64]| {
        let (c1, c2) = coefs(theta, rho);
        for (o, &w) in out.iter_mut().zip(&ws) {
            *o = section_lag(c1, c2, w);
        }
    };
    // Residuals for a table of per-section lags: phase error at partial n
    // over n (about its relative frequency error), partials 2..count.
    let residuals = |lags: &[f64], out: &mut [f64]| {
        let total = |n: usize| fixed[n] + (0..sections).map(|k| lags[k * count + n]).sum::<f64>();
        let need = (TAU - total(0)) / ws[0];
        let delay = (need - 0.5).floor().max(1.0);
        let theta = (need - delay).max(0.05) * ws[0];
        let eta = (0.5 * (ws[0] - theta)).sin() / (0.5 * (ws[0] + theta)).sin();
        for n in 1..count {
            out[n - 1] = (delay * ws[n] + tuning_lag(eta, ws[n]) + total(n) - TAU * (n + 1) as f64) / (n + 1) as f64;
        }
        // keep room for a delay line
        out[count - 1] = 10.0 * (3.0 - need).max(0.0);
    };
    let cost = |r: &[f64]| r.iter().map(|x| x * x).sum::<f64>();
    let cents = |r: &[f64]| 1200.0 / std::f64::consts::LN_2 / TAU * (cost(&r[..count - 1]) / (count - 1) as f64).sqrt();
    let np = 2 * sections;
    let mut lags = vec![0.0; sections * count];
    for k in 0..sections {
        lags_of(params[2 * k], params[2 * k + 1], &mut lags[k * count..(k + 1) * count]);
    }
    let mut r = vec![0.0; count];
    residuals(&lags, &mut r);
    let before = cents(&r);
    let mut best = cost(&r);
    let mut lambda = 1e-3;
    let mut jac = vec![0.0; count * np];
    let mut trial = vec![0.0; count];
    let mut saved = vec![0.0; count];
    for _ in 0..24 {
        for p in 0..np {
            let k = p / 2;
            let step = 1e-6;
            let (mut theta, mut rho) = (params[2 * k], params[2 * k + 1]);
            if p % 2 == 0 {
                theta += step;
            } else {
                rho -= step;
            }
            saved.copy_from_slice(&lags[k * count..(k + 1) * count]);
            lags_of(theta, rho, &mut lags[k * count..(k + 1) * count]);
            residuals(&lags, &mut trial);
            lags[k * count..(k + 1) * count].copy_from_slice(&saved);
            let sign = if p % 2 == 0 { 1.0 } else { -1.0 };
            for i in 0..count {
                jac[i * np + p] = sign * (trial[i] - r[i]) / step;
            }
        }
        let mut improved = false;
        for _ in 0..8 {
            let mut a = vec![0.0; np * np];
            let mut g = vec![0.0; np];
            for i in 0..count {
                for p in 0..np {
                    g[p] -= jac[i * np + p] * r[i];
                    for q in 0..np {
                        a[p * np + q] += jac[i * np + p] * jac[i * np + q];
                    }
                }
            }
            for p in 0..np {
                a[p * np + p] = a[p * np + p] * (1.0 + lambda) + 1e-12;
            }
            solve(&mut a, &mut g);
            let next: Vec<f64> = (0..np)
                .map(|p| if p % 2 == 0 { (params[p] + g[p]).clamp(1e-4, std::f64::consts::PI - 1e-4) } else { (params[p] + g[p]).clamp(0.0, 0.9995) })
                .collect();
            let mut next_lags = vec![0.0; sections * count];
            for k in 0..sections {
                lags_of(next[2 * k], next[2 * k + 1], &mut next_lags[k * count..(k + 1) * count]);
            }
            residuals(&next_lags, &mut trial);
            let c = cost(&trial);
            if c < best {
                // stop once a step gains less than a thousandth
                improved = best - c > 1e-3 * best;
                best = c;
                params = next;
                lags = next_lags;
                r.copy_from_slice(&trial);
                lambda = (lambda / 3.0).max(1e-9);
                break;
            }
            lambda *= 4.0;
        }
        if !improved {
            break;
        }
    }
    for k in 0..sections {
        let (c1, c2) = coefs(params[2 * k], params[2 * k + 1]);
        filters.c1[k] = c1;
        filters.c2[k] = c2;
    }
    (before, cents(&r))
}

/// One-pole loss for sigma(f) = b1 + b3 f^2 (1/s) on a loop of period 1/f1.
pub fn design_loss(filters: &mut LoopFilters, f1: f64, b1: f64, b3: f64, fs: f64) {
    let q = b3 * fs * fs / (TAU * TAU * f1);
    let a = if q > 1e-12 { ((4.0 * q + 1.0) - (8.0 * q + 1.0).sqrt()) / (4.0 * q) } else { 0.0 };
    let g = (-b1 / f1).exp();
    filters.loss_a = a;
    filters.loss_g0 = g * (1.0 - a);
}

/// Phase lag (rad, positive) of the lumped filters at `w` rad/sample.
pub fn filters_lag(f: &LoopFilters, w: f64) -> f64 {
    let (s1, c1) = w.sin_cos();
    let (s2, c2) = (2.0 * w).sin_cos();
    let mut lag = (f.loss_a * s1).atan2(1.0 - f.loss_a * c1);
    for k in 0..f.sections {
        let re = 1.0 + f.c1[k] * c1 + f.c2[k] * c2;
        let im = -f.c1[k] * s1 - f.c2[k] * s2;
        lag += 2.0 * w + 2.0 * im.atan2(re);
    }
    lag
}

fn tuning_lag(eta: f64, w: f64) -> f64 {
    w - 2.0 * (eta * w.sin()).atan2(1.0 + eta * w.cos())
}

/// Loop length that puts the loop's first resonance on `f1` Hz.
pub fn tune(filters: &LoopFilters, f1: f64, fs: f64) -> Tuning {
    let w = TAU * f1 / fs;
    let need = (TAU - filters_lag(filters, w)) / w;
    tune_at(filters, f1, fs, ((need - 0.5).floor().max(1.0)) as usize)
}

/// The same on a loop of `delay` whole samples, the fraction left to the
/// tuning allpass. The strings of one unison share their whole samples:
/// the allpass bends the upper partials by an amount that depends on its
/// fraction, so strings on either side of a whole sample would disagree up
/// there by several cents, far more than their mistuning, and their upper
/// partials would beat fast, a different brightness on every key.
pub fn tune_at(filters: &LoopFilters, f1: f64, fs: f64, delay: usize) -> Tuning {
    let w = TAU * f1 / fs;
    let need = (TAU - filters_lag(filters, w)) / w;
    let theta = (need - delay as f64).max(0.05) * w;
    let eta = (0.5 * (w - theta)).sin() / (0.5 * (w + theta)).sin();
    Tuning { delay, eta }
}

/// Frequency (Hz) of the loop's resonance number `n`: where the whole loop's
/// phase lag is n turns.
pub fn loop_partial(filters: &LoopFilters, tuning: Tuning, n: usize, fs: f64) -> f64 {
    let total = |w: f64| tuning.delay as f64 * w + filters_lag(filters, w) + tuning_lag(tuning.eta, w) - TAU * n as f64;
    bisect(0.0, std::f64::consts::PI, total) * fs / TAU
}

/// Gain of the loss filter at `f` Hz.
pub fn loss_gain(filters: &LoopFilters, f: f64, fs: f64) -> f64 {
    let w = TAU * f / fs;
    filters.loss_g0 / (1.0 - 2.0 * filters.loss_a * w.cos() + filters.loss_a * filters.loss_a).sqrt()
}

/// The stiff string's partial `n` for a first partial `f1`.
pub fn string_partial(f1: f64, b: f64, n: usize) -> f64 {
    let n = n as f64;
    f1 / (1.0 + b).sqrt() * n * (1.0 + b * n * n).sqrt()
}

/// Radix-2 FFT in place; the length is a power of two.
pub(crate) fn fft(re: &mut [f64], im: &mut [f64]) {
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
        let (wi, wr) = (-std::f64::consts::TAU / len as f64).sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in start..start + len / 2 {
                let (br, bi) = (re[k + len / 2], im[k + len / 2]);
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                (re[k + len / 2], im[k + len / 2]) = (re[k] - tr, im[k] - ti);
                re[k] += tr;
                im[k] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}
