//! Property tests of the oscillator family (TESTS.md section B): pitch,
//! level, harmonic profiles, purity, unison spread, FM sidebands,
//! determinism.

use super::*;
use crate::kit::core::SlotValue;
use std::sync::Arc;

const FS_OS: f32 = 96000.0;

struct Setup {
    ty: u8,
    slots: [SlotValue; 7],
    pitch: f32,
    stereo: bool,
    retrigger: bool,
    character: u8,
    seed: u32,
    fm: Option<(f32, f32)>,
    audio: Option<f32>,
}

impl Setup {
    fn new(ty: u8) -> Setup {
        Setup { ty, slots: default_slots(ty), pitch: 69.0, stereo: false, retrigger: true, character: 1, seed: 7, fm: None, audio: None }
    }
}

/// Render `blocks` blocks; returns (left, right).
fn render(s: &Setup, blocks: usize) -> (Vec<f32>, Vec<f32>) {
    let mut osc = Osc::new(s.ty, s.seed);
    let extra = default_extra(s.ty);
    let mut outl = Vec::with_capacity(blocks * 64);
    let mut outr = Vec::with_capacity(blocks * 64);
    let mut fm_buf = [0.0f32; 64];
    let mut inl = [0.0f32; 64];
    let mut fm_phase = 0.0f64;
    let mut in_phase = 0.0f64;
    for blk in 0..blocks {
        if let Some((hz, _)) = s.fm {
            for x in fm_buf.iter_mut() {
                *x = (2.0 * std::f64::consts::PI * fm_phase).sin() as f32;
                fm_phase = (fm_phase + hz as f64 / FS_OS as f64).fract();
            }
        }
        if let Some(hz) = s.audio {
            for x in inl.iter_mut() {
                *x = 0.5 * (2.0 * std::f64::consts::PI * in_phase).sin() as f32;
                in_phase = (in_phase + hz as f64 / FS_OS as f64).fract();
            }
        }
        let inr = inl;
        let b = OscBlock {
            pitch: s.pitch,
            drift: 0.0,
            stereo: s.stereo,
            fm: s.fm.is_some(),
            fmdepth: s.fm.map(|f| f.1).unwrap_or(0.0),
            fm_buf: &fm_buf,
            slots: &s.slots,
            retrigger: s.retrigger,
            character: s.character,
            gate: true,
            table: None,
            extra: &extra,
            audio_in: if s.audio.is_some() { Some((&inl, &inr)) } else { None },
            fs_os: FS_OS,
            tsr: 1.0,
        };
        if blk == 0 {
            osc.init(&b, false);
        }
        let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
        osc.process(&b, &mut l, &mut r);
        outl.extend_from_slice(&l);
        outr.extend_from_slice(&r);
    }
    (outl, outr)
}

