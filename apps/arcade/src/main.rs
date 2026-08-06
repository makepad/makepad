// Makepad Arcade — networked AI game sandbox. Plan: repo-root game.md.
pub use makepad_widgets;

pub mod ai;
pub mod arcade_view;
pub mod audio;
pub mod bigworld;
pub mod authoring;
pub mod browser;
pub mod capability;
pub mod chat;
pub mod coedit;
pub mod intent;
pub mod library;
pub mod pair_server;
pub mod pairing;
pub mod settings;
pub mod synth;
pub mod xr_input;

use crate::authoring::{Applied, Authoring};
use crate::capability::{arcade_home, Capabilities};
use crate::chat::{ChatData, ChatRole};
// Named rather than glob-imported: `makepad_ai::*` and the widgets prelude
// both re-export a `makepad_widgets`, and the collision is a warning.
use makepad_ai::agent::{Agent, AgentEvent, PromptId, SessionId};
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.ArcadeView
    use mod.widgets.ArcadeSettings
    use mod.widgets.ArcadeBrowser
    use mod.widgets.ArcadeChat

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1360, 860)
                window.title: "Makepad Arcade"
                // Push-to-talk on Escape is set from Rust in handle_startup:
                // without the `voice` feature the caption bar's VoiceWave is a
                // stub View, and naming its properties here is a script error.
                body +: {
                    flow: Down
                    show_bg: true
                    draw_bg.color: #x0b0d14

                    split_view := Splitter {
                        width: Fill
                        height: Fill
                        axis: SplitterAxis.Horizontal
                        align: SplitterAlign.FromA(420.0)
                        size: 8.0

                        a: View {
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 8
                            padding: Inset{left: 12 top: 10 right: 8 bottom: 10}

                            View {
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 8
                                align: Align{y: 0.5}

                                Label {
                                    text: "Arcade"
                                    draw_text.color: #xb9c4d6
                                    draw_text.text_style: theme.font_regular{font_size: 14}
                                }
                                View { width: Fill height: 1 }
                                games_button := Button { text: "Games" }
                                settings_button := Button { text: "Settings" }
                            }

                            // Hidden until asked for: this is a game, not a
                            // control panel.
                            settings_panel := ArcadeSettings { visible: false }
                            browser_panel := ArcadeBrowser { visible: false }

                            chat := ArcadeChat {}

                            View {
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 6
                                align: Align{y: 1.0}

                                // No `voice_wave` node here on purpose: the
                                // Window already declares one in its caption
                                // bar, and a second with the same id breaks
                                // ids!(voice_wave) and spawns a second worker.
                                input := TextInput {
                                    width: Fill
                                    height: 42
                                    empty_text: "Ask for a game..."
                                }
                                send_button := Button { text: "Go" }
                                stop_button := Button { text: "Stop" visible: false }
                            }

                            status_label := Label {
                                width: Fill
                                height: Fit
                                text: "Starting up..."
                                draw_text.color: #x8fa2b8
                                draw_text.text_style: theme.font_regular{font_size: 11}
                            }
                        }

                        b: View {
                            width: Fill
                            height: Fill
                            flow: Overlay
                            arcade_view := ArcadeView{}
                        }
                    }
                }
            }
        }
    }
}

/// Frames of the "still working" indicator: an agent can spend many seconds
/// inside one tool call without emitting a word.
const SPINNER: [&str; 4] = ["•  ", "•• ", "•••", " ••"];
const SPINNER_PERIOD: f64 = 0.18;

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    settings_open: bool,
    #[rust]
    browser_open: bool,
    #[rust]
    caps: Option<Capabilities>,
    #[rust]
    agent: Option<Box<dyn Agent>>,
    #[rust]
    session_id: Option<SessionId>,
    #[rust]
    current_prompt: Option<PromptId>,
    /// The intent log. Every edit — local agent or remote Claude — goes
    /// through it; there is deliberately no faster path for the local one.
    #[rust]
    authoring: Option<Authoring>,
    #[rust]
    speech: Option<makepad_converse::SpeechOutput>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    spinner_phase: usize,
    #[rust]
    spinner_at: f64,
    #[rust]
    focus_armed: bool,
}

impl App {
    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    fn game_path(&self) -> std::path::PathBuf {
        arcade_home().join("current").join("game.splash")
    }

