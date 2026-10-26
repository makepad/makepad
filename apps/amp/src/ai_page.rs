//! The AI music page: a tab of the list area beside Tracks and Equalizer
//! (`ai_music.rs` has the mode itself), and the AI pages of the side panel.
//!
//! 1. **The prompt** across the top, many lines; beside it the length, the
//!    model, **Generate now** (also ⌘↩ in the prompt), **Auto** and Skip.
//!    Generate now makes one song from the prompt right away; it plays next
//!    (at once when nothing plays). Auto: every time the last song of the
//!    AI list starts playing, one more is made from the prompt and goes at
//!    the end, so the list never runs dry; off, nothing is made on its own.
//!    Skip is the player's Next.
//! 2. **The list**: the AI songs in the playlist's table (`ai_table.rs`), in
//!    play order, then the songs being made with their step and progress.
//!    Double-click or Return plays the list from that song; songs Auto makes
//!    join the playing list at its end.
//! 3. **The side panel** (beside the list area, where "Next in queue" is):
//!    while an AI song plays or this page is open it has the pages Queue,
//!    Lyrics (the playing song's lyrics in time with the music, each line
//!    with its translation, `lyrics_view.rs`; a picked row shows that song),
//!    Song (its style and what made it) and Writer chat. An AI song that
//!    starts opens the panel on its lyrics.

use crate::ai_music::{clock, created_secs, created_text, uri_of, AiCommand, AiEvent, Making, Place, ReadySong, Side, SongPrompt, DEFAULT_REQUEST, LANGUAGES, LENGTHS};
use crate::ai_table::{AiTableRow, AiTableWidgetRefExt};
use crate::layout::HwStageWidgetRefExt;
use crate::local::LocalCommand;
use crate::lyrics_view::HwLyricsWidgetRefExt;
use crate::model::Engine;
use crate::queue::QueueItem;
use crate::source::Transport;
use crate::App;
use makepad_widgets::*;
use std::sync::atomic::Ordering;

/// One row of the list: a made song (an index into `AiMode::songs`) or a
/// song being made (its job; its written song once there is one).
enum RowOf {
    Made(usize),
    Making(u64),
}

impl App {
    fn ai_epoch(&self) -> u64 {
        self.workers.as_ref().map_or(0, |w| w.ai_epoch.load(Ordering::SeqCst))
    }

    /// Cancel what the worker runs (each job is cancelled on its node).
    fn ai_cancel_running(&mut self) {
        if let Some(workers) = &self.workers {
            workers.ai_epoch.fetch_add(1, Ordering::SeqCst);
        }
        self.ai.jobs.clear();
        self.model.requests.redraw = true;
    }

    /// Make one song from what the prompt says now; `place`: where it goes
    /// in the play order once made.
    fn ai_generate(&mut self, cx: &mut Cx, place: Place) {
        let mut request = self.ui.text_input(cx, ids!(ai_prompt)).text();
        if request.trim().is_empty() {
            request = DEFAULT_REQUEST.to_string();
        }
        if self.ai.models.is_empty() {
            if let Some(workers) = &self.workers {
                workers.ai_music(AiCommand::Models);
            }
        }
        let epoch = self.ai_epoch();
        let id = self.ai.next_id;
        let Some(workers) = &self.workers else { return };
        if workers.ai_music(AiCommand::Next { ids: vec![id], request, model: self.ai.model.clone(), minutes: self.ai.minutes, language: self.ai.language.clone(), epoch }) {
            self.ai.next_id += 1;
            self.ai.error = None;
            let now = Self::now(cx);
            self.ai.jobs.push(Making { id, place, started_at: now, stage: "writing".to_string(), permille: 0, at_work: String::new(), song: None });
            self.ai_animate(cx);
        } else {
            self.ai.error = Some("The AI music worker is busy; try again in a moment".into());
        }
        self.model.requests.redraw = true;
    }

