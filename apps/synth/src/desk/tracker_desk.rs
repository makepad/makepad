//! Between the desk and its tracker: the model built per revision (and
//! the read-only parts' notes played through off the UI thread), where the
//! music is each turn, and the tracker's edits written into the code as
//! one undo step each.

use super::tracker::{ScoreTracker, TrackerAction, TrackerLive};
use super::tracker_model::{self as tm, Played};
use super::code::ScoreEditor;
use super::Desk;
use crate::audio;
use makepad_code_editor::code_view::CodeView;
use makepad_widgets::*;
use std::sync::mpsc::Receiver;

#[derive(Default)]
pub struct TrackerDesk {
    /// The revision the model was built for, and the played parts it has.
    built: (u64, u64),
    /// The read-only parts as played, for a revision; the job making the next.
    played: (u64, Played),
    rx: Option<(u64, Receiver<Played>)>,
    wired: bool,
    /// The revision the settings' catalogue was handed over for.
    catalog: Option<u64>,
    /// The model last handed over (MIDI channels' live notes go into it).
    model: Option<tm::TrackModel>,
    /// Per loop MIDI in plays: the notes it started (song beat, pitch,
    /// velocity) and the stream's count when last read.
    live: Vec<(String, u64, Vec<(f64, f32, f32)>)>,
}

/// The notes a live MIDI channel keeps.
const LIVE_NOTES: usize = 1024;

impl Desk {
    /// The tracker's turn: a model when the source changed, where the
    /// music is, the catalogue while the settings are open.
    pub(super) fn pump_tracker(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let widget = ui.widget(cx, ids!(score_tracker));
        if !self.tracker.wired {
            if let Some(mut t) = widget.borrow_mut::<ScoreTracker>() {
                t.set_meters(self.engine.meters.clone(), audio::slot());
                self.tracker.wired = true;
            }
        }
        // What the code parts play, again when the source changes.
        if let Some((revision, rx)) = &self.tracker.rx {
            if let Ok(played) = rx.try_recv() {
                self.tracker.played = (*revision, played);
                self.tracker.rx = None;
            }
        }
        if self.tracker.played.0 != self.revision && self.tracker.rx.as_ref().is_none_or(|(r, _)| *r != self.revision) {
            let (tx, rx) = std::sync::mpsc::channel();
            let source = self.source.clone();
            std::thread::spawn(move || {
                let _ = tx.send(tm::played_parts(&source));
            });
            self.tracker.rx = Some((self.revision, rx));
        }
        if self.tracker.rx.is_some() {
            self.frame = cx.new_next_frame();
        }
        // The model is the source's only when the source loads: else the
        // last good one shows, read only.
        let current = self.doc.as_ref().is_some_and(|d| d.source == self.source);
        let built = (self.revision, self.tracker.played.0);
        if self.tracker.built != built {
            if let Some(doc) = self.doc.as_ref().filter(|_| current) {
                let mut model = tm::build(doc, &self.source);
                model.set_played(&self.tracker.played.1);
                // The channels MIDI in plays keep what they played.
                let midi: Vec<String> = doc.loops.iter().filter(|l| l.input.is_some()).map(|l| l.name.clone()).collect();
                self.tracker.live.retain(|(name, ..)| midi.contains(name));
                for (name, _, notes) in &self.tracker.live {
                    model.set_live_notes(name, notes);
                }
                if let Some(mut t) = widget.borrow_mut::<ScoreTracker>() {
                    t.set_source(&self.source);
                    t.set_model(cx, model.clone(), true, "");
                }
                self.tracker.model = Some(model);
                self.tracker.built = built;
            } else if let Some(mut t) = widget.borrow_mut::<ScoreTracker>() {
                t.set_editable(cx, false, "The code does not load: the tracker shows the last good version, read only");
                self.tracker.built = built;
            }
        }
        self.pump_live_midi(cx, &widget);
        let live = TrackerLive {
            playing: self.status.playing,
            beat: makepad_score_player::Meters::beat(&*self.engine.meters),
            song_off: self.song_beat().map(|s| s - self.status.beat),
            bpm: self.status.bpm,
            looping: self.status.looping.or(self.looping),
        };
        let mut wants_catalog = false;
        if let Some(mut t) = widget.borrow_mut::<ScoreTracker>() {
            t.set_live(cx, live);
            wants_catalog = t.settings_open().is_some() && self.tracker.catalog != Some(self.revision);
        }
        if wants_catalog {
            self.tracker.catalog = Some(self.revision);
            let doc = self.doc.as_ref();
            let available = doc.map(|doc| {
                let named = |list: Vec<makepad_score_live::model::AssetRef>| list.into_iter().map(|a| (a.id.chars().take(12).collect::<String>(), a)).collect();
                makepad_score_live::instruments::Available { fonts: named(doc.files()), samples: named(doc.samples()), singers: named(doc.singers()), mine: Vec::new() }
            }).unwrap_or_default();
            let catalog = makepad_score_live::instruments::catalog(doc, &available);
            if let Some(mut t) = widget.borrow_mut::<ScoreTracker>() {
                t.set_catalog(cx, catalog);
            }
        }
    }

