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
use makepad_score_live::source::Splice;
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
}

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
                if let Some(mut t) = widget.borrow_mut::<ScoreTracker>() {
                    t.set_source(&self.source);
                    t.set_model(cx, model, true, "");
                }
                self.tracker.built = built;
            } else if let Some(mut t) = widget.borrow_mut::<ScoreTracker>() {
                t.set_editable(cx, false, "The code does not load: the tracker shows the last good version, read only");
                self.tracker.built = built;
            }
        }
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

    pub(super) fn handle_tracker_actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) {
        let tracker = ui.widget(cx, ids!(score_tracker));
        for action in actions.filter_widget_actions_cast::<TrackerAction>(tracker.widget_uid()) {
            match action {
                TrackerAction::Edit { splices, status } => {
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
                    self.write_splices(cx, ui, &splices);
                    ui.label(cx, ids!(score_state)).set_text(cx, &status);
                }
                TrackerAction::Seek(beat) => {
                    let quant = if self.status.playing { -1.0 } else { 0.0 };
                    self.seek(beat, quant);
                }
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

    /// One gesture's splices (normalized, of the current source) into the
    /// code view as one undo step, and into the source.
    fn write_splices(&mut self, cx: &mut Cx, ui: &WidgetRef, splices: &[Splice]) {
        let ok = splices.iter().all(|s| s.start <= s.end && s.end <= self.source.len() && self.source.is_char_boundary(s.start) && self.source.is_char_boundary(s.end));
        if !ok || splices.is_empty() {
            return;
        }
        let code = ui.widget(cx, ids!(score_code)).widget(cx, ids!(code));
        if let Some(mut view) = code.borrow_mut::<CodeView>() {
            if let Some(session) = view.session.as_mut() {
                let src = &self.source;
                let to_pos = |at: usize| {
                    let before = &src[..at];
                    let line = before.matches('\n').count();
                    let col = at - before.rfind('\n').map_or(0, |nl| nl + 1);
                    makepad_code_editor::text::Position { line_index: line, byte_index: col }
                };
                let ranges: Vec<_> = splices.iter().map(|s| (to_pos(s.start), to_pos(s.end), s.text.as_str().into())).collect();
                session.document().replace_ranges(&ranges);
                session.handle_changes();
            }
            view.redraw(cx);
        }
        self.source = tm::apply_all(&self.source, splices);
        self.revision += 1;
        self.code_shown = 0;
        self.send();
        self.frame = cx.new_next_frame();
    }
}
