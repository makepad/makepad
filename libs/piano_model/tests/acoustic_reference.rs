//! Native-recording measurements and ABSOLUTE default-model gates: the
//! stock instrument (Piano::new, dry) must sit within fixed tolerances of the
//! Salamander recordings' own metrics — not merely closer than the raw model.
//! Gates that only asked the calibrated model to beat the uncalibrated one
//! passed while the bottom octave still decayed like a plucked bass (2-8 kHz
//! falling 13 dB/s faster than the recordings, late 2-8 kHz share 11 dB
//! low). Regenerate the reference-only TSV with tools/acoustic.py.
mod common;

use common::{ev, fft, render, FS};
use makepad_piano_model::{Piano, PianoEvent, PIANO_PRESETS};
use std::sync::OnceLock;

const FIXTURE: &str = include_str!("data/salamander_v3.tsv");
const METRICS: [&str; 11] = [
    "early_mid_share_db", "early_high_share_db", "late_mid_share_db", "late_high_share_db",
    "p1_cluster_50_300_db", "p1_cluster_1_2_db", "rms_register_db", "onset_energy_5_over_50",
    "decay_low_db_s", "decay_mid_db_s", "decay_high_db_s",
];
const NOTES: [u8; 12] = [21, 24, 30, 33, 36, 45, 48, 60, 69, 72, 84, 96];
const BANDS: [(f64, f64); 3] = [(20.0, 500.0), (500.0, 2000.0), (2000.0, 8000.0)];

struct Reference {
    note: u8,
    velocity: u8,
    rms: f64,
    metrics: [f64; 11],
}

fn fixture() -> (Vec<Reference>, Vec<f64>) {
    assert!(FIXTURE.starts_with("# salamander-acoustic-v1\n"));
    let limits = FIXTURE.lines().find_map(|line| line.strip_prefix("# thresholds_abs\t"))
        .unwrap().split('\t').map(|s| s.parse().unwrap()).collect::<Vec<f64>>();
    let mut lines = FIXTURE.lines().filter(|line| !line.starts_with('#'));
    let header = lines.next().unwrap().split('\t').collect::<Vec<_>>();
    assert_eq!(&header[10..], &METRICS);
    let rows = lines.map(|line| {
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(fields.len(), 10 + METRICS.len());
        let note = fields[0].parse().unwrap();
        let velocity = fields[1].parse().unwrap();
        let (layer, lo, hi) = match velocity {
            28 => (2, 27, 34), 68 => (9, 65, 72), 112 => (14, 105, 112),
            _ => panic!("unexpected fixture velocity"),
        };
        assert_eq!(fields[2].parse::<u8>().unwrap(), layer);
        assert_eq!(fields[3].parse::<u8>().unwrap(), lo);
        assert_eq!(fields[4].parse::<u8>().unwrap(), hi);
        assert!(fields[5].starts_with("48khz24bit/"));
        assert!(fields[5].ends_with(&format!("v{layer}.wav")));
        assert_eq!(fields[6].len(), 64);
        assert!(fields[6].bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(fields[7].parse::<usize>().unwrap() < 24000);
        assert_eq!(fields[8].parse::<u32>().unwrap(), FS as u32);
        let metrics = std::array::from_fn(|i| fields[10 + i].parse::<f64>().unwrap());
        assert!(metrics.iter().all(|value| value.is_finite()));
        Reference { note, velocity, rms: fields[9].parse().unwrap(), metrics }
    }).collect();
    (rows, limits)
}

fn energy(l: &[f32], r: &[f32]) -> f64 {
    assert_eq!(l.len(), r.len());
    l.iter().zip(r).map(|(&a, &b)| 0.5 * ((a as f64).powi(2) + (b as f64).powi(2))).sum()
}

fn onset(l: &[f32], r: &[f32]) -> usize {
    let frame = (FS as f64 * 0.001).round() as usize;
    let end = l.len().min((FS * 0.5) as usize);
    let powers = (0..end / frame).map(|i| energy(&l[i * frame..(i + 1) * frame], &r[i * frame..(i + 1) * frame]))
        .collect::<Vec<_>>();
    let peak = powers.iter().copied().fold(0.0, f64::max);
    assert!(peak > 0.0, "silent first 0.5s; cannot align onset");
    powers.iter().position(|&p| p > peak * 1e-4).unwrap() * frame
}

// Same periodic Hann, padding, per-channel power and normalization as Python.
fn spectrum(l: &[f32], r: &[f32]) -> (f64, Vec<f64>) {
    assert_eq!(l.len(), r.len());
    let n = l.len().next_power_of_two();
    let window = (0..l.len()).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / l.len() as f64).cos()).collect::<Vec<_>>();
    let norm = n as f64 * window.iter().map(|w| w * w).sum::<f64>();
    let mut power = vec![0.0; n / 2 + 1];
    for channel in [l, r] {
        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        for (i, &value) in channel.iter().enumerate() {
            re[i] = value as f64 * window[i];
        }
        fft(&mut re, &mut im);
        for k in 0..=n / 2 {
            let one_sided = if k == 0 || k == n / 2 { 1.0 } else { 2.0 };
            power[k] += 0.5 * one_sided * (re[k] * re[k] + im[k] * im[k]) / norm;
        }
    }
    (FS as f64 / n as f64, power)
}