    /// Send whatever is in the input box. The agent edits `game.splash`; the
    /// resulting file is submitted to the intent log when the turn lands.
    fn send_message(&mut self, cx: &mut Cx) {
        let input = self.ui.text_input(cx, ids!(input));
        let text = input.text();
        if text.trim().is_empty() {
            return;
        }
        ChatData::push(ChatRole::User, text.clone());
        input.set_text(cx, "");
        // Voice injects into whatever holds key focus, so the input must keep
        // it or a spoken sentence lands nowhere.
        input.set_key_focus(cx);

        let (Some(agent), Some(session_id)) = (&mut self.agent, self.session_id) else {
            ChatData::push(
                ChatRole::System,
                "No AI is connected. Open Settings to choose a provider and add a key.",
            );
            self.ui.redraw(cx);
            return;
        };
        if self.current_prompt.is_some() {
            // The newest instruction wins; the partial reply stays in the log.
            if let Some(prompt) = self.current_prompt.take() {
                agent.cancel_prompt(cx, prompt);
            }
        }
        if let Some(authoring) = &mut self.authoring {
            authoring.begin_turn();
        }
        ChatData::begin_stream();
        ChatData::set_activity("Thinking");
        self.current_prompt = Some(agent.send_prompt(cx, session_id, &text));
        self.ui.widget(cx, ids!(stop_button)).set_visible(cx, true);

        if let Some(speech) = self.speech.as_mut() {
            speech.unhush();
        }
        self.ui.redraw(cx);
    }

    fn cancel_request(&mut self, cx: &mut Cx) {
        if let Some(speech) = self.speech.as_mut() {
            speech.stop();
        }
        let (Some(agent), Some(prompt)) = (&mut self.agent, self.current_prompt.take()) else {
            return;
        };
        agent.cancel_prompt(cx, prompt);
        ChatData::end_stream();
        self.ui.widget(cx, ids!(stop_button)).set_visible(cx, false);
        self.set_status(cx, "Stopped.");
        self.ui.redraw(cx);
    }

    /// Submit the agent's edit to the intent log and reload if the head moved.
    fn land_edit(&mut self, cx: &mut Cx, intent: &str) {
        let Some(authoring) = &mut self.authoring else {
            return;
        };
        if let Applied::Reload(pending) = authoring.submit_from_disk(intent) {
            let path = authoring.path().to_path_buf();
            let generation = pending.generation;
            let view = self.ui.widget(cx, ids!(arcade_view));
            let error = view
                .borrow_mut::<crate::arcade_view::ArcadeView>()
                .and_then(|mut v| v.load_game(cx, &path));
            match error {
                Some(error) => {
                    // The world keeps running last-good; the proposer hears
                    // about it (and nobody else does).
                    if let Some(a) = &mut self.authoring {
                        a.note_eval_error(generation, error);
                    }
                }
                None => {
                    if let Some(a) = &mut self.authoring {
                        a.note_eval_ok(generation);
                    }
                }
            }
        }
        self.drain_authoring(cx);
    }

    /// Route coedit responses addressed to this device's agent into the chat.
    fn drain_authoring(&mut self, cx: &mut Cx) {
        let Some(authoring) = &mut self.authoring else {
            return;
        };
        let lines: Vec<String> = authoring
            .drain_local()
            .iter()
            .filter_map(Authoring::describe)
            .collect();
        for line in lines {
            ChatData::push(ChatRole::System, line);
        }
        self.ui.redraw(cx);
    }

    fn draw_activity(&mut self, cx: &mut Cx) {
        let activity = match chat::CHAT.read() {
            Ok(data) => data.activity.clone(),
            Err(_) => return,
        };
        if activity.is_empty() {
            return;
        }
        self.set_status(cx, &format!("{} {activity}", SPINNER[self.spinner_phase]));
        cx.redraw_all();
    }