    /// One message from the ai-music worker.
    pub(crate) fn ai_message(&mut self, cx: &mut Cx, event: AiEvent) {
        let epoch = self.ai_epoch();
        match event {
            AiEvent::Models(models) => {
                self.ai.models = models;
                if !self.ai.models.iter().any(|m| m.id == self.ai.model) {
                    if let Some(first) = self.ai.models.first() {
                        self.ai.model = first.id.clone();
                    }
                }
                self.ai_models_shown = false;
            }
            AiEvent::Restored { request, model, minutes, language } => {
                self.ai.minutes = minutes;
                self.ai.language = language;
                let field = self.ui.text_input(cx, ids!(ai_prompt));
                if field.text().is_empty() || field.text() == DEFAULT_REQUEST {
                    field.set_text(cx, &request);
                }
                self.ai.model = model;
                self.ai_models_shown = false;
            }
            AiEvent::Conversation(text) => self.ai.conversation = text,
            AiEvent::Songs(songs) => self.ai.songs = songs,
            AiEvent::Where { epoch: at, id, text } if at == epoch => {
                if let Some(job) = self.ai.job_mut(id) {
                    job.at_work = text;
                }
            }
            AiEvent::Stage { epoch: at, id, text, permille } if at == epoch => {
                if let Some(job) = self.ai.job_mut(id) {
                    job.stage = text;
                    job.permille = permille;
                }
            }
            AiEvent::Song { epoch: at, id, index, song } if at == epoch => {
                if let Some(job) = self.ai.job_mut(id) {
                    job.song = Some((index, song));
                }
            }
            AiEvent::Ready { epoch: at, id, song } if at == epoch => {
                let place = self.ai.jobs.iter().find(|j| j.id == id).map_or(Place::End, |j| j.place);
                self.ai.jobs.retain(|j| j.id != id);
                self.ai_file_ready(&song);
                self.ai.ready.push((song, place));
            }
            AiEvent::Failed { epoch: at, id, error } if at == epoch => {
                self.ai.jobs.retain(|j| j.id != id);
                // "unavailable: no GPU hub ..." reads as a sentence.
                let mut error = error.strip_prefix("unavailable: ").unwrap_or(&error).to_string();
                if let Some(first) = error.get(..1) {
                    error = first.to_uppercase() + &error[1..];
                }
                self.ai.error = Some(error);
            }
            AiEvent::Cancelled { epoch: at, id } if at == epoch => self.ai.jobs.retain(|j| j.id != id),
            // A Next cancelled since: its file (if any) stays in the library.
            AiEvent::Ready { song, .. } => self.ai_file_ready(&song),
            AiEvent::Stage { .. } | AiEvent::Song { .. } | AiEvent::Failed { .. } | AiEvent::Cancelled { .. } | AiEvent::Where { .. } => {}
        }
        self.model.requests.redraw = true;
    }