fn band(spec: &(f64, Vec<f64>), lo: f64, hi: f64) -> f64 {
    let start = (lo / spec.0).ceil() as usize;
    let end = ((hi / spec.0).ceil() as usize).min(spec.1.len());
    if start >= end { 0.0 } else { spec.1[start..end].iter().sum() }
}

fn db_ratio(a: f64, b: f64) -> f64 {
    assert!(b > 0.0);
    10.0 * (a / b).max(1e-15).log10()
}

fn section(x: &[f32], a: f64, b: f64) -> &[f32] {
    &x[(a * FS as f64).round() as usize..(b * FS as f64).round() as usize]
}

struct Measurement {
    rms: f64,
    metrics: [f64; 11],
}

fn measure(l: &[f32], r: &[f32], note: u8) -> Measurement {
    let offset = onset(l, r);
    let (l, r) = (&l[offset..], &r[offset..]);
    assert!(l.len() >= 2 * FS as usize && l.len() == r.len());
    let spec = |a, b| spectrum(section(l, a, b), section(r, a, b));
    let mut metrics = [0.0; 11];
    for (i, (a, b)) in [(0.05, 0.1), (1.0, 2.0)].into_iter().enumerate() {
        let s = spec(a, b);
        let total = band(&s, 20.0, 20000.0);
        metrics[2 * i] = db_ratio(band(&s, 500.0, 2000.0), total);
        metrics[2 * i + 1] = db_ratio(band(&s, 2000.0, 8000.0), total);
    }
    let f0 = 440.0 * 2.0f64.powf((note as f64 - 69.0) / 12.0);
    for (i, (a, b)) in [(0.05, 0.3), (1.0, 2.0)].into_iter().enumerate() {
        let s = spec(a, b);
        let partials = (1..=6).map(|p| band(&s, (p as f64 - 0.4) * f0, (p as f64 + 0.4) * f0)).collect::<Vec<_>>();
        metrics[4 + i] = db_ratio(partials[0], partials[1..].iter().copied().fold(0.0, f64::max));
    }
    let mean_square = energy(section(l, 0.0, 2.0), section(r, 0.0, 2.0)) / (2.0 * FS as f64);
    metrics[7] = energy(section(l, 0.0, 0.005), section(r, 0.0, 0.005))
        / energy(section(l, 0.0, 0.05), section(r, 0.0, 0.05));
    let times = (0..17).map(|i| 0.1 + i as f64 * 0.05).collect::<Vec<_>>();
    let spectra = times.iter().map(|&t| spec(t, t + 0.1)).collect::<Vec<_>>();
    for (i, (lo, hi)) in BANDS.into_iter().enumerate() {
        let powers = spectra.iter().map(|s| 10.0 * band(s, lo, hi).max(mean_square * 1e-15).log10()).collect::<Vec<_>>();
        metrics[8 + i] = -common::linreg_slope(&times, &powers).unwrap();
    }
    Measurement { rms: mean_square.sqrt(), metrics }
}

