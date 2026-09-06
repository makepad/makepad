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

use crate::decks::{DeckId, SpinMotion};
use crate::mixer::fixtures::{
    const_pcm, const_stereo_pcm, render, split_pcm, stereo_tone_pcm, tone_pcm,
};
use crate::mixer::{Mixer, TrackPcm};
use crate::mixer_golden_refs::reference;
use crate::wave_analysis::TrackGrid;
use std::path::PathBuf;
use std::sync::Arc;

/// A steady 120 BPM grid, for a scenario that needs a beat to echo on.
fn grid_120() -> TrackGrid {
    TrackGrid { bpm: 120.0, beat_secs: 0.5, first_beat_secs: 0.0, downbeat_phase: 0, confidence: 0.9 }
}

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
fn capture(rig: &Rig, frames: usize, mut at: impl FnMut(usize)) -> (Vec<f32>, Vec<f32>) {
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    let mut index = 0;
    while left.len() < frames {
        at(index);
        let count = WINDOW.min(frames - left.len());
        let block = rig.render(RATE, count);
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

/// A handle and the engine its device callback would own, together.
///
/// The scenarios drive both, exactly as the app does across its two
/// threads: commands go through the handle and are drained by the engine
/// at the top of the next buffer, and buffers come out of the engine. A
/// scenario that set a knob and rendered in the same breath would
/// otherwise be testing an order the app never has.
///
/// Derefs to the handle so every `mixer.set_x()` in these tests reads the
/// way it always did.
struct Rig {
    mixer: Mixer,
    engine: std::cell::RefCell<crate::mixer::MixEngine>,
}

impl std::ops::Deref for Rig {
    type Target = Mixer;
    fn deref(&self) -> &Mixer {
        &self.mixer
    }
}

impl Rig {
    fn new(mixer: Mixer) -> Rig {
        let engine = mixer.take_engine().expect("one engine per mixer");
        Rig { mixer, engine: std::cell::RefCell::new(engine) }
    }

    fn render(&self, rate: f64, frames: usize) -> makepad_widgets::makepad_platform::audio::AudioBuffer {
        render(&mut self.engine.borrow_mut(), rate, frames)
    }
}

/// A mixer at unity with one track on deck A and the fader on it.
fn deck_a(pcm: Arc<TrackPcm>) -> Rig {
    let mixer = Mixer::new();
    mixer.set_master(1.0);
    mixer.set_crossfader(0.0);
    mixer.install_deck(DeckId::A, pcm);
    Rig::new(mixer)
}

fn settle(rig: &Rig, frames: usize) {
    rig.render(RATE, frames);
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
    mixer.set_deck_loop_span(DeckId::A, Some((1.0, 1.23)), crate::decks::LoopSeek::MovedOut);
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

/// A tone whose period shares no whole number of cycles with the delay
/// (333 Hz against a half beat at 120 BPM), so the reference actually
/// carries the echo rather than a phase-aligned coincidence of it.
#[test]
fn golden_echo_half_beat() {
    let mixer = deck_a(tone_pcm(333.0, 48_000, 3.0));
    mixer.set_deck_keylock(DeckId::A, false);
    mixer.set_deck_grid(DeckId::A, Some(grid_120()));
    mixer.set_deck_echo(DeckId::A, Some((1, 2)));
    mixer.set_deck_echo_feedback(DeckId::A, 0.5);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("echo_half_beat", &left, &right);
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

/// Driven well past full scale on purpose: a loud record, the channel
/// gain up half again and the master up a fifth on top, which asks the
/// bus for about 1.76 of full scale.
///
/// What this pins is that the bus tops out AT full scale and does not
/// run past it -- which is all a constant source can show. A constant
/// has no waveform to ruin, so a clipper and a limiter treat it
/// identically, and this reference reads the same either way.
///
/// It is left as a constant deliberately. A tone would show the real
/// difference -- a clamp flat-tops each crest into a square while the
/// limiter turns the whole waveform down -- but the stock tone fixture
/// is 0.37 of full scale and the gain and master controls clamp at 2.0
/// and 1.2, so the loudest a tone can be driven here is 0.88, which
/// never reaches the ceiling at all. The waveform behaviour is covered
/// where it can be driven properly: see the limiter's own tests in
/// `music_dsp`, which hold it under its ceiling from full scale to
/// twenty times over.
#[test]
fn golden_gain_and_limit() {
    let mixer = deck_a(const_pcm(32_000, 480_000, 48_000));
    mixer.set_deck_gain(DeckId::A, 1.5);
    mixer.set_master(1.2);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("gain_and_limit", &left, &right);
}

/// The whole load-over-playing gesture, pinned window by window: the
/// outgoing track leaving, the silent gap while the swap waits for a
/// buffer boundary, and the new track coming up in its place.
#[test]
fn golden_load_over_playing() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |index| {
        if index == 16 {
            mixer.install_deck_over(DeckId::A, const_pcm(8_192, 480_000, 48_000), true);
        }
    });
    assert_golden("load_over_playing", &left, &right);
}

/// A held beat and its release, pinned window by window: the press
/// crossfading in, the lap repeating on itself, and the release
/// crossfading back to the live tone wherever it has got to. 443 Hz,
/// not 440: a 0.25 s lap of 440 Hz is exactly 110 whole cycles, so the
/// frozen loop and the live tone underneath it would be numerically
/// identical and this reference would not move if freeze were deleted
/// outright -- diffed against golden_tone_play and confirmed exactly
/// that before 443 Hz replaced it.
#[test]
fn golden_freeze_hold() {
    let mixer = deck_a(tone_pcm(443.0, 48_000, 3.0));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |index| match index {
        // Not before buffer 20: SETTLE (4 096 frames) plus 20 buffers is
        // comfortably more than the lap itself needs to have been
        // freshly written, so the reference is not pinning a lap that
        // is a third silence.
        20 => mixer.set_deck_freeze(DeckId::A, Some(0.25)),
        40 => mixer.set_deck_freeze(DeckId::A, None),
        _ => {}
    });
    assert_golden("freeze_hold", &left, &right);
}

/// The finest rung on the sub-beat ladder at 60 BPM: a lap barely
/// longer than the seam crossfade itself, pinning the small-buffer case
/// where the blend covers most of the lap.
#[test]
fn golden_freeze_glitch() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |index| match index {
        20 => mixer.set_deck_freeze(DeckId::A, Some(1.0 / 32.0)),
        40 => mixer.set_deck_freeze(DeckId::A, None),
        _ => {}
    });
    assert_golden("freeze_glitch", &left, &right);
}

