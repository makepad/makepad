//! The AI music page: a tab of the list area beside Tracks and Equalizer
//! (`ai_music.rs` has the mode itself). Its flow, top to bottom:
//!
//! 1. **Go**: what to hear, the model, Start (Stop while on);
//! 2. **running**: the song being made is the first row, from the moment
//!    Start is pressed: its step in words (writing on Claude Code / generating
//!    on which node and GPU), a progress bar, the elapsed time, Cancel and
//!    Restart;
//! 3. **output**: a made song becomes a playable row (highlighted for a
//!    moment), plays at once or next, and is listed in the sidebar's
//!    "AI music" playlist;
//! 4. **inspect**: a click on a row shows its title, style tags, lyrics and
//!    what made it on the right; "Writer chat" shows the writer conversation.

use crate::ai_music::{clock, uri_of, AiCommand, AiEvent, ReadySong, SongPrompt, AHEAD, DEFAULT_REQUEST, LENGTHS};
use crate::layout::HwStageWidgetRefExt;
use crate::local::LocalCommand;
use crate::model::Engine;
use crate::queue::QueueItem;
use crate::App;
use makepad_widgets::*;
use std::sync::atomic::Ordering;

/// Rows the page shows (the newest songs; the playlist has them all).
const ROWS: usize = 12;
/// How long a just-made song's row stays highlighted (s).
const FRESH_S: f64 = 4.0;

fn row_id(n: usize) -> LiveId {
    LiveId::from_str(&format!("ai_row_{n}"))
}

/// One row as shown.
struct Row {
    /// Index into `AiMode::songs` (None: the song being written).
    song: Option<usize>,
    num: String,
    title: String,
    style: String,
    length: String,
    status: String,
    running: bool,
    playable: bool,
}

impl App {
    fn ai_epoch(&self) -> u64 {
        self.workers.as_ref().map_or(0, |w| w.ai_epoch.load(Ordering::SeqCst))
    }

    /// Cancel what the worker runs (its job is cancelled on its node).
    fn ai_cancel_running(&mut self) {
        if let Some(workers) = &self.workers {
            workers.ai_epoch.fetch_add(1, Ordering::SeqCst);
        }
        self.ai.busy = false;
        self.ai.stage.clear();
        self.ai.at_work.clear();
        self.ai.permille = 0;
        self.ai.making = None;
    }

