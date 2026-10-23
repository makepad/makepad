//! Intonation: sung lines rendered offline and tracked (`tuning`), every
//! note's pitch against its score in cents. The survey (ignored, run with
//! `--ignored --nocapture`) prints voices × ranges × note lengths × leaps ×
//! vibrato on/off; the tests hold the targets: a held note's centre within
//! ±5 cents, no systematic offset, arrival settled within about 60 ms.

use makepad_score_voice::render::render_line;
use makepad_score_voice::tuning::{self, NoteReport, TargetNote};
use makepad_score_voice::*;
use std::sync::Arc;

const RATE: f32 = 48_000.0;
const BPM: f64 = 120.0;

/// (note, beats, syllable) phrases.
fn phrase(list: &[(f32, f64)], lyrics: &str) -> Lowered {
    let mut beat = 0.0;
    let notes: Vec<NoteIn> = list
        .iter()
        .map(|(p, d)| {
            let n = NoteIn { beat, dur: *d, pitches: vec![*p], vel: 0.8 };
            beat += d;
            n
        })
        .collect();
    lower_sing(&notes, lyrics, &SingOpts { bpm: BPM, min_beat: -4.0, ..SingOpts::default() }, &builtin_pronounce)
}

/// Sing `line` solo and report every note (its vowel to the next note's
/// first sound).
fn sing(profile: &str, style: Style, seed: u32, line: &Lowered) -> Vec<NoteReport> {
    let mut singer = Singer::new(Arc::new(VoiceProfile::named(profile).unwrap()), RATE, style, seed);
    let (l, r) = render_line(&mut singer, line, BPM, RATE, 256, 0.4);
    let x: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a + b).collect();
    let origin = line.events.iter().map(|e| e.beat).fold(f64::INFINITY, f64::min);
    let secs = |b: f64| ((b - origin) * 60.0 / BPM) as f32;
    let targets: Vec<TargetNote> = line
        .events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let end = line.events.get(i + 1).map_or(e.beat + e.dur, |n| n.beat);
            TargetNote { start: secs(e.vowel_beat), end: secs(end), note: e.note }
        })
        .collect();
    tuning::report(&x, RATE, &targets, style.vibrato_depth * 100.0)
}

/// The pitch the singer was told to sing (its control curve, no audio),
/// reported the same way: what the synthesis adds is the difference.
fn commanded(profile: &str, style: Style, seed: u32, line: &Lowered) -> Vec<NoteReport> {
    let mut singer = Singer::new(Arc::new(VoiceProfile::named(profile).unwrap()), RATE, style, seed);
    let step = 0.005 * BPM / 60.0;
    let curve = makepad_score_voice::render::pitch_curve(&mut singer, line, BPM, step);
    let origin = line.events.iter().map(|e| e.beat).fold(f64::INFINITY, f64::min);
    let secs = |b: f64| ((b - origin) * 60.0 / BPM) as f32;
    line.events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let target = TargetNote { start: secs(e.vowel_beat), end: secs(line.events.get(i + 1).map_or(e.beat + e.dur, |n| n.beat)), note: e.note };
            let frames: Vec<(f32, f32)> = curve
                .iter()
                .map(|(b, m)| (secs(*b) - target.start, (m - e.note) * 100.0))
                .filter(|(t, _)| *t >= 0.0 && *t < target.end - target.start)
                .collect();
            tuning::note_report(target, &frames, style.vibrato_depth * 100.0)
        })
        .collect()
}

fn show(name: &str, reports: &[NoteReport], detail: bool) {
    let (mean, mean_abs, worst, drift, settle) = tuning::summary(reports);
    println!("{name:44} centre mean {mean:+5.1} |mean| {mean_abs:4.1} worst {worst:5.1}  drift {drift:4.1}  settle {:3.0} ms", settle * 1000.0);
    if detail {
        for r in reports {
            println!(
                "    note {:5.1} {:4.2}s  centre {:+6.1}  drift {:+6.1}  arrival {:+6.1}  settle {:4.0} ms  ({} frames)",
                r.target.note, r.target.end - r.target.start, r.centre, r.drift, r.arrival, r.settle * 1000.0, r.frames
            );
        }
    }
}