    /// Turn a tool call into something a child can understand.
    fn describe_tool(tool_name: &str, subject: &str) -> Option<String> {
        let file = subject.rsplit('/').next().unwrap_or(subject);
        match tool_name {
            "Edit" | "Write" => Some("Changing the game".to_string()),
            "Read" => Some(format!("Looking at {file}")),
            "Glob" | "Grep" => Some("Looking around".to_string()),
            "Bash" => Some("Working on it".to_string()),
            _ => None,
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(settings_button)).clicked(actions) {
            self.settings_open = !self.settings_open;
            self.ui
                .widget(cx, ids!(settings_panel))
                .set_visible(cx, self.settings_open);
            self.ui.redraw(cx);
        }
        if self.ui.button(cx, ids!(games_button)).clicked(actions) {
            self.browser_open = !self.browser_open;
            self.ui
                .widget(cx, ids!(browser_panel))
                .set_visible(cx, self.browser_open);
            self.ui.redraw(cx);
        }
        if self.ui.button(cx, ids!(send_button)).clicked(actions) {
            self.send_message(cx);
        }
        if self.ui.button(cx, ids!(stop_button)).clicked(actions) {
            self.cancel_request(cx);
        }
        if self.ui.text_input(cx, ids!(input)).returned(actions).is_some() {
            self.send_message(cx);
        }

        // A transcript arrives as a synthetic TextInput event, which only a
        // TextInput holding key focus consumes. Take focus the moment the mic
        // opens, or the words are dispatched into the tree and dropped.
        #[cfg(feature = "voice")]
        {
            let voice_wave = self.ui.voice_wave(cx, ids!(voice_wave));
            for action in
                actions.filter_widget_actions_cast::<VoiceWaveAction>(voice_wave.widget_uid())
            {
                if let VoiceWaveAction::RecordVoice(true) = action {
                    // Quiet the speaker while the mic is open, or Whisper
                    // transcribes the AI's own voice back at it.
                    if let Some(speech) = self.speech.as_mut() {
                        speech.stop();
                    }
                    self.ui.text_input(cx, ids!(input)).set_key_focus(cx);
                }
            }
        }

        let browser = self.ui.widget(cx, ids!(browser_panel));
        let action = match browser.borrow_mut::<crate::browser::ArcadeBrowser>() {
            Some(mut b) => b.handle_actions(cx, actions),
            None => crate::browser::BrowserAction::None,
        };
        if let crate::browser::BrowserAction::Play(slug) = action {
            // Anything the browser lists was installed from a package, so it
            // runs sandboxed — a game from a stranger is untrusted code.
            let path = crate::browser::games_root()
                .join(&slug)
                .join(makepad_game_pkg::GAME_FILE);
            let view = self.ui.widget(cx, ids!(arcade_view));
            let err = view
                .borrow_mut::<crate::arcade_view::ArcadeView>()
                .and_then(|mut v| {
                    v.load_game_with_trust(cx, &path, makepad_game_script::Trust::Downloaded)
                });
            if let Some(err) = err {
                log!("arcade: {slug} failed to load: {err}");
            }
            self.browser_open = false;
            self.ui
                .widget(cx, ids!(browser_panel))
                .set_visible(cx, false);
            self.ui.redraw(cx);
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        let caps = Capabilities::detect();
        log!("{}", caps.report());
        let tier = caps.tier();

        // The mic only exists where the chain behind it does; the text box is
        // in every tier (game.md §"Per-platform capability fallback").
        if !tier.shows_mic() {
            self.ui
                .widget(cx, ids!(voice_wave))
                .set_visible(cx, false);
        } else {
            // Escape is the big friendly push-to-talk key. Only meaningful
            // with the `voice` feature, where VoiceWave is the real widget.
            #[cfg(feature = "voice")]
            {
                let mut wave = self.ui.widget(cx, ids!(voice_wave));
                script_apply_eval!(cx, wave, { ptt_use_escape: true });
            }
        }

        // No explicit preference: prefer a CLI agent (no key needed), else
        // whichever direct API has a key. Settings can override per device.
        let backend = ai::choose_backend(None);
        match &backend {
            ai::BackendChoice::Ready(provider) => {
                if let Some(mut agent) = ai::build_agent(*provider, None) {
                    let dir = self.game_path().parent().map(|p| p.to_path_buf());
                    if let Some(dir) = &dir {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let config = ai::authoring_session(
                        dir.map(|d| d.to_string_lossy().to_string()),
                        &makepad_game_script::api_text(),
                        None,
                    );
                    self.session_id = Some(agent.create_session(cx, config));
                    self.agent = Some(agent);
                }
            }
            ai::BackendChoice::NeedsKey(p) => {
                ChatData::push(
                    ChatRole::System,
                    format!("{p:?} needs an API key — open Settings to add one, or pair from another device."),
                );
            }
            ai::BackendChoice::None => {
                ChatData::push(
                    ChatRole::System,
                    "No AI backend found. Open Settings to choose a provider.",
                );
            }
        }

        // The intent log starts from whatever game is on disk.
        let path = self.game_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        self.authoring = Some(Authoring::new(&path));
        if path.exists() {
            let view = self.ui.widget(cx, ids!(arcade_view));
            let err = view
                .borrow_mut::<crate::arcade_view::ArcadeView>()
                .and_then(|mut v| v.load_game(cx, &path));
            if let Some(err) = err {
                log!("arcade: startup eval failed: {err}");
            }
        }

        // Speech: the worker fills `playback`, the audio callback drains it.
        // Absent voice pack -> text-only replies, which is a tier, not a fault.
        let speech = caps
            .tts_voice
            .as_ref()
            .map(|_| makepad_converse::SpeechOutput::new("bm_fable.mkvoice"));
        let playback = speech.as_ref().map(|s| s.playback());
        cx.audio_output(0, move |info, output| {
            output.zero();
            // Game SFX first, speech layered on top of it.
            crate::synth::mix_into(output, info.sample_rate);
            if let Some(playback) = &playback {
                if let Ok(mut playback) = playback.lock() {
                    playback.mix_into(output, info.sample_rate);
                }
            }
        });
        self.speech = speech;

        self.set_status(cx, &format!("Ready — {}", tier.label()));
        self.caps = Some(caps);
        self.focus_armed = true;
        self.next_frame = cx.new_next_frame();
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        cx.use_audio_outputs(&devices.default_output());
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_game_render::script_mod(vm);
        crate::arcade_view::script_mod(vm);
        crate::settings::script_mod(vm);
        crate::browser::script_mod(vm);
        crate::chat::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if let Some(frame) = self.next_frame.is_event(event) {
            if self.focus_armed {
                self.focus_armed = false;
                self.ui.text_input(cx, ids!(input)).set_key_focus(cx);
            }
            // Drain the game's audio queue against the local listener. Doing
            // it here rather than in the view keeps the synth (a host concern)
            // out of the engine crates.
            let view = self.ui.widget(cx, ids!(arcade_view));
            let drained = view
                .borrow_mut::<crate::arcade_view::ArcadeView>()
                .map(|mut v| v.drain_audio());
            if let Some((requests, listener)) = drained {
                for name in audio::play_all(&requests, &listener) {
                    // A silent typo costs an agent a whole test cycle.
                    log!("arcade: unknown sound '{name}'");
                }
            }
            if chat::CHAT.read().map(|d| d.is_streaming).unwrap_or(false)
                && frame.time - self.spinner_at >= SPINNER_PERIOD
            {
                self.spinner_at = frame.time;
                self.spinner_phase = (self.spinner_phase + 1) % SPINNER.len();
                self.draw_activity(cx);
            }
            self.next_frame = cx.new_next_frame();
        }

        let Some(agent) = &mut self.agent else { return };
        for event in agent.handle_event(cx, event) {
            match event {
                AgentEvent::TextDelta { text, .. } => {
                    ChatData::push_delta(&text);
                    if let Some(speech) = self.speech.as_mut() {
                        speech.feed(&text);
                    }
                    cx.redraw_all();
                }
                AgentEvent::ToolRequest {
                    tool_name,
                    tool_input,
                    ..
                } => {
                    if let Some(activity) = Self::describe_tool(&tool_name, &tool_input) {
                        ChatData::set_activity(&activity);
                    }
                    // An edit landed: propose it now so the world rebuilds
                    // while the agent keeps working.
                    if matches!(tool_name.as_str(), "Edit" | "Write") {
                        self.land_edit(cx, "edit");
                    }
                }
                AgentEvent::TurnComplete { .. } => {
                    ChatData::end_stream();
                    if let Some(speech) = self.speech.as_mut() {
                        speech.flush();
                    }
                    self.current_prompt = None;
                    self.ui.widget(cx, ids!(stop_button)).set_visible(cx, false);
                    // Catch a final edit the tool stream did not announce.
                    self.land_edit(cx, "turn");
                    self.set_status(cx, "Ready.");
                    self.ui.text_input(cx, ids!(input)).set_key_focus(cx);
                    cx.redraw_all();
                }
                AgentEvent::SessionError { error, .. } => {
                    ChatData::push(ChatRole::System, format!("Session error: {error}"));
                    cx.redraw_all();
                }
                AgentEvent::PromptError { prompt_id, error } => {
                    if self.current_prompt == Some(prompt_id) {
                        ChatData::end_stream();
                        self.current_prompt = None;
                        self.ui.widget(cx, ids!(stop_button)).set_visible(cx, false);
                    }
                    ChatData::push(ChatRole::System, format!("Error: {error}"));
                    cx.redraw_all();
                }
                AgentEvent::SessionReady { .. } => {}
            }
        }
    }
}
