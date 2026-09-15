//! Native-recording measurements and targeted default-model promotion gates.
//! Regenerate the reference-only TSV with tools/acoustic.py; no new dependencies.
//! The promotion contract targets missing bass body, early partial balance,
//! register loudness and treble brightness, while protecting C3 attack and touch.
//! It does not gate late high-register subfundamental/noise bands or divide by
//! small raw errors. These numerical regressions are neither full reference
//! matching nor perceptual proof; the old all-metrics test remains diagnostic.
mod common;

use common::{ev, fft, render, FS};
use makepad_piano_model::{calibration_data::DEFAULT_CALIBRATION, Piano, PianoEvent};
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

struct PromotionRow {
    reference: Reference,
    model: Measurement,
    raw: Measurement,
}

fn promotion_rows() -> &'static [PromotionRow] {
    // Fail every promotion gate explicitly until a real default is installed.
    assert!(!DEFAULT_CALIBRATION.is_empty(),
        "DEFAULT_CALIBRATION is empty; Piano::new cannot be promoted as an unchanged raw model");
    static ROWS: OnceLock<Vec<PromotionRow>> = OnceLock::new();
    ROWS.get_or_init(|| {
        let mut rows = fixture().0.into_iter().filter(|r| {
            [21, 24, 30, 36, 48, 60, 84].contains(&r.note)
                || ([69, 72].contains(&r.note) && r.velocity == 112)
        }).map(|reference| {
            // Fresh state for EACH constructor/note/velocity, with identical
            // dry 48 kHz settings. Share only the resulting stereo measurements.
            let dry_measure = |mut piano: Piano| {
                piano.set_reverb_mix(0.0);
                piano.set_early_reflection_level(0.0);
                piano.set_soft_clip(false);
                let event = ev(0.0, PianoEvent::NoteOn { key: reference.note, velocity: reference.velocity });
                let (l, r) = render(&mut piano, &[event], (4.0 * FS) as usize, 256);
                measure(&l, &r, reference.note)
            };
            let model = dry_measure(Piano::new(FS));
            let raw = dry_measure(Piano::new_uncalibrated(FS));
            PromotionRow { reference, model, raw }
        }).collect::<Vec<_>>();
        assert_eq!(rows.len(), 23);
        for i in 0..rows.len() {
            let c4 = rows.iter().position(|r| r.reference.note == 60
                && r.reference.velocity == rows[i].reference.velocity).unwrap();
            rows[i].model.metrics[6] = 20.0 * (rows[i].model.rms / rows[c4].model.rms).log10();
            rows[i].raw.metrics[6] = 20.0 * (rows[i].raw.rms / rows[c4].raw.rms).log10();
        }
        rows
    })
}

fn promotion_row(note: u8, velocity: u8) -> &'static PromotionRow {
    promotion_rows().iter().find(|r| r.reference.note == note && r.reference.velocity == velocity)
        .unwrap_or_else(|| panic!("missing promotion measurement: MIDI {note} v{velocity}"))
}

fn promotion_register(notes: &[u8]) -> Vec<&'static PromotionRow> {
    notes.iter().flat_map(|&note| [28, 68, 112].map(|v| promotion_row(note, v))).collect()
}

// The optional individual bound is max(absolute floor, raw error + margin).
// Compare aggregate errors by multiplication, never division by raw error.
#[track_caller]
fn assert_reference_promotion(metric: usize, rows: &[&PromotionRow], mean_factor: Option<f64>, individual: Option<(f64, f64)>) {
    assert!(!rows.is_empty());
    let name = METRICS[metric];
    let mut report = vec!["metric\tnote\tvelocity\tmodel\treference\traw\tmodel_abs_error\traw_abs_error\tlimit_db\tstatus".to_string()];
    let (mut model_sum, mut raw_sum) = (0.0, 0.0);
    let mut passed = true;
    for row in rows {
        let (model, reference, raw) = (row.model.metrics[metric], row.reference.metrics[metric], row.raw.metrics[metric]);
        let (model_error, raw_error) = ((model - reference).abs(), (raw - reference).abs());
        let limit = individual.map(|(floor, margin)| floor.max(raw_error + margin));
        let ok = [model, reference, raw, model_error, raw_error].iter().all(|v| v.is_finite())
            && limit.map_or(true, |limit| model_error <= limit);
        passed &= ok;
        model_sum += model_error;
        raw_sum += raw_error;
        report.push(format!("{name}\t{}\t{}\t{model:.6}\t{reference:.6}\t{raw:.6}\t{model_error:.6}\t{raw_error:.6}\t{}\t{}",
            row.reference.note, row.reference.velocity,
            limit.map_or_else(|| "-".into(), |v| format!("{v:.6}")), if ok { "ok" } else { "FAIL" }));
    }
    let (model_mean, raw_mean) = (model_sum / rows.len() as f64, raw_sum / rows.len() as f64);
    let mean_limit = mean_factor.map(|factor| factor * raw_mean);
    let mean_ok = model_mean.is_finite() && raw_mean.is_finite()
        && mean_limit.map_or(true, |limit| model_mean <= limit);
    report.push(format!("{name} mean_abs_error_db (n={}): before(raw)={raw_mean:.6} after(default)={model_mean:.6} {} [{}]",
        rows.len(), mean_limit.map_or_else(|| "individual bounds only".into(), |v| {
            format!("limit={v:.6} ({} * raw)", mean_factor.unwrap())
        }), if mean_ok { "ok" } else { "FAIL" }));
    assert!(passed && mean_ok, "targeted default-model promotion failed:\n{}", report.join("\n"));
}

