//! TESTS.md B6: every type's small-signal response at cutoff 0 (440 Hz),
//! resonance 0 / 0.5 / 0.9, fs 96 kHz, against the closed form its
//! coefficient formulas imply (linear types), or measurable small-signal
//! properties (nonlinear ones); stability on a -60..70 sweep at reso 1.

use super::approx::{kernel_tap, tan_r};
use super::*;
use std::f64::consts::PI;

const FS: f32 = 96000.0;
/// Impulse size: small enough that every saturator and clip register is in
/// its linear region even at a resonant peak.
const AMP: f32 = 1e-7;
/// Small-signal level of the nonlinear types (-60 dBFS).
const AMP_NL: f32 = 1e-3;
const LEN: usize = 64 * 512;

#[derive(Clone, Copy, Debug)]
struct C(f64, f64);

impl C {
    fn ei(w: f64) -> C {
        C(w.cos(), w.sin())
    }
    fn add(self, o: C) -> C {
        C(self.0 + o.0, self.1 + o.1)
    }
    fn sub(self, o: C) -> C {
        C(self.0 - o.0, self.1 - o.1)
    }
    fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn scale(self, k: f64) -> C {
        C(self.0 * k, self.1 * k)
    }
    fn div(self, o: C) -> C {
        let d = o.0 * o.0 + o.1 * o.1;
        C((self.0 * o.0 + self.1 * o.1) / d, (self.1 * o.0 - self.0 * o.1) / d)
    }
    fn abs(self) -> f64 {
        (self.0 * self.0 + self.1 * self.1).sqrt()
    }
    fn powi(self, n: u32) -> C {
        (0..n).fold(C(1.0, 0.0), |a, _| a.mul(self))
    }
}

const ONE: C = C(1.0, 0.0);

/// The impulse response (per unit input) after one block of silence.
fn impulse(ty: u8, sub: u8, reso: f32) -> Vec<f32> {
    impulse_at(ty, sub, reso, AMP, LEN)
}

fn impulse_at(ty: u8, sub: u8, reso: f32, amp: f32, len: usize) -> Vec<f32> {
    let mut u = FilterUnit::new();
    let mut h = Vec::with_capacity(len);
    let mut n = 0usize;
    for blk in 0..len / 64 + 1 {
        u.set_block(ty, sub, 0.0, reso, FS, true);
        for _ in 0..64 {
            let x = if n == 64 { amp } else { 0.0 };
            let y = u.tick(x);
            if blk > 0 {
                h.push(y / amp);
            }
            n += 1;
        }
    }
    h
}

fn dft(h: &[f32], w: f64) -> C {
    let mut acc = C(0.0, 0.0);
    for (n, v) in h.iter().enumerate() {
        acc = acc.add(C::ei(-w * n as f64).scale(*v as f64));
    }
    acc
}

fn test_freqs() -> Vec<f64> {
    let mut f = vec![];
    let mut x = 20.0;
    while x < 0.4 * FS as f64 {
        f.push(x);
        x *= 1.37;
    }
    f.push(440.0);
    f
}

fn check(what: &str, h: &[f32], want: impl Fn(C) -> C) {
    check_tol(what, h, want, 1e-4)
}

/// Pass within 0.1 dB, or within `abs` absolute (deep stopbands / nulls),
/// or within -120 dB of the response's peak (f32 state rounding of very
/// sharp cascades).
fn check_tol(what: &str, h: &[f32], want: impl Fn(C) -> C, abs: f64) {
    let peak = test_freqs().iter().map(|f| want(C::ei(-2.0 * PI * f / FS as f64)).abs()).fold(0.0, f64::max);
    let abs = abs.max(1e-6 * peak);
    for f in test_freqs() {
        let w = 2.0 * PI * f / FS as f64;
        let m = dft(h, w).abs();
        let e = want(C::ei(-w)).abs();
        let db = 20.0 * (m / e).log10();
        assert!(db.abs() < 0.1 || (m - e).abs() < abs, "{what} at {f:.0} Hz: measured {m:.6} want {e:.6} ({db:.3} dB)");
    }
}

/// H of a normalised biquad at zi = z^-1.
fn bq(c: [f32; 5], zi: C) -> C {
    let [b0, b1, b2, a1, a2] = c.map(|v| v as f64);
    let num = C(b0, 0.0).add(zi.scale(b1)).add(zi.mul(zi).scale(b2));
    let den = ONE.add(zi.scale(a1)).add(zi.mul(zi).scale(a2));
    num.div(den)
}

