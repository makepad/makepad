mod common;

use common::{render, render_mt, Ev, FS};
use makepad_piano_model::calibration::{CalibrationNote, CALIBRATION_PARTIALS, CALIBRATION_VELOCITIES};
use makepad_piano_model::{calibration_data::DEFAULT_CALIBRATION, DesignParams, Piano, PianoEvent, TimedEvent};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

const SAMPLE_RATES: [f32; 5] = [8000.0, 44100.0, 48000.0, 96000.0, 192000.0];

// Count only the measured thread, so the other tests can run concurrently.
thread_local! {
    static COUNTS: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}
struct CountingAlloc;
fn count(alloc: usize, dealloc: usize) {
    let _ = COUNTS.try_with(|c| {
        if let Some((a, d)) = c.get() {
            c.set(Some((a + alloc, d + dealloc)));
        }
    });
}
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count(0, 1);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(1, 1);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn note(key: u8, gains: [f32; 3], scale: f32) -> CalibrationNote {
    CalibrationNote {
        key,
        gain_db: gains.map(|db| [db; CALIBRATION_PARTIALS]),
        decay_scale: [scale; CALIBRATION_PARTIALS],
    }
}

fn fitted() -> [CalibrationNote; 3] {
    [note(30, [-12.0, -6.0, 0.0], 0.5), note(60, [-6.0, 0.0, 6.0], 1.0), note(90, [0.0, 6.0, 12.0], 2.0)]
}

fn score() -> Vec<Ev> {
    use PianoEvent::*;
    [
        (0, Sustain { value: 1.0 }),
        (1, NoteOn { key: 21, velocity: 28 }),
        (63, NoteOn { key: 60, velocity: 68 }),
        (64, NoteOn { key: 108, velocity: 112 }),
        (127, Sostenuto { on: true }),
        (253, NoteOff { key: 21 }),
        (513, SoftPedal { on: true }),
        (781, NoteOn { key: 60, velocity: 112 }),
        (1001, Sustain { value: 0.5 }),
        (1025, NoteOn { key: 36, velocity: 90 }),
        (1799, NoteOn { key: 60, velocity: 0 }),
        (2001, Sostenuto { on: false }),
        (2300, SoftPedal { on: false }),
        (2500, NoteOn { key: 21, velocity: 127 }),
        (3001, Sustain { value: 0.0 }),
        (4700, AllSoundOff),
        (4711, NoteOn { key: 90, velocity: 48 }),
    ].into_iter().map(|(at, ev)| Ev { at, ev }).collect()
}

#[track_caller]
fn assert_bits(a: &(Vec<f32>, Vec<f32>), b: &(Vec<f32>, Vec<f32>)) {
    assert_eq!(a.0.len(), b.0.len());
    for (i, (a, b)) in a.0.iter().chain(&a.1).zip(b.0.iter().chain(&b.1)).enumerate() {
        assert!(a.is_finite() && b.is_finite());
        assert_eq!(a.to_bits(), b.to_bits(), "sample {i}");
    }
}

#[test]
fn empty_and_neutral_tables_are_exactly_raw() {
    assert_empty_and_neutral_tables_are_exactly_raw(FS, 257, 17);
}

#[test]
fn empty_and_neutral_tables_are_exactly_raw_at_all_rates() {
    // Equal callback histories isolate constructor identity. The original
    // 48 kHz cross-block comparison remains covered separately above.
    for fs in SAMPLE_RATES {
        assert_empty_and_neutral_tables_are_exactly_raw(fs, 64, 64);
    }
}

fn assert_empty_and_neutral_tables_are_exactly_raw(fs: f32, first_block: usize, reset_block: usize) {
    let events = score();
    for scalar in [true, false] {
        let mut raw = Piano::new_with_params(fs, &DesignParams::default());
        raw.set_force_scalar(scalar);
        let expected = render(&mut raw, &events, 6000, 64);
        // reset() retains the existing limiter history. Compare equal
        // histories here, rather than claiming reset equals reconstruction.
        raw.reset();
        let after_reset = render(&mut raw, &events, 6000, 64);
        let mut constructors = vec![
            Piano::new_uncalibrated(fs),
            Piano::new_with_calibration(fs, &[]),
            Piano::new_with_calibration(fs, &[note(60, [0.0; 3], 1.0)]),
        ];
        if DEFAULT_CALIBRATION.is_empty() {
            constructors.push(Piano::new(fs));
        }
        for mut piano in constructors {
            piano.set_force_scalar(scalar);
            assert_bits(&expected, &render(&mut piano, &events, 6000, first_block));
            piano.reset();
            assert_bits(&after_reset, &render(&mut piano, &events, 6000, reset_block));
        }
    }
    assert!(Piano::new_uncalibrated(fs).calibration_debug(60).is_none());
}