    /// A song file was written: the library lists its folder (added the
    /// first time) and reads the file now.
    fn ai_file_ready(&mut self, song: &ReadySong) {
        let Some(workers) = &self.workers else { return };
        let folder = std::path::Path::new(&song.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let command = if self.model.folders.iter().any(|f| *f == folder) { LocalCommand::FileAdded } else { LocalCommand::AddFolder(folder) };
        if !workers.local(command) {
            log!("amp: ai-music: the folder reader is busy; the song shows with its next scan");
        }
    }

    /// The AI list as queue items, oldest first: the made songs the library
    /// lists (a file it has not read yet cannot play).
    fn ai_items(&self) -> Vec<QueueItem> {
        self.ai.made().map(ai_item).filter(|item| self.model.index.track(&item.uri).is_some()).collect()
    }

    /// After every event: an AI song that starts opens the side panel on its
    /// lyrics and, being the last of the list with Auto on, asks for the
    /// next one; a made song the library lists goes where it was made for;
    /// the model knows whether to hold the end of the playing song.
    pub(crate) fn ai_tick(&mut self, cx: &mut Cx) {
        let uri = self.model.playback.uri().map(str::to_string);
        let playing_ai = uri.as_deref().is_some_and(|u| self.ai.is_ai(u));
        // A song that starts, or one that turns out to be an AI song once
        // the worker's song list names its file (a just-made song can start
        // before that).
        if uri != self.ai.last_uri || playing_ai != self.ai.last_was_ai {
            self.ai.last_uri = uri.clone();
            if playing_ai {
                // The side panel follows the song that starts.
                self.ai.selected = None;
                if !self.ai.last_was_ai {
                    self.ai.side = Side::Lyrics;
                    if !self.up_next_open {
                        self.up_next_open = true;
                        self.apply_up_next(cx);
                        self.save_up_next(cx);
                    }
                }
                if self.ai.auto && self.ai_last_in_play() && !self.ai.auto_pending() {
                    self.ai_generate(cx, Place::End);
                }
            }
            self.ai.last_was_ai = playing_ai;
            self.model.requests.redraw = true;
        }
        // While a song is made, the playing AI song's end plays again rather
        // than silence when nothing follows it.
        self.model.ai_hold = playing_ai && self.ai.busy() && self.model.engine == Engine::Local;
        // Made songs the library lists go where they were made for, in the
        // order made.
        while let Some((first, _)) = self.ai.ready.first() {
            let item = QueueItem { uri: uri_of(&first.path), name: first.song.title.clone(), artist: format!("AI · {}", first.song.model) };
            if self.model.index.track(&item.uri).is_none() {
                break;
            }
            let (_, place) = self.ai.ready.remove(0);
            // Nothing plays (or it played out): the AI list plays from it.
            if self.model.playback.uri().is_none() || self.model.local_done {
                let mut items = self.ai_items();
                let start = match items.iter().position(|i| i.uri == item.uri) {
                    Some(start) => start,
                    None => {
                        items.push(item);
                        items.len() - 1
                    }
                };
                self.model.ai_play(items, start);
                continue;
            }
            match place {
                Place::Next => self.model.ai_play_next(item),
                Place::End => self.model.ai_append(item),
            }
        }
    }

    /// Play the AI list from row `at` of the table.
    fn ai_play_row(&mut self, at: usize) {
        let Some(RowOf::Made(index)) = self.ai_rows_of().into_iter().nth(at) else { return };
        let Some(song) = self.ai.songs.get(index) else { return };
        let uri = uri_of(&song.path);
        let items = self.ai_items();
        match items.iter().position(|i| i.uri == uri) {
            Some(start) => self.model.ai_play(items, start),
            None => {
                if let Some(workers) = &self.workers {
                    workers.local(LocalCommand::FileAdded);
                }
                self.ai.error = Some(format!("“{}” is not in the library yet; try again in a moment", song.title));
            }
        }
        self.model.requests.redraw = true;
    }

    /// The page's and the side panel's controls.
    pub(crate) fn ai_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let now = self.ui.button(cx, ids!(ai_now)).clicked(actions) || self.ui.button(cx, ids!(ai_now_icon)).clicked(actions);
        // ⌘↩ in the prompt (a plain ↩ is a new line).
        let returned = self.ui.text_input(cx, ids!(ai_prompt)).returned(actions).is_some();
        if now || returned {
            self.ai_generate(cx, Place::Next);
        }
        if let Some(auto) = self.ui.check_box(cx, ids!(ai_auto)).changed(actions) {
            self.ai.auto = auto;
            // Turned on while the last song plays: its follower starts now.
            if auto && self.ai_last_in_play() && !self.ai.auto_pending() {
                self.ai_generate(cx, Place::End);
            }
            self.ai_save(cx);
            self.model.requests.redraw = true;
        }
        let skip = self.ui.button(cx, ids!(ai_skip)).clicked(actions) || self.ui.button(cx, ids!(ai_skip_icon)).clicked(actions);
        if skip {
            self.model.next();
            self.model.requests.redraw = true;
        }
        if self.ui.button(cx, ids!(ai_cancel)).clicked(actions) {
            self.ai_cancel_running();
        }
        if let Some(at) = self.ui.drop_down(cx, ids!(ai_length)).selected(actions) {
            // From the next song on.
            self.ai.minutes = LENGTHS.get(at).copied().unwrap_or(LENGTHS[0]);
            self.model.requests.redraw = true;
        }
        if let Some(at) = self.ui.drop_down(cx, ids!(ai_model)).selected(actions) {
            if let Some(model) = self.ai.models.get(at) {
                self.ai.model = model.id.clone();
                self.model.requests.redraw = true;
            }
        }
        if let Some(at) = self.ui.drop_down(cx, ids!(ai_language)).selected(actions) {
            // From the next song on (the writer translates as it writes).
            self.ai.language = LANGUAGES.get(at).unwrap_or(&LANGUAGES[0]).to_string();
            self.model.requests.redraw = true;
        }
        if let Some(at) = self.ui.hw_lyrics(cx, ids!(ai_lyrics)).seek(actions) {
            self.model.transport(Transport::Seek((at * 1000.0) as u32));
            self.model.requests.redraw = true;
        }
        let table = self.ui.ai_table(cx, ids!(ai_table));
        if let Some(at) = table.activated(actions) {
            self.ai_play_row(at);
        }
        if let Some((rows, window)) = table.drag_out(actions) {
            // AI songs are local Ogg files: they go as tagged MP3s with their
            // lyrics. A song still being made has no file yet.
            let of = self.ai_rows_of();
            let paths: Vec<Option<String>> = rows
                .into_iter()
                .filter_map(|at| match of.get(at) {
                    Some(RowOf::Made(index)) => Some(self.ai.songs.get(*index).map(|song| song.path.clone())),
                    _ => None,
                })
                .collect();
            if !paths.is_empty() {
                self.drag_out(cx, window, paths);
            }
        }
        if table.layout_changed(actions) {
            self.ai_save(cx);
        }
        if let Some(at) = table.picked(actions) {
            let picked = match self.ai_rows_of().into_iter().nth(at) {
                Some(RowOf::Made(index)) => Some(index),
                Some(RowOf::Making(id)) => self.ai.jobs.iter().find(|j| j.id == id).and_then(|j| j.song.as_ref().map(|(index, _)| *index)),
                None => None,
            };
            self.ai.selected = picked;
            if self.ai.side == Side::Chat || self.ai.side == Side::Queue {
                self.ai.side = Side::Lyrics;
            }
            self.model.requests.redraw = true;
        }
        for (key, side) in [(ids!(side_queue), Side::Queue), (ids!(ai_show_lyrics), Side::Lyrics), (ids!(ai_show_song), Side::Song), (ids!(ai_show_chat), Side::Chat)] {
            if self.ui.button(cx, key).clicked(actions) {
                self.ai.side = side;
                self.model.requests.redraw = true;
            }
        }
        if self.ui.button(cx, ids!(ai_new)).clicked(actions) {
            if let Some(workers) = &self.workers {
                workers.ai_music(AiCommand::NewConversation);
            }
        }
    }