/// |DFT| of the Hann-windowed signal at `f` Hz (normalised: a unit sine
/// gives 0.5).
fn mag(x: &[f32], f: f64) -> f64 {
    let n = x.len();
    let w = 2.0 * std::f64::consts::PI * f / FS_OS as f64;
    let (mut re, mut im, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
    let (cw, sw) = (w.cos(), w.sin());
    let (mut c, mut s) = (1.0f64, 0.0f64);
    for (i, v) in x.iter().enumerate() {
        let h = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
        re += *v as f64 * h * c;
        im -= *v as f64 * h * s;
        wsum += h;
        let c2 = c * cw - s * sw;
        s = s * cw + c * sw;
        c = c2;
    }
    (re * re + im * im).sqrt() / wsum
}

/// The strongest frequency within +-span Hz of f0 (0.02 Hz steps + parabola).
fn peak(x: &[f32], f0: f64, span: f64) -> f64 {
    let step = 0.02;
    let k = (span / step) as i64;
    let mut best = (f64::MIN, f0);
    for i in -k..=k {
        let f = f0 + i as f64 * step;
        let m = mag(x, f);
        if m > best.0 {
            best = (m, f);
        }
    }
    let (a, b, c) = (mag(x, best.1 - step), best.0, mag(x, best.1 + step));
    let den = a - 2.0 * b + c;
    if den.abs() > 1e-30 { best.1 + 0.5 * step * (a - c) / den } else { best.1 }
}

fn cents(f: f64, f0: f64) -> f64 {
    1200.0 * (f / f0).log2()
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

/// Settled tail of a render (drop the first quarter second).
fn tail(x: &[f32], secs: f32) -> &[f32] {
    let n = (secs * FS_OS) as usize;
    &x[x.len() - n..]
}

#[test]
fn every_type_renders_440_at_note_69() {
    for ty in 0..12u8 {
        let mut s = Setup::new(ty);
        if ty == AUDIOIN {
            s.audio = Some(440.0);
        }
        let blocks = (2.5 * FS_OS / 64.0) as usize;
        let (l, _) = render(&s, blocks);
        assert!(l.iter().all(|v| v.is_finite()), "{} NaN", TYPE_NAMES[ty as usize]);
        if ty == TWIST {
            // Its engines are pitch-checked in twist's own tests (engine-dependent settings).
            continue;
        }
        let x = tail(&l, 2.0);
        let level = rms(x);
        assert!(level > 0.02 && level < 2.0, "{} rms {level}", TYPE_NAMES[ty as usize]);
        if ty == SHNOISE {
            // Two random steps per 440 Hz cycle: the step rate's sinc null at 880 Hz.
            let around: f64 = [500.0, 600.0, 700.0].iter().map(|f| mag(x, *f)).sum::<f64>() / 3.0;
            assert!(mag(x, 880.0) < 0.2 * around, "S&H null {} vs {around}", mag(x, 880.0));
            continue;
        }
        let f = peak(x, 440.0, 12.0);
        let c = cents(f, 440.0);
        let tol = if ty == STRING { 12.0 } else { 1.0 };
        eprintln!("{:<12} rms {:.4} f {:.4} Hz ({:+.3} cents)", TYPE_NAMES[ty as usize], level, f, c);
        assert!(c.abs() < tol, "{} {f} Hz = {c} cents", TYPE_NAMES[ty as usize]);
    }
}

#[test]
fn classic_saw_and_square_harmonics() {
    // Saw: every harmonic ~1/n.
    let s = Setup::new(CLASSIC);
    let (l, _) = render(&s, (1.5 * FS_OS / 64.0) as usize);
    let x = tail(&l, 1.0);
    let h1 = mag(x, 440.0);
    for n in 1..=16 {
        let d = db(mag(x, 440.0 * n as f64) / h1) - db(1.0 / n as f64);
        assert!(d.abs() < 1.0, "saw harmonic {n}: {d:+.2} dB");
    }
    // Square (shape -1, width 0.5): odd harmonics 1/n, even ones absent.
    let mut s = Setup::new(CLASSIC);
    s.slots[0].v = -1.0;
    let (l, _) = render(&s, (1.5 * FS_OS / 64.0) as usize);
    let x = tail(&l, 1.0);
    let h1 = mag(x, 440.0);
    for n in 1..=16 {
        let m = mag(x, 440.0 * n as f64) / h1;
        if n % 2 == 1 {
            let d = db(m) - db(1.0 / n as f64);
            assert!(d.abs() < 1.0, "square harmonic {n}: {d:+.2} dB");
        } else {
            assert!(db(m) < -40.0, "square even harmonic {n}: {:.1} dB", db(m));
        }
    }
}

#[test]
fn sine_pure_shape_is_pure() {
    let s = Setup::new(SINE);
    let (l, _) = render(&s, (1.5 * FS_OS / 64.0) as usize);
    let x = tail(&l, 1.0);
    let h1 = mag(x, 440.0);
    assert!((h1 - 0.5).abs() < 0.01, "{h1}");
    for n in 2..=40 {
        let m = db(mag(x, 440.0 * n as f64) / h1);
        assert!(m < -100.0, "harmonic {n}: {m:.1} dB");
    }
    // Every shape is finite and bounded; the legacy engine is pure too.
    for shape in 0..32 {
        let mut s = Setup::new(SINE);
        s.slots[0].v = shape as f32;
        let (l, _) = render(&s, 50);
        assert!(l.iter().all(|v| v.is_finite() && v.abs() < 2.5), "shape {shape}");
    }
    let mut s = Setup::new(SINE);
    s.slots[2].v = 0.0;
    let (l, _) = render(&s, (1.5 * FS_OS / 64.0) as usize);
    let x = tail(&l, 1.0);
    let h1 = mag(x, 440.0);
    for n in 2..=20 {
        assert!(db(mag(x, 440.0 * n as f64) / h1) < -100.0, "legacy harmonic {n}");
    }
}

#[test]
fn unison_4_spreads_detune() {
    let spread_at = |n: f32| {
        let mut s = Setup::new(CLASSIC);
        s.slots[6].v = n;
        s.slots[5].v = 0.1; // +-10 cents
        let (l, _) = render(&s, (4.5 * FS_OS / 64.0) as usize);
        let x = tail(&l, 4.0).to_vec();
        let outer = 440.0 * 2f64.powf(0.1 / 12.0);
        let inner = 440.0 * 2f64.powf(0.1 / 36.0);
        (mag(&x, outer), mag(&x, inner), mag(&x, 440.0))
    };
    let (o1, _, c1) = spread_at(1.0);
    let (o4, i4, _) = spread_at(4.0);
    assert!(o1 < 0.01 * c1, "single voice leaks to +10 cents: {o1} vs {c1}");
    assert!(o4 > 0.3 * i4 && i4 > 0.3 * o4, "4 voices: outer {o4} inner {i4}");
    assert!(o4 > 20.0 * o1);
    // Wide: the voices pan apart, L != R.
    let mut s = Setup::new(CLASSIC);
    s.slots[6].v = 4.0;
    s.stereo = true;
    let (l, r) = render(&s, 200);
    let diff: f64 = l.iter().zip(r.iter()).map(|(a, b)| ((a - b) as f64).abs()).sum();
    assert!(diff > 1.0);
}

fn bessel_j(n: i32, x: f64) -> f64 {
    let mut sum = 0.0;
    let mut fact_k = 1.0;
    for k in 0..30 {
        if k > 0 {
            fact_k *= k as f64;
        }
        let fact_kn: f64 = (1..=(k + n)).map(|v| v as f64).product();
        sum += (-1f64).powi(k) / (fact_k * fact_kn) * (x / 2.0).powi(2 * k + n);
    }
    sum
}

#[test]
fn fm2_and_fm3_sidebands() {
    // Ratio 3 modulator: carrier 440, sidebands at 440 +- 1320 -> 1760 and |-880|.
    for (ty, amount, beta) in [(FM2, 0.3f32, 8.0 * std::f64::consts::PI * 0.027), (FM3, 0.2f32, 32.0 * std::f64::consts::PI * 0.008)] {
        let mut s = Setup::new(ty);
        s.slots[0].v = amount;
        s.slots[1].v = 3.0;
        let (l, _) = render(&s, (3.0 * FS_OS / 64.0) as usize);
        let x = tail(&l, 1.0);
        let c = mag(x, 440.0);
        let up = mag(x, 1760.0);
        let lo = mag(x, 880.0);
        let want = (bessel_j(1, beta) / bessel_j(0, beta)).abs();
        for (name, m) in [("upper", up), ("lower", lo)] {
            let d = db(m / c) - db(want);
            assert!(d.abs() < 0.5, "{} {name}: {:.2} dB off (beta {beta})", TYPE_NAMES[ty as usize], d);
        }
        // Nothing at the non-sideband 1100 Hz.
        assert!(db(mag(x, 1100.0) / c) < -80.0);
    }
}

#[test]
fn retrigger_is_deterministic() {
    for ty in [CLASSIC, SINE, WAVETABLE, FM3, FM2, WINDOW, MODERN, ALIAS] {
        let mut a = Setup::new(ty);
        a.slots[6].v = if ty == FM3 || ty == FM2 { a.slots[6].v } else { 3.0 };
        let mut b = Setup::new(ty);
        b.slots = a.slots;
        b.seed = 12345;
        let (la, _) = render(&a, 100);
        let (lb, _) = render(&b, 100);
        assert_eq!(la, lb, "{}", TYPE_NAMES[ty as usize]);
    }
}

#[test]
fn modern_saw_is_band_limited_and_1_over_n() {
    let mut s = Setup::new(MODERN);
    s.slots[0].v = 1.0;
    let (l, _) = render(&s, (1.5 * FS_OS / 64.0) as usize);
    let x = tail(&l, 1.0);
    let h1 = mag(x, 440.0);
    for n in 1..=8 {
        let d = db(mag(x, 440.0 * n as f64) / h1) - db(1.0 / n as f64);
        assert!(d.abs() < 1.0, "modern saw harmonic {n}: {d:+.2} dB");
    }
}

#[test]
fn migrations() {
    let mut s = default_slots(SINE);
    let mut rt = false;
    s[0].v = 4.0;
    migrate(SINE, 12, &mut s, &mut rt, false);
    assert_eq!(s[0].v, 1.0);
    assert!(rt && s[3].deactivated && s[4].deactivated);
    migrate_patch(SINE, 12, &mut s, false);
    assert_eq!(s[0].v, 28.0);
    let mut w = default_slots(WAVETABLE);
    migrate(WAVETABLE, 16, &mut w, &mut rt, false);
    assert!(w[0].extend && w[0].deform == 0);
    let mut st = default_slots(STRING);
    st[1].deform = 0;
    migrate(STRING, 18, &mut st, &mut rt, false);
    assert_eq!(st[1].deform, string::DEFORM_DEFAULT_LEVEL);
    for ty in 0..12u8 {
        let d = slot_defs(ty);
        if ty != TWIST {
            assert!(d.iter().any(|x| x.used()), "{ty}");
        }
    }
}

/// Every type x mono/wide x unison x FM x random start x odd settings:
/// finite and bounded.
#[test]
fn smoke_all_types_all_modes() {
    for ty in 0..12u8 {
        for stereo in [false, true] {
            for retrigger in [true, false] {
                for fm in [None, Some((660.0f32, 0.5f32))] {
                    let mut s = Setup::new(ty);
                    s.stereo = stereo;
                    s.retrigger = retrigger;
                    s.fm = fm;
                    s.audio = Some(300.0);
                    s.character = if stereo { 0 } else { 2 };
                    if !matches!(ty, AUDIOIN | FM3 | FM2 | STRING) {
                        s.slots[6].v = 5.0;
                    }
                    match ty {
                        CLASSIC => {
                            s.slots[4].v = 17.0;
                            s.slots[3].v = 0.4;
                            s.slots[0].v = 0.3;
                        }
                        SHNOISE => {
                            s.slots[4].v = 24.0;
                            s.slots[0].v = 0.9;
                        }
                        MODERN => {
                            s.slots[4].v = 12.0;
                            s.slots[1].v = 0.7;
                            s.slots[2].v = 0.5;
                            s.slots[2].deform = 2 | modern::SUB_BIT;
                        }
                        SINE => {
                            s.slots[1].v = -0.8;
                            s.slots[3].deactivated = false;
                            s.slots[3].v = -10.0;
                        }
                        WAVETABLE => {
                            s.slots[0].v = 0.6;
                            s.slots[3].v = 12.0;
                            s.slots[4].v = 0.5;
                            s.slots[2].v = 0.7;
                        }
                        WINDOW => {
                            s.slots[0].v = 0.5;
                            s.slots[0].extend = true;
                            s.slots[1].v = 7.0;
                        }
                        STRING => s.slots[1].deform = string::OS_2X | string::READ_LINEAR,
                        ALIAS => s.slots[4].v = 3.0,
                        _ => {}
                    }
                    for pitch in [10.0f32, 69.0, 127.0] {
                        s.pitch = pitch;
                        let (l, r) = render(&s, 60);
                        // Classic under FM: the ramp term is not FM-scaled (spec 2.5),
                        // so high notes wobble at the modulator rate; bounded by the leak.
                        let bound = if ty == CLASSIC && fm.is_some() { 32.0 } else { 8.0 };
                        let ok = l.iter().chain(r.iter()).all(|v| v.is_finite() && v.abs() < bound);
                        assert!(ok, "{} stereo {stereo} retrig {retrigger} fm {:?} pitch {pitch}", TYPE_NAMES[ty as usize], fm.is_some());
                    }
                }
            }
        }
    }
    for shape in 0..18 {
        let mut s = Setup::new(ALIAS);
        s.slots[0].v = shape as f32;
        s.slots[1].v = 0.3;
        s.slots[2].v = 0.2;
        s.audio = Some(200.0);
        let (l, _) = render(&s, 40);
        assert!(l.iter().all(|v| v.is_finite()) && rms(&l) > 1e-3, "alias shape {shape}");
    }
    for ex in 0..16 {
        let mut s = Setup::new(STRING);
        s.slots[0].v = ex as f32;
        s.audio = Some(200.0);
        let (l, _) = render(&s, 40);
        assert!(l.iter().all(|v| v.is_finite()), "string exciter {ex}");
        if ex < 15 {
            assert!(rms(&l) > 1e-4, "string exciter {ex} silent");
        }
    }
}

#[test]
fn wavetable_sample_mode_plays_then_stops() {
    // A 16-frame "sample" of 64-sample frames, played once at 1 frame per cycle.
    let data: Vec<f32> = (0..16 * 64).map(|i| (i as f32 * 0.3).sin() * 0.5).collect();
    let t = Arc::new(RefTable::from_frames(64, 16, table::FLAG_SAMPLE, &data));
    assert_eq!(t.n, 19);
    let slots = default_slots(WAVETABLE);
    let mut osc = Osc::new(WAVETABLE, 1);
    let fm_buf = [0.0f32; 64];
    let b = OscBlock {
        pitch: 69.0,
        drift: 0.0,
        stereo: false,
        fm: false,
        fmdepth: 0.0,
        fm_buf: &fm_buf,
        slots: &slots,
        retrigger: true,
        character: 1,
        gate: true,
        table: Some(&t),
        extra: &[],
        audio_in: None,
        fs_os: FS_OS,
        tsr: 1.0,
    };
    osc.init(&b, false);
    let mut out = Vec::new();
    for _ in 0..200 {
        let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
        osc.process(&b, &mut l, &mut r);
        out.extend_from_slice(&l);
    }
    // 16 cycles of 440 Hz ~ 3500 samples of sound, then the integrator leaks to silence.
    assert!(rms(&out[..3000]) > 0.05);
    assert!(rms(&out[out.len() - 2000..]) < 1e-3, "{}", rms(&out[out.len() - 2000..]));
}