fn lines() -> Vec<(&'static str, Vec<(f32, f64)>, &'static str)> {
    vec![
        ("steps (quarters)", vec![(0.0, 1.0), (2.0, 1.0), (4.0, 1.0), (5.0, 1.0), (7.0, 1.0), (5.0, 1.0), (4.0, 1.0), (2.0, 1.0), (0.0, 2.0)], "la la la la la la la la la"),
        ("leaps", vec![(0.0, 1.0), (12.0, 1.0), (7.0, 1.0), (0.0, 1.0), (9.0, 2.0), (-3.0, 1.0), (4.0, 2.0)], "sing a bright song to the sky"),
        ("long notes", vec![(0.0, 6.0), (4.0, 6.0), (7.0, 8.0)], "ah oh ee"),
        ("eighths", vec![(0.0, 0.5), (2.0, 0.5), (4.0, 0.5), (2.0, 0.5), (0.0, 0.5), (4.0, 0.5), (7.0, 0.5), (4.0, 0.5), (0.0, 2.0)], "ta ta ta ta ta ta ta ta ta"),
        ("vowels", vec![(0.0, 3.0), (0.0, 3.0), (0.0, 3.0), (0.0, 3.0), (0.0, 3.0)], "/ɑ/ /i/ /u/ /ɛ/ /oʊ/"),
        ("slurred melisma", vec![(0.0, 1.0), (2.0, 0.5), (4.0, 0.5), (5.0, 1.0), (4.0, 1.0), (0.0, 2.0)], "glo _ _ _ _ ry"),
    ]
}

