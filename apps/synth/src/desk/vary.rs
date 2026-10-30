//! The voice tree's data and the melody variants of the score's panel: each
//! voice's part frozen from the score (one cycle), its variants as cards
//! (procedural and seeded at once; the AI's from short conversations at
//! once), heard in place: while a card plays, the composer plays the score
//! with that part instead (same instrument, in sync, the written part not
//! heard), from where the voice plays when the song was stopped; kept,
//! applied or merged bar by bar (one edit).

use super::Desk;
use makepad_score_live::vary::{self, Part};
use makepad_music_iter::gen::TurnMonitor;
use makepad_synth_panel::host::{Card, Track, Variants, VaryAction};
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// A voice's variants.
#[derive(Default)]
pub struct VoiceVariants {
    pub original: Option<Part>,
    /// (label, part, kept).
    pub cards: Vec<(String, Part, bool)>,
    pub region: Option<(f64, f64)>,
    /// The next seed (each make moves it on: the same session, the same
    /// variants; a new make, new ones).
    pub seed: u64,
    pub busy: Option<String>,
    pub playing: Option<usize>,
    /// The AI's variants on their way.
    pub ai: Option<AiAsk>,
}

/// An ask of the agent for `n` variants, each its own conversation.
pub struct AiAsk {
    started: f64,
    kind: String,
    rx: Receiver<(usize, Result<String, String>)>,
    tx: std::sync::mpsc::Sender<(usize, Result<String, String>)>,
    /// Each variant's prompt, its tries, and what it came to: None while
    /// asked, Some(Ok(part)) or Some(Err(why)).
    texts: Vec<String>,
    tries: Vec<u8>,
    done: Vec<Option<Result<Part, String>>>,
    cancel: Arc<AtomicBool>,
    monitor: TurnMonitor,
}

impl Drop for AiAsk {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// An answer the same as the original or another variant (its notes and
/// their places; velocities and lengths aside) says nothing new.
fn same_notes(a: &Part, b: &Part) -> bool {
    a.notes.len() == b.notes.len() && a.notes.iter().zip(&b.notes).all(|(x, y)| (x.beat - y.beat).abs() < 1e-3 && x.note == y.note && x.hit == y.hit)
}

/// Asks a variant (again).
fn ask_variant(tx: &std::sync::mpsc::Sender<(usize, Result<String, String>)>, k: usize, text: String, cancel: Arc<AtomicBool>, monitor: TurnMonitor) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let _ = tx.send((k, super::panel::write_with(VARY_SYSTEM, text, &cancel, &monitor, &mut |_| {})));
    });
}

/// The desk's side of the variants and the tree.
#[derive(Default)]
pub struct VaryDesk {
    pub voices: std::collections::HashMap<String, VoiceVariants>,
    /// The tree's voices, made for a score revision (off the UI thread).
    pub tracks: Vec<Track>,
    tracks_rev: u64,
    tracks_rx: Option<(u64, Receiver<Vec<Track>>)>,
    /// The card the composer plays in place: (voice, card), and whether
    /// the audition started the song (it stops it again).
    pub live: Option<(String, usize)>,
    started_song: bool,
}

impl VaryDesk {
    pub(super) fn not_responding(&self) -> bool {
        self.voices.values().filter_map(|voice| voice.ai.as_ref()).any(|ask| ask.monitor.not_responding())
    }

    pub(super) fn stop_writers(&mut self) {
        for voice in self.voices.values_mut() {
            if voice.ai.take().is_some() {
                voice.busy = None;
            }
        }
    }

    pub fn view(&self, node: &str) -> Variants {
        let Some(v) = self.voices.get(node) else { return Variants::default() };
        let card = |label: &str, part: &Part, kept: bool| Card {
            label: label.to_string(),
            notes: part.notes.iter().map(|n| (n.beat, n.note, n.dur, n.vel, n.hit)).collect(),
            kept,
        };
        let mut cards = Vec::new();
        if let Some(o) = &v.original {
            cards.push(card("Original", o, true));
        }
        cards.extend(v.cards.iter().map(|(l, p, k)| card(l, p, *k)));
        Variants { beats: v.original.as_ref().map_or(0.0, |o| o.beats), cards, busy: v.busy.clone(), playing: v.playing, region: v.region }
    }
}