/// 233 Hz: shares no whole number of cycles with the flanger's swept
/// delay (which never sits still long enough to matter, but a round
/// multiple of the CENTRE delay would still land some windows on a
/// coincidence), so the sweep's comb-filtering actually shows up in
/// every window rather than a phase-aligned few of them.
#[test]
fn golden_flanger_sweep() {
    let mixer = deck_a(tone_pcm(233.0, 48_000, 3.0));
    mixer.set_deck_flanger(DeckId::A, true);
    mixer.set_deck_flanger_rate(DeckId::A, 1.5);
    mixer.set_deck_flanger_depth(DeckId::A, 0.8);
    mixer.set_deck_flanger_feedback(DeckId::A, 0.4);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("flanger_sweep", &left, &right);
}

/// 317 Hz against a 3 kHz crush rate: the hold captures roughly every
/// 9.46 cycles of the tone, a non-integer relationship, so the held
/// staircase actually carries the tone's own motion across windows
/// rather than landing on the same phase every hold and reading as a
/// steady, uninteresting DC-like step.
#[test]
fn golden_bitcrusher_crush() {
    let mixer = deck_a(tone_pcm(317.0, 48_000, 3.0));
    mixer.set_deck_bitcrusher(DeckId::A, true);
    mixer.set_deck_bitcrusher_rate(DeckId::A, 3_000.0);
    mixer.set_deck_bitcrusher_bits(DeckId::A, 5.0);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("bitcrusher_crush", &left, &right);
}

