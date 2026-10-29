//! Offline render: the score's player on its own clock (the same nodes
//! Stage plays it on), with the composer feeding its loops pass by pass. A render is a pure function of the source, the seed, the params
//! and the rate.

use crate::build::Samples;
use crate::compose::{Composer, Played};
use crate::model::{self, Diag, ScoreDoc};
use crate::player::{sample_name, ClockMode, Player};
use makepad_audio_engine::feed::{forget_instance, tempo_feed, tempo_seconds, TempoEvent};
use makepad_audio_engine::CHUNK;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct RenderOpts {
    pub rate: f32,
    /// Seconds to render; `None` is the arrangement plus a tail, or 16 bars.
    pub seconds: Option<f64>,
    /// Replace the score's seed.
    pub seed: Option<u64>,
    pub params: Vec<(String, f32)>,
    /// Start at this beat of the arrangement (its section from there).
    pub start: Option<f64>,
    /// Fields held live during the render, as an editor's controls do
    /// ([`crate::held`]): on the one prepared graph, no new revision.
    pub held: Vec<crate::held::Held>,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts { rate: 48_000.0, seconds: None, seed: None, params: Vec::new(), start: None, held: Vec::new() }
    }
}

pub struct Rendered {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub rate: f32,
    pub bpm: f64,
    pub beats_per_bar: f64,
    pub played: Vec<Played>,
    /// Section starts in beats.
    pub sections: Vec<(f64, String)>,
    /// The tempo changes it played (the tempo map from `bpm`):
    /// `seconds_at` turns a beat into its time.
    pub tempo: Vec<TempoEvent>,
    /// Loops whose code failed (they rendered nothing for those cycles).
    pub errors: Vec<Diag>,
}

impl Rendered {
    /// Seconds from the start to `beat` (the beat and bar marks).
    pub fn seconds_at(&self, beat: f64) -> f64 {
        tempo_seconds(self.bpm, &self.tempo, beat)
    }

    pub fn seconds(&self) -> f64 {
        self.left.len() as f64 / self.rate as f64
    }
}

/// How long a score is, in seconds, with a tail for its echoes.
pub fn natural_length(doc: &ScoreDoc) -> f64 {
    let beats = doc.arrangement_beats();
    let beats = if beats > 0.0 { beats } else { 16.0 * doc.beats_per_bar() };
    tempo_seconds(transport_bpm(doc), &arrangement_tempo(doc), beats) + if doc.arrangement_beats() > 0.0 { tail_seconds(doc) } else { 0.0 }
}

/// How long the music rings after its last bar: the longest reverb, a
/// synth's release, the piano's decay, at least 2 s.
pub fn tail_seconds(doc: &ScoreDoc) -> f64 {
    let mut tail: f64 = 2.0;
    for fx in &doc.fx {
        if fx.kind == makepad_score_player::FxKind::Reverb {
            tail = tail.max(1.0 + fx.params[0] as f64 * 6.0);
        }
    }
    for inst in &doc.insts {
        let ring = match &inst.kind {
            crate::model::InstKind::Synth { recipe, .. } => recipe.layers().iter().map(|l| l.release as f64).fold(0.0, f64::max) + 0.5,
            crate::model::InstKind::Native { native, .. } if native == "piano" => 6.0,
            crate::model::InstKind::Voice { .. } | crate::model::InstKind::Ironfish { .. } | crate::model::InstKind::Morph { .. } | crate::model::InstKind::Tandem { .. } => 3.0,
            _ => 0.0,
        };
        tail = tail.max(ring);
    }
    tail.min(12.0)
}

/// The tempo changes the arrangement makes as written (its sections'
/// `bpm`), once through.
pub fn arrangement_tempo(doc: &ScoreDoc) -> Vec<TempoEvent> {
    let mut out = Vec::new();
    let mut beat = 0.0;
    for section in doc.arrange.iter().filter_map(|name| doc.sections.iter().find(|s| &s.name == name)) {
        if let Some(bpm) = section.bpm {
            out.push(TempoEvent { beat, bpm, ramp: section.ramp });
        }
        let len = section.bars * doc.beats_per_bar();
        out.extend(section.tempo.iter().filter(|(at, _)| *at < len).map(|&(at, bpm)| TempoEvent { beat: beat + at, bpm, ramp: 0.0 }));
        beat += len;
    }
    out
}