/// The analog variable of a bilinear map with prewarped g.
fn s_of(zi: C, g: f64) -> C {
    ONE.sub(zi).div(ONE.add(zi)).scale(1.0 / g)
}

const RESOS: [f32; 3] = [0.0, 0.5, 0.9];

/// A dense grid around the 440 Hz cutoff (rad/sample).
fn dense_w(i: usize) -> f64 {
    2.0 * PI * 440.0 * (0.5 + i as f64 / 2000.0) / FS as f64
}

#[test]
fn driven_clean_notch_allpass_are_their_biquads() {
    for ty in [1u8, 2, 4, 5, 6, 23, 7, 24, 26] {
        let subs: &[u8] = match ty {
            7 | 24 => &[0, 1],
            26 => &[0],
            _ => &[1, 2],
        };
        for &sub in subs {
            for r in RESOS {
                let (c, _, _) = biquad::recipe(ty, sub, 0.0, r, FS);
                if c[4] >= 0.999 {
                    continue; // self-oscillating: no linear steady state
                }
                let two = matches!(ty, 2 | 5 | 23 | 24);
                let h = impulse(ty, sub, r);
                // A narrow notch's null depth is f32-conditioned: allow -40 dB there.
                let abs = if matches!(ty, 7 | 24) { 1e-2 } else { 1e-4 };
                check_tol(&format!("type {ty} sub {sub} r {r}"), &h, |zi| bq(c, zi).powi(if two { 2 } else { 1 }), abs);
            }
        }
    }
}

/// A linear system probed from a transcription of the update: H(z).
fn probe(n: usize, f: impl Fn(&[f64], f64) -> (Vec<f64>, f64)) -> impl Fn(C) -> C {
    let zero = vec![0.0; n];
    let (b, d) = f(&zero, 1.0);
    let mut a = vec![vec![0.0; n]; n];
    let mut c = vec![0.0; n];
    for j in 0..n {
        let mut e = zero.clone();
        e[j] = 1.0;
        let (s, y) = f(&e, 0.0);
        for i in 0..n {
            a[i][j] = s[i];
        }
        c[j] = y;
    }
    move |zi: C| {
        // (zI - A) x = B, z = 1/zi.
        let z = ONE.div(zi);
        let mut m: Vec<Vec<C>> = (0..n)
            .map(|i| {
                let mut row: Vec<C> = (0..n).map(|j| C(-a[i][j], 0.0)).collect();
                row[i] = row[i].add(z);
                row.push(C(b[i], 0.0));
                row
            })
            .collect();
        for k in 0..n {
            let p = (k..n).max_by(|&x, &y| m[x][k].abs().partial_cmp(&m[y][k].abs()).unwrap()).unwrap();
            m.swap(k, p);
            for i in 0..n {
                if i != k {
                    let r = m[i][k].div(m[k][k]);
                    for j in k..=n {
                        let v = m[k][j].mul(r);
                        m[i][j] = m[i][j].sub(v);
                    }
                }
            }
        }
        let mut y = C(d, 0.0);
        for i in 0..n {
            y = y.add(m[i][n].div(m[i][i]).scale(c[i]));
        }
        y
    }
}

#[test]
fn standard_svf_matches_its_linear_model() {
    for ty in [1u8, 2, 4, 5, 6, 23] {
        for r in RESOS {
            let [f, q, _, g, ..] = biquad::make_svf(ty, 0.0, r, FS).map(|v| v as f64);
            if q <= 0.0 {
                continue; // negative damping: self-oscillates, no linear steady state
            }
            let four = matches!(ty, 2 | 5 | 23);
            let pick = move |l2: f64, h2: f64, b2: f64| match ty {
                1 | 2 => l2,
                4 | 5 => h2,
                _ => b2,
            };
            let stage = move |b: f64, l: f64, u: f64| {
                let l1 = l + f * b;
                let h1 = u - l1 - q * b;
                let b1 = b + f * h1;
                let l2 = l1 + f * b1;
                let h2 = u - l2 - q * b1;
                let b2 = b1 + f * h2;
                (b2, l2, pick(l2, h2, b2))
            };
            // Nearly undamped at 0.9: a long tail.
            let h = impulse_at(ty, 0, r, AMP, 64 * 4096);
            let model = probe(if four { 4 } else { 2 }, move |s, u| {
                let (b, l, y) = stage(s[0], s[1], u);
                if !four {
                    return (vec![b, l], g * y);
                }
                let (b2, l2, y2) = stage(s[2], s[3], y);
                (vec![b, l, b2, l2], g * y2)
            });
            check(&format!("svf {ty} r {r}"), &h, model);
        }
    }
}

