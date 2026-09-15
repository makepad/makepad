//! The brain of the standalone desktop build: the local Qwen dispatcher
//! (or Claude, when asked), the Claude escalation agent behind
//! `cloud_ask`, Kokoro speech for the replies, the whisper mic with its
//! attention gate, and DuckDuckGo image search.
//!
//! Everything here used to be state on the app; it now sits behind the
//! assistant seam so the view is the same with or without it. The brain
//! owns the sessions and workers, reports through [`AssistantEvent`], and
//! is told what the tools answered.

use super::{AssistantController, AssistantEvent, BeginTool, PromptOutcome};
use crate::broker;
use crate::ddg::{DdgEvent, DdgState};
use crate::voice::{GateResult, VoiceGate};
use makepad_converse::agent_seam::*;
use makepad_converse::SpeechOutput;
use makepad_widgets::*;

const SYSTEM_PROMPT: &str = "\
You are the route assistant inside a live map app (Netherlands detail, Europe-wide places), \
a conversational replacement for a car GPS. The user sees a full-screen map; you act only \
through tools and short replies.

Rules:
- Mirror everything on the map: plan trips with route_plan, drop markers for candidates you \
mention, fly the camera to places you talk about. The user must always see what you did.
- Replies are 1-3 short sentences, conversational, no coordinate dumps, no markdown. Tool \
digests are already shown in the transcript — summarize outcomes, don't repeat them.
- Stops and legs have stable ids (stop_2, leg_1); use them for changes and references.
- Coordinates are always lon,lat (WGS84). Waypoints accept place names, 'lon,lat', or 'here'.
- Each user message ends with an [app state] block (map center, trip digest) — trust it.
- If a tool reports data still loading or out of coverage, say so briefly; don't guess.";

/// Pull "ctx USED/MAX" out of the local timing status line.
fn parse_ctx_usage(timing: &str) -> Option<(usize, usize)> {
    let at = timing.rfind("ctx ")?;
    let rest = &timing[at + 4..];
    let (used, max) = rest.trim().split_once('/')?;
    Some((
        used.trim().parse().ok()?,
        max.trim()
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?,
    ))
}

fn read_secret(name: &str) -> Option<String> {
    if let Ok(v) = std::env::var(name) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Ok(v) = std::fs::read_to_string(name) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    None
}

/// Does this utterance override the current activity? Only the LEADING
/// words count — "add a charging stop" must not match on "stop".
fn is_override_command(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(2)
        .any(|w| {
            matches!(
                w,
                "stop" | "cancel" | "nevermind" | "never" | "forget" | "wait" | "actually"
            )
        })
}

/// "stop" / "nevermind" with no follow-up command: just halt.
fn is_pure_stop(text: &str) -> bool {
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words.len() <= 3
        && words.iter().all(|w| {
            matches!(
                w.to_lowercase().as_str(),
                "ok" | "okay" | "no" | "stop" | "cancel" | "nevermind" | "never" | "mind" | "it"
                    | "that" | "forget" | "computer"
            )
        })
}

#[derive(Default)]
pub struct AssistantService {
    agent: Option<Box<dyn Agent>>,
    session: Option<SessionId>,
    /// Claude escalation agent behind the cloud_ask tool (None = offline).
    cloud_agent: Option<Box<dyn Agent>>,
    cloud_session: Option<SessionId>,
    /// In-flight cloud_ask: (local tool_use_id, accumulated answer).
    pending_cloud: Option<(String, String)>,
    /// Turn timing shared with the LocalAgent worker.
    local_timing: Option<ToUIReceiver<String>>,
    busy: bool,
    /// In-flight dispatcher prompt, for user-override cancellation.
    current_prompt: Option<PromptId>,
    /// Tool calls executed for the current prompt (loop budget).
    tool_rounds: usize,
    /// Previous (name, args) this turn — breaks identical-call loops.
    last_tool_call: Option<(String, String)>,
    /// Kokoro voice output (🔊 button). None until `start`.
    speech: Option<SpeechOutput>,
    voice_gate: Option<VoiceGate>,
    /// SEND instructions that arrived while the agent was busy.
    voice_queue: Vec<String>,
    /// Continuous voice loop active (mic button).
    mic_on: bool,
    whisper_warmed: bool,
    ai_llm_ready: bool,
    ai_gate_ready: bool,
    ddg: DdgState,
    /// What happened since the view last asked.
    events: Vec<AssistantEvent>,
}

