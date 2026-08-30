// Reference-recording comparison tests — GENERATED, do not hand-edit the
// tables. Each row embeds the partial-amplitude ladder of one note of a
// real recorded acoustic grand (FluidR3 GM, one per octave A0..C7,
// measured from the recordings themselves) at onset / 100 ms / 300 ms,
// plus envelope and attack brackets. The test renders the SAME note from
// this model and asserts the weighted partial-ladder TREND distance and
// the envelope scalars stay inside tolerances anchored to the shipped
// instrument's achieved match. This is the gate that three earlier tuning
// passes lacked: they matched published aggregate numbers while the
// note-by-note ladder against a real instrument stayed wrong ("plucked
// wire"). If a change regresses toward that, these fail.
//
// Tolerances are the achieved distance at generation time * 1.35 + 0.75 dB:
// tight enough to catch a balance regression of a few dB, loose enough for
// benign drift.

mod common;

use common::*;
use makepad_piano_model::{Piano, PianoEvent::*};

struct RefNote {
    key: u8,
    name: &'static str,
    /// analysis window (s), ~4 periods of the fundamental
    win_s: f64,
    /// reference ladder, dB rel strongest partial, partial 1..N
    lad_on: &'static [f64],
    lad_100: &'static [f64],
    lad_300: &'static [f64],
    /// max weighted trend distance at each time (dB)
    tol: [f64; 3],
    attack_ms: (f64, f64),
    prompt: (f64, f64),
    after: (f64, f64),
    noise_hi: (f64, f64),
    hi_ratio: (f64, f64),
}

const VEL: u8 = 112;

