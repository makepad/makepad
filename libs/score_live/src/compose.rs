//! The composer: runs each loop's body once per cycle, ahead of the clock,
//! turning what it plays into beat-stamped events. Cycle `k` of a loop
//! draws its randomness from `(seed, loop, k)` alone, so what a cycle plays
//! does not depend on when it was evaluated. A re-evaluation replaces a
//! loop's body from its next cycle on: the cycle playing now finishes as it
//! was written.

use crate::key;
use crate::model::{Diag, InstKind, ListKind, Loaded, ScoreDoc, Sync};
use crate::vm::{Ctx, EmitKind, Evaluated};
use makepad_score_player::{Event, EventKind};
use std::collections::VecDeque;

/// Something a loop played, for the editor's lights and the AI's ears.
#[derive(Clone, Debug, PartialEq)]
pub struct Played {
    pub beat: f64,
    pub loop_name: String,
    /// MIDI note, or the drum slot for a hit.
    pub note: f32,
    pub vel: f32,
    pub dur: f32,
    pub hit: bool,
}

/// A loop's place in time and its memory between cycles.
#[derive(Clone, Debug)]
pub struct LoopState {
    pub name: String,
    pub key: u64,
    /// Where its next cycle starts.
    pub next: f64,
    pub cycle: u64,
    ticks: Vec<(String, i64)>,
    /// The last cycle generated: start, length, and what it played
    /// (offsets from its start).
    pub last_start: f64,
    pub last_len: f64,
    pub last_notes: Vec<(f64, f32, f32, bool)>,
    /// Each of `last_notes`' lengths in beats (0: the patch's own gate).
    pub last_durs: Vec<f32>,
    /// The syllables it sang (beats from its start).
    pub last_lyrics: Vec<makepad_score_player::sung::SyllableSpan>,
    /// What its code logged in its last cycle.
    pub last_logs: Vec<String>,
    /// Whether the last cycle was silent because of the section or `when`.
    pub resting: bool,
    pub error: Option<Diag>,
    /// A note-list loop's place in its cycle: the next entry, and whether
    /// the cycle (started at `last_start`, `last_len` long) is under way.
    stream: usize,
    streaming: bool,
    /// Its code cues: at the same beat it runs before loops that wait.
    cues_first: bool,
}

/// Which section plays from which beat, and its place in the arrangement
/// (None: reached by a goto or a `next` outside it), so a section the
/// arrangement repeats (`arrange: [verse verse chorus]`) plays on to what
/// follows that place.
#[derive(Clone, Debug, Default)]
struct Timeline {
    entries: Vec<(f64, usize, Option<usize>)>,
}

pub struct Composer {
    pub doc: ScoreDoc,
    vm: Evaluated,
    pub loops: Vec<LoopState>,
    timeline: Timeline,
    pub params: Vec<(String, f32)>,
    stingers: Vec<(String, f64)>,
    pub history: VecDeque<Played>,
    pub cues: VecDeque<(String, f64)>,
    /// Each loop's cues in its last cycle, for `wait`.
    cue_plans: Vec<crate::vm::CuePlan>,
    /// MIDI in: the keys held (note, channel 0..15, velocity) and the last
    /// value of each controller (0..1).
    pub held: Vec<(u8, u8, f32)>,
    pub ccs: Vec<f32>,
    /// OSC: the last arguments per address that arrived, and the packets
    /// loop code sends as (beat, "host:port", packet), for the host to send
    /// when the beat comes (a render drops them).
    pub osc_values: Vec<(String, Vec<crate::osc::OscArg>)>,
    pub osc_out: Vec<(f64, String, Vec<u8>)>,
    /// Where the arrangement is: (transport beat, song beat) from each
    /// jump on (NaN song: off the arrangement, after a goto), a seek
    /// waiting for its beat (transport, song), the loop range (song beats)
    /// and the next jump's beat, where cycles are cut.
    anchors: Vec<(f64, f64)>,
    seek_at: Option<(f64, f64)>,
    pub loop_range: Option<(f64, f64)>,
    cut: Option<f64>,
    /// Tempo changes for the transport, in beat order (the host drains
    /// them into its transport's tempo feed), and every one made so far
    /// (the tempo map: `tempo_seconds`).
    pub tempo_out: Vec<makepad_audio_engine::feed::TempoEvent>,
    /// What loops shared with `put` (key, beat, value), oldest first.
    shared: Vec<(String, f64, crate::vm::Stored)>,
    /// FX automation for the host to route: (FX node, event).
    pub automation_out: Vec<(String, makepad_audio_engine::feed::AutomationEvent)>,
    /// The section starts already looked at for their `set`.
    set_sections: usize,
    pub tempo_map: Vec<makepad_audio_engine::feed::TempoEvent>,
    /// How far the composer has scheduled; the section starts already
    /// looked at for their tempo.
    horizon: f64,
    tempo_sections: usize,
    /// In the section at `tempo_sections`: 0 before its `bpm`, then 1 +
    /// the tempo points sent.
    tempo_point: usize,
    /// An offline render: a neural phrase renders before the composer goes
    /// on (a render runs faster than the singer could keep up with), not on
    /// the singer's thread.
    pub wait_for_phrases: bool,
}

/// Most note-list entries sent in one go (the rest follow as the clock
/// reaches them).
const STREAM_BATCH: usize = 512;
/// How far ahead (seconds) a loop on a neural voice is composed: the
/// singer renders a 3 s phrase in about 0.15 s on an M4, so a second covers
/// several phrases changing at once.
pub const NEURAL_AHEAD: f64 = 1.0;

/// Recent events kept for the editor and the AI.
pub const HISTORY: usize = 2048;

fn cycle_seed(seed: u64, key: u64, cycle: u64) -> u64 {
    let mut h = seed ^ key.rotate_left(21) ^ cycle.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 31;
    h = h.wrapping_mul(0xd6e8_feb8_6659_fd93);
    h ^ (h >> 32)
}

