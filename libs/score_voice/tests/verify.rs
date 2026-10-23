//! Numerical verification of the singer (no listening): formant positions
//! per vowel against the classic measured averages, pitch accuracy, no
//! clicks at syllable joins (spectral flux), determinism, and the analysis
//! round trip. `--nocapture` prints the measurements.

use makepad_score_voice::analysis::{self, VocalTrack, TrackPlayer};
use makepad_score_voice::fft::Fft;
use makepad_score_voice::pitch::{hz_to_midi, midi_to_hz};
use makepad_score_voice::render::render_line;
use makepad_score_voice::*;
use makepad_score_voice::phoneme;
use std::sync::Arc;

const RATE: f32 = 48_000.0;

fn sing(profile: &str, style: Style, notes: &[(f32, f32, &str)], rate: f32) -> Vec<f32> {
    let mut singer = Singer::new(Arc::new(VoiceProfile::named(profile).unwrap()), rate, style, 7);
    let mut out = Vec::new();
    for (i, (note, dur, ipa)) in notes.iter().enumerate() {
        let flags = if i > 0 { PhonemeSeq::LEGATO } else { 0 };
        singer.note_on(0, SungNote { note: *note, vel: 0.8, dur: *dur, syl: PhonemeSeq::ipa(ipa).with_flags(flags) });
        let n = (*dur * rate) as usize;
        let mut buf = vec![0.0; n];
        singer.render(&mut buf);
        out.extend(buf);
    }
    let mut tail = vec![0.0; (0.3 * rate) as usize];
    singer.render(&mut tail);
    out.extend(tail);
    out
}

/// Formant frequencies of `x` by LPC: decimate to 12 kHz, pre-emphasise,
/// autocorrelation LPC of order 14, peaks of the model spectrum.
fn lpc_formants(x: &[f32], rate: f32) -> Vec<f32> {
    let factor = (rate / 12_000.0).round() as usize;
    let taps = 63;
    let cutoff = 5200.0 / rate;
    let h: Vec<f32> = (0..taps)
        .map(|i| {
            let m = i as f32 - (taps - 1) as f32 / 2.0;
            let sinc = if m == 0.0 { 2.0 * cutoff } else { (std::f32::consts::TAU * cutoff * m).sin() / (std::f32::consts::PI * m) };
            sinc * (0.54 - 0.46 * (std::f32::consts::TAU * i as f32 / (taps - 1) as f32).cos())
        })
        .collect();
    let y: Vec<f64> = (0..(x.len() - taps) / factor).map(|i| (0..taps).map(|k| h[k] * x[i * factor + k]).sum::<f32>() as f64).collect();
    let fs = rate as f64 / factor as f64;
    let mut e: Vec<f64> = y.windows(2).map(|w| w[1] - 0.94 * w[0]).collect();
    let n = e.len();
    for (i, v) in e.iter_mut().enumerate() {
        *v *= 0.54 - 0.46 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos();
    }
    let order = 14;
    let r: Vec<f64> = (0..=order).map(|k| (0..n - k).map(|i| e[i] * e[i + k]).sum()).collect();
    let mut a = vec![0.0f64; order + 1];
    a[0] = 1.0;
    let mut err = r[0];
    for i in 1..=order {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let k = -acc / err;
        let prev = a.clone();
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
        err *= 1.0 - k * k;
    }
    let spec: Vec<f64> = (0..5000)
        .map(|f| {
            let w = std::f64::consts::TAU * f as f64 / fs;
            let (mut re, mut im) = (0.0, 0.0);
            for (j, aj) in a.iter().enumerate() {
                re += aj * (w * j as f64).cos();
                im -= aj * (w * j as f64).sin();
            }
            -10.0 * (re * re + im * im).log10()
        })
        .collect();
    (150..4999).filter(|f| spec[*f] > spec[f - 1] && spec[*f] >= spec[f + 1]).map(|f| f as f32).collect()
}

fn nearest(peaks: &[f32], want: f32) -> f32 {
    peaks.iter().cloned().min_by(|a, b| (a - want).abs().partial_cmp(&(b - want).abs()).unwrap()).unwrap_or(0.0)
}