/// The beat reached `seconds` in, on a tempo map.
fn beat_at_seconds(bpm: f64, tempo: &[TempoEvent], seconds: f64) -> f64 {
    let (mut lo, mut hi) = (0.0f64, seconds * 400.0 / 60.0 + 1.0);
    for _ in 0..64 {
        let mid = (lo + hi) * 0.5;
        if tempo_seconds(bpm, tempo, mid) < seconds { lo = mid } else { hi = mid }
    }
    lo
}

/// The tempo a score plays at on a transport (its range).
pub fn transport_bpm(doc: &ScoreDoc) -> f64 {
    doc.clock.bpm.clamp(40.0, 240.0)
}

fn fail(message: impl Into<String>) -> Vec<Diag> {
    vec![Diag { line: 1, col: 1, node: None, message: message.into() }]
}

/// Register the samples and singers `doc` plays (its dependencies) for the
/// graphs prepared after this; a sample `samples` does not have stays
/// silent, a singer it does not have is sung by the built-in alto.
pub fn register_samples(doc: &ScoreDoc, samples: &dyn Samples) {
    for asset in doc.samples() {
        if let Some(sample) = samples.sample(&asset) {
            makepad_audio_engine::feed::samples::register(&sample_name(&asset), sample);
        }
    }
    for asset in doc.singers() {
        if let Some(profile) = samples.singer(&asset) {
            register_singer(&asset, profile);
        }
    }
    for asset in doc.files() {
        if let Some(bytes) = samples.file(&asset) {
            register_file(&asset, bytes);
        }
    }
}

/// The files `doc`'s native instruments load that nobody registered yet.
pub fn missing_files(doc: &ScoreDoc) -> Vec<crate::model::AssetRef> {
    doc.files().into_iter().filter(|asset| makepad_audio_engine::feed::files::get(&sample_name(asset)).is_none()).collect()
}

/// Register one library file (a SoundFont) under its library name.
pub fn register_file(asset: &crate::model::AssetRef, bytes: Arc<[u8]>) {
    makepad_audio_engine::feed::files::register(&sample_name(asset), bytes);
}

/// The samples `doc` plays that nobody registered yet.
pub fn missing_samples(doc: &ScoreDoc) -> Vec<crate::model::AssetRef> {
    doc.samples().into_iter().filter(|asset| makepad_audio_engine::feed::samples::get(&sample_name(asset)).is_none()).collect()
}

/// The library singers `doc` sings with that nobody registered yet.
pub fn missing_singers(doc: &ScoreDoc) -> Vec<crate::model::AssetRef> {
    doc.singers().into_iter().filter(|asset| makepad_audio_engine::feed::voices::get(&sample_name(asset)).is_none()).collect()
}

/// Register one singer profile under its library name.
pub fn register_singer(asset: &crate::model::AssetRef, profile: Arc<makepad_score_player::sung::VoiceProfile>) {
    makepad_audio_engine::feed::voices::register(&sample_name(asset), profile);
}

/// Register one decoded sample under its library name.
pub fn register_sample(asset: &crate::model::AssetRef, sample: Arc<makepad_score_player::Sample>) {
    makepad_audio_engine::feed::samples::register(&sample_name(asset), sample);
}

/// Prepare `doc` as a player of its own (a fresh name) at `rate`.
pub fn instantiate(doc: &ScoreDoc, rate: f32) -> Result<Player, Vec<Diag>> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let name = format!("score_{}", NEXT.fetch_add(1, Ordering::Relaxed));
    crate::player::prepare(doc, rate, &name, ClockMode::Own, None, 1)
}

pub fn render(source: &str, opts: &RenderOpts, samples: &dyn Samples) -> Result<Rendered, Vec<Diag>> {
    render_with(source, opts, samples, &mut |_, _| {})
}