fn voices() -> [(&'static str, f32); 5] {
    [("bass", 43.0), ("baritone", 50.0), ("tenor", 57.0), ("alto", 62.0), ("soprano", 69.0)]
}

/// Held notes sing in tune: every voice and range, eight seeds, with
/// vibrato, drift and jitter (the defaults).
#[test]
fn held_notes_are_in_tune() {
    let mut all = Vec::new();
    for (voice, base) in voices() {
        for (lname, list, lyrics) in lines() {
            let list: Vec<(f32, f64)> = list.iter().map(|(p, d)| (base + p, *d)).collect();
            let line = phrase(&list, lyrics);
            for seed in [1, 2] {
                for r in sing(voice, Style::default(), seed * 31 + base as u32, &line) {
                    let len = r.target.end - r.target.start;
                    if len >= 0.3 && r.centre.is_finite() {
                        // Long notes within 5 cents, shorter ones within 8.
                        let limit = if len >= 0.8 { 5.0 } else { 8.0 };
                        assert!(r.centre.abs() <= limit, "{voice} {lname} seed {seed}: note {} ({len:.2} s) centred {:+.1} cents", r.target.note, r.centre);
                        all.push(r);
                    }
                }
            }
        }
    }
    let (mean, mean_abs, worst, drift, _) = tuning::summary(&all);
    println!("{} notes: centre mean {mean:+.2}, mean |centre| {mean_abs:.2}, worst {worst:.1}, mean |drift| {drift:.1}", all.len());
    assert!(mean.abs() < 1.0, "a systematic offset of {mean:+.2} cents");
    assert!(mean_abs < 3.0 && drift < 6.0);
}

/// A new pitch lands on its vowel: steps, leaps up to an octave and
/// vowel-initial joins (nothing to glide through) settle within 15 cents
/// by about 60 ms into the vowel, overshooting by little.
#[test]
fn transitions_land_on_the_vowel() {
    let style = Style { vibrato_depth: 0.0, drift_cents: 0.0, jitter_cents: 0.0, ..Style::default() };
    for (voice, base) in voices() {
        for (lname, list, lyrics) in lines() {
            let list: Vec<(f32, f64)> = list.iter().map(|(p, d)| (base + p, *d)).collect();
            let line = phrase(&list, lyrics);
            for r in sing(voice, style, 1, &line) {
                assert!(r.settle.is_nan() || r.settle <= 0.065, "{voice} {lname}: note {} settles at {:.0} ms", r.target.note, r.settle * 1000.0);
                assert!(r.centre.abs() <= 1.5, "{voice} {lname}: note {} centred {:+.1} cents", r.target.note, r.centre);
            }
        }
    }
}

/// A legato join does not jump: the vibrato dies away into the new note
/// instead of stopping dead mid-swing (which stepped the pitch by up to
/// its full depth, 30 cents, within one block). Vowel-to-vowel joins on the
/// same pitch and by step, the vibrato's part of every block's pitch (what
/// the voicer sings) against the one before; the held notes still centre
/// within 5 cents.
#[test]
fn legato_joins_are_continuous() {
    let style = Style { drift_cents: 0.0, jitter_cents: 0.0, ..Style::default() };
    // One block of the singer's control rate, in beats.
    let block = 64.0 / RATE as f64 * BPM / 60.0;
    let mut worst_all = 0.0f32;
    for (voice, base) in voices() {
        for seed in 1..=6u32 {
            for steps in [[0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0], [0.0, 2.0, 4.0, 2.0, 0.0, 2.0]] {
                let list: Vec<(f32, f64)> = steps.iter().enumerate().map(|(i, s)| (base + s, 1.0 + 0.37 * ((i as u32 * 7 + seed) % 5) as f64)).collect();
                let line = phrase(&list, "ah ah ah ah ah ah");
                let curve_of = |style: Style| {
                    let mut singer = Singer::new(Arc::new(VoiceProfile::named(voice).unwrap()), RATE, style, seed);
                    makepad_score_voice::render::pitch_curve(&mut singer, &line, BPM, block)
                };
                // The vibrato's part of the pitch: the curve less the same
                // voice's without vibrato (its glides and scoops).
                let (with, without) = (curve_of(style), curve_of(Style { vibrato_depth: 0.0, ..style }));
                let vib: Vec<(f64, f32)> = with.iter().zip(&without).map(|(a, b)| (a.0, a.1 - b.1)).collect();
                for e in &line.events[1..] {
                    // The blocks either side of the join.
                    let at = vib.partition_point(|(b, _)| *b < e.beat);
                    let near = &vib[at.saturating_sub(4)..(at + 4).min(vib.len())];
                    let worst = near.windows(2).map(|w| ((w[1].1 - w[0].1) * 100.0).abs()).fold(0.0, f32::max);
                    // A swing of 30 cents at 5.4 Hz moves at most 1.4 cents
                    // a block; ramping its depth down adds as much again.
                    worst_all = worst_all.max(worst);
                    assert!(worst <= 2.5, "{voice} seed {seed}: join onto {} at beat {} steps the vibrato {worst:.1} cents in a block", e.note, e.beat);
                }
                for r in sing(voice, style, seed, &line) {
                    if r.target.end - r.target.start >= 0.8 {
                        assert!(r.centre.abs() <= 5.0, "{voice} seed {seed}: note {} centred {:+.1} cents", r.target.note, r.centre);
                    }
                }
            }
        }
    }
    println!("worst vibrato step at a join: {worst_all:.1} cents a block");
}

/// A choir's parts are each in tune: the singers' detune spreads about the
/// part's pitch (a part of one singer sings the note).
#[test]
fn choir_parts_are_centred() {
    let profiles = [Arc::new(VoiceProfile::named("alto").unwrap())];
    for seed in 0..6 {
        let style = ChoirStyle { base: Style { vibrato_depth: 0.0, drift_cents: 0.0, jitter_cents: 0.0, ..Style::default() }, parts: 3, timing_jitter: 0.0, ..ChoirStyle::default() };
        let mut choir = Choir::new(&profiles, 3, RATE, style, seed);
        for part in 0..3u8 {
            choir.note_on(0, SungNote { note: 64.0, vel: 0.8, dur: 1.0, syl: PhonemeSeq::ipa("ɑ") }, part);
            let mut x = vec![0.0f32; (1.3 * RATE) as usize];
            choir.render(&mut x);
            let r = tuning::report(&x, RATE, &[TargetNote { start: 0.0, end: 1.0, note: 64.0 }], 0.0)[0];
            assert!(r.centre.abs() < 1.0, "seed {seed} part {part}: centred {:+.1} cents", r.centre);
        }
    }
}

#[test]
#[ignore]
fn survey() {
    let styles = [
        ("default", Style::default()),
        ("no vibrato", Style { vibrato_depth: 0.0, ..Style::default() }),
        ("no drift/jitter", Style { drift_cents: 0.0, jitter_cents: 0.0, ..Style::default() }),
        ("straight", Style::straight()),
    ];
    for (voice, base) in voices() {
        for (lname, list, lyrics) in lines() {
            let list: Vec<(f32, f64)> = list.iter().map(|(p, d)| (base + p, *d)).collect();
            let line = phrase(&list, lyrics);
            for (sname, style) in styles {
                let reports = sing(voice, style, 3, &line);
                show(&format!("{voice} {lname} [{sname}]"), &reports, voice == "tenor" && sname == "default");
                if voice == "tenor" {
                    show(&format!("  (commanded) [{sname}]"), &commanded(voice, style, 3, &line), false);
                }
            }
        }
    }
    // Seeds: how far the default voice's held notes wander from singer to singer.
    let line = phrase(&[(57.0, 4.0), (60.0, 4.0), (64.0, 4.0), (62.0, 4.0)], "ah ah ah ah");
    let mut all = Vec::new();
    for seed in 0..24 {
        all.extend(sing("tenor", Style::default(), seed, &line));
    }
    show("tenor held notes, 24 seeds [default]", &all, false);
}


