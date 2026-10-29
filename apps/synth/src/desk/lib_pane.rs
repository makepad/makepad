//! The side panel's Library tab: the scores (unsaved drafts
//! first, then the library's files) and the samples a score can play by
//! name; and the document's files: Save, Save as, Open, Export to WAV,
//! samples added from anywhere.

use super::batch::{MusicSongs, SongRow, SongsAction};
use super::Desk;
use crate::library::{self, Document, FILE_REV};
use makepad_widgets::makepad_platform::file_dialogs::{FileDialog, FileDialogAction};
use makepad_widgets::*;
use std::path::PathBuf;

const ADD_SAMPLES: LiveId = live_id!(synth_add_samples);
const OPEN_FILE: LiveId = live_id!(synth_open_file);
const SAVE_AS: LiveId = live_id!(synth_save_as);
const EXPORT_WAV: LiveId = live_id!(synth_export_wav);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lane {
    #[default]
    Scores,
    Samples,
}

#[derive(Clone, Debug)]
enum Row {
    Draft { id: String, title: String, path: Option<PathBuf> },
    Score { title: String, path: PathBuf },
    Sample { path: PathBuf },
}

/// What the pane shows.
#[derive(Default)]
pub struct LibPane {
    lane: Lane,
    search: String,
    rows: Vec<Row>,
    /// The rows are made again on the next turn.
    pub dirty: bool,
    selected: Option<usize>,
}

impl LibPane {
    fn refresh(&mut self, open: &Document) {
        let want = self.search.to_lowercase();
        let hit = |text: &str| want.is_empty() || text.to_lowercase().contains(&want);
        self.rows = match self.lane {
            Lane::Scores => {
                let mut rows: Vec<Row> = library::drafts().into_iter().filter(|(id, title, _)| *id != open.id && hit(title))
                    .map(|(id, title, path)| Row::Draft { id, title, path }).collect();
                rows.extend(library::saved_scores().into_iter().filter(|(title, _)| hit(title)).map(|(title, path)| Row::Score { title, path }));
                rows
            }
            Lane::Samples => library::samples().into_iter().filter(|p| hit(&p.to_string_lossy())).map(|path| Row::Sample { path }).collect(),
        };
        self.selected = None;
    }

    fn row_view(row: &Row, open: &Document) -> SongRow {
        let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match row {
            Row::Draft { title, path, .. } => SongRow {
                title: title.clone(),
                detail: match path {
                    Some(p) => format!("Edited, not saved: {}", name(p)),
                    None => "A draft, never saved".into(),
                },
                facts: "draft".into(),
            },
            Row::Score { title, path } => SongRow {
                title: title.clone(),
                detail: name(path),
                facts: if open.path.as_ref() == Some(path) { "open".into() } else { String::new() },
            },
            Row::Sample { path } => SongRow {
                title: name(path),
                detail: if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sf2")) { "a SoundFont: double-click puts it at the cursor".into() } else { "audio: double-click puts it at the cursor".into() },
                facts: String::new(),
            },
        }
    }
}

/// A file dialog for score files.
fn score_dialog(id: LiveId, title: &str) -> FileDialog {
    FileDialog::new().set_id(id).set_title(title.to_string()).set_location(crate::paths::scores_dir()).add_filter("Scores".to_string(), vec!["score".into(), "splash".into()])
}