#[test]
fn legacy_ladder_closed_form() {
    for sub in 0..4u8 {
        for r in RESOS {
            let [b, q, c0, ..] = ladder::make_legacy(0.0, r, FS).map(|v| v as f64);
            let h = impulse(3, sub, r);
            check(&format!("legacy {sub} r {r}"), &h, |zi| {
                let p = C(b, 0.0).div(ONE.sub(zi.scale(1.0 - b)));
                let fb = zi.add(zi.mul(zi)).scale(q).mul(p.powi(4));
                let y1 = p.scale(c0).div(ONE.add(fb));
                y1.mul(p.powi(sub as u32))
            });
        }
    }
}

#[test]
fn two_pole_closed_form() {
    for (ty, mode) in [(11u8, 0), (20, 1), (21, 2), (22, 3)] {
        for push in [false, true] {
            for r in RESOS {
                let [g, rr, ..] = twopole::make_ob2(0.0, r, FS).map(|v| v as f64);
                let re = rr + if push { -0.035 } else { 0.0 };
                if re <= 0.0 {
                    continue;
                }
                let h = impulse(ty, push as u8, r);
                check(&format!("ob2 {ty} push {push} r {r}"), &h, |zi| {
                    let s = s_of(zi, g);
                    let d = s.mul(s).add(s.scale(2.0 * re)).add(ONE);
                    let o = match mode {
                        0 => ONE.div(d),
                        1 => s.mul(s).div(d),
                        2 => ONE.add(s.mul(s)).div(d).scale(0.5),
                        _ => s.div(d).scale(0.5),
                    };
                    o.scale(0.74)
                });
            }
        }
    }
}

#[test]
fn four_pole_and_cascade_closed_form() {
    for (ty, n) in [(12u8, 5u8), (35, 15)] {
        for sub in 0..n {
            for r in RESOS {
                let [g, k, ..] = twopole::make_ob4(0.0, r, FS).map(|v| v as f64);
                let Kind::Ob4(w) = kind_of(ty, sub) else { panic!() };
                let h = impulse(ty, sub, r);
                check(&format!("ob4 {ty} sub {sub} r {r}"), &h, |zi| {
                    let l = ONE.div(ONE.add(s_of(zi, g)));
                    let y0 = ONE.div(ONE.add(l.powi(4).scale(k)));
                    let mut m = C(0.0, 0.0);
                    for (j, wj) in w.iter().enumerate() {
                        m = m.add(y0.mul(l.powi(j as u32)).scale(*wj as f64));
                    }
                    m.scale((1.0 + 0.45 * k) * 0.6)
                });
            }
        }
    }
}

#[test]
fn fast_svf_closed_form() {
    for sub in 0..9u8 {
        for r in RESOS {
            let c = va::make_fastsvf(sub, 0.0, r, FS).map(|v| v as f64);
            let [_, _, _, m0, m1, m2, ..] = c;
            let g = tan_r(std::f32::consts::PI * (440.0 / FS)) as f64;
            let k = (2.0 - 2.0 * r.clamp(0.0, 0.99) as f64) / if sub == 6 { 0.001 } else { 1.0 };
            // Bell's k / 0.001 puts a pole near 0.2 Hz: a very long tail.
            let h = impulse_at(34, sub, r, AMP, if sub == 6 { 64 * 16384 } else { LEN });
            check(&format!("fast svf {sub} r {r}"), &h, |zi| {
                let s = s_of(zi, g);
                let d = s.mul(s).add(s.scale(k)).add(ONE);
                C(m0, 0.0).add(s.div(d).scale(m1)).add(ONE.div(d).scale(m2))
            });
        }
    }
}

