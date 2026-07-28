//! The conversational pipeline glue: transcripts in, spoken replies out.
//!
//! Voice input (or a text box) produces utterances; the [`TranscriptFilter`]
//! judges and rewrites them off-thread; accepted instructions go to the agent
//! backend; the streamed reply is spoken sentence-by-sentence through
//! [`SpeechOutput`] and surfaced to the app as [`ConverseAction`]s for its own
//! UI. The agent backend is pluggable via `makepad_ai` — Claude Code today, a
//! local OpenAI-compatible endpoint or an in-process model tomorrow.
//!
//! The pipeline owns neither microphone nor UI: apps keep their own input
//! path (the Window voice plumbing already injects transcripts as text) and
//! call [`ConversePipeline::submit_transcript`] / [`submit_direct`] with
//! whatever the user said or typed.

use crate::filter::{FilterDecision, FilterJob, FilterWorker, TranscriptFilter};
use crate::speech::SpeechOutput;
use makepad_ai::agent::{Agent, AgentEvent, PromptId, SessionConfig, SessionId};
use makepad_widgets::{Cx, Event};
use std::collections::VecDeque;

/// How many recent dialog lines the filter sees for context.
const RECENT_DIALOG_LINES: usize = 12;

/// What happened in the pipeline this event; the app renders these however it
/// likes (chat log, status bar, nothing).
#[derive(Clone, Debug)]
pub enum ConverseAction {
    /// The filter decided this utterance was not for the assistant.
    UtteranceDropped { utterance: String, reason: String },
    /// An instruction went out to the backend. `raw` is what was actually
    /// heard; `instruction` is what the filter forwarded.
    PromptSent {
        prompt_id: PromptId,
        raw: String,
        instruction: String,
    },
    /// Streamed reply text (already being spoken; render it too).
    ReplyDelta { prompt_id: PromptId, text: String },
    /// The backend is using a tool; `label` is a short human-readable name.
    ToolActivity { prompt_id: PromptId, label: String },
    /// The turn finished; any unspoken tail has been flushed to speech.
    ReplyComplete { prompt_id: PromptId },
    /// Session or prompt failure.
    Error { error: String },
}

pub struct ConversePipeline {
    agent: Box<dyn Agent>,
    session_id: Option<SessionId>,
    current_prompt: Option<PromptId>,
    /// Prompt text waiting for the session to come up.
    queued_prompts: VecDeque<(String, String)>,
    filter: FilterWorker,
    speech: SpeechOutput,
    /// Rolling dialog memory handed to the filter for reference resolution.
    recent_dialog: VecDeque<String>,
    /// Reply text of the in-flight turn, accumulated for `recent_dialog`.
    reply_accum: String,
    /// Actions produced outside `handle_event` (e.g. by `submit_direct`),
    /// delivered on the next pump.
    pending_actions: Vec<ConverseAction>,
    /// Speak replies aloud (on unless the app runs its own TTS policy).
    pub speak_replies: bool,
}

impl ConversePipeline {
    /// `voice` is the TTS voice pack name, e.g. `"bm_fable.mkvoice"`.
    /// `make_filter` runs on the filter worker thread, so filters holding
    /// single-thread resources (a local LLM session) are fine.
    pub fn new(
        agent: Box<dyn Agent>,
        make_filter: impl FnOnce() -> Box<dyn TranscriptFilter> + Send + 'static,
        voice: &str,
    ) -> Self {
        Self {
            agent,
            session_id: None,
            current_prompt: None,
            queued_prompts: VecDeque::new(),
            filter: FilterWorker::new(make_filter),
            speech: SpeechOutput::new(voice),
            recent_dialog: VecDeque::new(),
            reply_accum: String::new(),
            pending_actions: Vec::new(),
            speak_replies: true,
        }
    }

    /// Create the backend session. Call once at startup (and again to switch
    /// models/backends — the old session is dropped).
    pub fn init_session(&mut self, cx: &mut Cx, config: SessionConfig) {
        self.session_id = Some(self.agent.create_session(cx, config));
    }

    pub fn session_id(&self) -> Option<SessionId> {
        self.session_id
    }

    pub fn is_busy(&self) -> bool {
        self.current_prompt.is_some()
    }

    /// The speech output, e.g. to hush, mute, or install the audio callback.
    pub fn speech(&mut self) -> &mut SpeechOutput {
        &mut self.speech
    }