impl Composer {
    pub fn new(loaded: Loaded) -> Composer {
        let Loaded { doc, vm } = loaded;
        let params = doc.clock.params.clone();
        let loops = doc.loops.iter().filter(|l| l.stinger.is_none()).map(|l| fresh(&l.name, 0.0)).collect();
        let mut composer = Composer {
            doc, vm, loops, timeline: Timeline::default(), params, stingers: Vec::new(), history: VecDeque::new(), cues: VecDeque::new(), cue_plans: Vec::new(), held: Vec::new(), ccs: vec![0.0; 128], osc_values: Vec::new(), osc_out: Vec::new(), anchors: vec![(0.0, 0.0)], seek_at: None, loop_range: None, cut: None,
            tempo_out: Vec::new(), tempo_map: Vec::new(), horizon: 0.0, tempo_sections: 0, tempo_point: 0,
            automation_out: Vec::new(), set_sections: 0, shared: Vec::new(), wait_for_phrases: false,
        };
        composer.reset_timeline(0.0);
        composer.mark_cuers();
        // Every FX setting that follows a param starts where the param is.
        let params = composer.params.clone();
        for (name, value) in params {
            composer.automate(&name, value, 0.0, false);
        }
        composer
    }

    /// Replace the document at `now` (the beat playing). Loops that stay
    /// keep their place, cycle count and ticks; new loops start on their
    /// sync point; params keep their current values.
    pub fn reload(&mut self, loaded: Loaded, now: f64) {
        let Loaded { doc, vm } = loaded;
        let bar = doc.beats_per_bar();
        let mut loops = Vec::new();
        for l in doc.loops.iter().filter(|l| l.stinger.is_none()) {
            if let Some(state) = self.loops.iter().find(|s| s.name == l.name) {
                let mut state = state.clone();
                state.error = None;
                loops.push(state);
                continue;
            }
            let start = match &l.sync {
                _ if now <= 0.0 => 0.0,
                Sync::Now => now.ceil(),
                Sync::Bar => (now / bar).ceil() * bar,
                Sync::Loop(other) => self.loops.iter().find(|s| &s.name == other).map_or((now / bar).ceil() * bar, |s| s.next),
            };
            loops.push(fresh(&l.name, start));
        }
        let mut params = doc.clock.params.clone();
        for (name, value) in &mut params {
            if let Some((_, current)) = self.params.iter().find(|(n, _)| n == name) {
                *value = *current;
            }
        }
        let arrangement_changed = doc.arrange != self.doc.arrange || doc.sections != self.doc.sections;
        self.doc = doc;
        self.vm = vm;
        self.loops = loops;
        self.mark_cuers();
        self.params = params;
        if arrangement_changed {
            self.reset_timeline(now);
        }
    }

    fn reset_timeline(&mut self, now: f64) {
        // Keep what already played, then follow the arrangement from the
        // section playing now.
        let playing = self.section_index_at(now);
        self.timeline.entries.retain(|(start, ..)| *start <= now);
        self.clip_tempo_sections(self.timeline.entries.len());
        if self.timeline.entries.is_empty() {
            if let Some(first) = self.doc.arrange.first().and_then(|name| self.doc.sections.iter().position(|s| &s.name == name)) {
                self.timeline.entries.push((0.0, first, Some(0)));
            }
        } else if let Some(index) = playing {
            if index >= self.doc.sections.len() {
                self.timeline.entries.clear();
                self.reset_timeline(0.0);
            }
        }
    }

    /// The timeline was cut to `len` entries: sections from there on are
    /// looked at again for their tempo.
    fn clip_tempo_sections(&mut self, len: usize) {
        self.set_sections = self.set_sections.min(len);
        if self.tempo_sections > len {
            self.tempo_sections = len;
            self.tempo_point = 0;
        }
    }

    /// The arrangement's first place for a section.
    fn place_of(&self, index: usize) -> Option<usize> {
        let name = &self.doc.sections.get(index)?.name;
        self.doc.arrange.iter().position(|n| n == name)
    }

    /// The section after `index` at arrangement place `place` (or its
    /// `next`), with its place.
    fn next_section(&self, index: usize, place: Option<usize>) -> Option<(usize, Option<usize>)> {
        let section = self.doc.sections.get(index)?;
        if let Some(next) = &section.next {
            if next == "stop" {
                return None;
            }
            let next = self.doc.sections.iter().position(|s| &s.name == next)?;
            return Some((next, self.place_of(next)));
        }
        // A section outside the arrangement (reached by a goto) repeats
        // until the next goto.
        let Some(at) = place.filter(|p| self.doc.arrange.get(*p) == Some(&section.name)).or_else(|| self.place_of(index)) else { return Some((index, None)) };
        match self.doc.arrange.get(at + 1) {
            Some(name) => self.doc.sections.iter().position(|s| &s.name == name).map(|next| (next, Some(at + 1))),
            // The last section repeats: a live score never falls silent.
            None => Some((index, Some(at))),
        }
    }

    fn section_index_at(&mut self, beat: f64) -> Option<usize> {
        let bar = self.doc.beats_per_bar();
        loop {
            let &(start, index, place) = self.timeline.entries.last()?;
            // After a `next: @stop` the timeline has ended; earlier beats
            // still have their sections.
            let Some(section) = self.doc.sections.get(index) else { break };
            let end = start + section.bars * bar;
            if beat < end {
                break;
            }
            match self.next_section(index, place) {
                Some((next, place)) => self.timeline.entries.push((end, next, place)),
                None => {
                    self.timeline.entries.push((end, usize::MAX, None));
                    break;
                }
            }
            if self.timeline.entries.len() > 100_000 {
                break;
            }
        }
        self.timeline.entries.iter().rev().find(|(start, ..)| *start <= beat).map(|(_, index, _)| *index)
    }

    /// The section playing at `beat`, by name.
    pub fn section_at(&mut self, beat: f64) -> Option<String> {
        let index = self.section_index_at(beat)?;
        self.doc.sections.get(index).map(|s| s.name.clone())
    }

