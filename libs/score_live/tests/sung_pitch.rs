//! A Voice sings with its configured vibrato, centred on the note.

use makepad_score_live::{render, NoSamples, RenderOpts};
use makepad_score_player::sung::tuning;

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