    /// Whether the playing song is an AI song and no AI song follows it in
    /// the play queue: Auto makes the next one then. The play order, not the
    /// table's sort, decides.
    fn ai_last_in_play(&self) -> bool {
        let Some(uri) = self.model.playback.uri() else { return false };
        self.ai.is_ai(uri) && !self.model.queue.upcoming().any(|(_, item)| self.ai.is_ai(&item.uri))
    }

    /// Keep Auto and the table's columns and sort for the next run.
    fn ai_save(&mut self, cx: &mut Cx) {
        let layout = self.ui.ai_table(cx, ids!(ai_table)).layout_text();
        self.save(crate::worker::Setting::AiMusic, format!("auto {}\n{layout}", self.ai.auto as u8));
    }

    /// Read back what [`Self::ai_save`] kept.
    pub(crate) fn ai_restore(&mut self, cx: &mut Cx, text: &str) {
        let mut layout = String::new();
        for line in text.lines() {
            match line.strip_prefix("auto ") {
                Some(on) => self.ai.auto = on.trim() == "1",
                None => {
                    layout.push_str(line);
                    layout.push('\n');
                }
            }
        }
        self.ui.ai_table(cx, ids!(ai_table)).restore_layout(cx, &layout);
        self.model.requests.redraw = true;
    }

    /// The rows in play order: the AI list (oldest first), then the songs
    /// being made, oldest first.
    fn ai_rows_of(&self) -> Vec<RowOf> {
        let made = self.ai.songs.iter().enumerate().filter(|(_, s)| !s.path.is_empty()).map(|(at, _)| RowOf::Made(at));
        made.chain(self.ai.jobs.iter().map(|j| RowOf::Making(j.id))).collect()
    }

    /// Whether the side panel offers its AI pages: an AI song plays, or the
    /// AI page is open.
    fn ai_side_offered(&mut self, cx: &mut Cx) -> bool {
        let playing_ai = self.model.playback.uri().is_some_and(|u| self.ai.is_ai(u));
        playing_ai || self.ui.hw_stage(cx, ids!(stage)).panels().1 == "ai"
    }