#[test]
fn warps_are_their_biquads_small_signal() {
    for ty in [16u8, 17, 18, 19, 27] {
        for sub in 0..12u8 {
            for r in RESOS {
                let c = warp::make_cwarp(ty, sub, 0.0, r, FS);
                let (bq5, m) = ([c[0], c[1], c[2], c[3], c[4]], c[5] as f64);
                let h = impulse(ty, sub, r);
                let stages = (sub & 3) as u32 + 1;
                // f32 transposed form at w = 0.03: poles near z = 1 lift the
                // rounding floor to ~-70 dB (stopbands) and ~-90 dB under the peak.
                let peak = (0..2000).map(|i| bq(bq5, C::ei(-dense_w(i))).scale(m).powi(stages).abs()).fold(0.0, f64::max);
                // (A notch's null depth is f32-conditioned: -60 dB there.)
                let floor = if ty == 18 { 1e-3 } else { 3e-4 };
                check_tol(&format!("cwarp {ty} sub {sub} r {r}"), &h, |zi| bq(bq5, zi).scale(m).powi(stages), (3e-5 * peak).max(floor));
            }
        }
    }
    for ty in 28..=32u8 {
        for sub in 0..8u8 {
            for r in RESOS {
                let c = warp::make_rwarp(ty, 0.0, r, FS);
                let bq5 = [c[0], c[1], c[2], c[3], c[4]];
                let h = impulse(ty, sub, r);
                let stages = (sub & 3) as u32 + 1;
                let peak = (0..2000).map(|i| bq(bq5, C::ei(-dense_w(i))).powi(stages).abs()).fold(0.0, f64::max);
                let floor = if ty == 30 { 1e-3 } else { 3e-4 };
                check_tol(&format!("rwarp {ty} sub {sub} r {r}"), &h, |zi| bq(bq5, zi).powi(stages), (3e-5 * peak).max(floor));
            }
        }
    }
}

#[test]
fn combs_closed_form() {
    for ty in [8u8, 25] {
        for sub in 0..2u8 {
            for r in RESOS {
                let [d, fb, dry, wet, ..] = misc::make_comb(ty, sub, 0.0, r, FS, true, false);
                let e = (d * 256.0).round_ties_even() as i32;
                let dd = (e >> 8) as i32;
                let ph = 255 - (e & 255) as usize;
                let h = impulse(ty, sub, r);
                check(&format!("comb {ty} sub {sub} r {r}"), &h, |zi| {
                    let mut rk = C(0.0, 0.0);
                    for i in 0..12 {
                        rk = rk.add(zi.powi((dd + 6 - i as i32) as u32).scale(kernel_tap(i, ph) as f32 as f64));
                    }
                    let loop_ = rk.div(ONE.sub(rk.scale(fb as f64)));
                    C(dry as f64, 0.0).add(loop_.scale(wet as f64))
                });
            }
        }
    }
}

/// |H| at f of a small-signal impulse response.
fn mag(h: &[f32], f: f64) -> f64 {
    dft(h, 2.0 * PI * f / FS as f64).abs()
}

#[test]
fn nonlinear_types_small_signal() {
    // (type, subtypes, low-pass?)
    let cases: [(u8, &[u8], bool); 6] = [
        (10, &[0, 1, 2, 3, 4, 5], true),
        (13, &[0, 1, 2, 3, 4], true),
        (14, &[0, 1, 2, 3, 4], false),
        (15, &[0, 1, 2, 3], true),
        (33, &[0, 4, 8], true),
        (33, &[3, 7], false),
    ];
    for (ty, subs, lp) in cases {
        for &sub in subs {
            let h0 = impulse_at(ty, sub, 0.0, AMP_NL, LEN);
            let h9 = impulse_at(ty, sub, 0.9, AMP_NL, LEN);
            for h in [&h0, &h9] {
                assert!(h.iter().all(|v| v.is_finite()), "type {ty} sub {sub} not finite");
            }
            let (lo, hi) = (mag(&h0, 30.0), mag(&h0, 0.4 * FS as f64));
            if lp {
                assert!(hi < 0.1 * lo, "type {ty} sub {sub}: LP passes highs ({lo} {hi})");
            } else {
                assert!(lo < 0.1 * hi, "type {ty} sub {sub}: HP passes lows ({lo} {hi})");
            }
            if ty == 33 {
                continue; // tri-pole: its loop gain is never zero and resonance is a large-signal effect
            }
            let pass = if lp { lo } else { hi };
            let peak0 = mag(&h0, 440.0) / pass;
            let peak9 = mag(&h9, 440.0) / mag(&h9, if lp { 30.0 } else { 0.4 * FS as f64 });
            assert!(peak9 > peak0, "type {ty} sub {sub}: resonance does not raise the cutoff ({peak0} {peak9})");
        }
    }
}