/// Render, calling `observe(frame, instance)` after every pass (the frame
/// is the pass's end): how per-node levels are measured offline.
pub fn render_with(source: &str, opts: &RenderOpts, samples: &dyn Samples, observe: &mut dyn FnMut(usize, &Player)) -> Result<Rendered, Vec<Diag>> {
    let mut loaded = model::load(source)?;
    register_samples(&loaded.doc, samples);
    // The General MIDI SoundFont, from where the installer put it: a score
    // that plays it renders with it or not at all (never silently).
    if loaded.doc.plays_gm_font() {
        crate::gm_font::ensure().map_err(fail)?;
    }
    if let Some(seed) = opts.seed {
        loaded.doc.clock.seed = seed;
    }
    let rate = opts.rate;
    let mut instance = instantiate(&loaded.doc, rate)?;
    // Each held value's targets, in time order, applied at its pass.
    let mut held: Vec<(usize, Vec<crate::held::Target>, f32)> = opts.held.iter()
        .map(|h| ((h.at.max(0.0) * rate as f64) as usize, crate::held::targets(&loaded.doc, &h.node, &h.field), h.value))
        .collect();
    held.sort_by_key(|(at, ..)| std::cmp::Reverse(*at));
    let bpm = transport_bpm(&loaded.doc);
    let beats_per_bar = loaded.doc.beats_per_bar();
    let seconds = opts.seconds.unwrap_or_else(|| natural_length(&loaded.doc)).clamp(0.0, 3600.0);
    let mut composer = Composer::new(loaded);
    composer.wait_for_phrases = true;
    for (name, value) in &opts.params {
        composer.set_param(name, *value);
    }
    if let Some(start) = opts.start {
        composer.seek(start, 0.0, 0.0);
    }
    let total = (seconds * rate as f64).round() as usize;
    let mut left = vec![0.0f32; total];
    let mut right = vec![0.0f32; total];
    let mut events = Vec::new();
    let mut played = Vec::new();
    let clock = tempo_feed(&crate::player::transport(&instance.name));
    // The most beats a pass can hold (the fastest tempo), so every event of
    // the next pass is queued before it runs.
    let reach = CHUNK as f64 * 400.0 / 60.0 / rate as f64;
    let mut frame = 0usize;
    let mut beat = 0.0f64;
    while frame < total {
        let n = CHUNK.min(total - frame);
        composer.pump(beat + reach + 1e-9, &mut events);
        // A render sends no OSC.
        composer.osc_out.clear();
        for change in composer.tempo_out.drain(..) {
            clock.send(change);
        }
        for (fx, event) in composer.automation_out.drain(..) {
            makepad_audio_engine::feed::automation_feed(&format!("{}/{fx}", instance.name)).send(event);
        }
        played.extend(composer.history.drain(..));
        for event in events.drain(..) {
            if let Some((_, feed)) = instance.feeds.iter().find(|(key, _)| *key == event.loop_key) {
                if !feed.send(event) {
                    forget_instance(&instance.name);
                    return Err(fail("too many events at once: a loop plays more than 4000 notes in a pass"));
                }
            }
        }
        while held.last().is_some_and(|(at, ..)| *at < frame + n) {
            let Some((_, targets, value)) = held.pop() else { break };
            crate::player::hold(&instance, &targets, value);
        }
        let (l, r) = (&mut left[frame..frame + n], &mut right[frame..frame + n]);
        instance.run(n, &mut |cue, a, b| {
            if !cue {
                l.copy_from_slice(&a[..n]);
                r.copy_from_slice(&b[..n]);
            }
        });
        beat = instance.schedule.clocks.first().map_or(beat, |c| c.tempo.end_beat(n));
        frame += n;
        observe(frame, &instance);
    }
    forget_instance(&instance.name);
    let tempo = composer.tempo_map.clone();
    let sections = sections_of(&mut composer, beat_at_seconds(bpm, &tempo, seconds), beats_per_bar);
    let errors = composer.errors();
    Ok(Rendered { left, right, rate, bpm, beats_per_bar, played, sections, tempo, errors })
}

/// Where each section starts (beats), bar by bar up to `end` beats.
fn sections_of(composer: &mut Composer, end: f64, beats_per_bar: f64) -> Vec<(f64, String)> {
    let mut sections = Vec::new();
    let mut beat = 0.0;
    while beat < end && !composer.doc.arrange.is_empty() {
        let Some(name) = composer.section_at(beat + 1e-9) else { break };
        if sections.last().is_none_or(|(_, last): &(f64, String)| *last != name) {
            sections.push((beat, name));
        }
        beat += beats_per_bar;
    }
    sections
}