/// Dry stock measurements for every fixture row, rendered once (fresh
/// instrument per note/velocity), with the register metric against the
/// model's own C4 at the same velocity.
fn stock_rows() -> &'static [(Reference, Measurement)] {
    static ROWS: OnceLock<Vec<(Reference, Measurement)>> = OnceLock::new();
    ROWS.get_or_init(|| {
        let mut rows = fixture().0.into_iter().map(|reference| {
            let mut piano = Piano::new(FS);
            piano.set_reverb_mix(0.0);
            piano.set_early_reflection_level(0.0);
            piano.set_soft_clip(false);
            let event = ev(0.0, PianoEvent::NoteOn { key: reference.note, velocity: reference.velocity });
            // measure() reads at most 2.0 s after an onset found in the
            // first 0.5 s
            let (l, r) = render(&mut piano, &[event], (2.5 * FS as f64) as usize, 256);
            let m = measure(&l, &r, reference.note);
            (reference, m)
        }).collect::<Vec<_>>();
        for i in 0..rows.len() {
            let c4 = rows.iter().position(|r| r.0.note == 60 && r.0.velocity == rows[i].0.velocity).unwrap();
            rows[i].1.metrics[6] = 20.0 * (rows[i].1.rms / rows[c4].1.rms).log10();
        }
        rows
    })
}

/// The same rows through the path the apps play: `Piano::new_with_preset`
/// with the default preset's voicing and room (reverb, early reflections,
/// limiter) left on.
fn preset_rows() -> &'static [(Reference, Measurement)] {
    static ROWS: OnceLock<Vec<(Reference, Measurement)>> = OnceLock::new();
    ROWS.get_or_init(|| {
        let preset = PIANO_PRESETS.iter().find(|p| p.is_default).unwrap();
        let mut rows = fixture().0.into_iter().filter(|r| r.note <= 60).map(|reference| {
            let mut piano = Piano::new_with_preset(FS, preset);
            let event = ev(0.0, PianoEvent::NoteOn { key: reference.note, velocity: reference.velocity });
            let (l, r) = render(&mut piano, &[event], (2.5 * FS as f64) as usize, 256);
            let m = measure(&l, &r, reference.note);
            (reference, m)
        }).collect::<Vec<_>>();
        for i in 0..rows.len() {
            let c4 = rows.iter().position(|r| r.0.note == 60 && r.0.velocity == rows[i].0.velocity).unwrap();
            rows[i].1.metrics[6] = 20.0 * (rows[i].1.rms / rows[c4].1.rms).log10();
        }
        rows
    })
}

fn stock_row(note: u8, velocity: u8) -> &'static (Reference, Measurement) {
    stock_rows().iter().find(|r| r.0.note == note && r.0.velocity == velocity)
        .unwrap_or_else(|| panic!("missing measurement: MIDI {note} v{velocity}"))
}

/// Per metric: the mean |model - reference| over `notes` x {28, 68, 112}
/// must stay within `mean_limit`, and every row within `row_limit`
/// (None skips the metric).
#[track_caller]
fn assert_matches_reference(notes: &[u8], mean_limit: [Option<f64>; 11], row_limit: [Option<f64>; 11]) {
    let rows = notes.iter().flat_map(|&n| [28, 68, 112].map(|v| stock_row(n, v))).collect::<Vec<_>>();
    assert_rows_match(&rows, mean_limit, row_limit);
}

#[track_caller]
fn assert_rows_match(rows: &[&(Reference, Measurement)], mean_limit: [Option<f64>; 11], row_limit: [Option<f64>; 11]) {
    let mut report = vec!["metric\tnote\tvelocity\tmodel\treference\tabs_error\trow_limit\tstatus".to_string()];
    let mut passed = true;
    for (i, name) in METRICS.iter().enumerate() {
        let Some(mean_lim) = mean_limit[i] else { continue };
        let mut sum = 0.0;
        for (reference, model) in rows.iter().copied() {
            let (m, r) = (model.metrics[i], reference.metrics[i]);
            let err = (m - r).abs();
            let ok = err.is_finite() && row_limit[i].map_or(true, |l| err <= l);
            passed &= ok;
            sum += err;
            if !ok {
                report.push(format!("{name}\t{}\t{}\t{m:.3}\t{r:.3}\t{err:.3}\t{:?}\tFAIL",
                    reference.note, reference.velocity, row_limit[i]));
            }
        }
        let mean = sum / rows.len() as f64;
        let ok = mean <= mean_lim;
        passed &= ok;
        report.push(format!("{name}: mean abs error {mean:.3} (limit {mean_lim}) {}", if ok { "ok" } else { "FAIL" }));
    }
    println!("{}", report.join("\n"));
    assert!(passed, "default model outside the native-reference tolerances:\n{}", report.join("\n"));
}