#[test]
fn sample_and_hold_holds() {
    let mut u = FilterUnit::new();
    let mut ys = vec![];
    let mut n = 0;
    for _ in 0..100 {
        u.set_block(9, 0, 12.0 * (4800.0f32 / 440.0).log2(), 0.0, FS, true);
        for _ in 0..64 {
            ys.push(u.tick((n as f32 * 0.001).sin()));
            n += 1;
        }
    }
    // 4.8 kHz at 96 kHz: a new value every 20 samples.
    let changes = ys.windows(2).filter(|w| w[0] != w[1]).count();
    assert!((315..=325).contains(&changes), "{changes}");
}

#[test]
fn bp12_legacy_double_push() {
    for sub in [3u8, 4] {
        let mut u = FilterUnit::new();
        u.set_block(6, sub, 0.0, 0.5, FS, true);
        let n12 = biquad::make(6, sub - 2, 0.0, 0.5, FS);
        let n24 = biquad::make(23, sub - 2, 0.0, 0.5, FS);
        for k in 0..NC {
            assert!((u.c[k] - n12[k]).abs() < 1e-6);
            assert!((u.t[k] - (0.8 * n12[k] + 0.2 * n24[k])).abs() < 1e-5);
        }
        for _ in 0..200 {
            for _ in 0..64 {
                u.tick(0.0);
            }
            u.set_block(6, sub, 0.0, 0.5, FS, true);
        }
        for k in 0..NC {
            let want = (0.16 * n12[k] + 0.2 * n24[k]) / 0.36;
            assert!((u.t[k] - want).abs() < 1e-4 * (1.0 + want.abs()), "{k}: {} {want}", u.t[k]);
        }
    }
}

#[test]
fn smoothing_lags_and_ramps() {
    let mut u = FilterUnit::new();
    u.set_block(34, 0, 0.0, 0.0, FS, true);
    let n0 = u.c;
    let n1 = va::make_fastsvf(0, 12.0, 0.0, FS);
    u.set_block(34, 0, 12.0, 0.0, FS, true);
    for k in 0..NC {
        let t = 0.8 * n0[k] + 0.2 * n1[k];
        assert!((u.t[k] - t).abs() < 1e-6);
        assert!((u.dc[k] - (t - n0[k]) / 64.0).abs() < 1e-7);
    }
    for _ in 0..64 {
        u.tick(0.0);
    }
    for k in 0..NC {
        assert!((u.c[k] - u.t[k]).abs() < 1e-5);
    }
    // A type change resets and snaps.
    u.set_block(34, 1, 12.0, 0.0, FS, true);
    assert_eq!(u.c, va::make_fastsvf(1, 12.0, 0.0, FS));
}

#[test]
fn every_type_survives_a_resonant_sweep() {
    let mut rng = crate::dsp::Rng::new(7);
    for ty in 0..36u8 {
        let n = subtype_count(ty).max(1);
        for sub in 0..n {
            for drive in [0.05f32, 1.0, 4.0] {
                let mut u = FilterUnit::new();
                let mut peak = 0.0f32;
                let blocks = 400;
                let mut ph = 0.0f32;
                for b in 0..blocks {
                    let cut = -60.0 + 130.0 * b as f32 / (blocks - 1) as f32;
                    u.set_block(ty, sub, cut, 1.0, FS, true);
                    for _ in 0..64 {
                        ph = (ph + 110.0 / FS).fract();
                        let x = drive * (0.7 * (2.0 * ph - 1.0) + 0.3 * rng.bipolar());
                        let y = u.tick(x);
                        assert!(y.is_finite(), "type {ty} sub {sub} drive {drive}: non-finite at block {b}");
                        peak = peak.max(y.abs());
                    }
                }
                assert!(peak < 1e3, "type {ty} sub {sub} drive {drive}: peak {peak}");
            }
        }
    }
}

#[test]
fn tilt_bank_is_flat_at_zero_and_tilts() {
    // Flat at colour 0: the output's power ~ the input's (uniform: 1/3).
    let pow = |colour: f32| {
        let mut t = TiltNoise::new(3);
        t.start(colour * 6.0, false);
        let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
        let (mut lo, mut hi, mut all) = (0.0f64, 0.0f64, 0.0f64);
        let (mut z, mut zd) = (0.0f64, 0.0f64);
        for _ in 0..2000 {
            t.block(colour, 1.0, false, &mut l, &mut r);
            for &v in &l {
                let v = v as f64;
                z += 0.01 * (v - z);
                lo += z * z;
                hi += (v - zd).powi(2);
                zd = v;
                all += v * v;
            }
        }
        (all / 128000.0, hi / lo)
    };
    let (p0, b0) = pow(0.0);
    assert!((p0 - 1.0 / 3.0).abs() < 0.03, "{p0}");
    let (_, bp) = pow(1.0);
    let (_, bn) = pow(-1.0);
    assert!(bp > 3.0 * b0 && bn < b0 / 3.0, "{bn} {b0} {bp}");
}