    /// The page and the side panel, from the mode. Labels are only set when
    /// their text changed.
    pub(crate) fn update_ai_page(&mut self, cx: &mut Cx) {
        let now = Self::now(cx);
        let error = self.ai.error.clone().unwrap_or_default();
        self.set_visible(cx, ids!(ai_status), !error.is_empty());
        self.set_label(cx, live_id!(ai_status), ids!(ai_status), &error);
        let busy = self.ai.busy();
        self.set_visible(cx, ids!(ai_cancel_cap), busy);
        let auto = self.ui.check_box(cx, ids!(ai_auto));
        if auto.active(cx) != self.ai.auto {
            auto.set_active(cx, self.ai.auto, Animate::No);
        }
        if !self.ai_models_shown {
            self.ai_models_shown = true;
            let mut labels: Vec<String> = self.ai.models.iter().map(|m| m.label()).collect();
            if labels.is_empty() {
                labels.push(crate::ai_music::MusicModel { id: self.ai.model.clone(), restriction: crate::ai_music::restriction_of(&self.ai.model).into() }.label());
            }
            let picker = self.ui.drop_down(cx, ids!(ai_model));
            picker.set_labels(cx, labels);
            let at = self.ai.models.iter().position(|m| m.id == self.ai.model).unwrap_or(0);
            picker.set_selected_item(cx, at);
            let length = self.ui.drop_down(cx, ids!(ai_length));
            let at = LENGTHS.iter().position(|m| *m == self.ai.minutes).unwrap_or(0);
            if length.selected_item() != at {
                length.set_selected_item(cx, at);
            }
            let language = self.ui.drop_down(cx, ids!(ai_language));
            let at = LANGUAGES.iter().position(|l| *l == self.ai.language).unwrap_or(0);
            if language.selected_item() != at {
                language.set_selected_item(cx, at);
            }
        }

        // The list.
        let playing = self.model.playback.uri().map(str::to_string);
        let rows: Vec<AiTableRow> = self
            .ai_rows_of()
            .into_iter()
            .enumerate()
            .filter_map(|(n, row)| match row {
                RowOf::Made(index) => {
                    let song = self.ai.songs.get(index)?;
                    Some(AiTableRow {
                        num: (n + 1).to_string(),
                        title: song.title.clone(),
                        style: song.prompt.clone(),
                        model: song.model.clone(),
                        created: created_secs(&song.path).map(created_text),
                        created_secs: created_secs(&song.path),
                        length: song.length,
                        time: (song.length > 0.0).then(|| clock(song.length)),
                        status: String::new(),
                        progress: None,
                        current: playing.as_deref() == Some(uri_of(&song.path).as_str()),
                        running: false,
                    })
                }
                RowOf::Making(id) => {
                    let job = self.ai.jobs.iter().find(|j| j.id == id)?;
                    let (title, style, model) = match &job.song {
                        Some((_, song)) => (song.title.clone(), song.prompt.clone(), song.model.clone()),
                        None => ("New song".to_string(), "being written from your prompt".to_string(), self.ai.model.clone()),
                    };
                    Some(AiTableRow {
                        num: (n + 1).to_string(),
                        title,
                        style,
                        model,
                        created: None,
                        created_secs: None,
                        length: 0.0,
                        time: None,
                        status: job.status(now),
                        progress: Some(job.permille as f64 / 1000.0),
                        current: false,
                        running: true,
                    })
                }
            })
            .collect();
        self.set_visible(cx, ids!(ai_empty), rows.is_empty());
        self.ui.ai_table(cx, ids!(ai_table)).set_rows(cx, rows);

        // The side panel: the queue, or (while an AI song plays or this page
        // is open) the page picked.
        let offered = self.ai_side_offered(cx);
        let side = if offered { self.ai.side } else { Side::Queue };
        self.set_visible(cx, ids!(side_tabs), offered);
        for (cap, page) in [(ids!(side_queue_cap), Side::Queue), (ids!(ai_show_lyrics_cap), Side::Lyrics), (ids!(ai_show_song_cap), Side::Song), (ids!(ai_show_chat_cap), Side::Chat)] {
            self.ui.relief_view(cx, cap).set_lit(cx, if side == page { 1.0 } else { 0.0 });
        }
        self.set_visible(cx, ids!(up_next), side == Side::Queue);
        self.set_visible(cx, ids!(ai_lyrics_pane), side == Side::Lyrics);
        self.set_visible(cx, ids!(ai_detail), side == Side::Song || side == Side::Chat);
        self.set_visible(cx, ids!(ai_new_cap), side == Side::Chat);
        // The picked song, else the playing AI song, else the one being
        // made, else the newest.
        let shown = self
            .ai
            .selected
            .and_then(|at| self.ai.songs.get(at))
            .or_else(|| self.ai.made().find(|s| playing.as_deref() == Some(uri_of(&s.path).as_str())))
            .or_else(|| self.ai.jobs.first().and_then(|j| j.song.as_ref().map(|(_, s)| s)))
            .or_else(|| self.ai.made().last())
            .cloned();
        self.update_lyrics(cx, shown.as_ref(), playing.as_deref());
        let (title, meta, body) = if side == Side::Chat {
            (
                "The writer conversation".to_string(),
                "One conversation for all songs (kept between runs), so the writer remembers what it already wrote.".to_string(),
                if self.ai.conversation.is_empty() { "Nothing yet.".to_string() } else { self.ai.conversation.clone() },
            )
        } else {
            match &shown {
                Some(song) => inspect_song(song),
                None => ("Song".to_string(), "Pick a song to see its style tags and what made it.".to_string(), String::new()),
            }
        };
        self.set_label(cx, live_id!(ai_detail_title), ids!(ai_detail_title), &title);
        self.set_label(cx, live_id!(ai_detail_meta), ids!(ai_detail_meta), &meta);
        self.set_label(cx, live_id!(ai_detail_body), ids!(ai_detail_body), &body);
    }