/// A score's timing without rendering it: what a timeline draws (beats,
/// bars, section bands) for a render of `seconds` (None: its natural
/// length).
#[derive(Clone, Debug, PartialEq)]
pub struct Timing {
    pub bpm: f64,
    pub beats_per_bar: f64,
    pub seconds: f64,
    /// (name, start, end) in seconds.
    pub sections: Vec<(String, f64, f64)>,
    /// The tempo changes from `bpm` (sections' `bpm`): `seconds_at` places
    /// every beat and bar mark.
    pub tempo: Vec<TempoEvent>,
}

impl Timing {
    /// Seconds from the start to `beat`.
    pub fn seconds_at(&self, beat: f64) -> f64 {
        tempo_seconds(self.bpm, &self.tempo, beat)
    }
}

/// Each loop's notes in its first cycle that plays (the arrangement's
/// sections gating it), computed without playing: (loop, cycle start,
/// cycle length, notes as (offset beat, note, velocity, hit), lengths).
/// What a stopped score shows of its generated loops.
pub fn cycle_notes(source: &str) -> Vec<(String, f64, f64, Vec<(f64, f32, f32, bool)>, Vec<f32>)> {
    let Ok(loaded) = model::load(source) else { return Vec::new() };
    let bar = loaded.doc.beats_per_bar();
    let end = loaded.doc.arrangement_beats().max(16.0 * bar).min(512.0);
    let mut composer = Composer::new(loaded);
    let mut out: Vec<(String, f64, f64, Vec<(f64, f32, f32, bool)>, Vec<f32>)> = Vec::new();
    let mut events = Vec::new();
    let mut until = 0.0;
    while until < end && out.len() < composer.loops.len() {
        until += bar;
        composer.pump(until, &mut events);
        events.clear();
        for state in &composer.loops {
            if !state.resting && state.error.is_none() && !state.last_notes.is_empty() && !out.iter().any(|(n, ..)| *n == state.name) {
                out.push((state.name.clone(), state.last_start, state.last_len, state.last_notes.clone(), state.last_durs.clone()));
            }
        }
    }
    out
}

/// The first error loop `node`'s code meets over the arrangement (1024
/// beats at most), if any.
pub fn loop_error(source: &str, node: &str) -> Option<String> {
    let loaded = model::load(source).ok()?;
    let bar = loaded.doc.beats_per_bar();
    let end = loaded.doc.arrangement_beats().max(16.0 * bar).min(1024.0);
    let mut composer = Composer::new(loaded);
    let index = composer.loops.iter().position(|l| l.name == node)?;
    let mut events = Vec::new();
    let mut until = 0.0;
    while until < end {
        until += bar;
        composer.pump(until, &mut events);
        events.clear();
        if let Some(error) = &composer.loops[index].error {
            return Some(error.message.clone());
        }
    }
    None
}

/// A loop's phrase ([`phrase_notes`]).
pub struct Phrase {
    /// The phrase's length in beats, and its cycles' length.
    pub beats: f64,
    pub cycle: f64,
    /// The number of its first cycle (what the loop's code is given).
    pub first: u32,
    /// It comes round again (it was found repeating within what was read).
    pub repeats: bool,
    /// As [`cycle_notes`] gives them, beats from the phrase's start.
    pub notes: Vec<(f64, f32, f32, bool)>,
    pub durs: Vec<f32>,
}