impl AssistantController for AssistantService {
    fn configure_ui(&self, _cx: &mut Cx, _ui: &WidgetRef) {}

    fn unavailable_reply(&self, _prompt: &str) -> Option<&'static str> {
        None
    }
}

impl AssistantService {
    /// Bring the voice and the dispatcher up. Kokoro af_heart when weights
    /// are in reach (this process, the machine node, a LAN box), else the
    /// OS voice — the hub decides.
    pub fn start(&mut self, cx: &mut Cx) {
        let speech = SpeechOutput::new("af_heart", cx.thread_spawner());
        speech.install_audio_output(cx, 0);
        self.speech = Some(speech);
        self.init_agent(cx);
        let status = self.status_text();
        self.events.push(AssistantEvent::Status(status));
    }

    fn make_claude(api_key: String) -> Box<dyn Agent> {
        let model = std::env::var("MAKEPAD_ROUTE_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-5".to_string());
        Box::new(crate::claude_agent::ClaudeAgent::new(model, api_key))
    }

    /// Dispatcher: the in-process local model by default (pure-Rust ggml,
    /// no external processes); MAKEPAD_ROUTE_CLOUD=1 keeps Claude as the
    /// dispatcher instead. Claude, when a key exists, otherwise serves as
    /// the cloud_ask escalation tool.
    fn init_agent(&mut self, cx: &mut Cx) {
        // AI on by default again (perf campaign over); MAKEPAD_NO_AI=1
        // keeps the GPU clear of the 9B/4B/whisper chain when profiling
        // the map renderer.
        if std::env::var_os("MAKEPAD_NO_AI").is_some() {
            return;
        }
        let api_key = read_secret("ANTHROPIC_API_KEY");
        let cloud_dispatch = std::env::var("MAKEPAD_ROUTE_CLOUD").is_ok() && api_key.is_some();

        let mut agent: Box<dyn Agent> = if cloud_dispatch {
            Self::make_claude(api_key.clone().unwrap())
        } else {
            let model_path = std::env::var("MAKEPAD_ROUTE_LOCAL_MODEL")
                .unwrap_or_else(|_| crate::local_agent::DEFAULT_LOCAL_MODEL.to_string());
            let timing = ToUIReceiver::default();
            let timing_sender = timing.sender();
            self.local_timing = Some(timing);
            Box::new(crate::local_agent::LocalAgent::new(model_path, timing_sender))
        };
        let session = agent.create_session(
            cx,
            SessionConfig {
                system_prompt: Some(SYSTEM_PROMPT.to_string()),
                tools: broker::tool_definitions(),
                ..Default::default()
            },
        );
        self.session = Some(session);
        self.agent = Some(agent);

        // Escalation valve: only when a key exists and Claude isn't
        // already the dispatcher.
        if !cloud_dispatch {
            if let Some(api_key) = api_key {
                let mut cloud = Self::make_claude(api_key);
                let cloud_session = cloud.create_session(
                    cx,
                    SessionConfig {
                        system_prompt: Some(
                            "You answer knowledge questions for a navigation assistant \
                             (sights, reviews, rankings, world knowledge). Be concise: a short \
                             paragraph or compact list, no markdown."
                                .to_string(),
                        ),
                        ..Default::default()
                    },
                );
                self.cloud_session = Some(cloud_session);
                self.cloud_agent = Some(cloud);
            }
        }
    }