    /// The notes the MIDI-in channels started since the last turn, from
    /// their streams (`makepad_audio_engine::feed::streams`), into the
    /// model: the tracker shows them as they are played.
    fn pump_live_midi(&mut self, cx: &mut Cx, widget: &WidgetRef) {
        let Some(doc) = self.doc.as_ref() else { return };
        let song_off = self.song_beat().map_or(0.0, |s| s - self.status.beat);
        let mut changed = false;
        for l in doc.loops.iter().filter(|l| l.input.is_some()) {
            let view = makepad_audio_engine::feed::streams::view(&format!("{}/{}", audio::SLOT, l.name));
            let snap = view.snapshot();
            let at = match self.tracker.live.iter().position(|(n, ..)| *n == l.name) {
                Some(at) => at,
                None => {
                    self.tracker.live.push((l.name.clone(), snap.started, Vec::new()));
                    continue;
                }
            };
            let (_, seen, notes) = &mut self.tracker.live[at];
            let new = (snap.started.saturating_sub(*seen) as usize).min(snap.recent.len());
            *seen = snap.started;
            if new == 0 {
                continue;
            }
            for (beat, note, vel) in &snap.recent[snap.recent.len() - new..] {
                notes.push((beat + song_off, *note as f32, *vel as f32 / 127.0));
            }
            let keep = notes.len().saturating_sub(LIVE_NOTES);
            notes.drain(..keep);
            changed = true;
        }
        if !changed {
            return;
        }
        if let Some(model) = self.tracker.model.as_mut() {
            for (name, _, notes) in &self.tracker.live {
                model.set_live_notes(name, notes);
            }
            if let Some(mut t) = widget.borrow_mut::<ScoreTracker>() {
                t.set_model(cx, model.clone(), true, "");
            }
        }
    }

    pub(super) fn handle_tracker_actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) {
        let tracker = ui.widget(cx, ids!(score_tracker));
        for action in actions.filter_widget_actions_cast::<TrackerAction>(tracker.widget_uid()) {
            match action {
                TrackerAction::Edit { edit, status } => {
                    // Typing not yet applied: the tracker's bytes are not the
                    // code's (unless the change was this view's own echo).
                    if self.typed {
                        let text = ui.widget(cx, ids!(score_code)).borrow::<ScoreEditor>().map(|editor| editor.text(cx));
                        if text.as_deref() == Some(self.source.as_str()) {
                            self.typed = false;
                        }
                    }
                    if self.tracker.built.0 != self.revision || self.typed {
                        log!("tracker: edit refused (model of revision {}, source {}, typing {})", self.tracker.built.0, self.revision, self.typed);
                        ui.label(cx, ids!(score_state)).set_text(cx, "The code is still applying: try again");
                        continue;
                    }
                    ui.label(cx, ids!(score_state)).set_text(cx, &status);
                    self.edit(cx, ui, edit);
                }
                // The ruler moves the playhead at once, playing or stopped
                // (a drag scrubs).
                TrackerAction::Seek(beat) => self.seek(beat, 0.0),
                TrackerAction::Loop(range) => {
                    self.looping = range;
                    self.set_loop(range);
                }
                TrackerAction::Hold { channel, field, value } => {
                    audio::slot().set_control(&format!("{}/{channel}", audio::SLOT), field, value);
                }
                TrackerAction::Preview(choice) => self.panel.preview = choice,
                TrackerAction::Settings => self.tracker.catalog = None,
                TrackerAction::Undo | TrackerAction::Redo => {
                    let undo = matches!(action, TrackerAction::Undo);
                    let code = ui.widget(cx, ids!(score_code)).widget(cx, ids!(code));
                    let text = code.borrow_mut::<CodeView>().and_then(|mut view| {
                        let session = view.session.as_mut()?;
                        let done = if undo { session.undo() } else { session.redo() };
                        session.handle_changes();
                        view.redraw(cx);
                        done.then(|| view.session.as_ref().map(|s| s.document().as_text().to_string())).flatten()
                    });
                    if let Some(text) = text.filter(|t| *t != self.source) {
                        self.source = text;
                        self.revision += 1;
                        self.send();
                        self.frame = cx.new_next_frame();
                    }
                }
                TrackerAction::Status(text) => ui.label(cx, ids!(score_state)).set_text(cx, &text),
                TrackerAction::Voice(name) => {
                    makepad_synth_panel::host::select(&name);
                    makepad_editor_panel::changed();
                    ui.widget(cx, ids!(score_panel)).redraw(cx);
                }
                TrackerAction::None => {}
            }
        }
    }
}