#[test]
fn vowel_formants_match_measured_averages() {
    // (vowel, male F1..F3, female F1..F3): the classic averages.
    let table: [(&str, [f32; 3], [f32; 3]); 8] = [
        ("i", [270.0, 2290.0, 3010.0], [310.0, 2790.0, 3310.0]),
        ("ɪ", [390.0, 1990.0, 2550.0], [430.0, 2480.0, 3070.0]),
        ("ɛ", [530.0, 1840.0, 2480.0], [610.0, 2330.0, 2990.0]),
        ("æ", [660.0, 1720.0, 2410.0], [860.0, 2050.0, 2850.0]),
        ("ɑ", [730.0, 1090.0, 2440.0], [850.0, 1220.0, 2810.0]),
        ("ɔ", [570.0, 840.0, 2410.0], [590.0, 920.0, 2710.0]),
        ("u", [300.0, 870.0, 2240.0], [370.0, 950.0, 2670.0]),
        ("ʌ", [640.0, 1190.0, 2390.0], [760.0, 1400.0, 2780.0]),
    ];
    let mut worst: f32 = 0.0;
    for (profile, note) in [("baritone", 45.0), ("alto", 55.0)] {
        for (v, male, female) in &table {
            let want = if profile == "baritone" { male } else { female };
            let x = sing(profile, Style::straight(), &[(note, 1.2, v)], RATE);
            let mid = &x[(0.4 * RATE) as usize..(0.9 * RATE) as usize];
            let peaks = lpc_formants(mid, RATE);
            let got: Vec<f32> = want.iter().map(|w| nearest(&peaks, *w)).collect();
            let err: Vec<f32> = got.iter().zip(want.iter()).map(|(g, w)| (g - w).abs() / w * 100.0).collect();
            println!("{profile:9} /{v}/ F1 {:5.0} ({:4.1}%)  F2 {:5.0} ({:4.1}%)  F3 {:5.0} ({:4.1}%)", got[0], err[0], got[1], err[1], got[2], err[2]);
            worst = worst.max(err.iter().cloned().fold(0.0, f32::max));
        }
    }
    println!("worst formant error {worst:.1}%");
    assert!(worst < 12.0, "worst formant error {worst:.1}%");
}

fn f0_track(x: &[f32], rate: f32) -> Vec<f32> {
    analysis::analyze(x, rate).iter().map(|f| f.f0).collect()
}

#[test]
fn pitch_is_accurate_across_the_range() {
    let mut worst: f32 = 0.0;
    for (profile, note) in [("bass", 40.0), ("baritone", 50.0), ("tenor", 60.0), ("alto", 62.0), ("soprano", 72.0), ("soprano", 81.0)] {
        let x = sing(profile, Style::straight(), &[(note, 1.0, "ɑ")], RATE);
        let f0 = f0_track(&x, RATE);
        let steady: Vec<f32> = f0[80..180].iter().cloned().filter(|f| *f > 0.0).collect();
        let mean = steady.iter().map(|f| hz_to_midi(*f)).sum::<f32>() / steady.len() as f32;
        let cents = (mean - note) * 100.0;
        println!("{profile:9} note {note}: sung {mean:.3} ({cents:+.1} cents, {} voiced frames)", steady.len());
        worst = worst.max(cents.abs());
        assert!(steady.len() > 90);
    }
    assert!(worst < 5.0, "worst {worst} cents");

    // With vibrato: the mean still sits on the note, the extent is the depth.
    let style = Style { drift_cents: 0.0, jitter_cents: 0.0, ..Style::default() };
    let x = sing("tenor", style, &[(57.0, 2.0, "ɑ")], RATE);
    let f0 = f0_track(&x, RATE);
    let steady: Vec<f32> = f0[120..380].iter().map(|f| hz_to_midi(*f)).collect();
    let mean = steady.iter().sum::<f32>() / steady.len() as f32;
    let (lo, hi) = steady.iter().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
    println!("vibrato: mean {:+.1} cents, extent ±{:.2} st (depth {})", (mean - 57.0) * 100.0, (hi - lo) / 2.0, style.vibrato_depth);
    assert!(((mean - 57.0) * 100.0).abs() < 8.0);
    assert!(((hi - lo) / 2.0 - style.vibrato_depth).abs() < 0.08);
}

/// Positive spectral flux above 3 kHz (dB per bin), per 1.3 ms step.
fn hf_flux(x: &[f32], rate: f32) -> Vec<f32> {
    let n = 256;
    let hop = 64;
    let fft = Fft::new(n);
    let win: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
    let lo = (3000.0 / rate * n as f32) as usize;
    let (mut re, mut im, mut p) = (Vec::new(), Vec::new(), Vec::new());
    let mut prev: Option<Vec<f32>> = None;
    let mut out = Vec::new();
    let mut at = 0;
    while at + n <= x.len() {
        let frame: Vec<f32> = (0..n).map(|i| x[at + i] * win[i]).collect();
        fft.power(&frame, &mut re, &mut im, &mut p);
        let db: Vec<f32> = p[lo..].iter().map(|v| 10.0 * (v + 1e-12).log10()).collect();
        let flux = prev.as_ref().map_or(0.0, |q| db.iter().zip(q).map(|(a, b)| (a - b).max(0.0)).sum::<f32>() / db.len() as f32);
        out.push(flux);
        prev = Some(db);
        at += hop;
    }
    out
}