/// No frequency-coincidence concern here the way the flanger's sweep or
/// the bitcrusher's hold have: a pure amplitude multiply does not
/// comb-filter or alias against the tone underneath it, so any steady
/// tone already shows the pulse.
#[test]
fn golden_tremolo_pulse() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    mixer.set_deck_tremolo(DeckId::A, true);
    mixer.set_deck_tremolo_rate(DeckId::A, 6.0);
    mixer.set_deck_tremolo_depth(DeckId::A, 0.7);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("tremolo_pulse", &left, &right);
}

/// A pure waveshaper adds harmonics to whatever is already there rather
/// than comb-filtering or aliasing it, so -- like the tremolo's -- no
/// special frequency choice is needed here either.
#[test]
fn golden_distortion_drive() {
    let mixer = deck_a(tone_pcm(233.0, 48_000, 3.0));
    mixer.set_deck_distortion(DeckId::A, true);
    mixer.set_deck_distortion_drive(DeckId::A, 8.0);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("distortion_drive", &left, &right);
}

/// 317 Hz, not a round number: the phaser's sweeping notches comb-filter
/// the tone the same way the flanger's swept delay does, so a frequency
/// that shares no tidy relationship with the sweep is what makes the
/// reference actually carry that motion rather than a phase-aligned
/// coincidence of it.
#[test]
fn golden_phaser_sweep() {
    let mixer = deck_a(tone_pcm(317.0, 48_000, 3.0));
    mixer.set_deck_phaser(DeckId::A, true);
    mixer.set_deck_phaser_rate(DeckId::A, 0.8);
    mixer.set_deck_phaser_feedback(DeckId::A, 0.5);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("phaser_sweep", &left, &right);
}

/// A steady tone is enough here too: the autopan does not comb-filter
/// or alias anything, it only redistributes the same content between
/// channels, so the reference's left and right summaries diverging from
/// each other window by window is exactly the sweep showing up.
#[test]
fn golden_autopan_sweep() {
    let mixer = deck_a(tone_pcm(440.0, 48_000, 3.0));
    mixer.set_deck_autopan(DeckId::A, true);
    mixer.set_deck_autopan_rate(DeckId::A, 2.0);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("autopan_sweep", &left, &right);
}

/// A genuinely stereo source (independent left and right tones, not the
/// mono `tone_pcm` every other golden test above uses): the whole point
/// of the effect is what it does to the DIFFERENCE between the channels,
/// which is exactly zero on a mono source, so a mono tone here would
/// pass regardless of whether the mid/side math is right.
#[test]
fn golden_stereo_width_narrow() {
    let mixer = deck_a(stereo_tone_pcm(233.0, 317.0, 48_000, 3.0));
    mixer.set_deck_stereo_width(DeckId::A, true);
    mixer.set_deck_stereo_width_amount(DeckId::A, 0.3);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("stereo_width_narrow", &left, &right);
}

/// 233 Hz, not a round number: the comb bank inside the tank is a bank
/// of comb filters, the same reasoning the flanger and the phaser above
/// already follow, so a frequency with no tidy relationship to the comb
/// spacings is what keeps the reference from landing on a numerically
/// degenerate coincidence with them.
#[test]
fn golden_plate_reverb_tail() {
    let mixer = deck_a(tone_pcm(233.0, 48_000, 3.0));
    mixer.set_deck_plate_reverb(DeckId::A, true);
    mixer.set_deck_plate_reverb_size(DeckId::A, 0.6);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("plate_reverb_tail", &left, &right);
}