#[test]
fn pitch_interpolation_is_continuous_and_clamped() {
    assert_eq!(CALIBRATION_PARTIALS, 240);
    assert_eq!(CALIBRATION_VELOCITIES, [28, 68, 112]);
    let piano = Piano::new_with_calibration(FS, &fitted());
    for key in 21..=108 {
        let c = piano.calibration_debug(key).unwrap();
        assert_eq!(c.key, key);
        let t = (key.clamp(30, 90) - 30) as f32 / 60.0;
        for m in 0..CALIBRATION_PARTIALS {
            for v in 0..3 {
                assert!((c.gain_db[v][m] - (-12.0 + 6.0 * v as f32 + 12.0 * t)).abs() < 2e-6);
            }
            assert!((c.decay_scale[m] - 0.5 * 4.0f32.powf(t)).abs() < 3e-7);
        }
    }
    for key in [0, 20, 109, 255] {
        assert!(piano.calibration_debug(key).is_none());
    }
}

#[test]
fn invalid_tables_are_rejected_at_construction() {
    let rejects = |notes: &[CalibrationNote]| {
        assert!(std::panic::catch_unwind(|| Piano::new_with_calibration(FS, notes)).is_err());
    };
    for key in [0, 20, 109, 255] {
        rejects(&[note(key, [0.0; 3], 1.0)]);
    }
    rejects(&[note(60, [0.0; 3], 1.0), note(60, [0.0; 3], 1.0)]);
    rejects(&[note(61, [0.0; 3], 1.0), note(60, [0.0; 3], 1.0)]);
    for gain in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -36.001, 24.001] {
        let mut n = note(60, [0.0; 3], 1.0);
        n.gain_db[2][CALIBRATION_PARTIALS - 1] = gain;
        rejects(&[n]);
    }
    for scale in [
        f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0, 0.099, 4.001,
        f32::from_bits(0.1f32.to_bits() - 1), f32::from_bits(4.0f32.to_bits() + 1),
    ] {
        let mut n = note(60, [0.0; 3], 1.0);
        n.decay_scale[CALIBRATION_PARTIALS - 1] = scale;
        rejects(&[n]);
    }
    for fs in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 7999.0, 192001.0] {
        assert!(std::panic::catch_unwind(|| Piano::new_with_calibration(fs, &fitted())).is_err());
    }
    for fs in SAMPLE_RATES {
        let _ = Piano::new_with_calibration(fs, &[note(21, [-36.0; 3], 0.1), note(108, [24.0; 3], 4.0)]);
    }
}

