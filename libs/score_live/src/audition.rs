//! The instrument picker's audition: a short phrase on the choice under
//! the pointer, rendered off the audio thread ([`render_phrase`]) and
//! played on a small preview bus ([`bus`]) the host adds to its output.

use crate::instruments::Choice;
use crate::render::{render, RenderOpts};

/// How long a phrase plays, in seconds (with its tail).
pub const SECONDS: f64 = 2.0;

/// A tiny score playing a phrase on `choice`: a drum groove on a kit, a
/// sung line on a voice, a rising arpeggio and a chord on anything else.
/// None for what makes no sound here (MIDI out, the audio input).
pub fn phrase(choice: &Choice) -> Option<String> {
    let field = |key: &str| choice.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    if choice.type_name == "Instrument" && matches!(field("kind"), Some("@midi") | Some("@input")) {
        return None;
    }
    let fields: Vec<String> = choice.fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    let ctor = format!("{}{{{}}}", choice.type_name, fields.join(" "));
    let drums = choice.type_name == "Drums" || field("kind") == Some("@drums");
    let code = if drums {
        "hit(@kick) hit(@hat, {amp: 0.5}) sleep(0.5) hit(@hat, {amp: 0.4}) sleep(0.5) hit(@snare) hit(@hat, {amp: 0.5}) sleep(0.5) hit(@hat, {amp: 0.4}) sleep(0.25) hit(@kick, {amp: 0.7}) sleep(0.25) hit(@kick) hit(@hat, {amp: 0.5}) sleep(0.5) hit(@snare) sleep(0.5)"
    } else if choice.type_name == "Voice" {
        "sleep(sing([@c4 @e4 @g4 @e4], \"la la la ah\", {dur: 0.5}))"
    } else {
        "play(@c4, {dur: 0.2}) sleep(0.25) play(@e4, {dur: 0.2}) sleep(0.25) play(@g4, {dur: 0.2}) sleep(0.25) play(@c5, {dur: 0.2}) sleep(0.25) play([@c3 @g3 @c4 @e4], {dur: 1.4}) sleep(2)"
    };
    Some(format!("use mod.score.*\nlet clock = Clock{{bpm: 120}}\nlet sound = {ctor}\nlet phrase = Loop{{play: sound beats: 4 code: fn(i) {{ {code} }}}}\nScore{{clock sound phrase}}\n"))
}

/// The score with `node` switched to `choice`, as heard in place: while
/// it plays, the whole mix from `song_beat` (the next bar) for `bars`;
/// stopped, only the loops that play `node` (soloed), from the first
/// section they play in. None when the switch does not apply or the
/// choice makes no sound here.
pub fn in_place(source: &str, node: &str, choice: &Choice, song_beat: Option<f64>) -> Option<(String, Option<f64>)> {
    phrase(choice)?;
    let splice = crate::instruments::switch(source, node, choice).ok()?;
    let mut next = crate::source::apply(source, &splice);
    let doc = crate::model::ScoreDoc::load(&next).ok()?;
    let players: Vec<String> = doc.loops.iter().filter(|l| l.play.as_deref() == Some(node)).map(|l| l.name.clone()).collect();
    if players.is_empty() {
        return None;
    }
    if song_beat.is_some() {
        return Some((next, song_beat));
    }
    for name in &players {
        if let Ok(splice) = crate::source::set_field(&next, name, "solo", "true") {
            next = crate::source::apply(&next, &splice);
        }
    }
    Some((next, first_beat(&doc, &players)))
}

/// Where the first section that plays any of `loops` starts (song beats);
/// None when the score has no arrangement.
pub fn first_beat(doc: &crate::model::ScoreDoc, loops: &[String]) -> Option<f64> {
    let bar = doc.beats_per_bar();
    let mut at = 0.0;
    for name in &doc.arrange {
        let Some(section) = doc.sections.iter().find(|s| &s.name == name) else { continue };
        if section.loops.iter().any(|l| loops.contains(l)) {
            return Some(at);
        }
        at += section.bars * bar;
    }
    None
}