const NOTES: &[RefNote] = &[
    RefNote { key: 21, name: "A0", win_s: 0.1469,
        lad_on: &[-32.0, 0.0, -0.7, -7.0, -13.1, -9.3, -8.2, -27.9, -21.7, -15.9, -14.4, -21.5, -20.7, -20.2, -20.4, -31.5, -22.7, -31.0, -29.8, -29.3],
        lad_100: &[-31.8, 0.0, -2.4, -5.4, -13.4, -10.4, -9.0, -28.8, -22.2, -17.4, -15.3, -22.7, -18.5, -22.2, -20.1, -32.4, -23.8, -31.2, -31.5, -31.9],
        lad_300: &[-35.4, 0.0, -6.4, -8.5, -17.2, -14.3, -14.4, -31.1, -24.9, -20.2, -17.3, -26.6, -18.2, -29.9, -23.8, -37.6, -26.4, -34.9, -37.3, -33.5],
        tol: [8.19, 8.22, 8.64],
        attack_ms: (7.0, 69.9), prompt: (-1.2, 11.7), after: (0.4, 10.3),
        noise_hi: (-33.6, -10.4), hi_ratio: (-86.2, -41.9) },
    RefNote { key: 24, name: "C1", win_s: 0.1234,
        lad_on: &[-36.4, 0.0, -0.5, -6.9, -12.8, -9.3, -8.0, -27.8, -21.3, -15.4, -14.3, -21.3, -21.6, -20.9, -20.5, -31.7, -22.9, -30.8, -30.3, -28.2],
        lad_100: &[-43.9, 0.0, -3.0, -4.8, -13.5, -11.1, -9.7, -29.1, -22.1, -17.4, -15.4, -23.2, -18.0, -23.6, -19.8, -30.6, -24.9, -32.3, -32.9, -32.1],
        lad_300: &[-49.1, 0.0, -6.4, -8.8, -16.7, -14.6, -14.4, -30.6, -24.5, -20.4, -17.3, -26.0, -15.9, -30.1, -24.0, -37.3, -26.2, -34.4, -39.0, -34.9],
        tol: [9.17, 9.59, 12.86],
        attack_ms: (6.0, 59.5), prompt: (0.0, 18.6), after: (1.4, 10.2),
        noise_hi: (-28.4, -9.6), hi_ratio: (-51.9, -30.1) },
    RefNote { key: 33, name: "A1", win_s: 0.0711,
        lad_on: &[-7.5, 0.0, -3.1, -21.2, -15.6, -9.4, -13.7, -31.4, -12.1, -15.8, -23.1, -23.4, -12.4, -19.3, -18.2, -31.0, -23.9, -22.7, -20.3, -16.9],
        lad_100: &[-7.6, -1.3, 0.0, -14.1, -16.3, -6.2, -13.9, -22.9, -9.9, -15.0, -26.5, -23.2, -10.1, -16.5, -18.5, -28.6, -21.7, -18.2, -17.4, -13.6],
        lad_300: &[-7.0, -0.2, 0.0, -12.6, -15.5, -6.4, -11.4, -25.9, -10.6, -13.4, -26.0, -24.2, -10.2, -16.4, -19.3, -28.1, -22.0, -19.6, -16.9, -14.9],
        tol: [4.62, 6.81, 6.53],
        attack_ms: (4.4, 44.4), prompt: (2.5, 11.9), after: (2.4, 10.6),
        noise_hi: (-25.3, -9.8), hi_ratio: (-51.0, -24.5) },
    RefNote { key: 36, name: "C2", win_s: 0.0617,
        lad_on: &[0.0, -5.5, -11.9, -11.5, -7.2, -20.1, -10.7, -20.8, -15.8, -14.8, -9.4, -22.2, -19.6, -17.9, -15.6, -23.1, -27.4, -17.3, -20.8, -22.9],
        lad_100: &[0.0, -14.5, -16.6, -12.5, -11.9, -25.6, -14.1, -24.8, -17.2, -14.8, -10.5, -22.1, -18.2, -18.2, -20.3, -24.2, -32.8, -15.7, -24.2, -26.9],
        lad_300: &[0.0, -7.5, -17.1, -14.5, -13.3, -24.6, -16.5, -23.9, -16.6, -17.0, -12.0, -23.6, -23.8, -18.8, -22.7, -24.2, -34.5, -18.9, -22.3, -24.2],
        tol: [7.00, 6.56, 7.12],
        attack_ms: (2.8, 27.8), prompt: (-1.9, 17.5), after: (2.4, 10.9),
        noise_hi: (-26.8, -9.8), hi_ratio: (-42.1, -19.7) },
    RefNote { key: 45, name: "A2", win_s: 0.0460,
        lad_on: &[0.0, -14.8, -7.3, -19.2, -23.0, -19.4, -25.4, -35.1, -29.8, -19.7, -17.7, -24.4, -20.6, -17.5, -19.1, -29.1, -26.6, -20.6, -25.4, -28.7],
        lad_100: &[0.0, -8.9, -10.7, -17.8, -27.2, -22.2, -32.1, -38.0, -27.3, -27.2, -19.7, -25.0, -30.1, -22.3, -21.8, -30.9, -30.2, -23.5, -38.7, -29.9],
        lad_300: &[0.0, -11.7, -9.9, -19.5, -23.8, -23.3, -34.9, -40.5, -27.9, -26.1, -18.5, -22.5, -26.9, -24.4, -21.2, -37.5, -31.7, -22.6, -33.3, -26.2],
        tol: [5.91, 7.51, 6.10],
        attack_ms: (4.7, 50.5), prompt: (-2.6, 20.2), after: (1.5, 12.0),
        noise_hi: (-20.9, -6.9), hi_ratio: (-43.6, -25.9) },
    RefNote { key: 48, name: "C3", win_s: 0.0460,
        lad_on: &[-1.9, 0.0, -11.5, -14.6, -16.2, -16.4, -17.3, -21.4, -18.3, -10.1, -34.6, -20.8, -20.1, -18.9, -16.3, -20.5, -41.5, -18.1, -19.8, -25.5],
        lad_100: &[-1.3, 0.0, -10.3, -9.8, -16.9, -16.9, -16.2, -21.4, -19.5, -7.8, -27.0, -16.7, -17.8, -20.3, -17.1, -28.9, -33.8, -20.7, -20.9, -22.1],
        lad_300: &[-2.3, 0.0, -11.4, -11.7, -18.5, -18.3, -18.4, -29.6, -19.7, -9.6, -24.3, -19.0, -19.1, -25.2, -18.9, -31.1, -34.0, -25.7, -22.8, -22.4],
        tol: [5.67, 5.78, 5.51],
        attack_ms: (3.7, 36.8), prompt: (4.7, 15.0), after: (2.9, 12.9),
        noise_hi: (-23.5, -5.2), hi_ratio: (-36.9, -21.2) },
    RefNote { key: 57, name: "A3", win_s: 0.0460,
        lad_on: &[0.0, -14.5, -17.2, -24.1, -15.3, -26.2, -17.4, -17.6, -15.6, -11.9, -15.5, -17.9, -14.3, -15.0, -12.5, -24.7, -24.5, -25.9, -18.3, -25.1],
        lad_100: &[0.0, -7.5, -19.2, -23.9, -20.5, -30.7, -15.2, -20.5, -17.5, -13.5, -17.6, -20.9, -14.6, -24.1, -16.8, -27.4, -27.4, -32.8, -28.1, -29.0],
        lad_300: &[0.0, -6.4, -20.3, -21.9, -19.9, -29.6, -13.9, -22.8, -19.3, -17.3, -18.0, -25.4, -17.8, -31.0, -28.6, -47.9, -46.7, -40.3, -32.1, -38.7],
        tol: [6.14, 7.78, 8.89],
        attack_ms: (2.3, 22.6), prompt: (6.2, 19.9), after: (3.5, 15.8),
        noise_hi: (-15.2, 1.5), hi_ratio: (-35.7, -20.5) },
    RefNote { key: 60, name: "C4", win_s: 0.0460,
        lad_on: &[0.0, -8.0, -16.9, -14.9, -15.7, -16.4, -11.2, -14.5, -10.6, -19.4, -11.1, -16.4, -21.9, -16.7, -13.1, -13.1, -23.7, -27.8, -26.1, -32.9],
        lad_100: &[0.0, -6.7, -18.9, -17.0, -21.4, -20.9, -17.7, -15.7, -20.2, -26.0, -14.6, -22.4, -31.4, -28.4, -17.5, -17.5, -21.0, -35.5, -28.8, -39.1],
        lad_300: &[0.0, -7.9, -19.8, -15.8, -25.3, -21.8, -19.5, -20.0, -32.7, -43.0, -25.1, -28.5, -41.0, -30.8, -22.6, -22.6, -40.3, -38.0, -36.0, -36.9],
        tol: [5.81, 6.37, 8.80],
        attack_ms: (3.8, 52.0), prompt: (5.9, 18.9), after: (3.9, 17.7),
        noise_hi: (-14.8, -0.2), hi_ratio: (-37.6, -22.9) },
    RefNote { key: 69, name: "A4", win_s: 0.0460,
        lad_on: &[0.0, -21.9, -23.9, -10.4, -16.5, -11.9, -19.2, -17.9, -27.7, -32.2, -27.4, -26.1, -28.1, -40.3, -28.4, -32.7, -35.0, -56.2, -60.0, -60.0],
        lad_100: &[0.0, -20.3, -27.7, -22.1, -19.1, -13.0, -25.6, -25.6, -20.7, -28.6, -33.2, -43.0, -43.2, -40.1, -38.4, -39.5, -42.7, -60.0, -60.0, -60.0],
        lad_300: &[0.0, -22.8, -20.7, -19.9, -27.9, -21.9, -29.2, -34.8, -37.0, -39.4, -48.0, -33.7, -60.0, -51.6, -49.2, -48.3, -51.0, -60.0, -60.0, -60.0],
        tol: [4.78, 7.23, 7.58],
        attack_ms: (4.5, 45.2), prompt: (8.8, 28.3), after: (2.2, 17.0),
        noise_hi: (-19.8, -4.7), hi_ratio: (-36.9, -14.0) },
    RefNote { key: 72, name: "C5", win_s: 0.0460,
        lad_on: &[-3.0, -11.2, 0.0, -14.7, -4.8, -12.2, -14.9, -15.8, -10.4, -22.5, -39.3, -21.5, -31.3, -23.2, -24.0, -60.0, -60.0, -50.7, -60.0],
        lad_100: &[0.0, -7.8, -3.9, -11.8, -8.9, -18.7, -24.3, -19.7, -13.2, -28.3, -45.5, -24.7, -36.0, -35.2, -34.4, -60.0, -60.0, -60.0, -60.0],
        lad_300: &[0.0, -10.2, -7.9, -20.0, -9.8, -15.7, -29.3, -20.2, -18.4, -26.5, -55.8, -39.5, -43.9, -50.2, -60.0, -60.0, -60.0, -60.0, -60.0],
        tol: [2.81, 3.05, 4.57],
        attack_ms: (6.1, 60.6), prompt: (1.7, 31.3), after: (4.8, 21.8),
        noise_hi: (-14.1, 0.6), hi_ratio: (-30.1, -15.6) },
    RefNote { key: 81, name: "A5", win_s: 0.0460,
        lad_on: &[0.0, -10.8, -11.7, -10.4, -13.3, -16.6, -19.9, -37.2, -45.3, -36.9, -60.0],
        lad_100: &[0.0, -4.1, -11.9, -11.2, -17.1, -15.1, -23.9, -39.1, -47.9, -38.6, -37.1],
        lad_300: &[-1.4, 0.0, -10.7, -0.7, -13.2, -9.8, -19.4, -44.6, -50.6, -39.6, -60.0],
        tol: [6.58, 5.01, 12.95],
        attack_ms: (3.7, 37.0), prompt: (14.5, 46.3), after: (6.5, 29.2),
        noise_hi: (-18.4, -2.0), hi_ratio: (-29.8, -7.5) },
    RefNote { key: 84, name: "C6", win_s: 0.0460,
        lad_on: &[0.0, -9.6, -14.2, -11.2, -14.4, -19.0, -26.4, -54.4, -48.0],
        lad_100: &[0.0, -5.3, -10.7, -15.4, -16.0, -14.6, -22.3, -58.4, -56.1],
        lad_300: &[-2.4, 0.0, -19.1, -4.1, -15.7, -17.1, -37.9, -57.5, -59.8],
        tol: [4.85, 8.83, 13.93],
        attack_ms: (1.3, 13.0), prompt: (15.1, 48.4), after: (5.4, 36.3),
        noise_hi: (-20.6, -0.9), hi_ratio: (-29.0, -5.5) },
    RefNote { key: 93, name: "A6", win_s: 0.0460,
        lad_on: &[0.0, -11.0, -29.0, -40.9, -41.2],
        lad_100: &[0.0, -29.9, -45.2, -47.5, -46.6],
        lad_300: &[0.0, -15.1, -34.8, -44.0, -53.5],
        tol: [3.08, 13.28, 6.30],
        attack_ms: (1.6, 17.2), prompt: (24.7, 79.0), after: (-3.0, 6.4),
        noise_hi: (-11.9, 16.5), hi_ratio: (-15.9, -1.5) },
    RefNote { key: 96, name: "C7", win_s: 0.0460,
        lad_on: &[0.0, -7.7, -29.6, -43.3],
        lad_100: &[0.0, -16.4, -20.5, -31.9],
        lad_300: &[0.0, -24.0, -35.2, -60.0],
        tol: [5.99, 2.64, 2.00],
        attack_ms: (2.5, 24.9), prompt: (9.2, 112.2), after: (-3.0, 8.3),
        noise_hi: (-7.1, 16.2), hi_ratio: (-14.6, 0.3) },

];