/// A tone above the cutoff, so the reference actually carries the
/// ladder's own character (a resonant peak near the cutoff, a rolled
/// off fundamental) rather than passing the tone through unchanged.
#[test]
fn golden_moog_ladder_lowpass() {
    let mixer = deck_a(tone_pcm(2_000.0, 48_000, 3.0));
    mixer.set_deck_moog_ladder(DeckId::A, true);
    mixer.set_deck_moog_ladder_cutoff(DeckId::A, 800.0);
    mixer.set_deck_moog_ladder_resonance(DeckId::A, 0.6);
    mixer.set_deck_playing(DeckId::A, true);
    // Cutoff and resonance ramp over 144ms (widened after a click was
    // found in a large single-jump gesture), longer than the ordinary
    // SETTLE -- SETTLE_STRETCH clears that with margin so the reference
    // captures the filter's steady-state character, not the tail of its
    // own engage ramp settling in.
    settle(&mixer, SETTLE_STRETCH);
    let (left, right) = capture(&mixer, CAPTURE, |_| {});
    assert_golden("moog_ladder_lowpass", &left, &right);
}

// ---------------------------------------------------------------------------
// clicks
// ---------------------------------------------------------------------------

/// The biggest neighbouring-sample step on the left channel while `at`
/// acts on the mixer part-way through half a second of DC at half scale.
fn worst_step_across(rig: &Rig, at: impl FnMut(usize)) -> f32 {
    let (left, _) = capture(rig, CAPTURE, at);
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

/// A reverse hold moves only the read RATE, so DC would hide any step it
/// made. This is the split signal a raw splice cannot hide in, the same one
/// the seek test uses.
#[test]
fn a_censor_and_its_return_are_click_free() {
    let mixer = deck_a(split_pcm(16_384, -16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_censor(DeckId::A, true),
        24 => mixer.set_deck_censor(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "a reverse hold must flip and land, biggest step {worst}");
}

#[test]
fn a_brake_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.spin_deck(DeckId::A, SpinMotion::Brake);
        }
    });
    assert!(worst < CLICK, "a brake must wind down, biggest step {worst}");
}

/// Well clear of the split, which is at the half-way mark: a record thrown
/// backwards across a splice reproduces the splice, and that is the
/// material speaking rather than the gesture.
#[test]
fn a_spin_back_is_click_free() {
    let mixer = deck_a(split_pcm(16_384, -16_384, 480_000, 48_000));
    mixer.seek_deck_seconds(DeckId::A, 7.5);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.spin_deck(DeckId::A, SpinMotion::SpinBack);
        }
    });
    assert!(worst < CLICK, "a spin-back must throw and fall, biggest step {worst}");
}

#[test]
fn a_soft_start_is_click_free() {
    let mixer = deck_a(split_pcm(16_384, -16_384, 480_000, 48_000));
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.spin_deck(DeckId::A, SpinMotion::SoftStart);
        }
    });
    assert!(worst < CLICK, "a soft start must wind up, biggest step {worst}");
}

/// The test this whole load-over-playing path exists for. Against a plain
/// install -- which cuts the transport to zero on whatever sample the
/// decode landed on -- this fails outright.
#[test]
fn a_load_over_a_playing_deck_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.install_deck_over(DeckId::A, const_pcm(8_000, 480_000, 48_000), true);
        }
    });
    assert!(worst < CLICK, "a load over a playing deck must hand over, biggest step {worst}");
}