/// The voices of a score as the tree shows them (cycle notes included).
fn tracks_of(source: &str) -> Vec<Track> {
    let Ok(doc) = makepad_score_live::ScoreDoc::load(source) else { return Vec::new() };
    let cycles = makepad_score_live::render::cycle_notes(source);
    let bar = doc.beats_per_bar();
    let sections: Vec<(String, f64, Vec<String>)> = doc.arrange.iter().filter_map(|name| doc.sections.iter().find(|s| &s.name == name)).map(|s| (s.name.clone(), s.bars * bar, s.loops.clone())).collect();
    let outline = makepad_score_live::source::outline(source);
    doc.loops.iter().filter(|l| l.stinger.is_none()).map(|l| {
        let cycle = cycles.iter().find(|(n, ..)| *n == l.name);
        let notes = cycle.map(|(_, _, _, notes, durs)| notes.iter().enumerate().map(|(i, (b, n, v, h))| (*b, *n, durs.get(i).copied().filter(|d| *d > 0.0).unwrap_or(0.25), *v, *h)).collect()).unwrap_or_default();
        let inst_label = l.play.as_deref().and_then(|p| outline.node(p)).map(|c| {
            let fields: Vec<String> = c.fields.iter().filter(|f| matches!(f.key.as_str(), "preset" | "lib" | "kind" | "kit" | "program" | "osc" | "profile")).map(|f| f.value.text(source).trim_matches('"').to_string()).collect();
            if fields.is_empty() { c.type_name.clone() } else { format!("{} {}", c.type_name, fields.join(" ")) }
        }).unwrap_or_default();
        Track {
            name: l.name.clone(),
            instrument: l.play.clone(),
            inst_label,
            amp: l.amp as f64,
            pan: l.pan as f64,
            mute: l.mute,
            solo: l.solo,
            notes,
            beats: cycle.map_or(l.beats.unwrap_or(4.0), |c| c.2).max(1.0),
            beats_per_bar: bar,
            sections: if sections.is_empty() { vec![(0.0, 4.0 * bar, true)] } else {
                let mut at = 0.0;
                sections.iter().map(|(_, beats, loops)| {
                    let s = (at, *beats, loops.contains(&l.name));
                    at += beats;
                    s
                }).collect()
            },
        }
    }).collect()
}

/// What an AI variant writer is told.
const VARY_SYSTEM: &str = "You rewrite one part of a piece of live-coded music: you are given the part's notes for one \
cycle as `[[beat note dur vel] ...]` (beats from the cycle's start, note names like @a2, drum names like @kick, durations \
in beats, velocities 0..1), the cycle's length, its key, and how to change it. Answer with only the new notes in the same \
form, one list, nothing else: no prose, no code fences. Keep the cycle's length, the key, the register and the part's \
character unless the ask says otherwise; change only the bars you are told to. Make it musical: a phrase that breathes, \
strong beats where the groove wants them.";

