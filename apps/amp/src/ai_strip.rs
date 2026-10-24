//! The AI music strip at the top of the library panel (`ai_music.rs` has
//! the mode itself): the prompt field, the model picker, one status line,
//! Cancel and Restart, and a small expand toggle over the songs' prompts and
//! the writer conversation.

use crate::ai_music::{uri_of, AiCommand, AiEvent, ReadySong, DEFAULT_REQUEST};
use crate::local::LocalCommand;
use crate::model::Engine;
use crate::queue::QueueItem;
use crate::App;
use makepad_widgets::*;
use std::sync::atomic::Ordering;

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
        self.ai.permille = 0;
        self.ai.making = None;
    }

    /// Start making the next song with what the field says now, unless one
    /// is made, made already or waiting in the queue (one song ahead).
    fn ai_next(&mut self, cx: &mut Cx) {
        if self.ai.busy || self.ai.ready.is_some() || self.ai.queued.is_some() {
            return;
        }
        let mut request = self.ui.text_input(cx, ids!(ai_prompt)).text();
        if request.trim().is_empty() {
            request = DEFAULT_REQUEST.to_string();
        }
        let epoch = self.ai_epoch();
        let Some(workers) = &self.workers else { return };
        if workers.ai_music(AiCommand::Next { request, model: self.ai.model.clone(), epoch }) {
            self.ai.busy = true;
            self.ai.error = None;
            self.ai.stage.clear();
            self.ai.permille = 0;
        } else {
            self.ai.error = Some("the AI music worker is busy; try Restart".into());
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
            AiEvent::Restored { request, model } => {
                let field = self.ui.text_input(cx, ids!(ai_prompt));
                if field.text().is_empty() || field.text() == DEFAULT_REQUEST {
                    field.set_text(cx, &request);
                }
                self.ai.model = model;
                self.ai_models_shown = false;
            }
            AiEvent::Conversation(text) => self.ai.conversation = text,
            AiEvent::Stage { epoch: at, text, permille } if at == epoch => {
                self.ai.stage = text;
                self.ai.permille = permille;
            }
            AiEvent::Song { epoch: at, song } if at == epoch => self.ai.making = Some(song),
            AiEvent::Ready { epoch: at, song } if at == epoch => {
                self.ai.busy = false;
                self.ai.making = None;
                self.ai.songs.push((uri_of(&song.path), song.song.clone()));
                self.ai_file_ready(&song);
                self.ai.ready = Some(song);
            }
            AiEvent::Failed { epoch: at, error } if at == epoch => {
                self.ai.busy = false;
                self.ai.making = None;
                self.ai.error = Some(error);
            }
            AiEvent::Cancelled { epoch: at } if at == epoch => {
                self.ai.busy = false;
                self.ai.making = None;
            }
            // A Next cancelled since: its file (if any) stays in the library.
            AiEvent::Ready { song, .. } => self.ai_file_ready(&song),
            AiEvent::Stage { .. } | AiEvent::Song { .. } | AiEvent::Failed { .. } | AiEvent::Cancelled { .. } => {}
        }
        self.model.requests.redraw = true;
    }

    /// A song file was written: the library lists its folder (added the
    /// first time) and reads it.
    fn ai_file_ready(&mut self, song: &ReadySong) {
        let Some(workers) = &self.workers else { return };
        let folder = std::path::Path::new(&song.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let command = if self.model.folders.iter().any(|f| *f == folder) { LocalCommand::Rescan } else { LocalCommand::AddFolder(folder) };
        if !workers.local(command) {
            log!("amp: ai-music: the folder reader is busy; the song shows with its next scan");
        }
    }

    /// After every poll: an AI song that started asks for the next one; a
    /// made song the library lists goes into the queue (playing at once
    /// when nothing of the AI plays); the model knows whether to hold.
    pub(crate) fn ai_tick(&mut self, cx: &mut Cx) {
        let uri = self.model.playback.uri().map(str::to_string);
        let playing_ai = uri.as_deref().and_then(|u| self.ai.song_of(u)).cloned();
        if uri != self.ai.last_uri {
            self.ai.last_uri = uri.clone();
            if let Some(song) = playing_ai.clone() {
                self.ai.current = Some(song);
                if self.ai.queued.as_ref().is_some_and(|q| Some(uri_of(&q.path)) == uri) {
                    self.ai.queued = None;
                }
                if self.ai.on {
                    self.ai_next(cx);
                }
                self.model.requests.redraw = true;
            }
        }
        self.model.ai_hold = self.ai.on && playing_ai.is_some() && self.model.engine == Engine::Local;
        if !self.ai.on {
            return;
        }
        let listed = self.ai.ready.as_ref().map(|r| uri_of(&r.path)).filter(|u| self.model.index.track(u).is_some());
        if let Some(uri) = listed {
            if let Some(ready) = self.ai.ready.take() {
                let now = playing_ai.is_none() || self.model.ai_waiting;
                let item = QueueItem { uri, name: ready.song.title.clone(), artist: format!("AI · {}", ready.song.model) };
                self.ai.queued = Some(ready);
                self.model.ai_enqueue(item, now);
            }
        }
    }

    /// The strip's controls.
    pub(crate) fn ai_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(ai_on)).clicked(actions) {
            self.ai.on = !self.ai.on;
            if self.ai.on {
                if self.ai.models.is_empty() {
                    if let Some(workers) = &self.workers {
                        workers.ai_music(AiCommand::Models);
                    }
                }
                // A song made earlier and still waiting plays now.
                self.ai_next(cx);
            }
            self.model.requests.redraw = true;
        }
        if let Some(at) = self.ui.drop_down(cx, ids!(ai_model)).selected(actions) {
            if let Some(model) = self.ai.models.get(at) {
                // From the next song on.
                self.ai.model = model.id.clone();
                self.model.requests.redraw = true;
            }
        }
        if self.ui.button(cx, ids!(ai_cancel)).clicked(actions) && self.ai.busy {
            self.ai_cancel_running();
            self.ai.error = Some("cancelled; Restart makes a new one".into());
            self.model.requests.redraw = true;
        }
        if self.ui.button(cx, ids!(ai_restart)).clicked(actions) {
            self.ai_cancel_running();
            // The waiting next song makes way (its file stays in the library).
            if let Some(queued) = self.ai.queued.take() {
                let uri = uri_of(&queued.path);
                self.model.edit_queue(|queue| queue.user.retain(|item| item.uri != uri));
            }
            self.ai.ready = None;
            self.ai.on = true;
            self.ai_next(cx);
        }
        if self.ui.button(cx, ids!(ai_more)).clicked(actions) {
            self.ai.expanded = !self.ai.expanded;
            if self.ai.expanded && self.ai.models.is_empty() {
                if let Some(workers) = &self.workers {
                    workers.ai_music(AiCommand::Models);
                }
            }
            self.model.requests.redraw = true;
        }
        if self.ui.button(cx, ids!(ai_new)).clicked(actions) {
            if let Some(workers) = &self.workers {
                workers.ai_music(AiCommand::NewConversation);
            }
        }
    }

    /// The strip, from the mode. Labels are only set when their text changed.
    pub(crate) fn update_ai_strip(&mut self, cx: &mut Cx) {
        let status = self.ai.status();
        self.set_label(cx, live_id!(ai_status), ids!(ai_status), &status);
        self.ui.relief_view(cx, ids!(ai_on_cap)).set_lit(cx, if self.ai.on { 1.0 } else { 0.0 });
        self.ui.relief_view(cx, ids!(ai_on_cap)).set_latched(cx, if self.ai.on { 1.0 } else { 0.0 });
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
        }
        self.set_visible(cx, ids!(ai_details), self.ai.expanded);
        if self.ai.expanded {
            let note = self.ai.note();
            self.set_label(cx, live_id!(ai_note), ids!(ai_note), &note);
            let mut songs = String::new();
            if let Some(current) = &self.ai.current {
                songs.push_str(&format!("NOW: {}\n\n", current.describe()));
            }
            let next = self.ai.making.as_ref().or(self.ai.ready.as_ref().map(|r| &r.song)).or(self.ai.queued.as_ref().map(|r| &r.song));
            if let Some(next) = next {
                songs.push_str(&format!("NEXT: {}", next.describe()));
            }
            self.set_label(cx, live_id!(ai_songs), ids!(ai_songs), songs.trim());
            // The newest part: the label shows its first lines.
            let conversation = if self.ai.conversation.is_empty() { "No conversation yet.".to_string() } else { tail(&self.ai.conversation, 1200) };
            self.set_label(cx, live_id!(ai_conversation), ids!(ai_conversation), &conversation);
        }
    }
}

/// The last `max` bytes of `text` or so, from a line start.
fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.trim().to_string();
    }
    let mut from = text.len() - max;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let rest = &text[from..];
    let rest = rest.find('\n').map_or(rest, |at| &rest[at + 1..]);
    format!("…\n{}", rest.trim())
}