fn join_flux(x: &[f32], joins: &[f32]) -> (f32, f32) {
    let flux = hf_flux(x, RATE);
    let step = 64.0 / RATE;
    let at = |t: f32| (t / step) as usize;
    let mut worst = 0.0f32;
    let mut steady = Vec::new();
    for w in joins.windows(2) {
        for i in at(w[0] + 0.15)..at(w[1] - 0.15) {
            steady.push(flux[i]);
        }
    }
    steady.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let base = steady[steady.len() * 99 / 100];
    for j in &joins[1..joins.len() - 1] {
        for i in at(j - 0.03)..at(j + 0.03) {
            worst = worst.max(flux[i]);
        }
    }
    (worst, base)
}

#[test]
fn syllable_joins_do_not_click() {
    let rate = RATE;
    // Legato vowel changes and consonant joins across pitch changes.
    let line: [(f32, f32, &str); 6] = [(60.0, 0.5, "lɑ"), (62.0, 0.5, "li"), (64.0, 0.5, "lu"), (65.0, 0.5, "mɛ"), (64.0, 0.5, "noʊ"), (62.0, 0.5, "ɑ")];
    let x = sing("alto", Style::default(), &line, rate);
    let joins: Vec<f32> = (0..=6).map(|i| i as f32 * 0.5).collect();
    let (worst, base) = join_flux(&x, &joins);
    // A control: the same line with a hard cut in the middle of the
    // second syllable (samples jump).
    let mut cut = x.clone();
    let c = (0.75 * rate) as usize;
    for v in cut[c..c + (0.25 * rate) as usize].iter_mut() {
        *v = -*v * 0.3;
    }
    let (click, _) = join_flux(&cut, &[0.0, 0.5, 0.75, 1.0, 1.5]);
    println!("HF flux at joins {worst:.2} dB/bin, steady 99th pct {base:.2}, hard cut {click:.2}");
    assert!(click > worst * 2.0, "the control must stand out");
    assert!(worst < base * 1.5 + 1.0, "join flux {worst} vs steady {base}");
}

#[test]
fn renders_are_deterministic_whatever_the_chunking() {
    let n = |beat: f64, p: f32| NoteIn { beat, dur: 0.5, pitches: vec![p], vel: 0.8 };
    let notes = [n(0.0, 64.0), n(0.5, 67.0), n(1.0, 69.0), n(1.5, 67.0), n(2.5, 64.0), n(3.0, 62.0)];
    let line = lower_sing(&notes, "twin-kle lit-tle star how i", &SingOpts { bpm: 100.0, ..SingOpts::default() }, &builtin_pronounce);
    let render = |chunk: usize| {
        let profiles = [Arc::new(VoiceProfile::named("soprano").unwrap()), Arc::new(VoiceProfile::named("alto").unwrap())];
        let mut choir = Choir::new(&profiles, 4, 44_100.0, ChoirStyle::default(), 3);
        render_line(&mut choir, &line, 100.0, 44_100.0, chunk, 0.3)
    };
    let a = render(64);
    for chunk in [37, 500, 4096] {
        let b = render(chunk);
        assert!(a.0 == b.0 && a.1 == b.1, "chunk {chunk} differs");
    }
    assert!(a.0.iter().any(|v| v.abs() > 0.01));
    assert!(a.0.iter().all(|v| v.is_finite() && v.abs() < 1.0));
}