#[test]
fn default_promotes_bass_sustained_body() {
    // Relative late mid-band share measures missing body, not total bass gain.
    assert_reference_promotion(2, &promotion_register(&[21, 24, 30, 36]), Some(0.65), Some((0.0, 3.0)));
}

#[test]
fn default_promotes_bass_early_partial_balance() {
    assert_reference_promotion(4, &promotion_register(&[21, 24, 30, 36]), Some(0.70), Some((0.0, 3.0)));
}

#[test]
fn default_promotes_register_loudness() {
    // C4 at the SAME velocity is only the anchor, never a counted success.
    assert_reference_promotion(6, &promotion_register(&[21, 24, 30, 36, 84]), Some(0.65), None);
}

#[test]
fn default_promotes_treble_early_brightness() {
    let rows = [(69, 112), (72, 112), (84, 68), (84, 112)].map(|(n, v)| promotion_row(n, v));
    assert_reference_promotion(1, &rows, Some(0.70), Some((0.0, 2.0)));
}

#[test]
fn default_protects_c3_attack() {
    // An earlier fit improved aggregate sustain by losing C3's attack.
    assert_reference_promotion(4, &promotion_register(&[48]), None, Some((8.0, 3.0)));
}

#[test]
fn default_preserves_c4_velocity_dynamics() {
    // The reference WAVs deliberately omit SFZ amp_veltrack=73 during timbre
    // fitting. Their amplitudes are context only: preserve RAW touch response.
    let anchor = promotion_row(60, 68);
    let mut report = vec!["metric\tnote\tvelocity\tmodel\treference\traw\tabs_model_minus_raw\tlimit_db\tstatus".to_string()];
    let mut passed = true;
    let mut error_sum = 0.0;
    for velocity in [28, 112] {
        let row = promotion_row(60, velocity);
        let model = 20.0 * (row.model.rms / anchor.model.rms).log10();
        let reference = 20.0 * (row.reference.rms / anchor.reference.rms).log10();
        let raw = 20.0 * (row.raw.rms / anchor.raw.rms).log10();
        let error = (model - raw).abs();
        let ok = [model, reference, raw, error].iter().all(|v| v.is_finite()) && error <= 3.0;
        passed &= ok;
        error_sum += error;
        report.push(format!("rms_velocity_vs_68_db\t60\t{velocity}\t{model:.6}\t{reference:.6}\t{raw:.6}\t{error:.6}\t3.000000\t{}",
            if ok { "ok" } else { "FAIL" }));
    }
    report.push(format!("mean_abs_error_vs_raw_db (n=2): before(raw)=0.000000 after(default)={:.6}; each must be <=3 dB", error_sum / 2.0));
    assert!(passed, "targeted default-model touch protection failed:\n{}", report.join("\n"));
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
#[ignore = "requires calibrated model; run explicitly during voicing; provisional tolerances are not final acceptance"]
fn stock_matches_native_acoustic_reference() {
    let (rows, limits) = fixture();
    let mut model = rows.iter().map(|row| {
        let mut piano = Piano::new(FS);
        let event = ev(0.0, PianoEvent::NoteOn { key: row.note, velocity: row.velocity });
        let (l, r) = render(&mut piano, &[event], (4.0 * FS) as usize, 256);
        measure(&l, &r, row.note)
    }).collect::<Vec<_>>();
    for (i, row) in rows.iter().enumerate() {
        let c4 = rows.iter().position(|r| r.note == 60 && r.velocity == row.velocity).unwrap();
        model[i].metrics[6] = 20.0 * (model[i].rms / model[c4].rms).log10();
    }
    let mut failures = Vec::new();
    for (row, measured) in rows.iter().zip(&model) {
        for (i, &name) in METRICS.iter().enumerate() {
            let delta = measured.metrics[i] - row.metrics[i];
            if !delta.is_finite() || delta.abs() > limits[i] {
                failures.push(format!("MIDI {} v{} {name}: model={:.6} reference={:.6} delta={delta:+.6} limit={:.6}",
                    row.note, row.velocity, measured.metrics[i], row.metrics[i], limits[i]));
            }
        }
    }
    assert!(failures.is_empty(), "{} deviations from native recordings (provisional, NOT final acceptance):\n{}",
        failures.len(), failures.join("\n"));
}