    /// The lyrics view: the shown song's timed lyrics, where playback is in
    /// it, what the times come from and the translation.
    fn update_lyrics(&mut self, cx: &mut Cx, song: Option<&SongPrompt>, playing_uri: Option<&str>) {
        let now = Self::now(cx);
        let view = self.ui.hw_lyrics(cx, ids!(ai_lyrics));
        let Some(song) = song else {
            view.set_lyrics(cx, "", None);
            self.set_label(cx, live_id!(ai_lyrics_title), ids!(ai_lyrics_title), "Lyrics");
            self.set_label(cx, live_id!(ai_lyrics_info), ids!(ai_lyrics_info), "The lyrics of the playing AI song show here, in time with the music.");
            return;
        };
        let live = !song.path.is_empty() && playing_uri == Some(uri_of(&song.path).as_str());
        let key = if song.path.is_empty() { format!("making:{}", song.title) } else { song.path.clone() };
        let timed = song.timed_lyrics();
        let state = if song.path.is_empty() {
            "Being made".to_string()
        } else if live {
            if self.model.playback.is_playing() { "Playing".to_string() } else { "Paused".to_string() }
        } else {
            "Not playing (double-click it in the list)".to_string()
        };
        let mut info = vec![state];
        match (song.lyrics.trim().is_empty(), song.timing.as_deref()) {
            (true, _) => info.push("instrumental: no lyrics".to_string()),
            (false, Some("score")) => info.push("timed by the song's score".to_string()),
            (false, Some(_)) => info.push("times estimated".to_string()),
            (false, None) if song.path.is_empty() => {}
            (false, None) => info.push("times estimated (an older song)".to_string()),
        }
        match (&song.translation, &song.language) {
            (Some(_), Some(language)) => info.push(format!("translated to {language}")),
            (None, Some(language)) if !song.lyrics.trim().is_empty() => info.push(format!("already in {language}")),
            _ => {}
        }
        self.set_label(cx, live_id!(ai_lyrics_title), ids!(ai_lyrics_title), &song.title);
        self.set_label(cx, live_id!(ai_lyrics_info), ids!(ai_lyrics_info), &info.join(" · "));
        view.set_lyrics(cx, &key, timed);
        let position = if live { self.model.playback.position_ms(now).unwrap_or(0.0) / 1000.0 } else { 0.0 };
        view.set_clock(cx, live, position, live && self.model.playback.is_playing());
    }

