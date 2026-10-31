//! Intonation of a Score's sung loops as the engine renders them (ignored;
//! a measuring tool, not a gate):
//!
//!     SUNG_PITCH_SCORE=song.score.splash SUNG_PITCH_LOOP=lead_vox \
//!         [SUNG_PITCH_WAV=out.wav SUNG_PITCH_GRID=0.25 SUNG_PITCH_VIBRATO=30 SUNG_PITCH_DETAIL=1] cargo test --release -p makepad-score-live \
//!         --test sung_pitch -- --ignored --nocapture
//!
//! Every section plays that loop alone, the vocal reverb and delay sends are
//! muted (their tails would blur the tracking); every sung note is tracked
//! (`sung::tuning`) and reported in cents against its score pitch.
//!
//! `a_voice_sings_its_style_in_tune` (a gate): a `Voice` with several style
//! fields set sings with the vibrato it was given, centred on the note.

use makepad_score_live::{render, NoSamples, RenderOpts};
use makepad_score_player::sung::tuning::{self, TargetNote};

/// `name: [a b c]` lists of every section become `[solo]`, every Reverb /
/// Delay goes dry and the limiter's drive goes to unity.
fn solo(src: &str, solo: &str) -> String {
    let mut out = String::new();
    for line in src.lines() {
        let mut line = line.to_string();
        if line.contains("= Section{") {
            if let Some(a) = line.find("loops: [") {
                let b = a + line[a..].find(']').unwrap();
                line = format!("{}loops: [{solo}{}", &line[..a], &line[b..]);
            }
        }
        if line.contains("= Reverb{") || line.contains("= Delay{") {
            if let Some(a) = line.find(" mix: ") {
                let rest = &line[a + 6..];
                let end = rest.find(|c: char| c == ' ' || c == '}').unwrap_or(rest.len());
                line = format!("{} mix: 0{}", &line[..a], &rest[end..]);
            }
        }
        // The master limiter's drive clips a solo voice flat-topped, which
        // blurs the period; measure it at unity.
        if line.contains("= Limiter{") {
            if let Some(a) = line.find(" gain: ") {
                let rest = &line[a + 7..];
                let end = rest.find(|c: char| c == ' ' || c == '}').unwrap_or(rest.len());
                line = format!("{} gain: 1{}", &line[..a], &rest[end..]);
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[test]
#[ignore]
fn sung_pitch() {
    let Ok(path) = std::env::var("SUNG_PITCH_SCORE") else {
        println!("set SUNG_PITCH_SCORE and SUNG_PITCH_LOOP");
        return;
    };
    let name = std::env::var("SUNG_PITCH_LOOP").unwrap_or_else(|_| "lead_vox".into());
    let src = solo(&std::fs::read_to_string(&path).unwrap(), &name);
    let rendered = render(&src, &RenderOpts::default(), &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    let x: Vec<f32> = rendered.left.iter().zip(&rendered.right).map(|(l, r)| l + r).collect();
    if let Ok(wav) = std::env::var("SUNG_PITCH_WAV") {
        std::fs::write(&wav, makepad_score_player::sung::render::wav_bytes(rendered.rate as u32, &[&rendered.left, &rendered.right])).unwrap();
    }
    let mut played: Vec<_> = rendered.played.iter().filter(|p| p.loop_name == name && !p.hit).collect();
    played.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    let grid: f64 = std::env::var("SUNG_PITCH_GRID").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let targets: Vec<TargetNote> = played
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let end = (p.beat + p.dur as f64).min(played.get(i + 1).map_or(f64::INFINITY, |n| n.beat));
            // A sung event starts at its first consonant; its vowel is on
            // the written beat (the grid step at or after it, when given).
            let vowel = if grid > 0.0 { ((p.beat - 1e-6) / grid).ceil() * grid } else { p.beat };
            let vowel = if vowel < end { vowel } else { p.beat };
            TargetNote { start: rendered.seconds_at(vowel) as f32, end: rendered.seconds_at(end) as f32, note: p.note }
        })
        .collect();
    let vibrato: f32 = std::env::var("SUNG_PITCH_VIBRATO").ok().and_then(|v| v.parse().ok()).unwrap_or(30.0);
    let reports = tuning::report(&x, rendered.rate, &targets, vibrato);
    // SUNG_PITCH_TRACE=t0,t1: the raw track (every 10 ms) over that span.
    let prepared = tuning::prepare(&x, rendered.rate);
    if let Some((t0, t1)) = std::env::var("SUNG_PITCH_TRACE").ok().and_then(|v| v.split_once(',').map(|(a, b)| (a.parse::<f32>().unwrap(), b.parse::<f32>().unwrap()))) {
        let mut t = t0;
        while t < t1 {
            if let Some(n) = targets.iter().find(|n| n.start <= t && t < n.end) {
                let hz = makepad_score_player::sung::pitch::midi_to_hz(n.note);
                if let Some((f, ap)) = tuning::pitch_at(&prepared, rendered.rate, (t * rendered.rate) as usize, hz / 1.5, hz * 1.5) {
                    println!("  {t:7.3}s note {:4.1}  {:+7.1} cents  ap {ap:.3}", n.note, (makepad_score_player::sung::pitch::hz_to_midi(f) - n.note) * 100.0);
                }
            }
            t += 0.01;
        }
    }
    let detail = std::env::var("SUNG_PITCH_DETAIL").is_ok();
    let mut held = Vec::new();
    for r in &reports {
        let len = r.target.end - r.target.start;
        if detail {
            println!(
                "{:7.2}s note {:5.1} {:4.2}s  centre {:+6.1}  drift {:+6.1}  arrival {:+7.1}  settle {:4.0} ms ({} frames)",
                r.target.start, r.target.note, len, r.centre, r.drift, r.arrival, r.settle * 1000.0, r.frames
            );
        }
        if len >= 0.3 {
            held.push(*r);
        }
    }
    let (mean, mean_abs, worst, drift, settle) = tuning::summary(&reports);
    println!("{name}: {} notes: centre mean {mean:+.1} |centre| {mean_abs:.1} worst {worst:.1}, drift {drift:.1}, median settle {:.0} ms", reports.len(), settle * 1000.0);
    let (mean, mean_abs, worst, drift, settle) = tuning::summary(&held);
    println!("{name}: {} notes of 0.3 s or more: centre mean {mean:+.1} |centre| {mean_abs:.1} worst {worst:.1}, drift {drift:.1}, median settle {:.0} ms", held.len(), settle * 1000.0);
    let within = |r: &&tuning::NoteReport, c: f32| r.centre.is_finite() && r.centre.abs() <= c;
    let measured = reports.iter().filter(|r| r.centre.is_finite()).count();
    println!(
        "{name}: centres within 5 cents {}/{measured}, within 10 {}/{measured}",
        reports.iter().filter(|r| within(r, 5.0)).count(),
        reports.iter().filter(|r| within(r, 10.0)).count()
    );
}

/// Every style field a `Voice` sets reaches its singer: a list of numbers
/// with a negative placeholder once read `[0.3 -1.0 0.22 ...]` as
/// subtractions, which slowed the vibrato to 0.22 Hz, a slow ±30 cent
/// wander either side of the note.
#[test]
fn a_voice_sings_its_style_in_tune() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet vox = Voice{profile: @alto vibrato: 0.3 vibrato_delay: 0.22 breath: 0.18 portamento: 0.06}\nlet line = Loop{play: vox beats: 8 code: fn(i) { sing([74], \"ah\", {dur: 6}) sleep(8) }}\nlet a = Section{bars: 2 loops: [line]}\nScore{clock vox line a arrange: [a]}\n";
    let rendered = render(src, &RenderOpts::default(), &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    let x = tuning::prepare(&rendered.left, rendered.rate);
    let hz = makepad_score_player::sung::pitch::midi_to_hz(74.0);
    // The held vowel after the vibrato has faded in: 0.8 s to 2.8 s.
    let cents: Vec<f32> = (0..400)
        .filter_map(|i| tuning::pitch_at(&x, rendered.rate, ((0.8 + i as f32 * 0.005) * rendered.rate) as usize, hz / 1.5, hz * 1.5))
        .map(|(f, _)| (makepad_score_player::sung::pitch::hz_to_midi(f) - 74.0) * 100.0)
        .collect();
    assert_eq!(cents.len(), 400);
    let mean = cents.iter().sum::<f32>() / cents.len() as f32;
    let crossings = cents.windows(2).filter(|w| (w[0] - mean).signum() != (w[1] - mean).signum()).count();
    let rate = crossings as f32 / 2.0 / 2.0;
    let extent = cents.iter().fold(0.0f32, |m, c| m.max((c - mean).abs()));
    println!("vibrato {rate:.1} Hz, ±{extent:.0} cents, centred {mean:+.1} cents");
    assert!((rate - 5.4).abs() < 1.0, "vibrato at {rate:.1} Hz");
    assert!(extent > 20.0 && extent < 40.0, "vibrato ±{extent:.0} cents");
    assert!(mean.abs() < 5.0, "centred {mean:+.1} cents");
}