#[test]
fn bass_register_matches_native_reference() {
    // A0..C3, measured 2026-09-29 at (mean / worst row): early 0.5-2 kHz
    // share 1.8/6.8, early 2-8 kHz 4.0/12.2, late 0.5-2 kHz 1.9/5.6, late
    // 2-8 kHz 3.4/10.9, fundamental/cluster 3.7/9.1 and 6.3/13.3, register
    // 1.4/2.9 dB, onset share 0.03/0.09, decays 2.1/8.3, 2.8/8.2 and
    // 2.3/6.4 dB/s. The instrument before the low-register rework failed
    // four of these means (early/late 2-8 kHz share 6.4/11.1, 0.5-2 kHz
    // decay 4.5, 2-8 kHz decay 13.4 dB/s) and the late-share row limits.
    assert_matches_reference(
        &[21, 24, 30, 33, 36, 45, 48],
        [Some(2.5), Some(5.0), Some(2.5), Some(4.5), Some(4.5), Some(7.5), Some(2.0), Some(0.05), Some(3.0), Some(3.5), Some(3.0)],
        [Some(9.0), Some(15.0), Some(8.0), Some(14.0), Some(12.0), Some(16.0), Some(4.0), Some(0.15), Some(11.0), Some(11.0), Some(9.0)],
    );
}

#[test]
fn preset_path_keeps_the_bass() {
    // What Stage and the score apps play (audio_synth's PianoInstrument and
    // score_ui build Piano::new_with_preset): the room and the output stage
    // must not undo the bass voicing, so the dry bass gate's means hold
    // with the default room on (the late windows carry its reverb).
    let rows = preset_rows().iter().filter(|r| r.0.note < 60).collect::<Vec<_>>();
    assert_rows_match(
        &rows,
        [Some(2.5), Some(5.0), Some(2.5), Some(4.5), Some(4.5), Some(7.5), Some(2.0), Some(0.05), Some(3.0), Some(3.5), Some(3.0)],
        [None; 11],
    );
}

#[test]
fn mid_and_treble_hold_the_native_reference() {
    // C4, A4, C5, C6: limits are the previous instrument's mean errors plus
    // ~0.5 dB (the 20-500 Hz decay is skipped: above C5 that band holds no
    // partial, only the recordings' 50 Hz mains harmonics).
    assert_matches_reference(
        &[60, 69, 72, 84],
        [Some(1.4), Some(3.0), Some(3.2), Some(7.5), Some(1.4), Some(7.2), Some(1.3), Some(0.05), None, Some(12.4), Some(6.5)],
        [None; 11],
    );
    // C7: early metrics only. Its 1-2 s window sits ~5 dB above the
    // recording's own noise floor, and below 2 kHz that window is mostly
    // mains hum (50/100/150 Hz), so the late shares and the late
    // fundamental/cluster ratio measure the room, not the string.
    assert_matches_reference(
        &[96],
        [Some(21.7), Some(1.2), None, None, Some(8.2), None, Some(4.6), Some(0.22), None, Some(8.4), Some(4.7)],
        [None; 11],
    );
}

#[test]
fn default_follows_the_reference_touch() {
    // The fixture's layer levels carry no SFZ amp_veltrack (73); played
    // through its SFZ the reference instrument's level at v28 and v112 re
    // v68 is the layer ratio times (0.27 + 0.73 (v/127)^2). The previous
    // table followed the raw model's touch instead: pianissimo 1-3 dB and
    // fortissimo up to 6 dB too soft re mezzo (mean error 2.2 dB).
    let gain = |v: f64| 0.27 + 0.73 * (v / 127.0).powi(2);
    let mut report = Vec::new();
    let mut sum = 0.0;
    let mut worst: f64 = 0.0;
    for &note in &NOTES {
        let anchor = stock_row(note, 68);
        for velocity in [28u8, 112] {
            let row = stock_row(note, velocity);
            let model = 20.0 * (row.1.rms / anchor.1.rms).log10();
            let reference = 20.0 * (row.0.rms / anchor.0.rms).log10() + 20.0 * (gain(velocity as f64) / gain(68.0)).log10();
            let err = (model - reference).abs();
            sum += err;
            worst = worst.max(err);
            report.push(format!("MIDI {note} v{velocity} re v68: model {model:+.2} dB, reference {reference:+.2} dB"));
        }
    }
    let mean = sum / (2 * NOTES.len()) as f64;
    println!("{}\nmean abs error {mean:.2} dB, worst {worst:.2} dB", report.join("\n"));
    assert!(mean <= 0.5 && worst <= 2.0, "touch off the reference:\n{}", report.join("\n"));
}