    /// Jump to a section at the next `quant` boundary (beats; 0 is now,
    /// negative is the next bar) after `now`. The beat it starts.
    pub fn goto_section(&mut self, name: &str, now: f64, quant: f64) -> Result<f64, String> {
        let index = self.doc.sections.iter().position(|s| s.name == name).ok_or_else(|| format!("no section `{name}`"))?;
        let at = quantize(now, if quant < 0.0 { self.doc.beats_per_bar() } else { quant });
        // Where it goes in the arrangement, if it is in it.
        let song = self.song_entries(f64::INFINITY).iter().find(|(_, i)| *i == index).map_or(f64::NAN, |(s, _)| *s);
        self.anchor(at, song);
        self.section_index_at(at);
        self.timeline.entries.retain(|(start, ..)| *start < at);
        let place = self.place_of(index);
        self.timeline.entries.push((at, index, place));
        self.clip_tempo_sections(self.timeline.entries.len() - 1);
        Ok(at)
    }

    /// The arrangement from its start, as it plays with no gotos: (song
    /// beat, section) up to `until` (or its end).
    pub fn song_entries(&self, until: f64) -> Vec<(f64, usize)> {
        self.song_places(until).into_iter().map(|(start, index, _)| (start, index)).collect()
    }

    /// [`Composer::song_entries`] with each section's arrangement place.
    fn song_places(&self, until: f64) -> Vec<(f64, usize, Option<usize>)> {
        let bar = self.doc.beats_per_bar();
        let mut out: Vec<(f64, usize, Option<usize>)> = Vec::new();
        let Some(mut index) = self.doc.arrange.first().and_then(|name| self.doc.sections.iter().position(|s| &s.name == name)) else { return out };
        let mut place = Some(0);
        let mut start = 0.0;
        // Each place once: the last section repeating (or a `next` back
        // to one played) is where the arrangement's run ends.
        while start <= until && !out.iter().any(|(_, i, p)| *i == index && *p == place) {
            out.push((start, index, place));
            let Some(section) = self.doc.sections.get(index) else { break };
            start += section.bars * bar;
            match self.next_section(index, place) {
                Some(next) => (index, place) = next,
                None => break,
            }
        }
        out
    }

    fn anchor(&mut self, at: f64, song: f64) {
        self.anchors.retain(|(t, _)| *t < at);
        self.anchors.push((at, song));
        if self.anchors.len() > 16 {
            self.anchors.remove(0);
        }
    }

    /// Where the arrangement is at transport `beat`, in beats from its
    /// start (None off it: after a goto out of it, or past its end).
    pub fn song_beat(&self, beat: f64) -> Option<f64> {
        let &(t, song) = self.anchors.iter().rev().find(|(t, _)| *t <= beat).or(self.anchors.first())?;
        if !song.is_finite() {
            return None;
        }
        let at = song + (beat - t);
        let end = self.doc.arrangement_beats();
        (at >= 0.0 && (end <= 0.0 || at < end)).then_some(at)
    }

    /// Play the arrangement from `song` beats (whole bars) at the next
    /// `quant` after `now` (<0: the next bar): the section there from its
    /// offset, its loops, params and tempo as if reached. The seek happens
    /// as the composer gets there.
    pub fn seek(&mut self, song: f64, now: f64, quant: f64) -> f64 {
        let at = quantize(now, if quant < 0.0 { self.doc.beats_per_bar() } else { quant });
        self.seek_at = Some((at, song.max(0.0)));
        at
    }

    /// Repeat the arrangement's song beats [from, to) (None: play on).
    pub fn set_loop(&mut self, range: Option<(f64, f64)>) {
        self.loop_range = range.filter(|(from, to)| to > from);
    }