#[test]
fn a_keylock_toggle_is_click_free() {
    // A TONE, not DC. The two read paths hand the playhead over exactly but
    // do not agree on PHASE, and DC is phase-invariant, so this test could
    // not see the splice it is named for until the fixture had a waveform.
    // Low, because the click rule is a bound on the step between two
    // samples and a high tone's own slope would eat the whole budget: at
    // 40 Hz the material moves 0.003 a sample and a phase splice moves it
    // by up to a whole amplitude.
    let mixer = deck_a(tone_pcm(40.0, 48_000, 10.0));
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

/// Coming back after a long dry spell. The plan wanted a hold-off here --
/// ramps pinned for twice the crossover's group delay so a filter could
/// not return with a thump -- and this is the measurement that says
/// whether this structure needs one. It does not: the band gains sit
/// AFTER the filters, so a centred knob leaves nothing staled to thump
/// on, and the 12ms engage ramp covers the rest.
#[test]
fn a_filter_re_engaged_after_a_dry_spell_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        // Sweep well off centre, then back to centre and stay there long
        // enough for the wet ramp to bottom out and the handover to
        // drain, then come back the other way.
        4 => mixer.set_deck_filter(DeckId::A, 0.15),
        12 => mixer.set_deck_filter(DeckId::A, 0.5),
        40 => mixer.set_deck_filter(DeckId::A, 0.85),
        _ => {}
    });
    assert!(worst < CLICK, "a filter re-engage must not step, biggest step {worst}");
}

#[test]
fn engaging_and_releasing_the_echo_are_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_keylock(DeckId::A, false);
    mixer.set_deck_grid(DeckId::A, Some(grid_120()));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_echo(DeckId::A, Some((1, 2))),
        24 => mixer.set_deck_echo(DeckId::A, None),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the echo must ramp, biggest step {worst}");
}

#[test]
fn engaging_and_releasing_the_flanger_are_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_flanger(DeckId::A, true),
        24 => mixer.set_deck_flanger(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the flanger must ramp, biggest step {worst}");
}

/// 12345, not 16384: half scale is exactly 64/128, a value the default
/// 8-bit quantizer reproduces losslessly, which would make an engage or
/// a bit-depth change invisible to this test regardless of whether the
/// ramp/handover is implemented at all. 12345 lands off any few-bit
/// quantization grid, so crushing it actually moves the signal.
#[test]
fn engaging_and_releasing_the_bitcrusher_are_click_free() {
    let mixer = deck_a(const_pcm(12_345, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_bitcrusher(DeckId::A, true),
        24 => mixer.set_deck_bitcrusher(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the bitcrusher must ramp, biggest step {worst}");
}

/// The test that actually exercises this effect's central lesson: a
/// naive `.slew()` on `bits` alone would let a rounding-boundary
/// crossing step the output by close to a full quantization step the
/// instant the ramping scalar crosses it -- ramping the scalar into a
/// rounding function does not bound what comes out of it. The two-
/// stream handover in `Bitcrusher::set_bits` is what this pins.
#[test]
fn crushing_the_bit_depth_mid_stream_is_click_free() {
    let mixer = deck_a(const_pcm(12_345, 480_000, 48_000));
    mixer.set_deck_bitcrusher(DeckId::A, true);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_bitcrusher_bits(DeckId::A, 2.0);
        }
    });
    assert!(worst < CLICK, "a bit-depth change must hand over, biggest step {worst}");
}

/// The rate knob's counterpart: a hard step here only shifts the phase
/// of the effect's own staircase (which hold gets captured when), never
/// the amplitude mapping, so a plain ramp is expected to already be
/// enough -- this test is the confirmation that expectation holds, not
/// a search for a bug the way the bits test above is.
#[test]
fn changing_crush_rate_mid_stream_is_click_free() {
    let mixer = deck_a(const_pcm(12_345, 480_000, 48_000));
    mixer.set_deck_bitcrusher(DeckId::A, true);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_bitcrusher_rate(DeckId::A, 1_000.0);
        }
    });
    assert!(worst < CLICK, "a crush-rate change must ramp, biggest step {worst}");
}

#[test]
fn engaging_and_releasing_the_tremolo_are_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_tremolo(DeckId::A, true),
        24 => mixer.set_deck_tremolo(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the tremolo must ramp, biggest step {worst}");
}

/// A multiply, not a rounding function: unlike the bitcrusher's bits,
/// nothing here is expected to need a handover, and this test is the
/// confirmation of that, not a search for a bug.
#[test]
fn a_tremolo_depth_change_while_engaged_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_tremolo(DeckId::A, true);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_tremolo_depth(DeckId::A, 0.1);
        }
    });
    assert!(worst < CLICK, "a depth change must ramp, biggest step {worst}");
}