#[test]
fn reference_fixture_is_complete_and_attributed() {
    let (rows, limits) = fixture();
    assert_eq!(limits, [6.0, 6.0, 6.0, 6.0, 6.0, 6.0, 6.0, 0.15, 8.0, 8.0, 8.0]);
    assert!(FIXTURE.contains("Alexander Holm") && FIXTURE.contains("CC BY 3.0"));
    assert!(FIXTURE.contains("sfz_sha256") && FIXTURE.contains("readme_sha256"));
    let mut pairs = rows.iter().map(|row| (row.note, row.velocity)).collect::<Vec<_>>();
    pairs.sort_unstable();
    let expected = NOTES.into_iter().flat_map(|note| [28, 68, 112].map(|velocity| (note, velocity))).collect::<Vec<_>>();
    assert_eq!(pairs, expected);
    for row in &rows {
        assert!(row.rms.is_finite() && row.rms > 0.0);
        let c4 = rows.iter().find(|r| r.note == 60 && r.velocity == row.velocity).unwrap();
        assert!((row.metrics[6] - 20.0 * (row.rms / c4.rms).log10()).abs() < 1e-7);
        assert!((0.0..=1.0).contains(&row.metrics[7]));
        assert!(row.metrics[..4].iter().all(|&share| share <= 0.0));
    }
}

#[test]
fn independent_stereo_power_preserves_antiphase_and_gain_ratios() {
    let l = (0..12000).map(|i| {
        let phase = std::f64::consts::TAU * i as f64 / FS as f64;
        ((1000.0 * phase).sin() + 0.5 * (4000.0 * phase).sin()) as f32
    }).collect::<Vec<_>>();
    let r = l.iter().map(|&x| -x).collect::<Vec<_>>();
    let s = spectrum(&l, &r);
    assert!((band(&s, 500.0, 2000.0) - 0.5).abs() < 1e-7);
    assert!((band(&s, 2000.0, 8000.0) - 0.125).abs() < 1e-7);
    let ratio = db_ratio(band(&s, 2000.0, 8000.0), band(&s, 20.0, 20000.0));
    for gain in [0.0001, 0.1, 4.0] {
        let scaled_l = l.iter().map(|v| gain * v).collect::<Vec<_>>();
        let scaled_r = r.iter().map(|v| gain * v).collect::<Vec<_>>();
        let s = spectrum(&scaled_l, &scaled_r);
        let scaled_ratio = db_ratio(band(&s, 2000.0, 8000.0), band(&s, 20.0, 20000.0));
        assert!((ratio - scaled_ratio).abs() < 1e-6);
        assert_eq!(onset(&scaled_l, &scaled_r), onset(&l, &r));
    }
}

#[test]
fn onset_and_decay_have_the_documented_units() {
    let mut l = vec![0.0; 480];
    l.extend((0..105600).map(|i| {
        let t = i as f64 / FS as f64;
        ((std::f64::consts::TAU * 1000.0 * t).sin() * (-t).exp()) as f32
    }));
    let r = l.iter().map(|&x| -x).collect::<Vec<_>>();
    assert_eq!(onset(&l, &r), 480);
    let m = measure(&l, &r, 60);
    assert!((m.metrics[9] - 20.0 / 10.0f64.ln()).abs() < 1e-6);
}

#[test]
#[ignore = "diagnostic: every row within the provisional per-row limits is not yet met"]
fn stock_matches_native_acoustic_reference() {
    let (_, limits) = fixture();
    let mut failures = Vec::new();
    for (reference, measured) in stock_rows() {
        for (i, &name) in METRICS.iter().enumerate() {
            let delta = measured.metrics[i] - reference.metrics[i];
            if !delta.is_finite() || delta.abs() > limits[i] {
                failures.push(format!("MIDI {} v{} {name}: model={:.6} reference={:.6} delta={delta:+.6} limit={:.6}",
                    reference.note, reference.velocity, measured.metrics[i], reference.metrics[i], limits[i]));
            }
        }
    }
    assert!(failures.is_empty(), "{} deviations from native recordings (provisional, NOT final acceptance):\n{}",
        failures.len(), failures.join("\n"));
}

