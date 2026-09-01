// Verification of the learned (PianoForte-derived) engine: network parse
// sanity, the real-time contract (allocation-free, block-size determinism,
// scalar/SIMD agreement, bounded output), level calibration against the
// physical engine, and the perf measurement. See src/learned.rs for the
// provenance of the algorithm and the network.

use makepad_piano_model::learned::{EngineKind, LearnedPiano, PianoEngine};
use makepad_piano_model::{Instrument, Piano, PianoEvent, TimedEvent, PIANO_PRESETS};

const FS: f32 = 48000.0;

/// An absolute-time event script rendered through any Instrument in blocks.
fn render<I: Instrument>(p: &mut I, script: &[(u64, PianoEvent)], total: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0f32; total];
    let mut r = vec![0.0f32; total];
    let mut te: Vec<TimedEvent> = Vec::new();
    let mut pos = 0usize;
    while pos < total {
        let n = block.min(total - pos);
        te.clear();
        for &(at, ev) in script {
            if at >= pos as u64 && at < (pos + n) as u64 {
                te.push(TimedEvent { offset: (at - pos as u64) as u32, event: ev });
            }
        }
        p.process(&te, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    (l, r)
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

fn sec(at: f64) -> u64 {
    (at * FS as f64) as u64
}

#[test]
fn net_parses_and_outputs_are_sane() {
    let p = LearnedPiano::new(FS);
    let mut out = [0.0f32; 30];
    // Amplitudes must be finite and inside the tanh-mapped output range for
    // the whole input cube, and must actually vary with every input.
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for key in [21u8, 40, 60, 80, 108] {
        for vel in [1u8, 64, 127] {
            for t in [0.0, 0.1, 1.0, 10.0] {
                p.learned_partial_amps(key, vel, t, &mut out);
                for &a in &out {
                    assert!(a.is_finite() && (0.0..=1.0).contains(&a), "amp {a} out of range");
                    lo = lo.min(a);
                    hi = hi.max(a);
                }
            }
        }
    }
    assert!(hi - lo > 0.2, "network output barely varies ({lo}..{hi}) — parse suspect");
    // The network output is a NORMALISED spectral shape (it sums to ~1 at
    // every time; the absolute decay is the analytic envelope). Assert
    // both facts: near-unit sum, and a shape that moves with time.
    let mut early = [0.0f32; 30];
    let mut late = [0.0f32; 30];
    p.learned_partial_amps(60, 100, 0.05, &mut early);
    p.learned_partial_amps(60, 100, 3.0, &mut late);
    let se: f32 = early.iter().sum();
    let sl: f32 = late.iter().sum();
    assert!((0.5..=1.6).contains(&se) && (0.5..=1.6).contains(&sl), "ladder sums stray from ~1: {se}, {sl}");
    let shape_d: f32 = early.iter().zip(&late).map(|(a, b)| (a - b).abs()).sum();
    assert!(shape_d > 0.1, "learned ladder ignores the time coordinate");
    let env_early = p.learned_envelope(60, 0.05);
    let env_late = p.learned_envelope(60, 3.0);
    assert!(env_late < 0.5 * env_early, "analytic envelope does not decay: {env_early} -> {env_late}");
    // Velocity must matter.
    let mut soft = [0.0f32; 30];
    let mut loud = [0.0f32; 30];
    p.learned_partial_amps(60, 30, 0.05, &mut soft);
    p.learned_partial_amps(60, 120, 0.05, &mut loud);
    let ds: f32 = soft.iter().zip(&loud).map(|(a, b)| (a - b).abs()).sum();
    assert!(ds > 0.05, "learned ladder ignores velocity");
}

#[test]
fn deterministic_across_block_sizes() {
    let script = vec![
        (0, PianoEvent::NoteOn { key: 36, velocity: 100 }),
        (sec(0.2), PianoEvent::NoteOn { key: 60, velocity: 80 }),
        (sec(0.5), PianoEvent::Sustain { value: 1.0 }),
        (sec(0.7), PianoEvent::NoteOn { key: 84, velocity: 120 }),
        (sec(0.9), PianoEvent::NoteOff { key: 60 }),
        (sec(1.1), PianoEvent::NoteOn { key: 60, velocity: 90 }),
        (sec(1.3), PianoEvent::Sustain { value: 0.0 }),
    ];
    let total = (2.0 * FS) as usize;
    let mut a = LearnedPiano::new(FS);
    let (l1, r1) = render(&mut a, &script, total, 512);
    let mut b = LearnedPiano::new(FS);
    let (l2, r2) = render(&mut b, &script, total, 61);
    let mut c = LearnedPiano::new(FS);
    let (l3, _) = render(&mut c, &script, total, 4096);
    assert_eq!(l1, l2, "block 512 vs 61 differ");
    assert_eq!(r1, r2, "block 512 vs 61 differ (right)");
    assert_eq!(l1, l3, "block 512 vs 4096 differ");
}

#[test]
fn scalar_and_simd_agree() {
    let script = vec![
        (0, PianoEvent::NoteOn { key: 24, velocity: 110 }),
        (sec(0.1), PianoEvent::NoteOn { key: 60, velocity: 90 }),
        (sec(0.2), PianoEvent::NoteOn { key: 96, velocity: 70 }),
    ];
    let total = (1.0 * FS) as usize;
    let mut a = LearnedPiano::new(FS);
    a.set_force_scalar(true);
    let (ls, _) = render(&mut a, &script, total, 512);
    let mut b = LearnedPiano::new(FS);
    let (lv, _) = render(&mut b, &script, total, 512);
    let mut max_d = 0.0f64;
    let mut ref_pk = 0.0f64;
    for k in 0..total {
        max_d = max_d.max((ls[k] as f64 - lv[k] as f64).abs());
        ref_pk = ref_pk.max((ls[k] as f64).abs());
    }
    assert!(max_d < 1e-3 * ref_pk.max(1e-6), "scalar vs simd diverge: {max_d} (peak {ref_pk})");
}

#[test]
fn output_is_finite_and_decays_to_silence() {
    let mut p = LearnedPiano::new(FS);
    let script = vec![
        (0, PianoEvent::NoteOn { key: 21, velocity: 127 }),
        (0, PianoEvent::NoteOn { key: 108, velocity: 127 }),
        (sec(0.5), PianoEvent::NoteOff { key: 21 }),
        (sec(0.5), PianoEvent::NoteOff { key: 108 }),
    ];
    let total = (4.0 * FS) as usize;
    let (l, r) = render(&mut p, &script, total, 512);
    for (i, &v) in l.iter().chain(r.iter()).enumerate() {
        assert!(v.is_finite(), "non-finite sample at {i}");
        assert!(v.abs() <= 1.5, "runaway sample {v} at {i}");
    }
    let early = rms(&l[(0.1 * FS) as usize..(0.4 * FS) as usize]);
    let late = rms(&l[(3.5 * FS) as usize..]);
    assert!(early > 1e-4, "engine is silent when struck (rms {early:.6})");
    assert!(late < early * 0.02, "release does not decay: early {early:.5}, late {late:.5}");
}

/// Same material, both engines: the learned engine must land within a few
/// dB of the physical engine's calibrated loudness so an engine swap is not
/// a level jump. (LEARNED_MASTER in learned.rs is tuned to hold this.)
#[test]
fn level_matches_physical_engine() {
    let mut script = Vec::new();
    // A moderate two-hand texture across the compass at mezzo velocities.
    let keys = [36u8, 48, 55, 60, 64, 67, 72, 76];
    for (n, &k) in keys.iter().enumerate() {
        let at = sec(0.25 * n as f64);
        script.push((at, PianoEvent::NoteOn { key: k, velocity: 72 }));
        script.push((at + sec(1.2), PianoEvent::NoteOff { key: k }));
    }
    let total = (3.5 * FS) as usize;
    let mut phys = Piano::new(FS);
    phys.set_reverb_mix(0.0);
    phys.set_early_reflection_level(0.0);
    let (pl, pr) = render(&mut phys, &script, total, 512);
    let mut learned = LearnedPiano::new(FS);
    learned.set_reverb_mix(0.0);
    learned.set_early_reflection_level(0.0);
    let (ll, lr) = render(&mut learned, &script, total, 512);
    let p_rms = db(0.5 * (rms(&pl) + rms(&pr)));
    let l_rms = db(0.5 * (rms(&ll) + rms(&lr)));
    println!("physical {p_rms:.1} dBFS rms, learned {l_rms:.1} dBFS rms");
    assert!((p_rms - l_rms).abs() < 3.5, "engine swap is a level jump: physical {p_rms:.1} dB, learned {l_rms:.1} dB");
}

#[test]
fn engine_wrapper_forwards_and_swaps() {
    let preset = &PIANO_PRESETS[0];
    for kind in EngineKind::ALL {
        let mut e = PianoEngine::new(kind, FS, preset);
        assert_eq!(e.kind(), kind);
        assert_eq!(e.sample_rate(), FS);
        e.set_master_gain(0.8);
        assert!((e.master_gain() - 0.8).abs() < 1e-6);
        e.set_tone(2.0, -1.0);
        assert_eq!(e.tone(), (2.0, -1.0));
        let script = vec![(0, PianoEvent::NoteOn { key: 60, velocity: 90 })];
        let (l, _) = render(&mut e, &script, (0.5 * FS) as usize, 256);
        assert!(rms(&l) > 1e-5, "{kind:?} engine silent through the wrapper");
        e.reset();
    }
}

/// Cost measurement, reported beside the physical engine's numbers:
///   cargo test -p makepad-piano-model --release --test learned -- --ignored perf_ --nocapture
#[test]
#[ignore]
fn perf_learned_polyphony() {
    use std::time::Instant;
    let seconds = 10.0;
    for (name, scalar) in [("simd", false), ("scalar", true)] {
        let mut p = LearnedPiano::new(FS);
        p.set_force_scalar(scalar);
        let mut script = vec![(0u64, PianoEvent::Sustain { value: 1.0 })];
        let mut t = 0.0;
        while t < seconds - 0.1 {
            for key in 21..=108u8 {
                script.push((sec(t + (key as f64 - 21.0) * 0.0001), PianoEvent::NoteOn { key, velocity: 110 }));
            }
            t += 1.5;
        }
        let total = (seconds * FS as f64) as usize;
        let start = Instant::now();
        let (l, _) = render(&mut p, &script, total, 512);
        let wall = start.elapsed().as_secs_f64();
        assert!(l[total - 1].is_finite());
        println!(
            "learned {name}: 88 keys re-struck under pedal (176 slots) + full fx: {wall:.3} s wall for {seconds} s = {:.1}x realtime ({:.1}% of one core)",
            seconds / wall,
            100.0 * wall / seconds
        );
    }
}

/// The experimental hybrid hook (Piano::debug_shape_partials) must actually
/// shape what it claims: a partial's output gain scales its measured level,
/// and a sigma_scale above 1 shortens its ring. (The learned-hybrid
/// experiments in tests/learned_targets.rs and the offline listening pack
/// build on this hook; this pins its semantics.)
#[test]
fn shape_partials_hook_scales_gain_and_decay() {
    let key = 60u8;
    let render_mono = |p: &mut Piano| -> Vec<f32> {
        p.set_reverb_mix(0.0);
        p.set_early_reflection_level(0.0);
        p.set_soft_clip(false);
        let script = vec![(0u64, PianoEvent::NoteOn { key, velocity: 100 })];
        let (l, r) = render(p, &script, (1.2 * FS) as usize, 512);
        l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect()
    };
    let dft = |x: &[f32], f: f64, t0: f64| -> f64 {
        let win = (0.046 * FS as f64) as usize;
        let a = (t0 * FS as f64) as usize;
        let seg = &x[a..a + win];
        let (mut re, mut im) = (0.0f64, 0.0f64);
        let w0 = std::f64::consts::TAU * f / FS as f64;
        for (k, &v) in seg.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / seg.len() as f64).cos();
            re += w * v as f64 * (w0 * k as f64).cos();
            im -= w * v as f64 * (w0 * k as f64).sin();
        }
        (re * re + im * im).sqrt()
    };
    // pol_det = 0: with the polarisation false-beat on, a single
    // fixed-instant DFT window lands on different phases of the beat in
    // the two renders and the sigma probe stops being monotone (a
    // doubled-sigma render once measured +3 dB at 0.8 s purely from
    // beat phase). The hook under test is orthogonal to the beat.
    let no_beat = {
        let mut dp = makepad_piano_model::DesignParams::default();
        dp.pol_det = 0.0;
        dp.scatter = 0.0;
        // the held key's own sympathetic bank shadows its partials with
        // UNSCALED decays and holds the 0.8 s level after the scaled
        // string has died — silence the resonance beds for the probe
        dp.sym_out = 0.0;
        dp.sym_damped = 0.0;
        dp.duplex_gain = 0.0;
        dp
    };
    let mut base = Piano::new_with_params(FS, &no_beat);
    let f0 = base.key_info(key).unwrap().f0 as f64;
    let b = base.key_info(key).unwrap().b_coeff as f64;
    let f2 = 2.0 * f0 * (1.0 + b * 4.0).sqrt();
    let xb = render_mono(&mut base);
    // Gain: partial 2 cut 12 dB, others untouched.
    let mut cut = Piano::new_with_params(FS, &no_beat);
    cut.debug_shape_partials(key, &[1.0, 0.25, 1.0, 1.0], &[]);
    let xc = render_mono(&mut cut);
    let drop = 20.0 * (dft(&xc, f2, 0.1) / dft(&xb, f2, 0.1)).log10();
    let keep = 20.0 * (dft(&xc, f0, 0.1) / dft(&xb, f0, 0.1)).log10();
    assert!((drop + 12.0).abs() < 2.0, "partial 2 moved {drop:.1} dB, wanted -12");
    assert!(keep.abs() < 1.0, "partial 1 moved {keep:.1} dB, wanted 0");
    // Decay: sigma doubled on every partial -> the 0.8 s level falls well
    // below the untouched instrument's while the onset stays put.
    let mut fast = Piano::new_with_params(FS, &no_beat);
    fast.debug_shape_partials(key, &[], &[2.0; 24]);
    let xf = render_mono(&mut fast);
    // Broadband RMS, not a single-frequency DFT: a partial is now a set
    // of coupled modes at (nearly) one frequency, and their coherent sum
    // sweeps through interference nulls as the fast member dies — a
    // fixed-instant single-line probe measured +0.2 dB for doubled sigma
    // purely because base and scaled renders sat on opposite sides of a
    // null. Energy across the band is monotone in sigma.
    let band_rms = |x: &[f32], t0: f64, t1: f64| -> f64 {
        let a = (t0 * FS as f64) as usize;
        let b = ((t1 * FS as f64) as usize).min(x.len());
        (x[a..b].iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / (b - a) as f64).sqrt()
    };
    let late = 20.0 * (band_rms(&xf, 0.6, 1.1) / band_rms(&xb, 0.6, 1.1)).log10();
    let onset = 20.0 * (band_rms(&xf, 0.03, 0.08) / band_rms(&xb, 0.03, 0.08)).log10();
    assert!(late < -2.5, "doubled sigma only moved the 0.6-1.1 s energy {late:.1} dB");
    assert!(onset > -4.5, "doubled sigma should barely touch the onset, moved {onset:.1} dB");
}
