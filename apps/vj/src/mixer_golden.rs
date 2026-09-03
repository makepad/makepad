//! The audio path, pinned: what the engine renders for a fixed scenario,
//! summarised per window, against what it rendered the day the reference
//! was recorded.
//!
//! A reference is a list of windows, each the RMS and the peak of the left
//! and right channels over one device-sized buffer, in 16-bit steps. That is
//! coarse enough to survive another machine's rounding and fine enough that
//! a click, a ramp that became a step, a level that moved or a stretcher
//! that changed its mind all show up as a numbered window with a number
//! beside it.
//!
//! When a scenario has no reference, or disagrees with the one it has, the
//! test writes what it rendered under `target/golden/`: the summary as the
//! table entry to paste into `mixer_golden_refs`, and the raw samples as
//! CSV for a plot against the previous build's. Blessing a change is
//! pasting the entry in — on purpose, with the difference in hand.
//!
//! The click tests below use the same capture and one rule: across any
//! transition the biggest step between two neighbouring samples stays under
//! what an ear reads as a click. The transitions that fail that rule today
//! are kept, marked, and un-marked by the work that ramps them.

use crate::decks::DeckId;
use crate::mixer::fixtures::{const_pcm, render, split_pcm, tone_pcm};
use crate::mixer::{Mixer, TrackPcm};
use crate::mixer_golden_refs::reference;
use std::path::PathBuf;
use std::sync::Arc;

/// One window of the summary: the RMS and the peak of the left channel,
/// then of the right, in 16-bit steps.
pub(crate) type Window = [i16; 4];
/// One device buffer, the size the scenarios render with.
pub(crate) const WINDOW: usize = 512;
/// Half a second: long enough for a loop to wrap a few times and a fade to
/// finish, short enough that a reference reads on one screen.
const CAPTURE: usize = 24_000;
const RATE: f64 = 48_000.0;
/// Frames rendered before a capture starts, so the settling ramps of a
/// fresh mixer are not what gets pinned.
const SETTLE: usize = 4096;
/// The stretcher needs its first window before it says anything.
const SETTLE_STRETCH: usize = 8192;
/// How far a window may sit from its reference before the difference is a
/// change: four 16-bit steps, about 1.2e-4 of full scale — well under a
/// click, well over another CPU's rounding.
pub(crate) const TOLERANCE: i32 = 4;
/// The biggest step between neighbouring samples that is not yet a click,
/// on a signal at half scale.
pub(crate) const CLICK: f32 = 0.02;

/// The largest jump between two neighbouring samples.
pub(crate) fn worst_adjacent_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

fn quantise(value: f64) -> i16 {
    (value.clamp(0.0, 1.0) * 32767.0).round() as i16
}

fn rms(samples: &[f32]) -> i16 {
    let sum: f64 = samples.iter().map(|v| (*v as f64) * (*v as f64)).sum();
    quantise((sum / samples.len().max(1) as f64).sqrt())
}

fn peak(samples: &[f32]) -> i16 {
    quantise(samples.iter().fold(0.0f32, |most, v| most.max(v.abs())) as f64)
}

/// The summary of a capture, one window per device buffer.
pub(crate) fn summarise(left: &[f32], right: &[f32]) -> Vec<Window> {
    left.chunks(WINDOW)
        .zip(right.chunks(WINDOW))
        .map(|(l, r)| [rms(l), rms(r), peak(l), peak(r)])
        .collect()
}

/// Render `frames` in device-sized buffers, calling `at` with the buffer
/// index before each one so a scenario can act in the middle of a capture,
/// where a device would see it.
fn capture(mixer: &Mixer, frames: usize, mut at: impl FnMut(usize)) -> (Vec<f32>, Vec<f32>) {
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    let mut index = 0;
    while left.len() < frames {
        at(index);
        let count = WINDOW.min(frames - left.len());
        let block = render(mixer, RATE, count);
        left.extend_from_slice(&block.channel(0)[..count]);
        right.extend_from_slice(&block.channel(1)[..count]);
        index += 1;
    }
    (left, right)
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/golden")
}