const AFTERSOUND: &str = include_str!("data/salamander_bass_aftersound.tsv");

/// Median late (1.5-3.5 s) decay, dB/s, of partials `lo..=hi`, measured
/// the way tools produced the fixture: 0.2 s Hann band power around f_n.
fn late_group_slope(l: &[f32], r: &[f32], f0: f64, b: f64, lo: usize, hi: usize) -> f64 {
    let times = (0..41).map(|i| 1.5 + 0.05 * i as f64).collect::<Vec<_>>();
    let spectra = times.iter().map(|&t| spectrum(section(l, t, t + 0.2), section(r, t, t + 0.2))).collect::<Vec<_>>();
    let mut slopes = (lo..=hi).map(|n| {
        let n = n as f64;
        let f = n * f0 * (1.0 + b * n * n).sqrt();
        let half = (0.3 * f0).min(12.0);
        let db = spectra.iter().map(|s| 10.0 * band(s, f - half, f + half).max(1e-30).log10()).collect::<Vec<_>>();
        -common::linreg_slope(&times, &db).unwrap()
    }).collect::<Vec<_>>();
    slopes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let m = slopes.len();
    if m % 2 == 1 { slopes[m / 2] } else { 0.5 * (slopes[m / 2 - 1] + slopes[m / 2]) }
}

#[test]
fn bass_aftersound_and_scale_match_native_reference() {
    // The plucked-bass signature was a missing double decay: the old model's
    // bottom octaves decayed at 4-10 dB/s straight through where the
    // recordings settle onto a 1-5 dB/s aftersound (mean errors 2.8 and
    // 3.1 dB/s over these rows; now 2.0 and 1.7), on a nearly harmonic
    // scale (A0 B 2.4e-5 against the recording's 2.2e-4).
    assert!(AFTERSOUND.starts_with("# salamander-bass-aftersound-v1\n"));
    assert!(AFTERSOUND.contains("Alexander Holm") && AFTERSOUND.contains("CC BY 3.0"));
    let rows = AFTERSOUND.lines().filter(|l| !l.starts_with('#')).skip(1).map(|line| {
        let f = line.split('\t').collect::<Vec<_>>();
        (f[0].parse::<u8>().unwrap(), f[2].parse::<f64>().unwrap(), f[3].parse::<f64>().unwrap(), f[4].parse::<f64>().unwrap())
    }).collect::<Vec<_>>();
    assert_eq!(rows.len(), 10);
    let (mut e_low, mut e_mid) = (0.0, 0.0);
    let mut report = Vec::new();
    for &(note, ref_low, ref_mid, ref_b) in &rows {
        let mut piano = Piano::new(FS);
        piano.set_reverb_mix(0.0);
        piano.set_early_reflection_level(0.0);
        piano.set_soft_clip(false);
        let info = piano.key_info(note).unwrap();
        assert!((info.b_coeff as f64 / ref_b - 1.0).abs() < 0.12,
            "MIDI {note}: B {:.3e} vs the recording's {ref_b:.3e}", info.b_coeff);
        let event = ev(0.0, PianoEvent::NoteOn { key: note, velocity: 68 });
        let (l, r) = render(&mut piano, &[event], (3.8 * FS as f64) as usize, 256);
        let offset = onset(&l, &r);
        let (l, r) = (&l[offset..], &r[offset..]);
        let (f0, b) = (info.f0 as f64, info.b_coeff as f64);
        let low = late_group_slope(l, r, f0, b, 2, 6);
        let mid = late_group_slope(l, r, f0, b, 7, 20);
        e_low += (low - ref_low).abs();
        e_mid += (mid - ref_mid).abs();
        report.push(format!("MIDI {note}: partials 2-6 {low:+.2} dB/s (ref {ref_low:+.2}), 7-20 {mid:+.2} (ref {ref_mid:+.2})"));
    }
    let (e_low, e_mid) = (e_low / rows.len() as f64, e_mid / rows.len() as f64);
    println!("{}\nmean abs error: 2-6 {e_low:.2} dB/s, 7-20 {e_mid:.2} dB/s", report.join("\n"));
    assert!(e_low <= 2.5 && e_mid <= 2.3,
        "bass aftersound off the recordings (2-6: {e_low:.2}, 7-20: {e_mid:.2} dB/s):\n{}", report.join("\n"));
}