    /// Start making the next song with what the field says now, unless one
    /// is made, or [`AHEAD`] songs are made and waiting.
    fn ai_next(&mut self, cx: &mut Cx) {
        if self.ai.busy || self.ai.ready.len() + self.ai.queued.len() >= AHEAD {
            return;
        }
        let mut request = self.ui.text_input(cx, ids!(ai_prompt)).text();
        if request.trim().is_empty() {
            request = DEFAULT_REQUEST.to_string();
        }
        let epoch = self.ai_epoch();
        let Some(workers) = &self.workers else { return };
        if workers.ai_music(AiCommand::Next { request, model: self.ai.model.clone(), minutes: self.ai.minutes, epoch }) {
            self.ai.busy = true;
            self.ai.error = None;
            self.ai.stage.clear();
            self.ai.at_work.clear();
            self.ai.permille = 0;
            self.ai.started_at = Self::now(cx);
            self.ai_animate(cx);
        } else {
            self.ai.error = Some("The AI music worker is busy; press Restart".into());
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
            AiEvent::Restored { request, model, minutes } => {
                self.ai.minutes = minutes;
                let field = self.ui.text_input(cx, ids!(ai_prompt));
                if field.text().is_empty() || field.text() == DEFAULT_REQUEST {
                    field.set_text(cx, &request);
                }
                self.ai.model = model;
                self.ai_models_shown = false;
            }
            AiEvent::Conversation(text) => self.ai.conversation = text,
            AiEvent::Songs(songs) => self.ai.songs = songs,
            AiEvent::Where { epoch: at, text } if at == epoch => self.ai.at_work = text,
            AiEvent::Stage { epoch: at, text, permille } if at == epoch => {
                self.ai.stage = text;
                self.ai.permille = permille;
            }
            AiEvent::Song { epoch: at, song } if at == epoch => self.ai.making = Some(song),
            AiEvent::Ready { epoch: at, song } if at == epoch => {
                self.ai.busy = false;
                self.ai.making = None;
                self.ai.at_work.clear();
                self.ai.fresh = Some((uri_of(&song.path), Self::now(cx)));
                self.ai_file_ready(&song);
                self.ai.ready.push(song);
                // Two ahead: the next one starts right away.
                if self.ai.on {
                    self.ai_next(cx);
                }
            }
            AiEvent::Failed { epoch: at, error } if at == epoch => {
                self.ai.busy = false;
                self.ai.making = None;
                self.ai.at_work.clear();
                // "unavailable: no GPU hub ..." reads as a sentence.
                let error = error.strip_prefix("unavailable: ").unwrap_or(&error).to_string();
                let mut error = error;
                if let Some(first) = error.get(..1) {
                    error = first.to_uppercase() + &error[1..];
                }
                self.ai.error = Some(error);
                self.ai.on = false;
            }
            AiEvent::Cancelled { epoch: at } if at == epoch => {
                self.ai.busy = false;
                self.ai.making = None;
            }
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

    /// After every event: an AI song that started asks for the next one; a
    /// made song the library lists goes into the queue (playing at once
    /// when nothing of the AI plays); the model knows whether to hold.
    pub(crate) fn ai_tick(&mut self, cx: &mut Cx) {
        let uri = self.model.playback.uri().map(str::to_string);
        let playing_ai = uri.as_deref().is_some_and(|u| self.ai.is_ai(u));
        if uri != self.ai.last_uri {
            self.ai.last_uri = uri.clone();
            if playing_ai {
                self.ai.queued.retain(|q| Some(uri_of(&q.path)) != uri);
                self.ai.skip_waiting = false;
                if self.ai.on {
                    self.ai_next(cx);
                }
                self.model.requests.redraw = true;
            }
        }
        self.model.ai_hold = self.ai.on && playing_ai && self.model.engine == Engine::Local;
        if !self.ai.on {
            return;
        }
        // The transport's Next with nothing made after the playing song.
        if self.model.ai_waiting && playing_ai {
            self.ai.skip_waiting = true;
        }
        // Keep two made ahead (a failed one waits for Start or Restart).
        if self.ai.error.is_none() {
            self.ai_next(cx);
        }
        // Made songs the library lists go into the queue, in the order made
        // (after the ones queued before).
        while let Some(first) = self.ai.ready.first() {
            let uri = uri_of(&first.path);
            if self.model.index.track(&uri).is_none() {
                break;
            }
            let ready = self.ai.ready.remove(0);
            let now = !playing_ai || self.model.ai_waiting;
            let item = QueueItem { uri, name: ready.song.title.clone(), artist: format!("AI · {}", ready.song.model) };
            let at = self.ai.queued.len();
            self.ai.queued.push(ready);
            self.model.ai_enqueue(item, at, now);
        }
    }

    /// Play song `at` of the list now.
    fn ai_play(&mut self, at: usize) {
        let Some(song) = self.ai.songs.get(at) else { return };
        let uri = uri_of(&song.path);
        if song.path.is_empty() {
            return;
        }
        if self.model.index.track(&uri).is_none() {
            if let Some(workers) = &self.workers {
                workers.local(LocalCommand::FileAdded);
            }
            self.ai.error = Some(format!("“{}” is not in the library yet; try again in a moment", song.title));
            return;
        }
        let item = QueueItem { uri, name: song.title.clone(), artist: format!("AI · {}", song.model) };
        self.model.ai_enqueue(item, 0, true);
    }

    /// The page's controls.
    pub(crate) fn ai_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let go = self.ui.button(cx, ids!(ai_go)).clicked(actions) || self.ui.button(cx, ids!(ai_go_icon)).clicked(actions);
        if go {
            self.ai.on = !self.ai.on;
            if self.ai.on {
                if self.ai.models.is_empty() {
                    if let Some(workers) = &self.workers {
                        workers.ai_music(AiCommand::Models);
                    }
                }
                self.ai.error = None;
                self.ai_next(cx);
            } else if self.ai.busy {
                // Stop: the song being made is cancelled; what is made stays.
                self.ai_cancel_running();
            }
            self.model.requests.redraw = true;
        }
        let skip = self.ui.button(cx, ids!(ai_skip)).clicked(actions) || self.ui.button(cx, ids!(ai_skip_icon)).clicked(actions);
        if skip {
            self.ai_skip(cx);
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
        let rows = self.ai_rows();
        for (n, row) in rows.iter().enumerate().take(ROWS) {
            let id = row_id(n);
            if self.ui.button(cx, &[id, live_id!(cancel)]).clicked(actions) {
                self.ai_cancel_running();
                self.ai.on = false;
                self.ai.error = Some("Cancelled. Press Start to make a song again.".into());
                self.model.requests.redraw = true;
            }
            if self.ui.button(cx, &[id, live_id!(restart)]).clicked(actions) {
                self.ai_restart(cx);
            }
            let play = self.ui.button(cx, &[id, live_id!(play)]).clicked(actions) || self.ui.button(cx, &[id, live_id!(play_icon)]).clicked(actions);
            if play {
                if let Some(at) = row.song {
                    self.ai_play(at);
                }
            }
            if self.ui.view(cx, &[id]).finger_up(actions).is_some_and(|e| e.is_over) {
                self.ai.selected = row.song;
                self.ai.show_conversation = false;
                self.model.requests.redraw = true;
            }
        }
        if self.ui.button(cx, ids!(ai_show_song)).clicked(actions) {
            self.ai.show_conversation = false;
            self.model.requests.redraw = true;
        }
        if self.ui.button(cx, ids!(ai_show_chat)).clicked(actions) {
            self.ai.show_conversation = true;
            self.model.requests.redraw = true;
        }
        if self.ui.button(cx, ids!(ai_new)).clicked(actions) {
            if let Some(workers) = &self.workers {
                workers.ai_music(AiCommand::NewConversation);
            }
        }
    }

    /// Skip: the next made song now; none made yet, it starts the moment it
    /// is (the playing one goes on meanwhile). Each skip keeps two ahead.
    fn ai_skip(&mut self, cx: &mut Cx) {
        if !self.ai.on {
            self.ai.on = true;
        }
        if self.ai.queued.is_empty() {
            self.ai.skip_waiting = true;
            self.model.ai_waiting = true;
        } else {
            self.model.next();
        }
        self.ai_next(cx);
        self.model.requests.redraw = true;
    }

    /// Restart: the song being made (or waiting as next) makes way for one
    /// written from the prompt as it is now. Made files stay in the library.
    fn ai_restart(&mut self, cx: &mut Cx) {
        self.ai_cancel_running();
        let dropped: Vec<String> = self.ai.queued.drain(..).map(|q| uri_of(&q.path)).collect();
        self.model.edit_queue(|queue| queue.user.retain(|item| !dropped.contains(&item.uri)));
        self.ai.ready.clear();
        self.ai.on = true;
        self.ai_next(cx);
    }

    /// The rows, the song being made first, then the newest songs.
    fn ai_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let made = self.ai.songs.iter().filter(|s| !s.path.is_empty()).count();
        let playing = self.model.playback.uri().map(str::to_string);
        let next_uris: Vec<String> = self.ai.queued.iter().chain(self.ai.ready.iter()).map(|r| uri_of(&r.path)).collect();
        let running_song = self.ai.busy && self.ai.making.is_some();
        if self.ai.busy && self.ai.making.is_none() {
            rows.push(Row {
                song: None,
                num: format!("{}", made + 1),
                title: "New song".into(),
                style: "being written from your prompt".into(),
                length: String::new(),
                status: String::new(),
                running: true,
                playable: false,
            });
        }
        for (at, song) in self.ai.songs.iter().enumerate().rev() {
            let running = running_song && at + 1 == self.ai.songs.len() && song.path.is_empty();
            if song.path.is_empty() && !running {
                continue;
            }
            let uri = uri_of(&song.path);
            let status = if running {
                String::new()
            } else if playing.as_deref() == Some(uri.as_str()) {
                if self.model.playback.is_playing() { "Playing".into() } else { "Paused".into() }
            } else if let Some(at) = next_uris.iter().position(|u| *u == uri) {
                if at == 0 { "Next up".into() } else { format!("Up next ({})", at + 1) }
            } else {
                "In the AI music playlist".into()
            };
            let number = self.ai.songs[..=at].iter().filter(|s| !s.path.is_empty()).count() + usize::from(running);
            rows.push(Row {
                song: Some(at),
                num: number.to_string(),
                title: song.title.clone(),
                style: song.prompt.clone(),
                length: if song.length > 0.0 { clock(song.length) } else { String::new() },
                status,
                running,
                playable: !running,
            });
            if rows.len() >= ROWS {
                break;
            }
        }
        rows
    }

    /// The page, from the mode. Labels are only set when their text changed.
    pub(crate) fn update_ai_page(&mut self, cx: &mut Cx) {
        let now = Self::now(cx);
        let status = self.ai.status(now);
        self.set_label(cx, live_id!(ai_status), ids!(ai_status), &status);
        let licence = self.ai.licence();
        self.set_label(cx, live_id!(ai_licence), ids!(ai_licence), &licence);
        self.set_label(cx, live_id!(ai_go), ids!(ai_go), if self.ai.on { "Stop" } else { "Start" });
        self.set_label(cx, live_id!(ai_go_icon), ids!(ai_go_icon), if self.ai.on { "\u{f04d}" } else { "\u{f04b}" });
        self.ui.relief_view(cx, ids!(ai_go_cap)).set_latched(cx, if self.ai.on { 1.0 } else { 0.0 });
        if !self.ai_models_shown {
            self.ai_models_shown = true;
            let mut labels: Vec<String> = self.ai.models.iter().map(|m| m.label()).collect();
            if labels.is_empty() {
                labels.push(self.ai.model.clone());
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
        }

        // The rows.
        let rows = self.ai_rows();
        self.set_visible(cx, ids!(ai_empty), rows.is_empty());
        let theme = self.model.theme;
        let fresh = self.ai.fresh.clone().filter(|(_, at)| now - at < FRESH_S);
        for n in 0..ROWS {
            let id = row_id(n);
            let Some(row) = rows.get(n) else {
                self.set_visible(cx, &[id], false);
                continue;
            };
            self.set_visible(cx, &[id], true);
            let status = if row.running {
                let pct = if self.ai.permille > 0 { format!(" · {}%", self.ai.permille / 10) } else { String::new() };
                format!("{}{pct} · {}", self.ai.step(), clock(now - self.ai.started_at))
            } else {
                row.status.clone()
            };
            for (field, text) in [(live_id!(num), &row.num), (live_id!(title), &row.title), (live_id!(style), &row.style), (live_id!(length), &row.length), (live_id!(status), &status)] {
                let path = [id, field];
                let key = LiveId::from_str(&format!("ai_row_{n}_{field}"));
                self.set_label(cx, key, &path, text);
            }
            self.set_visible(cx, &[id, live_id!(play_cap)], row.playable);
            self.set_visible(cx, &[id, live_id!(cancel_cap)], row.running);
            self.set_visible(cx, &[id, live_id!(restart_cap)], row.running);
            self.set_visible(cx, &[id, live_id!(bar)], row.running);
            if row.running {
                let mut bar = self.ui.widget(cx, &[id, live_id!(bar)]);
                let progress = self.ai.permille as f64 / 1000.0;
                let phase = (now * 0.6).fract();
                let (fill, track) = (theme.accent, vec4(theme.muted.x, theme.muted.y, theme.muted.z, 0.25));
                script_apply_eval!(cx, bar, {draw_bg +: {progress: #(progress) phase: #(phase) fill: #(fill) track: #(track)}});
            }
            // Picked, just made, playing: shades of the accent.
            let uri = row.song.and_then(|at| self.ai.songs.get(at)).map(|s| uri_of(&s.path));
            let selected = row.song.is_some() && row.song == self.ai.selected;
            let just_made = fresh.as_ref().is_some_and(|(u, _)| Some(u) == uri.as_ref());
            let alpha = if just_made { 0.32 } else if selected { 0.18 } else if row.running { 0.08 } else { 0.0 };
            let color = vec4(theme.accent.x, theme.accent.y, theme.accent.z, alpha as f32);
            let mut view = self.ui.widget(cx, &[id]);
            script_apply_eval!(cx, view, {draw_bg +: {color: #(color)}});
        }

        // The inspector.
        self.ui.relief_view(cx, ids!(ai_show_song_cap)).set_lit(cx, if self.ai.show_conversation { 0.0 } else { 1.0 });
        self.ui.relief_view(cx, ids!(ai_show_chat_cap)).set_lit(cx, if self.ai.show_conversation { 1.0 } else { 0.0 });
        self.set_visible(cx, ids!(ai_new_cap), self.ai.show_conversation);
        let (title, meta, body) = if self.ai.show_conversation {
            (
                "The writer conversation".to_string(),
                "One conversation for all songs (kept between runs), so the writer remembers what it already wrote.".to_string(),
                if self.ai.conversation.is_empty() { "Nothing yet.".to_string() } else { self.ai.conversation.clone() },
            )
        } else {
            let picked = self.ai.selected.and_then(|at| self.ai.songs.get(at)).or(self.ai.making.as_ref()).or_else(|| self.ai.songs.iter().rev().find(|s| !s.path.is_empty()));
            match picked {
                Some(song) => inspect(song),
                None => ("Lyrics & prompt".to_string(), "Pick a song to see its lyrics, style tags and what made it.".to_string(), String::new()),
            }
        };
        self.set_label(cx, live_id!(ai_detail_title), ids!(ai_detail_title), &title);
        self.set_label(cx, live_id!(ai_detail_meta), ids!(ai_detail_meta), &meta);
        self.set_label(cx, live_id!(ai_detail_body), ids!(ai_detail_body), &body);
    }

    /// While a song is made and the page is open, its bar's sheen and the
    /// elapsed time move: a frame every 1/15 s for the page alone.
    fn ai_animate(&mut self, cx: &mut Cx) {
        if self.ai.busy {
            self.ai_frame = cx.new_next_frame();
        }
    }

    /// The bar's frames and the keys' tooltips.
    pub(crate) fn ai_page_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Some(frame) = self.ai_frame.is_event(event) {
            if self.ai.busy {
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
        let tips: [(&[LiveId], &str); 8] = [
            (ids!(ai_go_cap), if self.ai.on { "Stop AI music: no new songs are made (made ones stay in the AI music playlist)" } else { "Start AI music: songs are written from your prompt and made on your AI hub, one ahead of what plays" }),
            (ids!(ai_model), "The music model for the next song"),
            (ids!(ai_skip_cap), "Skip to the next AI song (two are kept made ahead; if none is ready yet, it starts the moment it is)"),
            (ids!(ai_length), "How long the next song is (the writer sizes its lyrics to it)"),
            (ids!(ai_prompt), "What to hear. Edits apply from the next song made; the playing song is never interrupted"),
            (ids!(ai_show_song_cap), "The picked song's lyrics, style tags and what made it"),
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

/// A song for the inspector: title, what it is and what made it, the lyrics.
fn inspect(song: &SongPrompt) -> (String, String, String) {
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