/// The table entry for `mixer_golden_refs`.
fn literal(name: &str, windows: &[Window]) -> String {
    let mut text = format!("    (\"{name}\", &[\n");
    for window in windows {
        text.push_str(&format!(
            "        [{}, {}, {}, {}],\n",
            window[0], window[1], window[2], window[3]
        ));
    }
    text.push_str("    ]),\n");
    text
}

/// Write what this engine rendered where a developer can read and plot it.
fn record(name: &str, windows: &[Window], left: &[f32], right: &[f32]) -> PathBuf {
    let dir = golden_dir();
    std::fs::create_dir_all(&dir).expect("golden directory");
    std::fs::write(dir.join(format!("{name}.rs")), literal(name, windows)).expect("write entry");
    let mut csv = String::with_capacity(left.len() * 24);
    for (l, r) in left.iter().zip(right) {
        csv.push_str(&format!("{l},{r}\n"));
    }
    std::fs::write(dir.join(format!("{name}.actual.csv")), csv).expect("write samples");
    dir
}

/// The capture must match its reference window for window.
pub(crate) fn assert_golden(name: &str, left: &[f32], right: &[f32]) {
    let actual = summarise(left, right);
    let Some(expected) = reference(name) else {
        let dir = record(name, &actual, left, right);
        panic!(
            "{name}: no reference recorded. This engine's summary is at {}\\{name}.rs; \
             paste it into mixer_golden_refs.rs to bless it",
            dir.display()
        );
    };
    let differs = |a: &Window, e: &Window| {
        a.iter().zip(e).any(|(x, y)| (*x as i32 - *y as i32).abs() > TOLERANCE)
    };
    let first = actual
        .iter()
        .zip(expected)
        .position(|(a, e)| differs(a, e))
        .or((actual.len() != expected.len()).then_some(actual.len().min(expected.len())));
    if let Some(index) = first {
        let dir = record(name, &actual, left, right);
        panic!(
            "{name}: window {index} (frames {}..{}) rendered {:?}, reference {:?} \
             ({} windows rendered, {} in the reference). This engine's summary is at \
             {}\\{name}.rs and its samples beside it; bless the change only on purpose",
            index * WINDOW,
            (index + 1) * WINDOW,
            actual.get(index),
            expected.get(index),
            actual.len(),
            expected.len(),
            dir.display()
        );
    }
}

/// A mixer at unity with one track on deck A and the fader on it.
fn deck_a(pcm: Arc<TrackPcm>) -> Mixer {
    let mixer = Mixer::new();
    mixer.set_master(1.0);
    mixer.set_crossfader(0.0);
    mixer.install_deck(DeckId::A, pcm);
    mixer
}

fn settle(mixer: &Mixer, frames: usize) {
    render(mixer, RATE, frames);
}

// ---------------------------------------------------------------------------
// the references
// ---------------------------------------------------------------------------

#[test]
fn golden_tone_play() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("tone_play", &left, &right);
}

#[test]
fn golden_play_pause_play() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |index| match index {
        16 => mixer.set_deck_playing(DeckId::A, false),
        32 => mixer.set_deck_playing(DeckId::A, true),
        _ => {}
    });
    assert_golden("play_pause_play", &left, &right);
}

#[test]
fn golden_seek_blend() {
    // Two levels: a seek from the first half into the second must be heard
    // as a blend, and the blend's shape is what is pinned.
    let mixer = deck_a(split_pcm(8_000, -8_000, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |index| {
        if index == 16 {
            mixer.seek_deck_seconds(DeckId::A, 7.0);
        }
    });
    assert_golden("seek_blend", &left, &right);
}

#[test]
fn golden_loop_wrap() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    // 0.23 s of a 440 Hz tone is not a whole number of cycles, so the wrap
    // is a real seam and its crossfade is what gets pinned; a quarter
    // second would have wrapped in phase and pinned nothing.
    mixer.set_deck_loop_span(DeckId::A, Some((1.0, 1.23)));
    mixer.seek_deck_seconds(DeckId::A, 1.0);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("loop_wrap", &left, &right);
}

