use makepad_ai::*;
use makepad_widgets2::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    mod.gc.set_static(mod)
    mod.gc.run()

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            $main_window: Window{
                window.inner_size: vec2(900, 700)
                window.title: "AI Chat"
                $body +: {
                    flow: Down
                    padding: Inset{left: 16 top: 16 right: 16 bottom: 16}
                    spacing: 12

                    // Header with backend selector
                    View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 12
                        align: Align{y: 0.5}

                        Label {
                            text: "AI Chat"
                            draw_text.text_style.font_size: 18
                        }

                        View { width: Fill height: 1 }

                        Label {
                            text: "Backend:"
                            draw_text.text_style.font_size: 12
                        }

                        $backend_dropdown: DropDown {
                            width: 150
                            labels: ["Claude" "OpenAI" "Gemini"]
                        }
                    }

                    // Chat messages area
                    $chat_scroll: ScrollYView {
                        width: Fill
                        height: Fill

                        View {
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 12
                            padding: Inset{right: 8}

                            $messages_container: View {
                                width: Fill
                                height: Fit
                                flow: Down
                                spacing: 12
                            }

                            // Streaming response indicator
                            $streaming_view: View {
                                width: Fill
                                height: Fit
                                visible: false

                                RoundedView {
                                    width: Fit
                                    height: Fit
                                    padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                                    draw_bg.color: #2a2a3a
                                    draw_bg.radius: 8.0

                                    $streaming_text: Label {
                                        width: Fit
                                        height: Fit
                                        text: ""
                                        draw_text.text_style.font_size: 13
                                        draw_text.color: #e0e0e0
                                    }
                                }
                            }
                        }
                    }

                    // Input area
                    View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 1.0}

                        $input: TextInput {
                            width: Fill
                            height: Fit
                            empty_text: "Type a message... (Enter to send)"
                        }

                        $send_button: Button {
                            text: "Send"
                            width: 80
                        }

                        $cancel_button: Button {
                            text: "Cancel"
                            width: 80
                            visible: false
                        }
                    }

                    // Status bar
                    View {
                        width: Fill
                        height: Fit

                        $status_label: Label {
                            width: Fill
                            height: Fit
                            text: "Set API keys via environment variables: ANTHROPIC_API_KEY, OPENAI_API_KEY, GOOGLE_API_KEY"
                            draw_text.text_style.font_size: 10
                            draw_text.color: #888
                        }
                    }
                }
            }
        }
    }

    // Message bubble template for user
    let UserBubble = View {
        width: Fill
        height: Fit
        flow: Right
        align: Align{x: 1.0}

        RoundedView {
            width: Fit
            height: Fit
            padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
            draw_bg.color: #3a5a8a
            draw_bg.radius: 8.0

            $text: Label {
                width: Fit
                height: Fit
                text: ""
                draw_text.text_style.font_size: 13
                draw_text.color: #fff
            }
        }
    }

    // Message bubble template for assistant
    let AssistantBubble = View {
        width: Fill
        height: Fit
        flow: Right
        align: Align{x: 0.0}

        RoundedView {
            width: Fit
            height: Fit
            padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
            draw_bg.color: #2a2a3a
            draw_bg.radius: 8.0

            $text: Label {
                width: Fit
                height: Fit
                text: ""
                draw_text.text_style.font_size: 13
                draw_text.color: #e0e0e0
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    ai_manager: AiManager,
    #[rust]
    messages: Vec<Message>,
    #[rust]
    current_response: String,
    #[rust]
    current_request: Option<RequestId>,
    #[rust]
    available_backends: Vec<String>,
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets2::script_mod(vm);

        let mut ai_manager = AiManager::new();
        let mut available_backends = vec![];

        // Add Claude backend if API key is available
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            ai_manager.add_backend(
                "claude",
                Box::new(ClaudeBackend::new(BackendConfig::Claude {
                    api_key: Some(key),
                    oauth_token: None,
                    model: "claude-sonnet-4-5-20250929".to_string(),
                })),
            );
            available_backends.push("claude".to_string());
        }

        // Check for Claude OAuth token (for Pro/Max subscribers)
        if let Ok(token) = std::env::var("CLAUDE_OAUTH_TOKEN") {
            if !available_backends.contains(&"claude".to_string()) {
                ai_manager.add_backend(
                    "claude",
                    Box::new(ClaudeBackend::new(BackendConfig::Claude {
                        api_key: None,
                        oauth_token: Some(token),
                        model: "claude-sonnet-4-5-20250929".to_string(),
                    })),
                );
                available_backends.push("claude".to_string());
            }
        }

        // Add OpenAI backend if API key is available
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            ai_manager.add_backend(
                "openai",
                Box::new(OpenAiBackend::new(BackendConfig::OpenAI {
                    api_key: key,
                    model: "gpt-4o".to_string(),
                    base_url: None,
                    reasoning_effort: None,
                })),
            );
            available_backends.push("openai".to_string());
        }

        // Add Gemini backend if API key is available
        if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
            ai_manager.add_backend(
                "gemini",
                Box::new(GeminiBackend::new(BackendConfig::Gemini {
                    api_key: key,
                    model: "gemini-2.0-flash".to_string(),
                })),
            );
            available_backends.push("gemini".to_string());
        }

        let mut app = App::from_script_mod(vm, self::script_mod);
        app.ai_manager = ai_manager;
        app.available_backends = available_backends;
        app.messages = vec![];
        app.current_response = String::new();
        app.current_request = None;
        println!("HERE!")
        app
    }

    fn send_message(&mut self, cx: &mut Cx) {
        let input = self.ui.text_input(ids!($input));
        let text = input.text();
        if text.trim().is_empty() {
            return;
        }

        // Add user message
        self.messages.push(Message::user(&text));
        input.set_text(cx, "");

        // Send to AI
        let request = AiRequest {
            messages: self.messages.clone(),
            system_prompt: Some(
                "You are a helpful assistant. Be concise but thorough.".to_string(),
            ),
            stream: true,
            ..Default::default()
        };

        self.current_response.clear();
        self.current_request = self.ai_manager.send_request(cx, request);

        // Update UI state
        self.ui.view(ids!($cancel_button)).set_visible(cx, true);
        self.ui.view(ids!($streaming_view)).set_visible(cx, true);

        self.rebuild_chat_ui(cx);
    }

    fn cancel_request(&mut self, cx: &mut Cx) {
        if let Some(request_id) = self.current_request.take() {
            self.ai_manager.cancel_request(cx, request_id);

            // If we had partial response, add it as a message
            if !self.current_response.is_empty() {
                self.messages
                    .push(Message::assistant(&self.current_response));
                self.current_response.clear();
            }

            self.ui.view(ids!($cancel_button)).set_visible(cx, false);
            self.ui.view(ids!($streaming_view)).set_visible(cx, false);
            self.rebuild_chat_ui(cx);
        }
    }

    fn rebuild_chat_ui(&mut self, cx: &mut Cx) {
        // For now, just update the streaming text
        // A full implementation would dynamically create message bubbles

        let streaming_text = self.ui.label(ids!($streaming_text));
        if self.current_request.is_some() {
            streaming_text.set_text(cx, &self.current_response);
        }

        // Update status
        let status = if self.available_backends.is_empty() {
            "No backends configured. Set API keys: ANTHROPIC_API_KEY, OPENAI_API_KEY, or GOOGLE_API_KEY"
        } else {
            let backend_name = self.ai_manager.active_backend_name().unwrap_or("none");
            &format!(
                "Active: {} | Available: {}",
                backend_name,
                self.available_backends.join(", ")
            )
        };
        self.ui.label(ids!($status_label)).set_text(cx, status);

        self.ui.redraw(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Handle send button
        if self.ui.button(ids!($send_button)).clicked(actions) {
            self.send_message(cx);
        }

        // Handle cancel button
        if self.ui.button(ids!($cancel_button)).clicked(actions) {
            self.cancel_request(cx);
        }

        // Handle Enter key in input
        if self.ui.text_input(ids!($input)).returned(actions).is_some() {
            self.send_message(cx);
        }

        // Handle backend dropdown
        if let Some(item) = self.ui.drop_down(ids!($backend_dropdown)).selected(actions) {
            let backend_name = match item {
                0 => "claude",
                1 => "openai",
                2 => "gemini",
                _ => "claude",
            };
            self.ai_manager.set_active(backend_name);
            self.rebuild_chat_ui(cx);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        // Handle AI events
        for ai_event in self.ai_manager.handle_event(cx, event) {
            match ai_event {
                AiEvent::StreamDelta { delta, .. } => match delta {
                    StreamDelta::TextDelta { text } => {
                        self.current_response.push_str(&text);
                        self.rebuild_chat_ui(cx);
                    }
                    StreamDelta::Error { message } => {
                        log!("AI Error: {}", message);
                        self.ui
                            .label(ids!($status_label))
                            .set_text(cx, &format!("Error: {}", message));
                    }
                    _ => {}
                },
                AiEvent::Complete { response, .. } => {
                    self.messages.push(response.message);
                    self.current_response.clear();
                    self.current_request = None;

                    self.ui.view(ids!($cancel_button)).set_visible(cx, false);
                    self.ui.view(ids!($streaming_view)).set_visible(cx, false);

                    let usage_str = format!(
                        "Tokens - in: {}, out: {}",
                        response.usage.input_tokens, response.usage.output_tokens
                    );
                    self.ui.label(ids!($status_label)).set_text(cx, &usage_str);

                    self.rebuild_chat_ui(cx);
                }
                AiEvent::Error { error, .. } => {
                    log!("AI Error: {}", error);
                    self.current_request = None;

                    self.ui.view(ids!($cancel_button)).set_visible(cx, false);
                    self.ui.view(ids!($streaming_view)).set_visible(cx, false);
                    self.ui
                        .label(ids!($status_label))
                        .set_text(cx, &format!("Error: {}", error));
                }
            }
        }
    }
}