    /// While a song is made and the page is open, its progress and elapsed
    /// time move: a frame every 1/15 s for the page alone.
    fn ai_animate(&mut self, cx: &mut Cx) {
        if self.ai.busy() {
            self.ai_frame = cx.new_next_frame();
        }
    }

    /// The progress frames and the keys' tooltips.
    pub(crate) fn ai_page_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Some(frame) = self.ai_frame.is_event(event) {
            if self.ai.busy() {
                let page_shown = self.ui.hw_stage(cx, ids!(stage)).panels().1 == "ai";
                if page_shown && frame.time - self.ai_frame_at >= 1.0 / 15.0 {
                    self.ai_frame_at = frame.time;
                    self.update_ai_page(cx);
                    self.ui.widget(cx, ids!(ai_page)).redraw(cx);
                }
                self.ai_frame = cx.new_next_frame();
            }
        }
        let Event::MouseMove(e) = event else { return };
        if self.ui.window(cx, ids!(main_window)).window_id() != Some(e.window_id) {
            return;
        }
        let tips: [(&[LiveId], &str); 13] = [
            (ids!(side_queue_cap), "Next in queue: what plays after this song"),
            (ids!(ai_show_lyrics_cap), "The lyrics in time with the music, with their translation; click a line to play from there"),
            (ids!(ai_language), "The language the writer translates the lyrics into, from the next song on"),
            (ids!(ai_now_cap), "Generate now (⌘↩): make one song from the prompt on your AI hub's GPUs; it plays next"),
            (ids!(ai_auto), "Auto: when the last song of the list starts playing, one more is made from the prompt and goes at the end"),
            (ids!(ai_cancel_cap), "Stop making the songs being made (made ones stay in the list)"),
            (ids!(ai_model), "The music model for the next song. YuE2-3B's weights are CC BY-NC 4.0: personal listening only"),
            (ids!(ai_skip_cap), "Skip to the next song"),
            (ids!(ai_length), "How long the next song is (the writer sizes its lyrics to it)"),
            (ids!(ai_prompt), "What to hear. Edits apply from the next song made; the playing song is never interrupted"),
            (ids!(ai_show_song_cap), "The song's style tags, length, what made it and its file"),
            (ids!(ai_show_chat_cap), "The conversation with the song writer (Claude Code)"),
            (ids!(ai_new_cap), "Start a new writer conversation (the old one is kept on disk)"),
        ];
        let mut over = None;
        for (path, tip) in tips {
            let widget = self.ui.widget(cx, path);
            let area = widget.area();
            if widget.visible() && area.is_valid(cx) && area.rect(cx).contains(e.abs) {
                over = Some((area.rect(cx), tip.to_string()));
                break;
            }
        }
        if over.as_ref().map(|(_, t)| t.as_str()) == self.ai_tip_shown.as_deref() {
            return;
        }
        self.ai_tip_shown = over.as_ref().map(|(_, t)| t.clone());
        let mut tip = self.ui.callout_tooltip(cx, ids!(ai_tip));
        match over {
            Some((rect, text)) => tip.show_with_options(cx, &text, rect, CalloutTooltipOptions { position: TooltipPosition::Bottom, ..Default::default() }),
            None => tip.hide(cx),
        }
    }
}

/// A made song as the queue holds it.
fn ai_item(song: &SongPrompt) -> QueueItem {
    QueueItem { uri: uri_of(&song.path), name: song.title.clone(), artist: format!("AI · {}", song.model) }
}

/// A song for the inspector: title, what it is and what made it, the lyrics.
fn inspect_song(song: &SongPrompt) -> (String, String, String) {
    let mut meta = format!("Style: {}", song.prompt);
    if song.length > 0.0 {
        meta.push_str(&format!("\nLength: {}", clock(song.length)));
    }
    if !song.made_on.is_empty() {
        meta.push_str(&format!("\nMade by {}", song.made_on));
    } else {
        meta.push_str(&format!("\nModel: {} (being made)", song.model));
    }
    if !song.path.is_empty() {
        meta.push_str(&format!("\nFile: {}", song.path));
    }
    let lyrics = if song.lyrics.trim().is_empty() { "(instrumental)".to_string() } else { song.lyrics.trim().to_string() };
    (song.title.clone(), meta, lyrics)
}