/// Locking an LFO to the beat while it is already sounding changes the
/// phase's future VELOCITY, never its value at any instant, so it needs
/// no ramp of its own -- this test is the confirmation of that
/// reasoning, not a search for a bug.
#[test]
fn locking_the_tremolo_to_the_beat_while_engaged_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_tremolo(DeckId::A, true);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_tremolo_sync_units(DeckId::A, 8),
        24 => mixer.set_deck_tremolo_sync_units(DeckId::A, 0),
        _ => {}
    });
    assert!(worst < CLICK, "a beat lock must not step the output, biggest step {worst}");
}

/// The same for the other three: each reads its rate from a different
/// place once synced, and none of them may step doing it.
#[test]
fn locking_the_other_lfos_to_the_beat_while_engaged_is_click_free() {
    for (name, engage, lock) in [
        (
            "autopan",
            &Mixer::set_deck_autopan as &dyn Fn(&Mixer, DeckId, bool),
            &Mixer::set_deck_autopan_sync_units as &dyn Fn(&Mixer, DeckId, u32),
        ),
        (
            "flanger",
            &Mixer::set_deck_flanger as &dyn Fn(&Mixer, DeckId, bool),
            &Mixer::set_deck_flanger_sync_units as &dyn Fn(&Mixer, DeckId, u32),
        ),
        (
            "phaser",
            &Mixer::set_deck_phaser as &dyn Fn(&Mixer, DeckId, bool),
            &Mixer::set_deck_phaser_sync_units as &dyn Fn(&Mixer, DeckId, u32),
        ),
    ] {
        let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
        engage(&mixer, DeckId::A, true);
        mixer.set_deck_playing(DeckId::A, true);
        settle(&mixer, SETTLE);
        let worst = worst_step_across(&mixer, |index| match index {
            8 => lock(&mixer, DeckId::A, 8),
            24 => lock(&mixer, DeckId::A, 0),
            _ => {}
        });
        assert!(worst < CLICK, "locking the {name} stepped the output, biggest step {worst}");
    }
}

/// A locked LFO retunes every buffer from the deck's tempo, so walking
/// the ladder under it is the ordinary case, not a special one: still
/// only a velocity change, still no step. Walked across the whole
/// range, an eighth of a cycle a beat up to sixty-four of them.
#[test]
fn walking_the_sync_ladder_under_a_locked_tremolo_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_tremolo(DeckId::A, true);
    mixer.set_deck_tremolo_sync_units(DeckId::A, 8);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let rungs = [1u32, 512, 4, 128, 2, 64, 8];
    let worst = worst_step_across(&mixer, |index| {
        if index >= 4 && (index - 4) % 4 == 0 {
            if let Some(units) = rungs.get((index - 4) / 4) {
                mixer.set_deck_tremolo_sync_units(DeckId::A, *units);
            }
        }
    });
    assert!(worst < CLICK, "walking the ladder must not step, biggest step {worst}");
}

/// Engaging a synced LFO jumps its phase to the chosen offset. That
/// jump happens under the engage ramp, which is what keeps it silent --
/// the same protection every other engage in this file leans on.
#[test]
fn engaging_a_synced_tremolo_at_an_offset_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_tremolo_sync_units(DeckId::A, 8);
    mixer.set_deck_tremolo_beat_offset(DeckId::A, 0.75);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_tremolo(DeckId::A, true),
        24 => mixer.set_deck_tremolo(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "an offset engage must ramp, biggest step {worst}");
}

#[test]
fn engaging_and_releasing_the_distortion_are_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_distortion(DeckId::A, true),
        24 => mixer.set_deck_distortion(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the distortion must ramp, biggest step {worst}");
}