fn wgt(rel_db: f64) -> f64 {
    ((rel_db + 50.0) / 50.0).clamp(0.0, 1.0).powi(2)
}

fn smooth(v: &[f64]) -> Vec<f64> {
    (0..v.len())
        .map(|j| {
            let a = j.saturating_sub(2);
            let b = (j + 3).min(v.len());
            v[a..b].iter().sum::<f64>() / (b - a) as f64
        })
        .collect()
}

/// Weighted L1 distance between the +-2-partial moving-average trends of
/// two rel-dB ladders (the objective's ladder-trend metric).
fn trend_dist(rl: &[f64], ml: &[f64]) -> f64 {
    let rt = smooth(rl);
    let mt = smooth(ml);
    let mut num = 0.0;
    let mut den = 0.0;
    for j in 0..rl.len() {
        let w = wgt(rl[j]);
        num += w * (rt[j] - mt[j]).abs().min(30.0);
        den += w;
    }
    if den > 0.0 { num / den } else { 0.0 }
}

/// Model ladder at the same partial numbers: peak magnitudes near the
/// key's own predicted partial frequencies (its stretched f0 and B), in dB
/// rel the strongest, floored at -60.
fn model_ladder(m: &[f32], f0: f64, b: f64, np: usize, center_s: f64, win_s: f64) -> Vec<f64> {
    let w = (win_s * FS as f64) as usize;
    let c = (center_s * FS as f64) as usize;
    let a = c.saturating_sub(w / 2).max((0.010 * FS as f64) as usize);
    let bnd = (a + w).min(m.len());
    let seg = &m[a..bnd];
    let mut out = Vec::new();
    for n in 1..=np {
        let nf = n as f64;
        let guess = nf * f0 * (1.0 + b * nf * nf).sqrt();
        if guess > 0.47 * FS as f64 {
            out.push(-60.0);
            continue;
        }
        let (_, mag) = peak_near(seg, guess, (f0 * 0.35).max(3.0));
        out.push(20.0 * mag.max(1e-12).log10());
    }
    let mx = out.iter().cloned().fold(-999.0, f64::max);
    out.iter().map(|v| (v - mx).max(-60.0)).collect()
}

