//! The shipped shader library: every document compiles, audits clean with
//! `check`, renders the same bits natively and interpreted, and does its
//! technique better than the native voice it replaces or extends
//! (tuning, aliasing), with CPU numbers.

use makepad_audio_synth::recipe::{Recipe, RecipeVoice};
use makepad_script_audio_aot::{compile_with, Backend, Instance, Kind};
use makepad_score_player::shader::{check, library, ShaderInst, LIBRARY};
use std::f32::consts::TAU;

const RATE: f32 = 48000.0;

#[test]
fn every_library_shader_checks_clean() {
    for (name, code) in LIBRARY {
        let report = check(code).unwrap_or_else(|e| {
            let (line, col) = e[0].line_col(code);
            panic!("{} {}:{}: {}", name, line, col, e[0].message)
        });
        eprintln!("{:12} {:?} peak {:.2} rms {:.3} {:>8.1} ops/frame tail {:?} {:?}", name, report.kind, report.peak, report.rms, report.ops_per_frame, report.tail_secs, report.warnings);
        assert!(report.warnings.is_empty(), "{}: {:?}", name, report.warnings);
    }
}

#[test]
fn library_backends_agree_bit_for_bit() {
    for (name, code) in LIBRARY {
        let native = compile_with(code, Backend::Native).unwrap();
        let interp = compile_with(code, Backend::Interp).unwrap();
        let render = |s: &makepad_script_audio_aot::AudioShaderRef| {
            let mut i = Instance::new(s.clone(), RATE);
            let n = 12000;
            let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
            if s.kind == Kind::Effect {
                let x: Vec<f32> = (0..n).map(|k| ((k as f32 * 0.021).sin() + if k % 3000 == 0 { 0.8 } else { 0.0 }) * 0.5).collect();
                i.process(&x, &x, &mut l, &mut r);
            } else {
                i.note_on(50.0, 0.9, 3);
                i.render(&mut l[..8000], &mut r[..8000]);
                i.note_off();
                i.render(&mut l[8000..], &mut r[8000..]);
            }
            l.iter().chain(&r).map(|x| x.to_bits()).collect::<Vec<_>>()
        };
        assert!(render(&native) == render(&interp), "{}: native and interpreter differ", name);
    }
}

/// Fundamental by autocorrelation with parabolic interpolation.
fn pitch(x: &[f32], lo: f32, hi: f32) -> f32 {
    let (a0, a1) = ((RATE / hi) as usize, (RATE / lo) as usize);
    let ac = |lag: usize| -> f64 { x.iter().zip(&x[lag..]).map(|(a, b)| *a as f64 * *b as f64).sum::<f64>() };
    let v: Vec<f64> = (a0..=a1 + 1).map(ac).collect();
    let top = v.iter().cloned().fold(f64::MIN, f64::max);
    let k = (1..v.len() - 1).find(|&k| v[k] > v[k - 1] && v[k] >= v[k + 1] && v[k] > 0.9 * top).unwrap_or(1);
    let s = 0.5 * (v[k - 1] - v[k + 1]) / (v[k - 1] - 2.0 * v[k] + v[k + 1]);
    RATE / ((a0 + k) as f64 + s) as f32
}

#[test]
fn the_pluck_is_in_tune_where_the_native_one_is_not() {
    let shader = compile_with(library("pluck").unwrap(), Backend::Native).unwrap();
    let recipe = Recipe::parse("pluck=0.35 freq=440 a=0.001 d=3 s=0 r=0.2 gain=0.5").unwrap();
    eprintln!("note   shader cents   native recipe cents");
    let mut worst_native = 0.0f32;
    for note in [48.0f32, 60.0, 72.0, 84.0, 91.0] {
        let want = 440.0 * 2f32.powf((note - 69.0) / 12.0);
        let mut i = Instance::new(shader.clone(), RATE);
        i.set_param("t60", 8.0);
        i.note_on(note, 1.0, 1);
        let (mut l, mut r) = (vec![0.0f32; 20000], vec![0.0f32; 20000]);
        i.render(&mut l, &mut r);
        let ours = 1200.0 * (pitch(&l[4000..16000], want * 0.7, want * 1.4) / want).log2();
        let mut v = RecipeVoice::new(&recipe, 1.0, Some(want), true, 1);
        let mut buf = vec![0.0f32; 20000];
        v.render(&mut buf, RATE, 1.0);
        let native = 1200.0 * (pitch(&buf[4000..16000], want * 0.7, want * 1.4) / want).log2();
        worst_native = worst_native.max(native.abs());
        eprintln!("{:>4}   {:+8.2}      {:+8.2}", note, ours, native);
        assert!(ours.abs() < 3.0, "note {}: {:+.1} cents", note, ours);
    }
    // About -10 cents at note 91 (an integer delay line); the voice's DC blocker moves the read by a few tenths.
    assert!(worst_native > 5.0, "the native pluck was expected to be out of tune up high");
}