/// `pade_tanh` is continuous everywhere it is defined, so unlike the
/// bitcrusher's bit depth, drive is expected to need no handover -- this
/// test is the confirmation of that, not a search for a bug.
#[test]
fn a_distortion_drive_change_while_engaged_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_distortion(DeckId::A, true);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_distortion_drive(DeckId::A, 18.0);
        }
    });
    assert!(worst < CLICK, "a drive change must ramp, biggest step {worst}");
}

#[test]
fn engaging_and_releasing_the_phaser_are_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_phaser(DeckId::A, true),
        24 => mixer.set_deck_phaser(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the phaser must ramp, biggest step {worst}");
}

#[test]
fn a_phaser_feedback_change_while_engaged_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_phaser(DeckId::A, true);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_phaser_feedback(DeckId::A, 0.8);
        }
    });
    assert!(worst < CLICK, "a feedback change must ramp, biggest step {worst}");
}

#[test]
fn engaging_and_releasing_the_autopan_are_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_autopan(DeckId::A, true),
        24 => mixer.set_deck_autopan(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the autopan must ramp, biggest step {worst}");
}

#[test]
fn engaging_and_releasing_the_stereo_width_are_click_free() {
    let mixer = deck_a(const_stereo_pcm(16_384, -8_192, 480_000, 48_000));
    mixer.set_deck_stereo_width_amount(DeckId::A, 1.8);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_stereo_width(DeckId::A, true),
        24 => mixer.set_deck_stereo_width(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the stereo width must ramp, biggest step {worst}");
}

#[test]
fn a_stereo_width_amount_change_while_engaged_is_click_free() {
    let mixer = deck_a(const_stereo_pcm(16_384, -8_192, 480_000, 48_000));
    mixer.set_deck_stereo_width(DeckId::A, true);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_stereo_width_amount(DeckId::A, 0.0);
        }
    });
    assert!(worst < CLICK, "a width change must ramp, biggest step {worst}");
}

#[test]
fn engaging_and_releasing_the_plate_reverb_are_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_plate_reverb_size(DeckId::A, 0.9);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_plate_reverb(DeckId::A, true),
        24 => mixer.set_deck_plate_reverb(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the plate reverb must ramp, biggest step {worst}");
}

#[test]
fn a_plate_reverb_size_change_while_engaged_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_plate_reverb(DeckId::A, true);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_plate_reverb_size(DeckId::A, 0.95);
        }
    });
    assert!(worst < CLICK, "a size change must ramp, biggest step {worst}");
}

#[test]
fn engaging_and_releasing_the_moog_ladder_are_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_moog_ladder_cutoff(DeckId::A, 800.0);
    mixer.set_deck_moog_ladder_resonance(DeckId::A, 0.8);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_moog_ladder(DeckId::A, true),
        24 => mixer.set_deck_moog_ladder(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "engaging or releasing the moog ladder must ramp, biggest step {worst}");
}

#[test]
fn a_moog_ladder_cutoff_change_while_engaged_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_moog_ladder(DeckId::A, true);
    mixer.set_deck_moog_ladder_resonance(DeckId::A, 0.8);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_moog_ladder_cutoff(DeckId::A, 8_000.0);
        }
    });
    assert!(worst < CLICK, "a cutoff change must ramp, biggest step {worst}");
}

#[test]
fn a_moog_ladder_resonance_change_while_engaged_is_click_free() {
    let mixer = deck_a(const_pcm(16_384, 480_000, 48_000));
    mixer.set_deck_moog_ladder(DeckId::A, true);
    mixer.set_deck_moog_ladder_cutoff(DeckId::A, 800.0);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_moog_ladder_resonance(DeckId::A, 1.0);
        }
    });
    assert!(worst < CLICK, "a resonance change must ramp, biggest step {worst}");
}