#[test]
fn names_and_counts() {
    assert_eq!(subtype_count(0), 0);
    assert_eq!(subtype_count(16), 12);
    assert_eq!(subtype_count(28), 8);
    assert_eq!(subtype_count(35), 15);
    assert_eq!(subtype_count(9), 0);
    assert_eq!(clamp_type(40, 3), (0, 0));
    assert_eq!(clamp_type(6, 9), (6, 4));
    let mut seen = [false; 36];
    for (_, ids) in MENU {
        for &i in ids {
            assert!(!seen[i as usize]);
            seen[i as usize] = true;
        }
    }
    assert!(seen.iter().all(|s| *s));
}

/// FILTERS 3.1: the per-sample coefficient step happens after the sample for
/// Cutoff Warp, Resonance Warp, Tri-pole and Vintage Type 3, before it for
/// every other per-sample type (Vintage Types 1 and 2 step inside their loops).
#[test]
fn coefficient_step_before_or_after_per_type() {
    let mut checked = 0;
    for ty in 1..=35u8 {
        for sub in 0..subtype_count(ty).max(1) {
            if ty == 10 && sub < 4 {
                continue;
            }
            let mut u = FilterUnit::new();
            u.set_block(ty, sub, 0.0, 0.5, FS, false);
            for i in 0..64 {
                u.tick(((i * 37 % 17) as f32 / 17.0 - 0.5) * 0.2);
            }
            u.set_block(ty, sub, 24.0, 0.7, FS, false);
            assert!(u.dc.iter().any(|d| *d != 0.0), "type {ty}/{sub}: no ramp to test");
            let after = matches!(ty, 16..=19 | 27..=33) || ty == 10;
            let mut v = u.clone();
            if !after {
                for k in 0..NC {
                    v.c[k] += v.dc[k];
                }
            }
            v.dc = [0.0; NC];
            let (a, b) = (u.tick(0.1), v.tick(0.1));
            assert_eq!(a.to_bits(), b.to_bits(), "type {ty}/{sub} ({}) {a} {b}", if after { "after" } else { "before" });
            checked += 1;
        }
    }
    assert!(checked > 60, "{checked}");
}

/// EFFECTS 5.16: the extended comb (2^17 line, delay clamp [12, 2^17 - 12],
/// feedback kept signed within [-2, 2]) next to the voice comb rules.
#[test]
fn long_comb_variant() {
    assert_eq!(misc::COMB_LONG, 131072);
    let voice = misc::make_comb(8, 1, -117.376, -0.5, FS, true, false);
    assert_eq!((voice[0], voice[1]), (4096.0 - 12.0, 0.0));
    assert_eq!(misc::make_comb(25, 1, 0.0, 3.0, FS, true, false)[1], -1.0);
    let long = misc::make_comb(8, 1, -117.376, -0.5, FS, true, true);
    assert_eq!((long[0], long[1]), (131072.0 - 12.0, -0.5));
    assert_eq!(misc::make_comb(8, 1, 0.0, 3.0, FS, true, true)[1], 2.0);
    assert_eq!(misc::make_comb(8, 1, 0.0, -3.0, FS, true, true)[1], -2.0);
    assert_eq!(misc::make_comb(8, 1, 120.0, 0.0, FS, true, true)[0], 12.0);
    // The echo comes back after the full clamped delay.
    let mut u = FilterUnit::new();
    u.set_long_comb(true);
    u.set_block(8, 1, -117.376, -0.5, FS, true);
    assert_eq!(u.comb.as_ref().unwrap().len(), misc::COMB_LONG);
    let mut best = (0usize, 0.0f32);
    for i in 0..131072 + 64 {
        let y = u.tick(if i == 0 { 0.01 } else { 0.0 });
        if y.abs() > best.1 {
            best = (i, y.abs());
        }
    }
    assert!((best.0 as i64 - 131060).abs() <= 1, "{best:?}");
    // The voice comb keeps its 4096 line and sign rule on the same unit type.
    let mut v = FilterUnit::new();
    v.set_block(8, 1, -117.376, -0.5, FS, true);
    assert_eq!(v.comb.as_ref().unwrap().len(), misc::COMB_LEN);
    assert_eq!(v.c[1], 0.0);
}