/// Energy at frequencies that are not harmonics of `f0` (aliases), relative
/// to the total, in dB. A DFT at every bin of a 1 Hz grid is too slow;
/// Goertzel over the band 50..20000 Hz in 25 Hz steps is enough.
fn alias_db(x: &[f32], f0: f32) -> f32 {
    let (mut alias, mut total) = (0.0f64, 0.0f64);
    let mut f = 50.0f32;
    while f < 20000.0 {
        let w = TAU * f / RATE;
        let c = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        let win = |i: usize| 0.5 - 0.5 * (TAU * i as f32 / x.len() as f32).cos();
        for (i, v) in x.iter().enumerate() {
            let s = v * win(i) + c * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let p = (s1 * s1 + s2 * s2 - c * s1 * s2) as f64;
        total += p;
        let h = f / f0;
        if (h - h.round()).abs() * f0 > 60.0 {
            alias += p;
        }
        f += 25.0;
    }
    (10.0 * (alias / total).log10()) as f32
}

#[test]
fn oversampled_drive_aliases_less_than_the_native_drive() {
    let n = 8192;
    let f0 = 4987.0;
    let x: Vec<f32> = (0..n).map(|i| (TAU * f0 * i as f32 / RATE).sin() * 0.8).collect();
    // The native Score Drive's algorithm (soft clip at 1x, then its tone
    // low-pass), at the same drive.
    let gain = 1.0 + 0.8 * 24.0;
    let soft = |x: f32| {
        let x = x.clamp(-3.0, 3.0);
        x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
    };
    let norm = 1.0 / soft(gain);
    let k = 1.0 - (-TAU * 18000.0 / RATE).exp();
    let mut lp = 0.0f32;
    let native: Vec<f32> = x
        .iter()
        .map(|v| {
            lp += (soft(v * gain) * norm - lp) * k;
            lp
        })
        .collect();
    let shader = compile_with(library("drive").unwrap(), Backend::Native).unwrap();
    let mut i = Instance::new(shader, RATE);
    i.set_param("amount", 0.9);
    i.set_param("tone", 18000.0);
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    i.process(&x, &x, &mut l, &mut r);
    let (a_native, a_shader) = (alias_db(&native[1024..], f0), alias_db(&l[1024..], f0));
    eprintln!("alias energy: native 1x drive {:.1} dB, shader 4x drive {:.1} dB", a_native, a_shader);
    assert!(a_shader < a_native - 15.0, "4x drive {:.1} dB vs 1x {:.1} dB", a_shader, a_native);
}

#[test]
fn acid_slides_without_retriggering() {
    let shader = compile_with(library("acid").unwrap(), Backend::Native).unwrap();
    let mut inst = ShaderInst::new(shader, RATE);
    let (mut l, mut r) = (vec![0.0f32; 24000], vec![0.0f32; 24000]);
    inst.note_on(36.0, 0.7, 0);
    inst.render(&mut l[..12000], &mut r[..12000]);
    inst.legato(48.0, 0.7, 0);
    inst.render(&mut l[12000..], &mut r[12000..]);
    // No new attack: the level right after the slide stays under the
    // level at the first note's start (the filter envelope kept decaying).
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let first = peak(&l[0..2400]);
    let after = peak(&l[12000..14400]);
    assert!(after < first, "legato retriggered: {} vs {}", after, first);
    // And the pitch got there: an octave up.
    let p0 = pitch(&l[6000..12000], 40.0, 120.0);
    let p1 = pitch(&l[18000..24000], 90.0, 200.0);
    assert!((p1 / p0 - 2.0).abs() < 0.03, "{} -> {}", p0, p1);
}

#[test]
fn echo_lands_on_the_beat_division() {
    let shader = compile_with(library("echo").unwrap(), Backend::Native).unwrap();
    let mut i = Instance::new(shader, RATE);
    i.set_param("bpm", 125.0);
    i.set_param("division", 0.5);
    i.set_param("side_ms", 0.0);
    i.set_param("wet", 1.0);
    let n = 30000;
    let mut x = vec![0.0f32; n];
    x[2000] = 1.0;
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    i.process(&x, &x, &mut l, &mut r);
    let expect = 2000 + (60.0 / 125.0 * 0.5 * RATE) as usize;
    let at = (2100..n).max_by(|a, b| (l[*a].abs() + r[*a].abs()).total_cmp(&(l[*b].abs() + r[*b].abs()))).unwrap();
    assert!((at as i64 - expect as i64).abs() <= 3, "first echo at {} expected {}", at, expect);
}

#[test]
fn cpu_against_the_native_voices() {
    // Supersaw: the shader (7 drifting saws + 2 SVFs, stereo) against the
    // native recipe supersaw (5 saw layers, each with its own low-pass).
    let recipe = Recipe::parse("saw freq=440 detune=5 a=0.01 d=0.5 s=0.8 r=0.35 gain=0.11 lp=8000; saw freq=440 detune=-5 a=0.01 d=0.5 s=0.8 r=0.35 gain=0.11 lp=8000; saw freq=440 detune=12 a=0.01 d=0.5 s=0.8 r=0.35 gain=0.1 lp=8000 pan=0.4; saw freq=440 detune=-12 a=0.01 d=0.5 s=0.8 r=0.35 gain=0.1 lp=8000 pan=-0.4; saw freq=440 detune=22 a=0.01 d=0.5 s=0.8 r=0.35 gain=0.08 lp=8000 pan=0.7; saw freq=440 detune=-22 a=0.01 d=0.5 s=0.8 r=0.35 gain=0.08 lp=8000 pan=-0.7").unwrap();
    let shader = compile_with(library("supersaw").unwrap(), Backend::Native).unwrap();
    let secs = 2.0;
    let n = (secs * RATE) as usize;
    let t0 = std::time::Instant::now();
    let mut v = RecipeVoice::new(&recipe, 1.0, Some(220.0), true, 1);
    let mut buf = vec![0.0f32; 256];
    for _ in 0..n / 256 {
        v.render(&mut buf, RATE, 1.0);
    }
    let native_ns = t0.elapsed().as_secs_f64() * 1e9 / n as f64;
    let mut inst = ShaderInst::new(shader, RATE);
    inst.note_on(57.0, 0.8, 0);
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let t0 = std::time::Instant::now();
    for _ in 0..n / 256 {
        inst.render(&mut l, &mut r);
    }
    let shader_ns = t0.elapsed().as_secs_f64() * 1e9 / n as f64;
    eprintln!("supersaw: native recipe (5 saws, mono) {:.1} ns/frame, shader (7 drifting saws, stereo) {:.1} ns/frame", native_ns, shader_ns);
}

/// The gated reverb gives each hit a burst of room, then silence: loud
/// while the gate holds, more than 30 dB down after it closes.
#[test]
fn gated_verb_cuts_its_tail() {
    let code = library("gated_verb").unwrap();
    let shader = compile_with(code, Backend::Native).unwrap();
    let mut inst = Instance::new(shader, RATE);
    let n = 48000;
    // A snare-like burst at 0.1 s: 20 ms of decaying noise.
    let mut seed = 1u32;
    let x: Vec<f32> = (0..n).map(|k| {
        let t = k as f32 / RATE - 0.1;
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        if (0.0..0.02).contains(&t) { ((seed >> 9) as f32 / 4194304.0 - 1.0) * (1.0 - t / 0.02) * 0.8 } else { 0.0 }
    }).collect();
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    inst.process(&x, &x, &mut l, &mut r);
    let rms = |a: f32, b: f32| {
        let (i, j) = ((a * RATE) as usize, (b * RATE) as usize);
        (l[i..j].iter().map(|v| v * v).sum::<f32>() / (j - i) as f32).sqrt()
    };
    let (held, after) = (rms(0.15, 0.3), rms(0.45, 0.8));
    eprintln!("gated_verb: held {:.4} after {:.6}", held, after);
    assert!(held > 0.005, "the room sounds while the gate holds ({held})");
    assert!(after < held * 0.03, "the tail is cut ({after} against {held})");
}