fn render_note(key: u8, secs: f64) -> Vec<f32> {
    let mut p = dry_piano();
    let total = (secs * FS as f64) as usize;
    let (l, r) = render(&mut p, &[ev(0.010, NoteOn { key, velocity: VEL })], total, 256);
    mono(&l, &r)
}

#[test]
fn partial_ladders_match_the_reference_recordings() {
    let mut failures = Vec::new();
    for rn in NOTES {
        let m = render_note(rn.key, 0.7);
        let p = Piano::new(FS);
        let ki = p.key_info(rn.key).unwrap();
        let (f0, b) = (ki.f0 as f64, ki.b_coeff as f64);
        for (ti, (lad, t_c)) in [
            (rn.lad_on, 0.010 + 0.5 * rn.win_s),
            (rn.lad_100, 0.110),
            (rn.lad_300, 0.310),
        ]
        .iter()
        .enumerate()
        {
            let ml = model_ladder(&m, f0, b, lad.len(), *t_c, rn.win_s);
            let d = trend_dist(lad, &ml);
            if d > rn.tol[ti] {
                failures.push(format!(
                    "{} ladder@t{}: trend distance {:.2} dB > tol {:.2}",
                    rn.name, ti, d, rn.tol[ti]
                ));
            }
        }
    }
    assert!(failures.is_empty(), "ladder regressions vs reference recordings:\n{}", failures.join("\n"));
}