    /// The next jump: a seek waiting, or the loop's end as the playhead
    /// reaches it from inside (a loop elsewhere catches it when it enters).
    fn next_jump(&self) -> Option<(f64, f64)> {
        let looped = self.loop_range.and_then(|(from, to)| {
            let now = self.horizon;
            let song = self.song_beat(now)?;
            (song < to).then(|| {
                let &(t, s) = self.anchors.iter().rev().find(|(t, _)| *t <= now).or(self.anchors.last())?;
                Some((t + (to - s), from))
            }).flatten()
        });
        match (self.seek_at, looped) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
            (a, b) => a.or(b),
        }
    }

    /// Jump to song beat `song` at transport beat `at`.
    fn jump(&mut self, at: f64, song: f64) {
        let entries = self.song_places(song);
        let Some(&(start, index, place)) = entries.iter().rev().find(|(s, ..)| *s <= song) else {
            // No arrangement: every loop goes on as if all had started at
            // `at - song`, mid-cycle (what its cycle plays before `at` is
            // dropped as it comes).
            for state in &mut self.loops {
                let len = self.doc.loops.iter().find(|l| l.name == state.name).and_then(|l| l.beats).unwrap_or(state.last_len);
                state.next = if len > 0.0 { at - song.rem_euclid(len) } else { at };
                state.streaming = false;
            }
            self.anchor(at, song);
            return;
        };
        let begin = at - (song - start);
        self.section_index_at(at);
        self.timeline.entries.retain(|(s, ..)| *s < begin);
        self.timeline.entries.push((begin, index, place));
        self.clip_tempo_sections(self.timeline.entries.len() - 1);
        for state in &mut self.loops {
            state.next = at;
            state.streaming = false;
        }
        self.anchor(at, song);
    }

    /// Queue a stinger to play at the next quantum after `now`.
    pub fn trigger(&mut self, name: &str, now: f64) -> Result<f64, String> {
        let l = self.doc.loops.iter().find(|l| l.name == name && l.stinger.is_some()).ok_or_else(|| format!("no stinger `{name}`"))?;
        let quant = l.stinger.unwrap_or(1.0);
        let at = quantize(now, if quant < 0.0 { self.doc.beats_per_bar() } else { quant });
        self.stingers.push((name.to_string(), at));
        Ok(at)
    }

    /// A MIDI message from the input at `beat` (the beat playing): keys
    /// held and controllers for held() and cc(), params the Clock maps to
    /// controllers (moving at once), and the notes of loops a keyboard
    /// plays (`midi_in`), for their feeds now.
    pub fn midi_in(&mut self, data: [u8; 3], beat: f64, out: &mut Vec<Event>) {
        let (status, a, b) = (data[0] & 0xf0, data[1] & 127, data[2] & 127);
        let channel = data[0] & 15;
        let on = status == 0x90 && b > 0;
        let off = status == 0x80 || (status == 0x90 && b == 0);
        if on {
            self.held.retain(|(n, c, _)| !(*n == a && *c == channel));
            self.held.push((a, channel, b as f32 / 127.0));
        } else if off {
            self.held.retain(|(n, c, _)| !(*n == a && *c == channel));
        } else if status == 0xb0 {
            let value = b as f32 / 127.0;
            self.ccs[a as usize] = value;
            let mapped: Vec<String> = self.doc.clock.midi.iter().filter(|(_, cc)| *cc == a).map(|(p, _)| p.clone()).collect();
            for param in mapped {
                if let Some(slot) = self.params.iter_mut().find(|(n, _)| *n == param) {
                    slot.1 = value;
                }
                self.automate(&param, value, beat, false);
                if let Some(t) = self.doc.clock.tempo.clone().filter(|t| t.param == param) {
                    self.set_tempo(t.from + (t.to - t.from) * value as f64, t.ramp);
                }
            }
            return;
        } else {
            return;
        }
        // A key's handle: unique per channel and note.
        let handle = 0x8000_0000 | ((channel as u32) << 7) | a as u32;
        for l in self.doc.loops.iter().filter(|l| l.midi_in.is_some_and(|c| c == 0 || c - 1 == channel)) {
            let opts = makepad_score_player::NoteOpts { handle, ..Default::default() };
            let (kind, vel, dur) = if on { (EventKind::Note, b as f32 / 127.0 * l.amp.max(0.0), 64.0) } else { (EventKind::Release, 0.0, 0.0) };
            out.push(Event { beat, loop_key: key(&l.name), kind, note: a as f32, vel, pan: l.pan, dur, opts });
        }
    }

    /// An OSC message that arrived at `beat`: its arguments for osc() in
    /// code; `/param/<name> value` (or `/<name>` naming a param) moves the
    /// param, and the FX following it, at once.
    pub fn osc_in(&mut self, addr: &str, args: Vec<crate::osc::OscArg>, beat: f64) {
        let value = args.iter().find_map(|a| a.num());
        match self.osc_values.iter().position(|(a, _)| a == addr) {
            Some(i) => self.osc_values[i].1 = args,
            None if self.osc_values.len() < 1024 => self.osc_values.push((addr.to_string(), args)),
            None => {}
        }
        let name = addr.strip_prefix("/param/").unwrap_or(&addr[1.min(addr.len())..]);
        if let (Some(value), true) = (value, self.params.iter().any(|(n, _)| n == name)) {
            let name = name.to_string();
            if let Some(slot) = self.params.iter_mut().find(|(n, _)| *n == name) {
                slot.1 = value;
            }
            self.automate(&name, value, beat, false);
            if let Some(t) = self.doc.clock.tempo.clone().filter(|t| t.param == name) {
                self.set_tempo(t.from + (t.to - t.from) * value.clamp(0.0, 1.0) as f64, t.ramp);
            }
        }
    }

    pub fn set_param(&mut self, name: &str, value: f32) -> Option<usize> {
        let index = self.params.iter().position(|(n, _)| n == name)?;
        let changed = self.params[index].1 != value;
        self.params[index].1 = value;
        if changed {
            let beat = self.horizon.max(0.0).ceil();
            self.automate(name, value, beat, true);
        }
        // A tempo that follows this param changes on the next beat not yet
        // scheduled.
        if let Some(t) = self.doc.clock.tempo.clone().filter(|t| t.param == name && changed) {
            let bpm = t.from + (t.to - t.from) * value.clamp(0.0, 1.0) as f64;
            self.set_tempo(bpm, t.ramp);
        }
        Some(index)
    }

    /// Move the FX settings that follow param `name` to `value` from
    /// `beat` (over their ramp, or at once).
    fn automate(&mut self, name: &str, value: f32, beat: f64, ramped: bool) {
        for fx in &self.doc.fx {
            for (i, t) in fx.follow.iter().filter(|(_, t)| t.param == name) {
                let v = t.from + (t.to - t.from) * value.clamp(0.0, 1.0) as f64;
                let event = makepad_audio_engine::feed::AutomationEvent { beat, param: *i as u8, value: v as f32, ramp: if ramped { t.ramp as f32 } else { 0.0 } };
                self.automation_out.push((fx.name.clone(), event));
            }
        }
    }

    /// The params the sections set as they start, before `until`.
    fn section_sets(&mut self, until: f64) {
        if self.doc.sections.iter().all(|s| s.set.is_empty()) {
            return;
        }
        self.section_index_at(until);
        while self.set_sections < self.timeline.entries.len() {
            let (start, index, _) = self.timeline.entries[self.set_sections];
            if start >= until {
                break;
            }
            self.set_sections += 1;
            let set = self.doc.sections.get(index).map(|s| s.set.clone()).unwrap_or_default();
            for (name, value) in set {
                if let Some(slot) = self.params.iter_mut().find(|(n, _)| *n == name) {
                    slot.1 = value;
                }
                self.automate(&name, value, start, true);
            }
        }
    }

    /// Change the tempo from the next beat not yet scheduled, reached over
    /// `ramp` beats (a game's mood, a tempo command).
    pub fn set_tempo(&mut self, bpm: f64, ramp: f64) {
        let beat = self.horizon.max(0.0).ceil();
        self.push_tempo(makepad_audio_engine::feed::TempoEvent { beat, bpm: bpm.clamp(20.0, 400.0), ramp: ramp.max(0.0) });
    }

    fn push_tempo(&mut self, event: makepad_audio_engine::feed::TempoEvent) {
        self.tempo_out.push(event);
        let at = self.tempo_map.partition_point(|e| e.beat <= event.beat);
        self.tempo_map.insert(at, event);
    }

    /// Seconds from beat 0 to `beat` on this score's tempo map.
    pub fn seconds_at(&self, beat: f64) -> f64 {
        makepad_audio_engine::feed::tempo_seconds(crate::render::transport_bpm(&self.doc), &self.tempo_map, beat)
    }

    /// The tempo changes of the sections up to `until`: a section's `bpm`
    /// at its start, then its `tempo` points as the horizon reaches them
    /// (a performance's thousand points never crowd the transport's feed).
    fn section_tempos(&mut self, until: f64) {
        if self.doc.sections.iter().all(|s| s.bpm.is_none() && s.tempo.is_empty()) {
            return;
        }
        self.section_index_at(until);
        while self.tempo_sections < self.timeline.entries.len() {
            let (start, index, _) = self.timeline.entries[self.tempo_sections];
            if start >= until {
                break;
            }
            let Some(section) = self.doc.sections.get(index) else {
                self.tempo_sections += 1;
                self.tempo_point = 0;
                continue;
            };
            let (bpm, ramp, points) = (section.bpm, section.ramp, section.tempo.clone());
            if self.tempo_point == 0 {
                if let Some(bpm) = bpm {
                    self.push_tempo(makepad_audio_engine::feed::TempoEvent { beat: start, bpm, ramp });
                }
                self.tempo_point = 1;
            }
            // Points past where the next section took over (a goto) never
            // play.
            let end = self.timeline.entries.get(self.tempo_sections + 1).map_or(f64::INFINITY, |(next, ..)| *next);
            while let Some(&(at, bpm)) = points.get(self.tempo_point - 1) {
                let beat = start + at;
                if beat >= end {
                    break;
                }
                if beat >= until {
                    return;
                }
                self.push_tempo(makepad_audio_engine::feed::TempoEvent { beat, bpm, ramp: 0.0 });
                self.tempo_point += 1;
            }
            self.tempo_sections += 1;
            self.tempo_point = 0;
        }
    }

    /// Generate every cycle that starts before `until`, in beat order (a
    /// note list only up to `until`).
    /// Loops whose code cues run first among loops starting together.
    fn mark_cuers(&mut self) {
        for state in &mut self.loops {
            state.cues_first = self.doc.outline.node(&state.name).is_some_and(|n| n.body.text(&self.doc.source).contains("cue("));
        }
    }

    pub fn pump(&mut self, until: f64, out: &mut Vec<Event>) {
        // After a jump, what cycles started before it play is dropped.
        let mut skip: Vec<(usize, f64)> = Vec::new();
        // Seeks and loop ends up to `until`: compose to each, then jump.
        for _ in 0..64 {
            match self.next_jump() {
                Some((at, song)) if at < until => {
                    self.cut = Some(at);
                    self.pump_to(at, out);
                    if self.seek_at.is_some_and(|(t, _)| t == at) {
                        self.seek_at = None;
                    }
                    self.jump(at, song);
                    self.horizon = self.horizon.max(at);
                    skip.push((out.len(), at));
                }
                _ => break,
            }
        }
        self.cut = self.next_jump().map(|(at, _)| at);
        self.pump_to(until, out);
        for (mark, at) in skip.into_iter().rev() {
            let mut i = mark;
            while i < out.len() {
                if out[i].beat < at - 1e-9 { out.remove(i); } else { i += 1; }
            }
        }
    }

    fn pump_to(&mut self, until: f64, out: &mut Vec<Event>) {
        self.horizon = self.horizon.max(until);
        self.section_tempos(until);
        self.section_sets(until);
        loop {
            // A loop on a neural voice runs ahead, so its phrases render before they play.
            let next = self.loops.iter().enumerate().filter(|(_, s)| s.next < until + self.ahead(&s.name)).min_by(|a, b| a.1.next.total_cmp(&b.1.next).then(b.1.cues_first.cmp(&a.1.cues_first)).then(a.0.cmp(&b.0)));
            let Some((index, _)) = next else { break };
            self.cycle(index, out);
        }
        let mut due = Vec::new();
        self.stingers.retain(|(name, at)| {
            if *at < until {
                due.push((name.clone(), *at));
                false
            } else {
                true
            }
        });
        for (name, at) in due {
            if let Some(def) = self.doc.loops.iter().position(|l| l.name == name) {
                let mut state = fresh(&name, at);
                self.run(def, &mut state, at, out);
            }
        }
    }

    /// How far ahead of the others (beats) a loop is composed: a loop on a
    /// neural voice by [`NEURAL_AHEAD`] seconds, so the singer has its
    /// phrases rendered before they play.
    fn ahead(&self, name: &str) -> f64 {
        if self.plays_neural(name) { NEURAL_AHEAD * self.doc.clock.bpm / 60.0 } else { 0.0 }
    }

    /// Whether a loop plays a neural voice (`Voice{engine: @neural}`).
    fn plays_neural(&self, name: &str) -> bool {
        self.doc.loops.iter().find(|l| l.name == name).and_then(|l| l.play.as_deref()).and_then(|p| self.doc.inst(p)).is_some_and(|i| matches!(i.kind, InstKind::Voice { neural: true, .. }))
    }

    fn cycle(&mut self, index: usize, out: &mut Vec<Event>) {
        let mut state = std::mem::replace(&mut self.loops[index], fresh("", 0.0));
        let start = state.next;
        let Some(def) = self.doc.loops.iter().position(|l| l.name == state.name) else {
            state.next = f64::INFINITY;
            self.loops[index] = state;
            return;
        };
        if self.doc.loops[def].notes.is_some() {
            self.stream(def, &mut state, out);
            self.loops[index] = state;
            return;
        }
        let mark = out.len();
        let len = self.run(def, &mut state, start, out);
        state.next = start + len;
        // A jump ahead cuts the cycle there.
        if let Some(cut) = self.cut.filter(|c| *c > start && *c < start + len) {
            let mut i = mark;
            while i < out.len() {
                if out[i].beat >= cut - 1e-9 { out.remove(i); } else { i += 1; }
            }
            state.next = cut;
        }
        state.cycle += 1;
        self.loops[index] = state;
    }

    /// A note-list loop: send its entries up to the horizon (a batch at a
    /// time), starting a cycle when the last one ended.
    fn stream(&mut self, def: usize, state: &mut LoopState, out: &mut Vec<Event>) {
        let l = &self.doc.loops[def];
        let Some(notes) = l.notes.clone() else { return };
        let (amp, pan, loop_key, name, beats) = (l.amp.max(0.0), l.pan, key(&l.name), l.name.clone(), l.beats);
        if !state.streaming {
            let start = state.next;
            let bar = self.doc.beats_per_bar();
            let end = notes.iter().map(|n| n.beat + n.dur as f64).fold(0.0f64, f64::max);
            let len = beats.unwrap_or_else(|| ((end / bar).ceil() * bar).max(bar));
            let gated = self.doc.arrange.is_empty() || match self.section_index_at(start + 1e-9) {
                Some(index) => self.doc.sections.get(index).is_some_and(|s| s.loops.contains(&name)),
                None => false,
            };
            state.last_start = start;
            state.last_len = len;
            state.last_notes.clear();
            state.last_durs.clear();
            state.resting = !gated;
            state.stream = if gated { 0 } else { notes.len() };
            state.streaming = true;
        }
        let start = state.last_start;
        let l = &self.doc.loops[def];
        let follow = l.follow.as_ref().map_or(1.0, |(param, from, to)| {
            let value = self.params.iter().find(|(p, _)| p == param).map_or(0.0, |(_, v)| *v);
            if to > from { ((value - from) / (to - from)).clamp(0.0, 1.0) } else if value >= *from { 1.0 } else { 0.0 }
        });
        let mut sent = 0;
        while state.stream < notes.len() && sent < STREAM_BATCH {
            let n = notes[state.stream];
            if n.beat >= state.last_len || start + n.beat >= self.horizon.min(self.cut.unwrap_or(f64::INFINITY)) {
                break;
            }
            let beat = start + n.beat;
            let (kind, note, vel) = match n.kind {
                ListKind::Program(program) => (EventKind::Program(program), 0.0, 0.0),
                ListKind::Pedal => (EventKind::Pedal, 0.0, n.vel),
                ListKind::Note => (EventKind::Note, n.note, n.vel * amp * follow),
            };
            let is_note = n.kind == ListKind::Note;
            if !is_note || vel > 0.0 {
                let (pan, opts) = match n.opts.checked_sub(1).and_then(|k| l.note_opts.get(k as usize)) {
                    Some(&(note_pan, opts)) => ((pan + note_pan).clamp(-1.0, 1.0), opts),
                    None => (pan, Default::default()),
                };
                out.push(Event { beat, loop_key, kind, note, vel: vel.clamp(0.0, 4.0), pan, dur: n.dur, opts });
                if is_note {
                    if state.last_notes.len() < 256 {
                        state.last_notes.push((n.beat, note, vel, false));
                        state.last_durs.push(n.dur);
                    }
                    self.history.push_back(Played { beat, loop_name: state.name.clone(), note, vel, dur: n.dur, hit: false });
                }
            }
            state.stream += 1;
            sent += 1;
        }
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        let done = state.stream >= notes.len() || notes[state.stream].beat >= state.last_len;
        if done {
            state.next = start + state.last_len;
            state.cycle += 1;
            state.streaming = false;
        } else {
            state.next = start + notes[state.stream].beat;
        }
    }

    /// Run one cycle of loop `def` at `start`; its length in beats.
    fn run(&mut self, def: usize, state: &mut LoopState, start: f64, out: &mut Vec<Event>) -> f64 {
        let l = self.doc.loops[def].clone();
        let bar = self.doc.beats_per_bar();
        let section_ok = self.doc.arrange.is_empty() || l.stinger.is_some() || {
            match self.section_index_at(start + 1e-9) {
                Some(index) => self.doc.sections.get(index).is_some_and(|s| s.loops.contains(&l.name)),
                None => false,
            }
        };
        let fallback = l.beats.unwrap_or(if state.last_len > 0.0 { state.last_len } else { bar });
        state.last_start = start;
        state.last_notes.clear();
        state.last_durs.clear();
        state.last_lyrics.clear();
        if !section_ok {
            state.resting = true;
            state.last_len = fallback;
            return fallback;
        }
        // The section playing as the cycle starts, and where it started.
        let section_now = self.section_index_at(start + 1e-9).and_then(|index| {
            let at = self.timeline.entries.iter().rev().find(|(from, i, _)| *i == index && *from <= start + 1e-9).map(|(from, ..)| *from)?;
            Some((self.doc.sections.get(index)?.name.clone(), at))
        });
        // The shared context for this cycle.
        {
            let mut ctx = self.vm.ctx.borrow_mut();
            *ctx = Ctx {
                active: true,
                cursor: 0.0,
                emits: std::mem::take(&mut ctx.emits),
                rng: cycle_seed(self.doc.clock.seed, state.key, state.cycle),
                ticks: std::mem::take(&mut state.ticks),
                params: self.params.clone(),
                cues: Vec::new(),
                cycle: state.cycle,
                amp: 1.0,
                dur: l.dur,
                errors: Vec::new(),
                bpm: self.doc.clock.bpm,
                start,
                beats_per_bar: bar,
                section: section_now.clone(),
                shared: std::mem::take(&mut self.shared),
                logs: Vec::new(),
                handles: 0,
                cue_plans: std::mem::take(&mut self.cue_plans),
                scopes: l.scopes.clone(),
                scope_stack: Vec::new(),
                routed: Vec::new(),
                fx_sets: Vec::new(),
                held: { let mut h: Vec<(u8, f32)> = self.held.iter().map(|(n, _, v)| (*n, *v)).collect(); h.sort_by_key(|(n, _)| *n); h.dedup_by_key(|(n, _)| *n); h },
                ccs: self.ccs.clone(),
                osc_values: self.osc_values.clone(),
                osc_sends: Vec::new(),
                lyrics: std::mem::take(&mut ctx.lyrics),
            };
            ctx.lyrics.clear();
            ctx.emits.clear();
        }
        let mut error = None;
        if let Some(when) = l.when {
            match self.vm.call(when, &[state.cycle as f64]) {
                Ok(value) if !Evaluated::truthy(value) => {
                    let mut ctx = self.vm.ctx.borrow_mut();
                    ctx.active = false;
                    state.ticks = std::mem::take(&mut ctx.ticks);
                    self.shared = std::mem::take(&mut ctx.shared);
                    self.cue_plans = std::mem::take(&mut ctx.cue_plans);
                    state.resting = true;
                    state.last_len = fallback;
                    return fallback;
                }
                Ok(_) => {}
                Err(e) => error = Some(e),
            }
        }
        // A pattern first, then the code (both may be present).
        if let Some(hits) = &l.hits {
            let mut ctx = self.vm.ctx.borrow_mut();
            for (i, vel) in hits.steps.iter().enumerate() {
                if let Some(vel) = vel {
                    let what = match hits.drum {
                        Some((slot, gm)) => EmitKind::Hit(slot, gm, 0.0),
                        None => EmitKind::Note(hits.note),
                    };
                    let dur = if hits.drum.is_some() { 0.0 } else { hits.step as f32 * 0.9 };
                    ctx.emits.push(crate::vm::Emit { beat: i as f64 * hits.step, what, vel: *vel, pan: 0.0, dur, opts: Default::default() });
                }
            }
        }
        if let Some(code) = l.code {
            self.vm.ctx.borrow_mut().cursor = 0.0;
            if let Err(e) = self.vm.call(code, &[state.cycle as f64]) {
                error = Some(e);
            }
        }
        let (routed, fx_sets, osc_sends) = {
            let mut ctx = self.vm.ctx.borrow_mut();
            (std::mem::take(&mut ctx.routed), std::mem::take(&mut ctx.fx_sets), std::mem::take(&mut ctx.osc_sends))
        };
        let (emits, cursor, ticks, cues) = {
            let mut ctx = self.vm.ctx.borrow_mut();
            ctx.active = false;
            state.last_lyrics.append(&mut ctx.lyrics);
            state.last_logs = std::mem::take(&mut ctx.logs);
            // Shared values stay; the oldest go past a bound.
            self.shared = std::mem::take(&mut ctx.shared);
            if self.shared.len() > 4096 {
                self.shared.drain(..self.shared.len() - 4096);
            }
            self.cue_plans = std::mem::take(&mut ctx.cue_plans);
            (std::mem::take(&mut ctx.emits), ctx.cursor, std::mem::take(&mut ctx.ticks), std::mem::take(&mut ctx.cues))
        };
        state.ticks = ticks;
        state.resting = false;
        let pattern_len = l.hits.as_ref().map(|h| h.steps.len() as f64 * h.step).unwrap_or(0.0);
        let mut len = match l.beats {
            Some(beats) => beats,
            None if l.code.is_some() && cursor > 0.0 => cursor.max(pattern_len),
            None if pattern_len > 0.0 => pattern_len,
            None => {
                let plays_input = l.play.as_deref().and_then(|p| self.doc.inst(p)).is_some_and(|i| matches!(i.kind, InstKind::Input { .. }));
                if error.is_none() && l.stinger.is_none() && l.midi_in.is_none() && !plays_input {
                    error = Some(format!("{}:1:1: loop `{}` never sleeps: give it `beats:` or sleep() in its code", crate::vm::DOC_FILE, l.name));
                }
                bar
            }
        };
        if !(len.is_finite() && len > 0.0) {
            len = bar;
        }
        state.error = error.map(|e| {
            let (line, col, message) = crate::vm::split_error(&e);
            Diag { line, col, node: Some(l.name.clone()), message }
        });
        if state.error.is_some() {
            // An error plays nothing this cycle; the loop keeps its place.
            state.last_len = len;
            return len;
        }
        let swing = l.swing.unwrap_or(self.doc.clock.swing);
        // OSC messages go out at their beats, to their target or the Clock's.
        for (at, addr, args, to) in osc_sends {
            match to.or_else(|| self.doc.clock.osc_out.clone()) {
                Some(target) if self.osc_out.len() < 4096 => self.osc_out.push((start + at, target, crate::osc::encode(&addr, &args))),
                Some(_) => {}
                None => {
                    if state.error.is_none() {
                        state.error = Some(Diag { line: 1, col: 1, node: Some(l.name.clone()), message: format!("osc_send(\"{addr}\"): where to? give Clock{{osc_out: \"127.0.0.1:57120\"}} or a target: osc_send(\"{addr}\", [..], \"host:port\")") });
                    }
                }
            }
        }
        // fx() scopes: each plays on its own node; settings go to its effect.
        let route_keys: Vec<u64> = std::iter::once(key(&l.name)).chain((1..=l.scopes.len()).map(|k| key(&crate::model::scope_play(&l.name, k)))).collect();
        let route_of = |i: usize| routed.iter().find(|(from, to, _)| (*from..*to).contains(&i)).map_or(0, |(.., k)| *k);
        for (k, param, value, at) in fx_sets {
            let event = makepad_audio_engine::feed::AutomationEvent { beat: start + at, param, value, ramp: 0.0 };
            self.automation_out.push((crate::model::scope_fx(&l.name, k), event));
        }
        // Humanize: small seeded offsets of time and velocity, drawn per
        // event from (seed, loop, cycle), so a cycle always plays the same.
        let mut feel = cycle_seed(self.doc.clock.seed ^ 0x6875_6d61, state.key, state.cycle);
        let mut jitter = || {
            feel = feel.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = feel;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        let humanize = l.humanize;
        #[cfg(not(target_arch = "wasm32"))]
        let neural = self.plays_neural(&l.name);
        #[cfg(not(target_arch = "wasm32"))]
        let singer = if neural { makepad_score_player::sung::neural::shared() } else { None };
        let sampler_root = l.play.as_deref().and_then(|p| self.doc.inst(p)).and_then(|inst| match &inst.kind {
            InstKind::Sampler { root, .. } => Some(*root),
            _ => None,
        });
        // A loop that follows a param fades in over its range: silent below
        // `from`, full from `to`.
        let follow = l.follow.as_ref().map_or(1.0, |(param, from, to)| {
            let value = self.params.iter().find(|(p, _)| p == param).map_or(0.0, |(_, v)| *v);
            if to > from { ((value - from) / (to - from)).clamp(0.0, 1.0) } else if value >= *from { 1.0 } else { 0.0 }
        });
        for (index, emit) in emits.into_iter().enumerate() {
            let loop_key = route_keys.get(route_of(index)).copied().unwrap_or(route_keys[0]);
            if emit.what == EmitKind::Pedal {
                // The pedal holds the loop's notes, scoped or not.
                for loop_key in &route_keys {
                    out.push(Event { beat: start + emit.beat, loop_key: *loop_key, kind: EventKind::Pedal, note: 0.0, vel: emit.vel, pan: 0.0, dur: 0.0, opts: Default::default() });
                }
                continue;
            }
            if let EmitKind::Glide(note) = emit.what {
                // To the node the note plays on (its handle is unique).
                for loop_key in &route_keys {
                    out.push(Event { beat: start + emit.beat, loop_key: *loop_key, kind: EventKind::Glide, note, vel: emit.vel, pan: 0.0, dur: emit.dur, opts: emit.opts });
                }
                continue;
            }
            let emit = crate::vm::Emit { vel: emit.vel * follow, ..emit };
            if emit.vel <= 0.0 {
                continue;
            }
            // A neural phrase: asked of the singer now, played at its own
            // start (never swung or humanized: its audio is one piece); a
            // loop on another instrument has no use for it.
            #[cfg(not(target_arch = "wasm32"))]
            if let EmitKind::Phrase(key) = emit.what {
                if !neural {
                    continue;
                }
                if let (Some(singer), Some(score)) = (&singer, makepad_score_player::sung::neural::recall(key)) {
                    if self.wait_for_phrases {
                        singer.render_now(key, &score);
                    } else {
                        singer.want(key, score);
                    }
                }
                out.push(Event { beat: start + emit.beat, loop_key, kind: EventKind::Phrase(key), note: 0.0, vel: emit.vel.clamp(0.0, 4.0), pan: emit.pan, dur: 0.0, opts: emit.opts });
                continue;
            }
            let mut beat = emit.beat;
            if swing != 0.0 {
                let sixteenth = beat / 0.25;
                if (sixteenth - sixteenth.round()).abs() < 1e-6 && (sixteenth.round() as i64).rem_euclid(2) == 1 {
                    beat += swing * 0.25;
                }
            }
            let mut emit = emit;
            if let Some((time, vel)) = humanize {
                beat = (beat + jitter() * time).max(0.0);
                emit.vel = (emit.vel * (1.0 + jitter() as f32 * vel)).clamp(0.0, 4.0);
            }
            let (kind, note) = match emit.what {
                EmitKind::Note(note) => (EventKind::Note, note),
                EmitKind::Hit(slot, gm, tune) => (EventKind::Hit(slot, gm), tune),
                EmitKind::Sample(note) => (EventKind::Sample, if note.is_nan() { sampler_root.unwrap_or(60.0) } else { note }),
                EmitKind::Sung(note, syl, part) => (EventKind::Sung(part, syl), note),
                EmitKind::Pedal | EmitKind::Glide(_) | EmitKind::Phrase(_) => continue,
            };
            // A note that is not a number (an id in arithmetic: `@c4 + 12`
            // is not a note; `note(@c4) + 12` is) is refused, never played.
            if !note.is_finite() || !emit.vel.is_finite() || !emit.dur.is_finite() {
                if state.error.is_none() {
                    state.error = Some(Diag { line: 1, col: 1, node: Some(l.name.clone()), message: "a note is not a number: an id in arithmetic (`@c4 + 12`, or a list of ids) gives no note; use note(@c4) + 12".into() });
                }
                continue;
            }
            let hit = matches!(kind, EventKind::Hit(..));
            let shown = if let EventKind::Hit(slot, _) = kind { slot as f32 } else { note };
            state.last_notes.push((beat, shown, emit.vel, hit));
            state.last_durs.push(emit.dur);
            self.history.push_back(Played { beat: start + beat, loop_name: l.name.clone(), note: shown, vel: emit.vel, dur: emit.dur, hit });
            out.push(Event { beat: start + beat, loop_key, kind, note, vel: emit.vel.clamp(0.0, 4.0), pan: emit.pan, dur: emit.dur, opts: emit.opts });
        }
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        // This cycle's cues replace what the loop cued before.
        self.cue_plans.retain(|p| p.loop_key != state.key);
        for (name, at) in &cues {
            let count = self.cue_plans.len();
            match self.cue_plans.iter_mut().find(|p| p.loop_key == state.key && &p.name == name) {
                Some(plan) => plan.offsets.push(*at),
                None if count < 256 => {
                    self.cue_plans.push(crate::vm::CuePlan { name: name.clone(), loop_key: state.key, start, len, offsets: vec![*at] });
                }
                None => {}
            }
        }
        for (name, at) in cues {
            self.cues.push_back((name, start + at));
            if self.cues.len() > 256 {
                self.cues.pop_front();
            }
        }
        state.last_len = len;
        len
    }

    /// Runtime errors per loop (the last cycle's).
    pub fn errors(&self) -> Vec<Diag> {
        self.loops.iter().filter_map(|s| s.error.clone()).collect()
    }
}

fn fresh(name: &str, start: f64) -> LoopState {
    LoopState {
        name: name.to_string(), key: key(name), next: start, cycle: 0, ticks: Vec::new(), last_start: start, last_len: 0.0,
        last_notes: Vec::new(), last_durs: Vec::new(), last_lyrics: Vec::new(), last_logs: Vec::new(), resting: false, error: None, stream: 0, streaming: false, cues_first: false,
    }
}

/// The first multiple of `quant` at or after `now` (a hair past a boundary
/// counts as that boundary).
pub fn quantize(now: f64, quant: f64) -> f64 {
    if quant <= 0.0 {
        return now;
    }
    let k = (now / quant - 1e-9).ceil();
    (k * quant).max(0.0)
}