impl Desk {
    /// The tree's voices, made again when the score changes.
    pub(super) fn pump_tracks(&mut self, cx: &mut Cx) {
        let desk = &mut self.vary;
        if let Some((rev, rx)) = &desk.tracks_rx {
            if let Ok(tracks) = rx.try_recv() {
                desk.tracks = tracks;
                desk.tracks_rev = *rev;
                desk.tracks_rx = None;
                makepad_editor_panel::changed();
            } else {
                self.frame = cx.new_next_frame();
            }
        }
        if self.vary.tracks_rev != self.revision && self.vary.tracks_rx.is_none() {
            let source = self.source.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(tracks_of(&source));
            });
            self.vary.tracks_rx = Some((self.revision, rx));
            self.frame = cx.new_next_frame();
        }
    }

    /// A voice's part read from the score as it is now (a voice selected:
    /// its timeline shows the part the variants change). Another part
    /// there (another score, an edit) starts its variants again.
    pub(super) fn vary_read(&mut self, node: &str) -> Result<(), String> {
        let now = vary::part_of(&self.source, node)?;
        let voice = self.vary.voices.entry(node.to_string()).or_default();
        if !voice.original.as_ref().is_some_and(|o| same_notes(o, &now)) {
            voice.original = Some(now);
            voice.cards.clear();
            voice.playing = None;
            voice.ai = None;
            voice.busy = None;
            voice.region = None;
            voice.seed = voice.seed.max(1);
        }
        Ok(())
    }

    /// A person's (or a tool's) action on a voice's variants.
    pub fn vary_action(&mut self, cx: &mut Cx, ui: &WidgetRef, node: &str, action: VaryAction) -> Result<String, String> {
        self.vary_read(node)?;
        let source = self.source.clone();
        let voice = self.vary.voices.entry(node.to_string()).or_default();
        let original = voice.original.clone().expect("set above");
        match action {
            VaryAction::Make { kind, ai: false, n } => {
                let made = vary::vary(&original, &kind, n, voice.seed, voice.region)?;
                voice.seed += 1;
                // One that changes no note here is not a card.
                let (made, same): (Vec<(String, Part)>, Vec<(String, Part)>) = made.into_iter().partition(|(_, p)| !same_notes(p, &original));
                if made.is_empty() {
                    return Err(format!("{} changes no note of this part", same.first().map_or(kind.clone(), |s| s.0.clone())));
                }
                let labels: Vec<String> = made.iter().map(|(l, _)| l.clone()).collect();
                voice.cards.extend(made.into_iter().map(|(l, p)| (l, p, false)));
                Ok(format!("made {}", labels.join(", ")))
            }
            VaryAction::Make { kind, ai: true, n } => {
                if voice.ai.is_some() {
                    return Err("the agent is still writing the last variants".into());
                }
                let (root, scale) = vary::key_of(&original);
                let key = format!("{} {}", makepad_score_live::theory::note_name(root), if scale == makepad_score_live::theory::scale_kind("major").unwrap() { "major" } else { "minor" });
                let bars = match voice.region {
                    Some((a, b)) => format!("bars {}-{} only (beats {}-{}); the rest exactly as it is", (a / original.beats_per_bar) as usize + 1, (b / original.beats_per_bar) as usize, a, b),
                    None => "the whole part".into(),
                };
                let n = n.clamp(1, 6);
                let (tx, rx) = std::sync::mpsc::channel();
                let cancel = Arc::new(AtomicBool::new(false));
                let monitor = TurnMonitor::default();
                let texts: Vec<String> = (0..n).map(|k| format!("The part ({} beats a cycle, {} beats a bar, in {key}):\n{}\nChange {bars}: {kind}. This is variant {} of {n}: make it different from the others, and change the notes (not only their loudness or length).", original.beats, original.beats_per_bar, original.text(), k + 1)).collect();
                for (k, text) in texts.iter().enumerate() {
                    ask_variant(&tx, k, text.clone(), cancel.clone(), monitor.clone());
                }
                voice.busy = Some(format!("the agent is writing {n} variants: {kind}"));
                voice.ai = Some(AiAsk { started: Cx::time_now(), kind, rx, tx, tries: vec![1; n], done: (0..n).map(|_| None).collect(), texts, cancel, monitor });
                self.frame = cx.new_next_frame();
                Ok(format!("asked for {n}"))
            }
            VaryAction::Region(region) => {
                voice.region = region;
                Ok("region set".into())
            }
            VaryAction::Keep(i) => {
                if let Some(card) = i.checked_sub(1).and_then(|k| voice.cards.get_mut(k)) {
                    card.2 = !card.2;
                }
                Ok("kept".into())
            }
            VaryAction::Clear => {
                voice.cards.retain(|c| c.2);
                Ok("cleared".into())
            }
            VaryAction::Audition(None) => {
                voice.playing = None;
                self.vary_stop();
                Ok("stopped".into())
            }
            VaryAction::Audition(Some(i)) => {
                let part = if i == 0 { original.clone() } else { voice.cards.get(i - 1).map(|c| c.1.clone()).ok_or("no such card")? };
                // The original is the document as written.
                let next = if i == 0 { None } else { Some(vary::apply(&source, node, &part)?) };
                for v in self.vary.voices.values_mut() {
                    v.playing = None;
                }
                if let Some(v) = self.vary.voices.get_mut(node) {
                    v.playing = Some(i);
                }
                self.ensure_live();
                self.send();
                self.variant_live = next;
                self.send();
                self.vary.live = Some((node.to_string(), i));
                // Stopped: the song starts where the voice first plays.
                if !self.engine.playing() {
                    let start = self.doc.as_ref().and_then(|d| makepad_score_live::audition::first_beat(d, &[node.to_string()]));
                    if let (Some(live), Some(beat)) = (&self.live, start) {
                        live.seek(beat, 0.0);
                    }
                    self.transport(true);
                    self.vary.started_song = true;
                }
                Ok(format!("{node} plays card {i} in place"))
            }
            VaryAction::Apply(i) => {
                let part = if i == 0 { original } else { voice.cards.get(i - 1).map(|c| c.1.clone()).ok_or("no such card")? };
                self.vary_write(cx, ui, node, &part)
            }
            VaryAction::Merge(picks) => {
                let parts: Vec<&Part> = std::iter::once(&original).chain(voice.cards.iter().map(|c| &c.1)).collect();
                let merged = vary::merge(&parts, &picks);
                self.vary_write(cx, ui, node, &merged)
            }
        }
    }

    /// The audition ends: the composer plays the document again, and the
    /// song stops when the audition started it.
    fn vary_stop(&mut self) {
        self.vary.live = None;
        self.variant_live = None;
        self.send();
        if std::mem::take(&mut self.vary.started_song) {
            self.transport(false);
        }
    }

    /// A voice made to play `part`: one undo step; its variants start from it.
    fn vary_write(&mut self, cx: &mut Cx, ui: &WidgetRef, node: &str, part: &Part) -> Result<String, String> {
        let next = vary::apply(&self.source, node, part)?;
        // What plays in place now is written: the song plays on with it.
        self.vary.live = None;
        self.vary.started_song = false;
        for v in self.vary.voices.values_mut() {
            v.playing = None;
        }
        self.set_source(cx, ui, next);
        if let Some(voice) = self.vary.voices.get_mut(node) {
            voice.original = Some(part.clone());
            voice.cards.retain(|c| c.2);
            voice.playing = None;
        }
        Ok(format!("{node} plays it now (revision {})", self.revision))
    }

    /// The AI's variants and the auditions as they come.
    pub(super) fn pump_vary(&mut self, cx: &mut Cx) {
        for voice in self.vary.voices.values_mut() {
            let Some(ask) = voice.ai.as_mut() else { continue };
            let Some(original) = voice.original.clone() else {
                voice.ai = None;
                continue;
            };
            // Each answer as it comes: one that changes no note, or repeats
            // another card, is asked again (twice at most).
            while let Ok((k, answer)) = ask.rx.try_recv() {
                let result = answer.and_then(|text| vary::parse_notes(&text, &original));
                let repeat = match &result {
                    Ok(part) => same_notes(part, &original) || voice.cards.iter().any(|c| same_notes(&c.1, part)) || ask.done.iter().flatten().flatten().any(|d| same_notes(d, part)),
                    Err(_) => false,
                };
                if (repeat || result.is_err()) && ask.tries[k] < 3 {
                    ask.tries[k] += 1;
                    let why = if repeat { "That answer changed no note of the part (or repeated another variant): change the notes themselves.".to_string() } else { format!("That answer could not be read ({}): answer with only the list.", result.as_ref().err().cloned().unwrap_or_default()) };
                    ask_variant(&ask.tx, k, format!("{}\n{why}", ask.texts[k]), ask.cancel.clone(), ask.monitor.clone());
                    continue;
                }
                ask.done[k] = Some(if repeat { Err("the same notes again".into()) } else { result });
            }
            let n = ask.done.len();
            if ask.done.iter().any(|d| d.is_none()) {
                voice.busy = Some(if ask.monitor.not_responding() {
                    "not responding — Stop?".into()
                } else {
                    format!("the agent is writing {n} variants: {} ({:.0} s)", ask.kind, Cx::time_now() - ask.started)
                });
                self.frame = cx.new_next_frame();
                continue;
            }
            let mut ask = voice.ai.take().expect("checked above");
            let secs = Cx::time_now() - ask.started;
            voice.busy = None;
            let mut made = 0;
            let mut why = None;
            let label: String = ask.kind.chars().take(24).collect();
            for (k, done) in ask.done.drain(..).enumerate() {
                match done.expect("all done") {
                    Ok(part) => {
                        made += 1;
                        voice.cards.push((format!("AI: {label} {}", k + 1), part, false));
                    }
                    Err(e) => why = Some(e),
                }
            }
            let retries: u8 = ask.tries.iter().map(|t| t - 1).sum();
            log!("score vary: {made} AI variants in {secs:.1} s ({retries} asked again){}", why.map(|w| format!(" (one failed: {w})")).unwrap_or_default());
            self.vary_ai_made = Some((made, secs));
        }
        // An edit ended the audition (the composer plays the document).
        if self.vary.live.as_ref().is_some_and(|(_, card)| *card > 0) && self.variant_live.is_none() {
            self.vary.live = None;
            self.vary.started_song = false;
            for v in self.vary.voices.values_mut() {
                v.playing = None;
            }
        }
    }
}