/// A loop's phrase: its cycles from the first it plays, one after another,
/// until they come round to the first again (a loop playing the same cycle
/// each time: that cycle; one walking a melody bar by bar: the melody), at
/// most `max_beats`.
pub fn phrase_notes(source: &str, node: &str, max_beats: f64) -> Option<Phrase> {
    let loaded = model::load(source).ok()?;
    let bar = loaded.doc.beats_per_bar();
    let step = loaded.doc.loop_def(node).and_then(|l| l.beats).unwrap_or(bar).min(bar).max(0.25);
    let end = loaded.doc.arrangement_beats().max(16.0 * bar).min(1024.0);
    let mut composer = Composer::new(loaded);
    let index = composer.loops.iter().position(|l| l.name == node)?;
    // Its cycles one after another from the first it plays, up to
    // `max_beats` (a rest or a gap ends them). A cycle is taken whole once
    // the next begins (a written list streams its notes as it goes).
    type Cycle = (f64, f64, Vec<(f64, f32, f32, bool)>, Vec<f32>);
    let mut cycles: Vec<Cycle> = Vec::new();
    let mut current: Option<Cycle> = None;
    let mut events = Vec::new();
    let mut until = 0.0;
    let mut ended = false;
    while until < end + step && !ended {
        until += step;
        composer.pump(until, &mut events);
        events.clear();
        let state = &composer.loops[index];
        if state.error.is_some() {
            break;
        }
        // Playing: from its first cycle with notes, every cycle it does not rest.
        let playing = !state.resting && (current.is_some() || !cycles.is_empty() || !state.last_notes.is_empty());
        let next_began = current.as_ref().is_some_and(|c| c.0 != state.last_start);
        if next_began || (current.is_some() && until >= end) {
            let done = current.take().expect("checked");
            if let Some(last) = cycles.last() {
                if (done.0 - (last.0 + last.1)).abs() > 1e-6 {
                    ended = true;
                }
            }
            if !ended && cycles.iter().map(|c| c.1).sum::<f64>() + done.1 <= max_beats + 1e-9 {
                cycles.push(done);
            } else {
                ended = true;
            }
        }
        if playing && !ended {
            match current.as_mut() {
                Some(c) if c.0 == state.last_start => {
                    c.2 = state.last_notes.clone();
                    c.3 = state.last_durs.clone();
                }
                _ => current = Some((state.last_start, state.last_len, state.last_notes.clone(), state.last_durs.clone())),
            }
        } else if !cycles.is_empty() && current.is_none() {
            ended = true;
        }
    }
    // The phrase: the fewest cycles that, repeated, make all of them.
    let same = |a: &(f64, f64, Vec<(f64, f32, f32, bool)>, Vec<f32>), b: &(f64, f64, Vec<(f64, f32, f32, bool)>, Vec<f32>)| a.1 == b.1 && a.2 == b.2 && a.3 == b.3;
    let period = (1..=cycles.len()).find(|p| (0..cycles.len()).all(|i| same(&cycles[i], &cycles[i % p]))).unwrap_or(cycles.len());
    let repeats = period < cycles.len();
    cycles.truncate(period);
    let first = cycles.first()?.0;
    let cycle = cycles[0].1.max(0.25);
    let mut notes = Vec::new();
    let mut durs = Vec::new();
    for (start, _, n, d) in &cycles {
        notes.extend(n.iter().map(|(b, note, v, h)| (b + start - first, *note, *v, *h)));
        durs.extend(d.iter().copied());
    }
    Some(Phrase { beats: cycles.iter().map(|c| c.1).sum(), cycle, first: (first / cycle).round().max(0.0) as u32, repeats, notes, durs })
}

pub fn timing(source: &str, seconds: Option<f64>) -> Result<Timing, Vec<Diag>> {
    let loaded = model::load(source)?;
    let bpm = transport_bpm(&loaded.doc);
    let beats_per_bar = loaded.doc.beats_per_bar();
    let seconds = seconds.unwrap_or_else(|| natural_length(&loaded.doc)).clamp(0.0, 3600.0);
    let mut composer = Composer::new(loaded);
    // The sections' tempo changes up to the end make the tempo map.
    let mut ignored = Vec::new();
    let end = {
        let mut end = seconds * 400.0 / 60.0 + 1.0;
        for _ in 0..4 {
            composer.pump(end, &mut ignored);
            ignored.clear();
            end = beat_at_seconds(bpm, &composer.tempo_map, seconds) + 1.0;
        }
        end - 1.0
    };
    let tempo = composer.tempo_map.clone();
    let starts = sections_of(&mut composer, end, beats_per_bar);
    let at = |beat: f64| tempo_seconds(bpm, &tempo, beat);
    let sections = starts.iter().enumerate().map(|(i, (beat, name))| {
        let stop = starts.get(i + 1).map_or(seconds, |(next, _)| at(*next));
        (name.clone(), at(*beat), stop)
    }).collect();
    Ok(Timing { bpm, beats_per_bar, seconds, sections, tempo })
}