/// Render an in-place audition: `seconds` from `start` (song beats).
pub fn render_in_place(source: &str, start: Option<f64>, seconds: f64, rate: f32) -> Result<(Vec<f32>, Vec<f32>, f32), String> {
    let r = render(source, &RenderOpts { rate, seconds: Some(seconds.clamp(0.5, 8.0)), seed: None, params: Vec::new(), start, held: Vec::new() }, &crate::NoSamples)
        .map_err(|d| d.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; "))?;
    Ok((r.left, r.right, r.rate))
}

/// The phrase on `choice`, rendered: (left, right, rate).
pub fn render_phrase(choice: &Choice, rate: f32) -> Result<(Vec<f32>, Vec<f32>, f32), String> {
    let source = phrase(choice).ok_or_else(|| format!("{} makes no sound to audition", choice.label))?;
    let r = render(&source, &RenderOpts { rate, seconds: Some(SECONDS), seed: Some(1), params: Vec::new(), start: None, held: Vec::new() }, &crate::NoSamples)
        .map_err(|d| d.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; "))?;
    Ok((r.left, r.right, r.rate))
}

/// A tiny score playing `node` of `source` as it is (its own fields) on
/// one `note` (MIDI): what an editor panel's keyboard plays. None when the
/// node is not an instrument of the score.
pub fn key(source: &str, node: &str, note: f32) -> Option<String> {
    let outline = crate::source::outline(source);
    let ctor = outline.node(node)?;
    if !crate::instruments::INSTRUMENT_TYPES.contains(&ctor.type_name.as_str()) {
        return None;
    }
    let body = ctor.body.text(source);
    let drums = ctor.type_name == "Drums";
    let code = if drums { "hit(@kick) sleep(0.5) hit(@snare) sleep(0.5) hit(@hat) sleep(1)".to_string() } else { format!("play({note}, {{dur: 1.2}}) sleep(4)") };
    Some(format!("use mod.score.*\nlet clock = Clock{{bpm: 120}}\nlet {node} = {}{body}\nlet key = Loop{{play: {node} beats: 4 code: fn(i) {{ {code} }}}}\nScore{{clock {node} key}}\n", ctor.type_name))
}

/// Render a [`key`] score with the fields an editor holds: (left, right, rate).
pub fn render_key(source: &str, held: &[crate::held::Held], rate: f32) -> Result<(Vec<f32>, Vec<f32>, f32), String> {
    let r = render(source, &RenderOpts { rate, seconds: Some(SECONDS), seed: Some(1), params: Vec::new(), start: None, held: held.to_vec() }, &crate::NoSamples)
        .map_err(|d| d.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; "))?;
    Ok((r.left, r.right, r.rate))
}

/// The preview bus: one clip at a time, written by the UI side and read
/// by the host's output callback with atomics only (no lock, no
/// allocation), resampled to the device's rate, at the gain the host sets.
/// It never reaches the audience unless asked: it plays on the cue
/// (headphone) channels 3–4 of an output that has them, else nowhere,
/// unless the host routes it to the main output.
pub mod bus {
    use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

    /// Where the bus plays.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Route {
        Off,
        /// Channels 3–4 of the output (a headphone cue).
        Cue,
        /// The main output (the audience hears it).
        Main,
    }

    static ROUTE: AtomicU8 = AtomicU8::new(0);
    static CHANNELS: AtomicUsize = AtomicUsize::new(2);

    pub fn set_route(route: Route) {
        ROUTE.store(match route { Route::Off => 0, Route::Cue => 1, Route::Main => 2 }, Ordering::Relaxed);
    }

    pub fn route() -> Route {
        match ROUTE.load(Ordering::Relaxed) { 1 => Route::Cue, 2 => Route::Main, _ => Route::Off }
    }

    /// The output's channel count (the callback reports it): 4 or more
    /// gives a cue on channels 3–4.
    pub fn set_channels(n: usize) {
        CHANNELS.store(n, Ordering::Relaxed);
    }

    pub fn has_cue() -> bool {
        CHANNELS.load(Ordering::Relaxed) >= 4
    }
    use std::sync::OnceLock;

    /// The longest clip, in stereo frames (at up to 48 kHz).
    pub const MAX_FRAMES: usize = 48_000 * 3;

    struct Bus {
        samples: Box<[AtomicU32]>,
        /// Frames of the clip playing (0: nothing plays).
        len: AtomicUsize,
        /// The read position in clip frames (f64 bits).
        pos: AtomicU64,
        rate: AtomicU32,
        gain: AtomicU32,
    }

    fn bus() -> &'static Bus {
        static BUS: OnceLock<Bus> = OnceLock::new();
        BUS.get_or_init(|| Bus {
            samples: (0..MAX_FRAMES * 2).map(|_| AtomicU32::new(0)).collect(),
            len: AtomicUsize::new(0),
            pos: AtomicU64::new(0),
            rate: AtomicU32::new(48_000f32.to_bits()),
            gain: AtomicU32::new(1f32.to_bits()),
        })
    }

    /// Play a clip from its start (the one playing stops).
    pub fn play(left: &[f32], right: &[f32], rate: f32) {
        play_after(left, right, rate, 0.0);
    }

    /// Play a clip `delay` seconds from now (on a bar to come).
    pub fn play_after(left: &[f32], right: &[f32], rate: f32, delay: f64) {
        let b = bus();
        b.len.store(0, Ordering::Release);
        let n = left.len().min(right.len()).min(MAX_FRAMES);
        for i in 0..n {
            b.samples[i * 2].store(left[i].to_bits(), Ordering::Relaxed);
            b.samples[i * 2 + 1].store(right[i].to_bits(), Ordering::Relaxed);
        }
        b.rate.store(rate.to_bits(), Ordering::Relaxed);
        // A delay not yet over starts before the clip; one already past
        // (the render took longer) starts into it.
        b.pos.store((-delay * rate as f64).to_bits(), Ordering::Relaxed);
        b.len.store(n, Ordering::Release);
    }

    pub fn stop() {
        bus().len.store(0, Ordering::Release);
    }

    pub fn playing() -> bool {
        bus().len.load(Ordering::Acquire) > 0
    }

    /// The host's level for the bus (its master; 0 mutes it).
    pub fn set_gain(gain: f32) {
        bus().gain.store(gain.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// The main output's share: the clip only when routed there.
    pub fn mix_main(rate: f64, left: &mut [f32], right: &mut [f32]) {
        if route() == Route::Main {
            mix_into(rate, left, right);
        }
    }

    /// The cue channels' share: the clip only when routed there.
    pub fn mix_cue(rate: f64, left: &mut [f32], right: &mut [f32]) {
        if route() == Route::Cue {
            mix_into(rate, left, right);
        }
    }

    /// Add the clip to a device buffer of `rate` Hz. Realtime: atomics only.
    pub fn mix_into(rate: f64, left: &mut [f32], right: &mut [f32]) {
        let b = bus();
        let n = b.len.load(Ordering::Acquire);
        if n < 2 {
            return;
        }
        let gain = f32::from_bits(b.gain.load(Ordering::Relaxed));
        let step = f32::from_bits(b.rate.load(Ordering::Relaxed)) as f64 / rate.max(1.0);
        let mut pos = f64::from_bits(b.pos.load(Ordering::Relaxed));
        let sample = |frame: usize, ch: usize| f32::from_bits(b.samples[frame * 2 + ch].load(Ordering::Relaxed));
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            // Before its start (a delayed clip): silence.
            if pos < 0.0 {
                pos += step;
                continue;
            }
            let frame = pos as usize;
            if frame + 1 >= n {
                // Done: nothing plays (unless a new clip took the bus).
                let _ = b.len.compare_exchange(n, 0, Ordering::AcqRel, Ordering::Relaxed);
                return;
            }
            if gain > 0.0 {
                let f = (pos - frame as f64) as f32;
                // A 5 ms fade in: a clip never starts with a click.
                let fade = (frame as f32 / 240.0).min(1.0);
                *l += (sample(frame, 0) * (1.0 - f) + sample(frame + 1, 0) * f) * gain * fade;
                *r += (sample(frame, 1) * (1.0 - f) + sample(frame + 1, 1) * f) * gain * fade;
            }
            pos += step;
        }
        b.pos.store(pos.to_bits(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instruments::{catalog, Available};

    #[test]
    fn every_instrument_auditions_or_says_why_not() {
        let all = catalog(None, &Available::default());
        let mut groups = vec!["Synths", "Morph", "Drum kits", "Piano", "Ironfish", "Voices", "Shader instruments", "MIDI out", "Audio in"];
        // The neural singer, where a voice is installed (MAKEPAD_CANTOR_MODEL).
        if makepad_score_player::sung::neural::model_path().is_file() {
            groups.push("Neural singers");
        }
        for group in groups {
            let choice = all.iter().find(|c| c.group == group).unwrap_or_else(|| panic!("no {group}"));
            match render_phrase(choice, 24_000.0) {
                Ok((l, r, rate)) => {
                    assert_eq!(rate, 24_000.0);
                    let peak = l.iter().chain(&r).fold(0.0f32, |a, x| a.max(x.abs()));
                    assert!(peak > 0.01, "{group} {}: silent", choice.label);
                }
                Err(e) => assert!(matches!(group, "MIDI out" | "Audio in"), "{group}: {e}"),
            }
        }
    }

    #[test]
    fn in_place_switches_the_node_and_plays_its_own_loops() {
        let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet bass = Synth{preset: @acid}\nlet kit = Drums{kit: @analog_hybrid}\nlet beat = Loop{play: kit hits: \"x...\"}\nlet line = Loop{play: bass beats: 4 code: fn(i) { play(@a2, {dur: 1}) sleep(4) }}\nlet intro = Section{bars: 2 loops: [beat]}\nlet main = Section{bars: 4 loops: [beat line]}\nScore{clock bass kit beat line intro main arrange: [intro main]}\n";
        let all = catalog(None, &Available::default());
        let piano = all.iter().find(|c| c.group == "Piano").unwrap();
        // Stopped: only `line` (soloed), from the section it plays in.
        let (candidate, start) = in_place(src, "bass", piano, None).unwrap();
        assert!(candidate.contains("let bass = Instrument{kind: @piano") && candidate.contains("solo: true"), "{candidate}");
        assert_eq!(start, Some(8.0));
        let (l, _, _) = render_in_place(&candidate, start, 2.0, 24_000.0).unwrap();
        assert!(l.iter().fold(0.0f32, |a, x| a.max(x.abs())) > 0.01);
        // Playing: the whole mix, from where it plays.
        let (candidate, start) = in_place(src, "bass", piano, Some(12.0)).unwrap();
        assert!(!candidate.contains("solo: true") && start == Some(12.0));
        // MIDI out or audio in: no audition.
        let midi = all.iter().find(|c| c.group == "MIDI out").unwrap();
        assert!(in_place(src, "bass", midi, None).is_none());
    }

    #[test]
    fn the_bus_plays_a_clip_once_at_the_device_rate() {
        let clip: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        bus::set_gain(1.0);
        bus::set_route(bus::Route::Main);
        bus::play(&clip, &clip, 24_000.0);
        let (mut l, mut r) = (vec![0.0; 1500], vec![0.0; 1500]);
        bus::mix_into(48_000.0, &mut l, &mut r);
        // Twice the device rate: every clip frame lasts two device frames
        // (past the 5 ms fade in).
        assert!((l[600] - 0.3).abs() < 1e-3, "{}", l[600]);
        assert!(l[20] < 0.01 * 0.5);
        bus::mix_into(48_000.0, &mut l[..600], &mut r[..600]);
        assert!(!bus::playing());
        bus::set_gain(0.0);
        bus::play(&clip, &clip, 48_000.0);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        bus::mix_into(48_000.0, &mut l, &mut r);
        assert!(l.iter().all(|x| *x == 0.0));
        // Routed to the cue, the main output never hears it.
        bus::set_gain(1.0);
        bus::set_route(bus::Route::Cue);
        bus::play(&clip, &clip, 48_000.0);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        bus::mix_main(48_000.0, &mut l, &mut r);
        assert!(l.iter().all(|x| *x == 0.0));
        bus::mix_cue(48_000.0, &mut l, &mut r);
        assert!(l[10] > 0.0);
        bus::stop();
        bus::set_route(bus::Route::Off);
    }
}