#[test]
fn decay_changes_only_radii_including_sympathetic_modes() {
    let mut max_raw = 0.0f64;
    let mut max_calibrated = (0.0f64, String::new());
    for fs in SAMPLE_RATES {
        let raw = Piano::new_uncalibrated(fs);
        assert_eq!(raw.keys_debug().len(), 88);
        for decay_scale in [0.1, 0.25, 1.0, 4.0] {
            let piano = Piano::new_with_calibration(fs, &[note(21, [0.0; 3], decay_scale)]);
            assert_eq!(piano.keys_debug().len(), 88);
            for (key, (a, b)) in (21..=108).zip(raw.keys_debug().iter().zip(piano.keys_debug())) {
                assert_eq!(a.gin, b.gin);
                assert_eq!(a.gout, b.gout);
                assert_eq!(a.gout_re, b.gout_re);
                assert_eq!(a.damp_mul, b.damp_mul);
                assert_eq!(a.sym_gin, b.sym_gin);
                assert_eq!(a.sym_gout, b.sym_gout);
                assert_eq!(a.sym_damp_mul, b.sym_damp_mul);
                for (bank, ar, ai, br, bi, stride) in [
                    ("active", &a.cr_sus, &a.ci_sus, &b.cr_sus, &b.ci_sus, a.modes_padded),
                    ("sympathetic", &a.sym_cr, &a.sym_ci, &b.sym_cr, &b.sym_ci, a.sym_modes),
                ] {
                    assert_eq!(ar.len(), br.len());
                    assert_eq!(ai.len(), bi.len());
                    for i in 0..ar.len() {
                        let m = i % stride;
                        let scale = if m < CALIBRATION_PARTIALS { decay_scale as f64 }
                            else { (decay_scale as f64).powf((CALIBRATION_PARTIALS + 15).saturating_sub(m) as f64 / 16.0) };
                        let r = (ar[i] as f64).hypot(ai[i] as f64);
                        let new_r = (br[i] as f64).hypot(bi[i] as f64);
                        assert!(r.is_finite() && new_r.is_finite());
                        assert!((new_r - r.powf(scale)).abs() < 8e-8);
                        assert!(new_r < 1.0, "fs={fs} scale={decay_scale} key={key} {bank} mode={i}: radius={new_r:.17}");
                        max_raw = max_raw.max(r);
                        if new_r > max_calibrated.0 {
                            max_calibrated = (new_r, format!(
                                "fs={fs} scale={decay_scale} key={key} {bank} mode={i} raw=({:?}, {:?}) calibrated=({:?}, {:?})",
                                ar[i], ai[i], br[i], bi[i],
                            ));
                        }
                        if r > 0.0 {
                            assert!(((ar[i] as f64).atan2(ai[i] as f64) - (br[i] as f64).atan2(bi[i] as f64)).abs() < 8e-8);
                        }
                        if scale == 1.0 || r == 0.0 {
                            assert_eq!(ar[i].to_bits(), br[i].to_bits());
                            assert_eq!(ai[i].to_bits(), bi[i].to_bits());
                        }
                    }
                }
            }
        }
    }
    // For every raw pole at these rates, exponent 0.1 gives the largest
    // ideal radius throughout the supported [0.1, 4] range (including the
    // tapered tail). Each component has magnitude < 1, so rounding to f32
    // moves it by at most 2^-25. The triangle inequality bounds the radius
    // error by sqrt(2) * 2^-25; also allow for f64 intermediate rounding.
    let radius_bound = max_raw.powf(0.1) + 2.0f64.sqrt() * 2.0f64.powi(-25) + 4.0 * f64::EPSILON;
    println!("maximum raw radius: {max_raw:.17}; all-scale rounding bound: {radius_bound:.17}");
    println!("maximum calibrated pole radius: {:.17} ({})", max_calibrated.0, max_calibrated.1);
    assert!(radius_bound < 1.0, "supported decay scales must leave room for f32 rounding");
}

#[test]
fn calibrated_render_is_deterministic_across_blocks_kernels_and_multicore() {
    let notes = fitted();
    let events = score();
    let total = 10000;
    let mut outputs = Vec::new();
    for scalar in [false, true] {
        let mut piano = Piano::new_with_calibration(FS, &notes);
        piano.set_force_scalar(scalar);
        let expected = render(&mut piano, &events, total, 64);
        for block in [1, 17, 257] {
            let mut piano = Piano::new_with_calibration(FS, &notes);
            piano.set_force_scalar(scalar);
            assert_bits(&expected, &render(&mut piano, &events, total, block));
        }
        let mut piano = Piano::new_with_calibration(FS, &notes);
        piano.set_force_scalar(scalar);
        assert_bits(&expected, &render_mt(&mut piano, &events, total, 1024, 3));
        outputs.push(expected);
    }
    for channel in [0, 1] {
        let a = if channel == 0 { &outputs[0].0 } else { &outputs[0].1 };
        let b = if channel == 0 { &outputs[1].0 } else { &outputs[1].1 };
        let err: f64 = a.iter().zip(b).map(|(a, b)| (*a as f64 - *b as f64).powi(2)).sum();
        let energy: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum();
        assert!(energy > 0.0);
        assert!((err / energy).sqrt() < 1e-3);
    }
}

#[test]
fn calibrated_first_strikes_restrikes_and_pedals_never_allocate() {
    let mut piano = Piano::new_with_calibration(FS, &fitted());
    let mut l = [0.0; 512];
    let mut r = [0.0; 512];
    COUNTS.with(|c| c.set(Some((0, 0))));
    // Include the very first note-on; calibration must not need a warm-up.
    for pass in 0..3 {
        for key in 21..=108 {
            piano.process(&[
                TimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity: [28, 68, 112][pass] } },
                TimedEvent { offset: 63, event: PianoEvent::Sustain { value: [0.0, 0.5, 1.0][pass] } },
                TimedEvent { offset: 127, event: PianoEvent::NoteOn { key, velocity: 90 } },
                TimedEvent { offset: 256, event: PianoEvent::NoteOff { key } },
            ], &mut l, &mut r);
        }
        piano.reset();
    }
    let counts = COUNTS.with(|c| c.replace(None)).unwrap();
    assert_eq!(counts, (0, 0), "callback allocated/deallocated");
}