impl Desk {
    pub(super) fn handle_library_actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) {
        // The library's lanes: its scores, its samples, and the batch (songs
        // written at once, with their ratings).
        let lanes = ui.widget(cx, ids!(lib_lanes));
        for action in actions.filter_widget_actions_cast::<super::tabs::TabsAction>(lanes.widget_uid()) {
            if let super::tabs::TabsAction::Selected(index) = action {
                let batch = index == 2;
                ui.view(cx, ids!(lib_browse)).set_visible(cx, !batch);
                ui.view(cx, ids!(score_batch)).set_visible(cx, batch);
                if !batch {
                    let lane = if index == 0 { Lane::Scores } else { Lane::Samples };
                    self.lib.lane = lane;
                    self.lib.dirty = true;
                    ui.text_input(cx, ids!(lib_search)).set_empty_text(cx, if lane == Lane::Scores { "Search scores".into() } else { "Search samples".into() });
                    ui.button(cx, ids!(lib_add)).set_visible(cx, lane == Lane::Samples);
                }
                ui.redraw(cx);
            }
        }
        if let Some(text) = ui.text_input(cx, ids!(lib_search)).changed(actions) {
            self.lib.search = text;
            self.lib.dirty = true;
        }
        if ui.button(cx, ids!(lib_add)).clicked(actions) {
            cx.open_select_file_dialog(FileDialog::new().set_id(ADD_SAMPLES).set_title("Add samples".to_string()).set_multiple(true)
                .add_filter("Audio and SoundFonts".to_string(), vec!["wav".into(), "mp3".into(), "ogg".into(), "flac".into(), "sf2".into()]));
        }
        if ui.button(cx, ids!(score_save)).clicked(actions) {
            self.save_now(cx, ui);
        }
        if ui.button(cx, ids!(score_save_as)).clicked(actions) {
            cx.open_save_file_dialog(score_dialog(SAVE_AS, "Save the score as").set_filename(format!("{}.score", self.document.title)));
        }
        if ui.button(cx, ids!(score_open_file)).clicked(actions) {
            cx.open_select_file_dialog(score_dialog(OPEN_FILE, "Open a score"));
        }
        if ui.button(cx, ids!(score_export)).clicked(actions) {
            cx.open_save_file_dialog(FileDialog::new().set_id(EXPORT_WAV).set_title("Export the score to WAV".to_string()).set_location(crate::paths::renders_dir())
                .set_filename(format!("{}.wav", self.document.title)).add_filter("WAV audio".to_string(), vec!["wav".into()]));
        }
        for action in actions {
            match action.downcast_ref::<FileDialogAction>() {
                Some(FileDialogAction::FileSelected { id, paths }) if *id == ADD_SAMPLES => {
                    let dir = crate::paths::samples_dir();
                    for path in paths {
                        if let Some(name) = path.file_name() {
                            if let Err(e) = std::fs::copy(path, dir.join(name)) {
                                ui.label(cx, ids!(score_state)).set_text(cx, &format!("{}: {e}", path.display()));
                            }
                        }
                    }
                    self.lib.dirty = true;
                }
                Some(FileDialogAction::FileSelected { id, paths }) if *id == OPEN_FILE => {
                    if let Some(path) = paths.first() {
                        match Document::open_file(path) {
                            Ok((document, source)) => self.open_document(cx, ui, document, source),
                            Err(why) => ui.label(cx, ids!(score_state)).set_text(cx, &why),
                        }
                    }
                }
                Some(FileDialogAction::SaveFileSelected { id, path }) if *id == SAVE_AS => {
                    let source = self.source.clone();
                    match self.document.save_to(&source, path.clone()) {
                        Ok(path) => ui.label(cx, ids!(score_state)).set_text(cx, &format!("saved as {}", path.display())),
                        Err(why) => ui.label(cx, ids!(score_state)).set_text(cx, &why),
                    }
                    self.lib.dirty = true;
                }
                Some(FileDialogAction::SaveFileSelected { id, path }) if *id == EXPORT_WAV => {
                    let (source, path) = (self.source.clone(), path.clone());
                    self.jobs.spawn(move || super::run::render_wav(&source, None, None, Some(path)));
                    ui.label(cx, ids!(score_state)).set_text(cx, "rendering…");
                    self.frame = cx.new_next_frame();
                }
                _ => {}
            }
        }
        let list = ui.widget(cx, ids!(lib_list));
        for action in actions.filter_widget_actions_cast::<SongsAction>(list.widget_uid()) {
            match action {
                SongsAction::Selected(i) => {
                    self.lib.selected = Some(i);
                    match self.lib.rows.get(i).cloned() {
                        Some(Row::Draft { id, .. }) => {
                            if let Some((document, source)) = Document::draft(&id) {
                                self.open_document(cx, ui, document, source);
                                self.lib.dirty = true;
                            }
                        }
                        Some(Row::Score { path, .. }) => match Document::open_file(&path) {
                            Ok((document, source)) => {
                                self.open_document(cx, ui, document, source);
                                self.lib.dirty = true;
                            }
                            Err(why) => ui.label(cx, ids!(score_state)).set_text(cx, &why),
                        },
                        _ => {}
                    }
                }
                // A sample: its reference at the code's cursor.
                SongsAction::Play(i) => {
                    if let Some(Row::Sample { path }) = self.lib.rows.get(i) {
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let text = format!("asset({}, {})", makepad_editor_panel::fmt_text(&name), makepad_editor_panel::fmt_text(FILE_REV));
                        let at = ui.widget(cx, ids!(score_code)).borrow::<super::code::ScoreEditor>().and_then(|editor| editor.cursor_offset(cx));
                        match at {
                            Some(at) => self.splice(cx, ui, at, at, &text),
                            None => ui.label(cx, ids!(score_state)).set_text(cx, "click in the code where the sample goes, then double-click it"),
                        }
                    }
                }
                SongsAction::None => {}
            }
        }
    }

    /// Save the open score (⌘S): its file, or a new one in the library.
    pub(super) fn save_now(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        match self.save(None) {
            Ok(path) => ui.label(cx, ids!(score_state)).set_text(cx, &format!("saved {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
            Err(why) => ui.label(cx, ids!(score_state)).set_text(cx, &why),
        }
        self.lib.dirty = true;
    }

    /// The rows, made again when asked.
    pub(super) fn pump_library(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if !self.lib.dirty {
            return;
        }
        self.lib.dirty = false;
        self.lib.refresh(&self.document);
        let rows: Vec<SongRow> = self.lib.rows.iter().map(|r| LibPane::row_view(r, &self.document)).collect();
        if let Some(mut list) = ui.widget(cx, ids!(lib_list)).borrow_mut::<MusicSongs>() {
            list.set_rows(cx, rows, self.lib.selected, None);
        }
    }
}