    /// Aggregate boot status: the whole AI pipeline loads at startup
    /// (9B dispatcher, 4B voice gate, whisper via the voice worker).
    pub fn status_text(&self) -> String {
        if self.local_timing.is_none() {
            return "AI loading — cloud dispatcher · voice gate…".to_string();
        }
        if self.ai_llm_ready && self.ai_gate_ready {
            "AI ready — dispatcher 9B ✓ · voice gate 4B ✓ · whisper warm".to_string()
        } else {
            format!(
                "AI loading — dispatcher 9B {} · voice gate 4B {} · whisper…",
                if self.ai_llm_ready { "✓" } else { "…" },
                if self.ai_gate_ready { "✓" } else { "…" },
            )
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    fn info(&mut self, line: impl Into<String>) {
        self.events.push(AssistantEvent::Info(line.into()));
    }

    fn status(&mut self) {
        let status = self.status_text();
        self.events.push(AssistantEvent::Status(status));
    }

    /// Send one turn. `prompt` is the full text, app-state block included.
    pub fn send_prompt(&mut self, cx: &mut Cx, prompt: &str) -> PromptOutcome {
        if self.agent.is_none() {
            return PromptOutcome::NoAgent;
        }
        // A new turn obsoletes whatever the voice was still saying — and a
        // typed command while the dispatcher runs is always an override.
        if self.busy {
            self.interrupt(cx);
        }
        if let Some(speech) = &mut self.speech {
            speech.stop();
        }
        let (agent, session) = (self.agent.as_mut().unwrap(), self.session.unwrap());
        let prompt_id = agent.send_prompt(cx, session, prompt);
        self.current_prompt = Some(prompt_id);
        self.tool_rounds = 0;
        self.last_tool_call = None;
        self.busy = true;
        PromptOutcome::Sent
    }

    /// User override: cancel the in-flight dispatcher turn NOW. The worker
    /// stops per-token, closes the assistant turn and drops its tool calls.
    pub fn interrupt(&mut self, cx: &mut Cx) -> bool {
        if !self.busy {
            return false;
        }
        if let (Some(agent), Some(prompt_id)) = (self.agent.as_mut(), self.current_prompt) {
            agent.cancel_prompt(cx, prompt_id);
        }
        self.busy = false;
        self.voice_queue.clear();
        if let Some(speech) = &mut self.speech {
            speech.stop();
        }
        self.info("⏹ interrupted");
        self.events.push(AssistantEvent::Status("ready".to_string()));
        true
    }

    pub fn send_tool_result(&mut self, cx: &mut Cx, tool_use_id: &str, text: &str, is_error: bool) {
        if let (Some(agent), Some(session)) = (self.agent.as_mut(), self.session) {
            agent.send_tool_result(cx, session, tool_use_id, text, is_error);
        }
    }

    /// A tool the dispatcher asked for: broken loops and the asynchronous
    /// tools this brain owns are answered here; anything else the view
    /// runs against the registry.
    pub fn begin_tool(&mut self, cx: &mut Cx, tool_use_id: &str, name: &str, input: &str) -> BeginTool {
        // Loop breakers: greedy decoding can wedge the dispatcher into
        // re-issuing the same call forever (seen: identical geo_search
        // spam). Repeats and over-budget turns get a corrective tool
        // result instead of execution.
        self.tool_rounds += 1;
        let this_call = (name.to_string(), input.to_string());
        let repeated = self.last_tool_call.as_ref() == Some(&this_call);
        self.last_tool_call = Some(this_call);
        if repeated || self.tool_rounds > 10 {
            let nudge = if repeated {
                "Error: identical tool call repeated — you already have this result. \
                 Do NOT call this tool again; answer the user now with what you know."
            } else {
                "Error: tool budget for this request is exhausted. \
                 Stop calling tools and answer the user now with what you have."
            };
            self.info("⛔ tool loop broken");
            self.send_tool_result(cx, tool_use_id, nudge, true);
            return BeginTool::Handled;
        }
        // images_search runs async over cx.http_request; the tool result is
        // sent when the thumbnails land.
        if name == "images_search" {
            let query = broker::parse_field(input, "query").unwrap_or_default();
            match self.start_image_search(cx, &query, Some(tool_use_id.to_string())) {
                Ok(()) => self.info(format!("🔎 images: {query}")),
                Err(error) => self.send_tool_result(cx, tool_use_id, &error, true),
            }
            return BeginTool::Handled;
        }
        // cloud_ask escalates asynchronously through the Claude side-agent;
        // the tool result is sent when that turn completes.
        if name == "cloud_ask" && self.cloud_agent.is_some() && self.pending_cloud.is_none() {
            let question = broker::parse_question(input);
            self.info("☁ asking cloud…");
            let (cloud, cloud_session) = (
                self.cloud_agent.as_mut().unwrap(),
                self.cloud_session.unwrap(),
            );
            cloud.send_prompt(cx, cloud_session, &question);
            self.pending_cloud = Some((tool_use_id.to_string(), String::new()));
            return BeginTool::Handled;
        }
        BeginTool::Execute
    }

    /// Search DuckDuckGo images; the thumbnails and the digest arrive as
    /// events. `Err` means nothing started (rate limit, empty query).
    pub fn start_image_search(
        &mut self,
        cx: &mut Cx,
        query: &str,
        tool_use_id: Option<String>,
    ) -> Result<(), String> {
        self.ddg.start(cx, query, tool_use_id)
    }

    // --- speech ---------------------------------------------------------

    pub fn speak(&self, text: &str) {
        if let Some(speech) = &self.speech {
            speech.enqueue(text);
        }
    }

    pub fn stop_speech(&mut self) {
        if let Some(speech) = &mut self.speech {
            speech.stop();
        }
    }

    /// Flip the 🔊 mute; the new state, or `None` without a voice.
    pub fn toggle_muted(&mut self) -> Option<bool> {
        let speech = self.speech.as_mut()?;
        let muted = !speech.is_muted();
        speech.set_muted(muted);
        Some(muted)
    }

    // --- voice in -------------------------------------------------------

    /// Mic toggle: starts/stops the continuous voice loop (VAD → STT →
    /// attention gate → dispatcher).
    fn toggle_mic(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        self.mic_on = !self.mic_on;
        let mut button = ui.button(cx, ids!(mic_button));
        if self.mic_on {
            button.set_text(cx, "🔴");
            script_apply_eval!(cx, button, {
                draw_text +: {
                    color: #(vec4(0.86, 0.20, 0.20, 1.0))
                }
            });
            self.info(if self.whisper_warmed {
                "🎤 listening — say 'computer, …' or address me directly"
            } else {
                "🎤 arming — loading whisper (first time takes a few seconds), then say 'computer, …'"
            });
            self.whisper_warmed = true;
        } else {
            button.set_text(cx, "🎤");
            self.info("🎤 mic off");
        }
        self.sync_voice_state(cx, ui);
    }

    /// Start/stop the VoiceWave capture and lazily spawn the gate worker.
    fn sync_voice_state(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if self.mic_on && self.voice_gate.is_none() {
            self.voice_gate = Some(VoiceGate::new(cx.thread_spawner()));
        }
        let wave = ui.voice_wave(cx, ids!(mic_wave));
        wave.set_enabled(cx, self.mic_on);
        log!(
            "voice: mic toggle requested={} widget_enabled={}",
            self.mic_on,
            wave.is_enabled()
        );
    }

    /// An endpointed transcript from the mic, with the recent dialog for
    /// the gate's in-context judgement.
    pub fn submit_utterance(&mut self, text: String, recent: Vec<String>) {
        match &self.voice_gate {
            Some(gate) => gate.submit(text, recent),
            None => self.info("(gate still loading)"),
        }
    }

    fn on_gate_result(&mut self, cx: &mut Cx, result: GateResult) {
        match result {
            GateResult::Ready { secs } => {
                self.info(format!("voice gate ready (4B loaded + warmed in {secs:.1}s)"));
                self.ai_gate_ready = true;
                self.status();
            }
            GateResult::Send { raw, instruction } => {
                let _ = &raw;
                self.info("→ directed");
                if self.busy && is_override_command(&instruction) {
                    // "ok nevermind, stop, go do X now" — cancel the turn;
                    // send the rest unless it was a bare stop.
                    self.interrupt(cx);
                    if !is_pure_stop(&instruction) {
                        self.events.push(AssistantEvent::Directed(instruction));
                    }
                } else if self.busy {
                    self.info("(queued until current turn ends)");
                    self.voice_queue.push(instruction);
                } else if is_pure_stop(&instruction) {
                    // Nothing running — just quiet the voice.
                    self.stop_speech();
                } else {
                    self.events.push(AssistantEvent::Directed(instruction));
                }
            }
            GateResult::Skip { raw, reason } => {
                let _ = &raw;
                self.info(format!("— skipped ({})", reason.trim()));
            }
        }
    }

    // --- the agents' events -----------------------------------------

    fn on_agent_event(&mut self, cx: &mut Cx, ui: &WidgetRef, event: AgentEvent) {
        match event {
            AgentEvent::SessionReady { .. } => {
                self.ai_llm_ready = true;
                // Eager but SERIALIZED: the gate's 4B loads after the 9B is
                // resident so startup peaks don't stack (iPad jetsam kills
                // on peak footprint, not steady state).
                if self.voice_gate.is_none() {
                    self.voice_gate = Some(VoiceGate::new(cx.thread_spawner()));
                }
                // Chain whisper after the LLMs (eager but serialized).
                ui.voice_wave(cx, ids!(mic_wave)).prewarm(cx);
                self.status();
            }
            AgentEvent::SessionError { error, .. } => {
                self.busy = false;
                self.info(format!("⚠ session error: {error}"));
            }
            AgentEvent::TextDelta { text, .. } => {
                if let Some(speech) = &mut self.speech {
                    speech.feed(&text);
                }
                self.events.push(AssistantEvent::Text(text));
            }
            AgentEvent::ToolRequest {
                tool_use_id,
                tool_name,
                tool_input,
                ..
            } => {
                self.events.push(AssistantEvent::ToolRequest {
                    tool_use_id,
                    name: tool_name,
                    input: tool_input,
                });
            }
            AgentEvent::TurnComplete { .. } => {
                if let Some(speech) = &mut self.speech {
                    speech.flush();
                }
                self.busy = false;
                self.current_prompt = None;
                let timing = self.local_timing.as_ref().and_then(|receiver| {
                    let mut latest = None;
                    while let Ok(value) = receiver.try_recv() {
                        latest = Some(value);
                    }
                    latest
                });
                let timing = timing
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "ready".to_string());
                self.events.push(AssistantEvent::TurnComplete { status: timing.clone() });
                // The session is append-only: once the context is nearly
                // full it cannot recover — restart with a fresh session
                // (mmap makes the reload cheap; chat history stays in the
                // transcript, the model just loses conversational memory).
                if let Some((used, max)) = parse_ctx_usage(&timing) {
                    if used * 10 > max * 9 {
                        self.info(format!("⚠ context {used}/{max} — restarting local session"));
                        self.agent = None;
                        self.session = None;
                        self.init_agent(cx);
                    }
                }
            }
            AgentEvent::PromptError { error, .. } => {
                self.busy = false;
                self.current_prompt = None;
                self.events.push(AssistantEvent::TurnComplete { status: "error — try again".to_string() });
                self.info(format!("⚠ {error}"));
            }
        }
    }

    /// Events from the Claude escalation agent: stream into the pending
    /// cloud_ask, then hand the answer back to the dispatcher as the tool
    /// result (visible in the transcript per route.md — cloud is explicit).
    fn on_cloud_event(&mut self, cx: &mut Cx, event: AgentEvent) {
        match event {
            AgentEvent::TextDelta { text, .. } => {
                if let Some((_, accum)) = &mut self.pending_cloud {
                    accum.push_str(&text);
                }
            }
            AgentEvent::TurnComplete { .. } => {
                if let Some((tool_use_id, answer)) = self.pending_cloud.take() {
                    let preview: String = answer.chars().take(240).collect();
                    self.info(format!("☁ cloud: {}", preview.trim()));
                    self.send_tool_result(cx, &tool_use_id, answer.trim(), false);
                }
            }
            AgentEvent::PromptError { error, .. } | AgentEvent::SessionError { error, .. } => {
                if let Some((tool_use_id, _)) = self.pending_cloud.take() {
                    self.info(format!("☁ cloud error: {error}"));
                    self.send_tool_result(cx, &tool_use_id, &format!("cloud unavailable: {error}"), true);
                }
            }
            _ => {}
        }
    }

    fn on_ddg_event(&mut self, cx: &mut Cx, event: DdgEvent) {
        match event {
            DdgEvent::Thumb(slot, data) => self.events.push(AssistantEvent::Thumb { slot, data }),
            DdgEvent::Done(tool_use_id, digest, is_error) => {
                if let Some(tool_use_id) = tool_use_id {
                    self.send_tool_result(cx, &tool_use_id, &digest, is_error);
                }
                self.events.push(AssistantEvent::SearchDone { digest, is_error });
            }
        }
    }

    /// Everything the workers and agents reported since the last call.
    /// `ui` is the panel, for the mic the dispatcher warms once it is up.
    pub fn handle_event(&mut self, cx: &mut Cx, ui: &WidgetRef, event: &Event) -> Vec<AssistantEvent> {
        match event {
            Event::AudioDevices(devices) => {
                // TTS playback device; mic input selection lives inside the
                // VoiceWave's own handler.
                cx.use_audio_outputs(&devices.default_output());
            }
            Event::NetworkResponses(responses) => {
                let mut ddg_events = Vec::new();
                for response in responses.iter() {
                    ddg_events.extend(self.ddg.handle_response(cx, response));
                }
                for ddg_event in ddg_events {
                    self.on_ddg_event(cx, ddg_event);
                }
            }
            _ => {}
        }
        if let Some(gate) = &mut self.voice_gate {
            let results = gate.poll();
            for result in results {
                self.on_gate_result(cx, result);
            }
        }
        if !self.busy && !self.voice_queue.is_empty() {
            let next = self.voice_queue.remove(0);
            self.events.push(AssistantEvent::Directed(next));
        }
        if let Some(mut agent) = self.agent.take() {
            let events = agent.handle_event(cx, event);
            self.agent = Some(agent);
            for agent_event in events {
                self.on_agent_event(cx, ui, agent_event);
            }
        }
        if let Some(mut cloud) = self.cloud_agent.take() {
            let events = cloud.handle_event(cx, event);
            self.cloud_agent = Some(cloud);
            for cloud_event in events {
                self.on_cloud_event(cx, cloud_event);
            }
        }
        std::mem::take(&mut self.events)
    }

    /// The panel's voice controls: the mic and speaker buttons, and the
    /// transcripts the mic widget endpointed.
    pub fn handle_actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) -> Vec<AssistantEvent> {
        if ui.button(cx, ids!(mic_button)).clicked(actions) {
            self.toggle_mic(cx, ui);
        }
        if ui.button(cx, ids!(speaker_button)).clicked(actions) {
            if let Some(muted) = self.toggle_muted() {
                ui.button(cx, ids!(speaker_button))
                    .set_text(cx, if muted { "🔇" } else { "🔊" });
                self.info(if muted { "🔇 voice off" } else { "🔊 voice on" });
            }
        }
        // Endpointed transcripts from the mic → attention gate.
        let mic_uid = ui.widget(cx, ids!(mic_wave)).widget_uid();
        for action in actions {
            let Some(action) = action.as_widget_action() else {
                continue;
            };
            if action.widget_uid != mic_uid {
                continue;
            }
            match action.cast::<VoiceWaveAction>() {
                VoiceWaveAction::VoiceActivity(active) => {
                    // Barge-in: the user talking mutes the assistant NOW —
                    // they are not waiting for it to finish. (Assistant echo
                    // can also trigger this; acceptable — the gate still
                    // decides what the words meant.)
                    if active {
                        if let Some(speech) = &mut self.speech {
                            if speech.is_speaking() {
                                speech.stop();
                            }
                        }
                    }
                }
                VoiceWaveAction::InjectText(text) => {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        // Raw transcript, immediately visible; the gate's
                        // verdict follows as its own line. Assistant echo is
                        // NOT filtered here — the gate skips non-directed
                        // lines, and blocking the mic while speaking made
                        // interruptions impossible.
                        self.events.push(AssistantEvent::Transcript(text));
                    }
                }
                VoiceWaveAction::RecordVoice(on) => {
                    if on != self.mic_on {
                        // widget-side toggle (click on the wave / F1)
                        self.mic_on = on;
                        self.sync_voice_state(cx, ui);
                    }
                }
                _ => {}
            }
        }
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_words_only_count_at_the_front() {
        assert!(is_override_command("stop, go to Utrecht instead"));
        assert!(is_override_command("actually forget that"));
        assert!(!is_override_command("add a charging stop near Zwolle"));
        assert!(is_pure_stop("ok stop"));
        assert!(is_pure_stop("nevermind computer"));
        assert!(!is_pure_stop("stop and find a charger"));
        assert_eq!(parse_ctx_usage("12.3 tok/s ctx 3900/4096 tokens"), Some((3900, 4096)));
        assert_eq!(parse_ctx_usage("ready"), None);
    }
}