#[test]
fn envelopes_and_attack_match_the_reference_recordings() {
    let mut failures = Vec::new();
    for rn in NOTES {
        let m = render_note(rn.key, 3.0);
        // 1 ms envelope attack time
        let sm = (0.001 * FS as f64) as usize;
        let mut env = vec![0.0f64; m.len()];
        let mut acc = 0.0f64;
        for k in 0..m.len() {
            acc += m[k].abs() as f64;
            if k >= sm {
                acc -= m[k - sm].abs() as f64;
            }
            env[k] = acc;
        }
        let onset = (0.010 * FS as f64) as usize;
        let horizon = ((0.5 * FS as f64) as usize).min(env.len());
        let peak = env[..horizon].iter().cloned().fold(0.0f64, f64::max);
        let thr = peak * 0.708;
        let attack = env[..horizon]
            .iter()
            .position(|&v| v >= thr)
            .map(|i| (i as f64 - onset as f64) / FS as f64 * 1000.0)
            .unwrap_or(999.0);
        if attack < rn.attack_ms.0 || attack > rn.attack_ms.1 {
            failures.push(format!("{} attack {:.1} ms outside ({:.1}, {:.1})", rn.name, attack, rn.attack_ms.0, rn.attack_ms.1));
        }
        // 20 ms RMS envelope in dB (floored at -72 like the references),
        // prompt/after slopes
        let ew = (0.02 * FS as f64) as usize;
        let hop = (0.01 * FS as f64) as usize;
        let mut edb = Vec::new();
        let mut a = onset;
        while a + ew <= m.len() {
            let r = (m[a..a + ew].iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / ew as f64).sqrt();
            edb.push((20.0 * r.max(1e-12).log10()).max(-72.0));
            a += hop;
        }
        let fit = |t0: f64, t1: f64| -> f64 {
            let i0 = (t0 / 0.01) as usize;
            let i1 = ((t1 / 0.01) as usize).min(edb.len());
            let ts: Vec<f64> = (i0..i1).map(|i| i as f64 * 0.01).collect();
            let ys: Vec<f64> = edb[i0..i1].to_vec();
            -linreg_slope(&ts, &ys).unwrap_or(0.0)
        };
        let prompt = fit(0.05, 0.8);
        let after = fit(1.0, 2.5);
        if prompt < rn.prompt.0 || prompt > rn.prompt.1 {
            failures.push(format!("{} prompt {:.1} dB/s outside ({:.1}, {:.1})", rn.name, prompt, rn.prompt.0, rn.prompt.1));
        }
        if after < rn.after.0 || after > rn.after.1 {
            failures.push(format!("{} after {:.1} dB/s outside ({:.1}, {:.1})", rn.name, after, rn.after.0, rn.after.1));
        }
        // attack noise and sustained high-band content
        let aw = ((0.03 * FS as f64) as usize).min(m.len() - onset);
        let (bin, ps) = power_spectrum(&m[onset..onset + aw]);
        let noise_hi = 10.0 * (band_power(bin, &ps, 2800.0, 9000.0).max(1e-18) / band_power(bin, &ps, 90.0, 1200.0).max(1e-18)).log10();
        if noise_hi < rn.noise_hi.0 || noise_hi > rn.noise_hi.1 {
            failures.push(format!("{} noise_hi {:.1} dB outside ({:.1}, {:.1})", rn.name, noise_hi, rn.noise_hi.0, rn.noise_hi.1));
        }
        let mw = ((0.3 * FS as f64) as usize).min(m.len() - onset);
        let (bin2, ps2) = power_spectrum(&m[onset..onset + mw]);
        let hi_ratio = 10.0 * (band_power(bin2, &ps2, 5000.0, 9000.0).max(1e-18) / band_power(bin2, &ps2, 100.0, 1500.0).max(1e-18)).log10();
        if hi_ratio < rn.hi_ratio.0 || hi_ratio > rn.hi_ratio.1 {
            failures.push(format!("{} hi_ratio {:.1} dB outside ({:.1}, {:.1})", rn.name, hi_ratio, rn.hi_ratio.0, rn.hi_ratio.1));
        }
    }
    assert!(failures.is_empty(), "envelope regressions vs reference recordings:\n{}", failures.join("\n"));
}