#[test]
fn golden_keylock_stretch() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    mixer.set_deck_rate(DeckId::A, 1.05);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE_STRETCH);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("keylock_stretch", &left, &right);
}

#[test]
fn golden_varispeed() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    mixer.set_deck_keylock(DeckId::A, false);
    mixer.set_deck_rate(DeckId::A, 1.05);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("varispeed", &left, &right);
}

#[test]
fn golden_key_shift() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    mixer.set_deck_key_shift(DeckId::A, 3.0);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE_STRETCH);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("key_shift", &left, &right);
}

#[test]
fn golden_eq_kill_and_filter() {
    let mixer = deck_a(tone_pcm(220.0, 48_000, 3.0));
    mixer.set_deck_eq_band(DeckId::A, 0, 0.0);
    mixer.set_deck_filter(DeckId::A, 0.25);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("eq_kill_and_filter", &left, &right);
}

#[test]
fn golden_crossfader_centre() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    mixer.install_deck(DeckId::B, tone_pcm(660.0, 48_000, 3.0));
    mixer.set_crossfader(0.5);
    mixer.set_deck_playing(DeckId::A, true);
    mixer.set_deck_playing(DeckId::B, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("crossfader_centre", &left, &right);
}

#[test]
fn golden_gain_and_clamp() {
    let mixer = deck_a(const_pcm(32_000, 480_000, 48_000));
    mixer.set_deck_gain(DeckId::A, 1.5);
    mixer.set_master(1.2);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("gain_and_clamp", &left, &right);
}

// ---------------------------------------------------------------------------
// clicks
// ---------------------------------------------------------------------------

/// The biggest neighbouring-sample step on the left channel while `at`
/// acts on the mixer part-way through half a second of DC at half scale.
fn worst_step_across(mixer: &Mixer, at: impl FnMut(usize)) -> f32 {
    let (left, _) = capture(mixer, CAPTURE, at);
    worst_adjacent_step(&left)
}

#[test]
fn the_step_finder_reports_the_largest_jump() {
    assert_eq!(worst_adjacent_step(&[0.0, 0.1, 0.1, 0.4, 0.35]), 0.3);
    assert_eq!(worst_adjacent_step(&[0.5]), 0.0);
    assert_eq!(worst_adjacent_step(&[]), 0.0);
}

#[test]
fn a_seek_is_click_free() {
    let mixer = deck_a(split_pcm(16_384, -16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.seek_deck_seconds(DeckId::A, 7.0);
        }
    });
    assert!(worst < CLICK, "a seek must blend, biggest step {worst}");
}

#[test]
fn play_and_pause_are_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_playing(DeckId::A, false),
        24 => mixer.set_deck_playing(DeckId::A, true),
        _ => {}
    });
    assert!(worst < CLICK, "play and pause must ramp, biggest step {worst}");
}

#[test]
fn a_keylock_toggle_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_rate(DeckId::A, 1.05);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE_STRETCH);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_keylock(DeckId::A, false),
        24 => mixer.set_deck_keylock(DeckId::A, true),
        _ => {}
    });
    assert!(worst < CLICK, "a keylock toggle must crossfade, biggest step {worst}");
}

#[test]
fn killing_a_band_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_eq_band(DeckId::A, 0, 0.0),
        24 => mixer.set_deck_eq_band(DeckId::A, 0, 1.0),
        _ => {}
    });
    assert!(worst < CLICK, "a kill must ramp, biggest step {worst}");
}

#[test]
fn a_filter_jump_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_filter(DeckId::A, 0.2),
        24 => mixer.set_deck_filter(DeckId::A, 0.5),
        _ => {}
    });
    assert!(worst < CLICK, "a filter jump must not step, biggest step {worst}");
}

#[test]
fn a_gain_change_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_gain(DeckId::A, 0.2),
        24 => mixer.set_deck_gain(DeckId::A, 1.0),
        _ => {}
    });
    assert!(worst < CLICK, "a gain change must ramp, biggest step {worst}");
}