#[test]
fn a_retune_under_a_tempo_move_is_click_free() {
    // 37 Hz, not 40: its period shares no whole number of cycles with
    // the delay below, so a hard jump between the old tap and the new
    // one would actually show up as a phase step -- a round multiple
    // would read the same phase either side and hide it, the same trap
    // the unit-level handover test avoids for the same reason.
    let mixer = deck_a(tone_pcm(37.0, 48_000, 10.0));
    mixer.set_deck_keylock(DeckId::A, false);
    mixer.set_deck_grid(DeckId::A, Some(grid_120()));
    // A quarter beat, not a half: by the time the retune below fires
    // (settle plus eight buffers in) enough has been written that the
    // OLD tap already carries real content, so a hard switch away from
    // it would actually be heard rather than jumping between two reads
    // that both still land before anything was ever written.
    mixer.set_deck_echo(DeckId::A, Some((1, 4)));
    mixer.set_deck_echo_feedback(DeckId::A, 0.7);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        if index == 8 {
            mixer.set_deck_rate(DeckId::A, 1.08);
        }
    });
    assert!(worst < CLICK, "a retune under a tempo move must hand over, biggest step {worst}");
}

/// A tone on the left channel only, silence on the right: ping-pong's
/// crossing writes a DIFFERENT value to each channel, which a signal
/// panned dead centre (equal on both) cannot show at all -- the two
/// feedback terms it would blend are identical either way.
fn panned_tone_pcm(frequency: f64, rate: u32, seconds: f64) -> Arc<TrackPcm> {
    let len = (rate as f64 * seconds) as usize;
    let frames = (0..len)
        .map(|index| {
            let value =
                (2.0 * std::f64::consts::PI * frequency * index as f64 / rate as f64).sin();
            [(value * 12_000.0) as i16, 0i16]
        })
        .collect();
    Arc::new(TrackPcm { frames, sample_rate: rate })
}

#[test]
fn switching_ping_pong_is_click_free() {
    let mixer = deck_a(panned_tone_pcm(40.0, 48_000, 10.0));
    mixer.set_deck_keylock(DeckId::A, false);
    mixer.set_deck_grid(DeckId::A, Some(grid_120()));
    mixer.set_deck_echo(DeckId::A, Some((1, 4)));
    mixer.set_deck_echo_feedback(DeckId::A, 0.5);
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        8 => mixer.set_deck_echo_pingpong(DeckId::A, true),
        24 => mixer.set_deck_echo_pingpong(DeckId::A, false),
        _ => {}
    });
    assert!(worst < CLICK, "a ping-pong switch must blend, biggest step {worst}");
}

/// A tone, not DC: DC would make the frozen lap and the live signal
/// identical and hide a press or a release that switched rather than
/// ramped. 101 Hz over a 0.25 s lap is 25.25 cycles -- not a whole
/// number, so frozen and live genuinely disagree in phase at the press
/// (a split signal was tried first, but a lap that reaches across the
/// split's own hard edit point carries that edit's own click once a
/// lap, which is the SOURCE's discontinuity, not a press or a release
/// failing to ramp).
#[test]
fn a_freeze_hold_and_release_are_click_free() {
    let mixer = deck_a(tone_pcm(101.0, 48_000, 3.0));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| match index {
        20 => mixer.set_deck_freeze(DeckId::A, Some(0.25)),
        40 => mixer.set_deck_freeze(DeckId::A, None),
        _ => {}
    });
    assert!(worst < CLICK, "a freeze press or release must ramp, biggest step {worst}");
}

/// The ordinary gesture: a sweep dragged from one side of the knob to the
/// other, through the dead zone. The jump test above drives DC, which a
/// low-pass passes untouched, so it cannot see a filter ringing on the
/// other filter's memory; this one drives a tone, which can.
#[test]
fn a_filter_sweep_through_centre_is_click_free() {
    let mixer = deck_a(tone_pcm(220.0, 48_000, 3.0));
    mixer.set_deck_playing(DeckId::A, true);
    settle(&mixer, SETTLE);
    let worst = worst_step_across(&mixer, |index| {
        let position = (0.2 + index as f32 * 0.02).min(0.85);
        mixer.set_deck_filter(DeckId::A, position);
    });
    assert!(worst < CLICK, "a sweep through the centre stepped by {worst}");
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