#[test]
fn a_vocal_track_resynthesises_repitches_and_retimes() {
    let rate = 24_000.0;
    let x = sing("tenor", Style::default(), &[(57.0, 0.6, "sɑ"), (59.0, 0.6, "loʊ"), (62.0, 0.8, "mi")], rate);
    let track = VocalTrack::analyze(&x, rate);
    let bytes = track.encode();
    let back = VocalTrack::decode(&bytes).unwrap();
    assert_eq!(back.frames.len(), track.frames.len());
    println!("track: {:.2} s, {} bytes ({:.1} KB/s)", track.duration(), bytes.len(), bytes.len() as f32 / track.duration() / 1000.0);

    let play = |t: &VocalTrack, speed: f64| {
        let mut p = TrackPlayer::new(Arc::new(t.clone()), rate, 1);
        p.speed = speed;
        let mut out = vec![0.0; ((t.duration() as f64 / speed) as f32 * rate) as usize];
        p.render(&mut out);
        out
    };
    // Same pitch: the resynthesis's envelope and pitch track the original's.
    let y = play(&back, 1.0);
    let (fa, fb) = (analysis::analyze(&x, rate), analysis::analyze(&y, rate));
    let (mut cents, mut env_db, mut count) = (0.0f32, 0.0f32, 0);
    for (a, b) in fa.iter().zip(&fb) {
        if a.f0 > 0.0 && b.f0 > 0.0 {
            cents += (hz_to_midi(a.f0) - hz_to_midi(b.f0)).abs() * 100.0;
            // Envelope difference over 200 Hz .. 4 kHz, where the energy is.
            let pts: Vec<usize> = (0..bands::ENV_POINTS).filter(|g| (200.0..4000.0).contains(&bands::env_hz(*g))).collect();
            env_db += pts.iter().map(|g| (a.env[*g] - b.env[*g]).abs()).sum::<f32>() / pts.len() as f32;
            count += 1;
        }
    }
    let (cents, env_db) = (cents / count as f32, env_db / count as f32);
    println!("resynthesis: mean pitch error {cents:.1} cents, mean envelope error {env_db:.2} dB over {count} frames");
    assert!(cents < 8.0 && env_db < 3.0);

    // Up a fourth: every voiced frame's pitch moves by five semitones.
    let mut up = back.clone();
    up.transpose(5.0);
    let z = play(&up, 1.0);
    let fz = analysis::analyze(&z, rate);
    let shifts: Vec<f32> = fa.iter().zip(&fz).filter(|(a, b)| a.f0 > 0.0 && b.f0 > 0.0).map(|(a, b)| hz_to_midi(b.f0) - hz_to_midi(a.f0)).collect();
    let mean = shifts.iter().sum::<f32>() / shifts.len() as f32;
    println!("transpose +5: measured {mean:.3} semitones");
    assert!((mean - 5.0).abs() < 0.1);
    // Twice as long at half speed, same pitch.
    let slow = play(&back, 0.5);
    assert!((slow.len() as f32 / x.len() as f32 - 2.0).abs() < 0.05);
    let fs = analysis::analyze(&slow, rate);
    let voiced: Vec<f32> = fs.iter().filter(|f| f.f0 > 0.0).map(|f| hz_to_midi(f.f0)).collect();
    assert!(voiced.iter().any(|m| (m - 62.0).abs() < 0.3) && voiced.iter().any(|m| (m - 57.0).abs() < 0.3));
    let _ = midi_to_hz(60.0);
}

#[test]
fn a_profile_analysed_from_known_speech_recovers_its_shapes_and_timing() {
    // "Speech" with known phone timings: short vowels of the built-in
    // baritone between pauses, the timings deliberately 250 ms late (as a
    // TTS's can run behind its audio).
    let rate = 24_000.0;
    let vowels = ["ɑ", "i", "u", "ɛ", "ɔ", "æ"];
    let mut pcm = Vec::new();
    let mut spans = Vec::new();
    for (k, v) in vowels.iter().enumerate() {
        let x = sing("baritone", Style::straight(), &[(48.0 + k as f32, 0.2, v)], rate);
        let start = pcm.len() as f64 / rate as f64;
        spans.push(analysis::PhoneSpan { phone: v.to_string(), start: start + 0.25, end: start + 0.2 + 0.25 });
        pcm.extend(x);
    }
    let frames = analysis::analyze(&pcm, rate);
    let offset = analysis::timing_offset(&frames, &spans);
    let profile = analysis::profile_from_speech(&pcm, rate, &spans, "test");
    let base = VoiceProfile::named("baritone").unwrap();
    println!("timing offset found {offset:+.3} s, f0_ref {:.0} Hz", profile.f0_ref);
    assert!((offset + 0.25).abs() < 0.06, "{offset}");
    for v in vowels {
        let ph = phoneme::by_ipa(v).unwrap();
        let (a, b) = (profile.shape(ph), base.shape(ph));
        // The measured envelope against the one that was sung, 150 Hz..4 kHz.
        let pts: Vec<usize> = (0..bands::ENV_POINTS).filter(|g| (150.0..4000.0).contains(&bands::env_hz(*g))).collect();
        let err = pts.iter().map(|g| (a.env[*g] - b.env[*g]).abs()).sum::<f32>() / pts.len() as f32;
        println!("/{v}/ envelope error {err:.2} dB, formants {:?} (sung {:?})", a.formants.map(|f| f as i32), b.formants.map(|f| f as i32));
        assert!(err < 3.0, "/{v}/ {err}");
        assert!((a.formants[1] / b.formants[1] - 1.0).abs() < 0.1, "/{v}/ F2");
    }
}