    /// An utterance heard by the microphone: judged by the filter first.
    pub fn submit_transcript(&mut self, utterance: &str) {
        let utterance = utterance.trim();
        if utterance.is_empty() {
            return;
        }
        self.filter.submit(FilterJob {
            utterance: utterance.to_string(),
            recent_dialog: self.recent_dialog.iter().cloned().collect(),
        });
    }

    /// Typed (or otherwise explicit) input: skips the filter.
    pub fn submit_direct(&mut self, cx: &mut Cx, text: &str) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.send_instruction(cx, text.clone(), text);
    }

    /// Cancel the in-flight turn and stop speaking.
    pub fn cancel(&mut self, cx: &mut Cx) {
        if let Some(prompt_id) = self.current_prompt.take() {
            self.agent.cancel_prompt(cx, prompt_id);
        }
        self.queued_prompts.clear();
        self.speech.stop();
    }

    fn remember(&mut self, line: String) {
        self.recent_dialog.push_back(line);
        while self.recent_dialog.len() > RECENT_DIALOG_LINES {
            self.recent_dialog.pop_front();
        }
    }

    fn send_instruction(&mut self, cx: &mut Cx, raw: String, instruction: String) {
        self.remember(format!("user: {instruction}"));
        let Some(session_id) = self.session_id else {
            self.queued_prompts.push_back((raw, instruction));
            return;
        };
        if !self.agent.is_session_ready(session_id) {
            self.queued_prompts.push_back((raw, instruction));
            return;
        }
        let prompt_id = self.agent.send_prompt(cx, session_id, &instruction);
        self.current_prompt = Some(prompt_id);
        self.speech.unhush();
        self.pending_actions.push(ConverseAction::PromptSent {
            prompt_id,
            raw,
            instruction,
        });
    }

    /// Pump the agent, the filter worker and speech. Call from the app's
    /// `handle_event`; render the returned actions as you see fit.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<ConverseAction> {
        let mut out = std::mem::take(&mut self.pending_actions);

        // Filter verdicts first: they may start a turn.
        for result in self.filter.poll() {
            match result.decision {
                FilterDecision::Forward { instruction } => {
                    self.send_instruction(cx, result.utterance, instruction);
                }
                FilterDecision::Drop { reason } => {
                    out.push(ConverseAction::UtteranceDropped {
                        utterance: result.utterance,
                        reason,
                    });
                }
            }
        }
        out.append(&mut self.pending_actions);

        for agent_event in self.agent.handle_event(cx, event) {
            match agent_event {
                AgentEvent::SessionReady { .. } => {
                    while let Some((raw, instruction)) = self.queued_prompts.pop_front() {
                        self.send_instruction(cx, raw, instruction);
                    }
                    out.append(&mut self.pending_actions);
                }
                AgentEvent::SessionError { error, .. } => {
                    out.push(ConverseAction::Error { error });
                }
                AgentEvent::TextDelta { prompt_id, text } => {
                    if Some(prompt_id) == self.current_prompt {
                        if self.speak_replies {
                            self.speech.feed(&text);
                        }
                        self.reply_accum.push_str(&text);
                        out.push(ConverseAction::ReplyDelta { prompt_id, text });
                    }
                }
                AgentEvent::ToolRequest {
                    prompt_id,
                    tool_name,
                    ..
                } => {
                    if Some(prompt_id) == self.current_prompt {
                        out.push(ConverseAction::ToolActivity {
                            prompt_id,
                            label: tool_name,
                        });
                    }
                }
                AgentEvent::TurnComplete { prompt_id, .. } => {
                    if Some(prompt_id) == self.current_prompt {
                        self.current_prompt = None;
                        if self.speak_replies {
                            self.speech.flush();
                        }
                        let reply = std::mem::take(&mut self.reply_accum);
                        if !reply.is_empty() {
                            self.remember(format!("assistant: {reply}"));
                        }
                        out.push(ConverseAction::ReplyComplete { prompt_id });
                    }
                }
                AgentEvent::PromptError { prompt_id, error } => {
                    if Some(prompt_id) == self.current_prompt {
                        self.current_prompt = None;
                        self.reply_accum.clear();
                    }
                    out.push(ConverseAction::Error { error });
                }
            }
        }
        out
    }
}
