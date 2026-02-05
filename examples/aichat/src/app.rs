use makepad_ai::*;
use makepad_widgets2::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.CodeView

    // Chat list widget
    let ChatList = #(ChatList::register_widget(vm)) {
        width: Fill
        height: Fill

        $list: PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: false
            auto_tail: true
            smooth_tail: true

            $User: RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 50 right: 8}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #3a5a8a
                    radius: 8.0
                }

                $md: Markdown {
                    width: Fill
                    height: Fit
                    body: ""
                }
            }

            $Assistant: RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 8 right: 50}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #2a2a3a
                    radius: 8.0
                }

                $md: Markdown {
                    width: Fill
                    height: Fit
                    body: ""
                }
            }

            $Streaming: RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 8 right: 50}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #2a3a2a
                    radius: 8.0
                }

                $md: Markdown {
                    width: Fill
                    height: Fit
                    body: "..."
                }
            }
        }
    }

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
                            labels: ["Gemini" "Claude" "OpenAI"]
                        }
                    }

                    // Chat messages area using PortalList
                    $chat_list: ChatList {}

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
                            text: "Put API keys in current directory (ANTHROPIC_API_KEY, OPENAI_API_KEY, GOOGLE_API_KEY)"
                            draw_text.text_style.font_size: 10
                            draw_text.color: #888
                        }
                    }
                }
            }
        }
    }
}

// Store for chat messages accessible to ChatList widget
pub static CHAT_DATA: std::sync::RwLock<ChatData> = std::sync::RwLock::new(ChatData {
    messages: Vec::new(),
    streaming_text: String::new(),
    is_streaming: false,
});

pub struct ChatData {
    pub messages: Vec<Message>,
    pub streaming_text: String,
    pub is_streaming: bool,
}

// ChatList widget that uses PortalList to display messages
#[derive(Script, ScriptHook, Widget)]
pub struct ChatList {
    #[deref]
    view: View,
}

impl Widget for ChatList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let data = CHAT_DATA.read().unwrap();

        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                let msg_count = data.messages.len();
                // If streaming, add one more item for the streaming response
                let items_len = if data.is_streaming {
                    msg_count + 1
                } else {
                    msg_count
                };
                list.set_item_range(cx, 0, items_len);

                while let Some(item_id) = list.next_visible_item(cx) {
                    // Check if this is a real message
                    if let Some(msg) = data.messages.get(item_id) {
                        // Get text content from message
                        let text: String = msg
                            .content
                            .iter()
                            .filter_map(|block| {
                                if let ContentBlock::Text { text } = block {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");

                        let template = match msg.role {
                            MessageRole::User => id!($User),
                            MessageRole::Assistant => id!($Assistant),
                            _ => id!($User),
                        };
                        let item_widget = list.item(cx, item_id, template);
                        item_widget.widget(ids!($md)).set_text(cx, &text);
                        item_widget.draw_all_unscoped(cx);
                        continue;
                    }

                    // Streaming placeholder (only when item_id equals msg_count)
                    if data.is_streaming && item_id == msg_count {
                        let item_widget = list.item(cx, item_id, id!($Streaming));
                        let text = if data.streaming_text.is_empty() {
                            "..."
                        } else {
                            &data.streaming_text
                        };
                        item_widget.widget(ids!($md)).set_text(cx, text);
                        item_widget.draw_all_unscoped(cx);
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl ChatListRef {
    pub fn scroll_to_end(&self, cx: &mut Cx) {
        if let Some(inner) = self.borrow() {
            let list = inner.view.portal_list(ids!($list));
            list.set_tail_range(true);
            list.scroll_to_end(cx);
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
    current_request: Option<RequestId>,
    #[rust]
    available_backends: Vec<String>,
}

impl App {
    /// Read API key from file, trimming whitespace
    fn read_key_file(path: &str) -> Option<String> {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets2::script_mod(vm);
        crate::makepad_code_editor2::script_mod(vm);

        let mut ai_manager = AiManager::new();
        let mut available_backends = vec![];

        // Add Claude backend if API key is available
        let claude_key = Self::read_key_file("ANTHROPIC_API_KEY");
        if let Some(key) = claude_key {
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
        let claude_oauth = Self::read_key_file("CLAUDE_OAUTH_TOKEN");
        if let Some(token) = claude_oauth {
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
        let openai_key = Self::read_key_file("OPENAI_API_KEY");
        if let Some(key) = openai_key {
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
        let google_key = Self::read_key_file("GOOGLE_API_KEY");
        if let Some(key) = google_key {
            ai_manager.add_backend(
                "gemini",
                Box::new(GeminiBackend::new(BackendConfig::Gemini {
                    api_key: key,
                    model: "gemini-2.0-flash".to_string(),
                })),
            );
            available_backends.push("gemini".to_string());
        }

        // Set Gemini as default if available
        if available_backends.contains(&"gemini".to_string()) {
            ai_manager.set_active("gemini");
        }

        let mut app = App::from_script_mod(vm, self::script_mod);
        app.ai_manager = ai_manager;
        app.available_backends = available_backends;
        app.current_request = None;
        app
    }

    fn send_message(&mut self, cx: &mut Cx) {
        let input = self.ui.text_input(ids!($input));
        let text = input.text();
        if text.trim().is_empty() {
            return;
        }

        // Add user message to shared data
        let items_len = {
            let mut data = CHAT_DATA.write().unwrap();
            data.messages.push(Message::user(&text));
            data.streaming_text.clear();
            data.is_streaming = true;
            // +1 for streaming placeholder
            data.messages.len() + 1
        };
        input.set_text(cx, "");

        // Send to AI
        let messages = CHAT_DATA.read().unwrap().messages.clone();
        let request = AiRequest {
            messages,
            system_prompt: Some(
                "You are a helpful assistant. Be concise but thorough.".to_string(),
            ),
            stream: true,
            ..Default::default()
        };

        self.current_request = self.ai_manager.send_request(cx, request);

        // Update UI state
        self.ui.view(ids!($cancel_button)).set_visible(cx, true);
        self.update_status(cx);

        // Scroll to end and enable tailing
        let chat_list = self.ui.widget(ids!($chat_list));
        let list = chat_list.portal_list(ids!($list));
        list.set_tail_range(true);
        list.set_first_id_and_scroll(items_len.saturating_sub(1), 0.0);

        self.ui.redraw(cx);
    }

    fn cancel_request(&mut self, cx: &mut Cx) {
        if let Some(request_id) = self.current_request.take() {
            self.ai_manager.cancel_request(cx, request_id);

            // If we had partial response, add it as a message
            {
                let mut data = CHAT_DATA.write().unwrap();
                if !data.streaming_text.is_empty() {
                    let text = std::mem::take(&mut data.streaming_text);
                    data.messages.push(Message::assistant(&text));
                }
                data.is_streaming = false;
            }

            self.ui.view(ids!($cancel_button)).set_visible(cx, false);
            self.ui.redraw(cx);
        }
    }

    fn update_status(&self, cx: &mut Cx) {
        let status = if self.available_backends.is_empty() {
            "No backends configured. Put API keys in current directory".to_string()
        } else {
            let backend_name = self.ai_manager.active_backend_name().unwrap_or("none");
            format!(
                "Active: {} | Available: {}",
                backend_name,
                self.available_backends.join(", ")
            )
        };
        self.ui.label(ids!($status_label)).set_text(cx, &status);
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
                0 => "gemini",
                1 => "claude",
                2 => "openai",
                _ => "gemini",
            };
            self.ai_manager.set_active(backend_name);
            self.update_status(cx);
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
                        {
                            let mut data = CHAT_DATA.write().unwrap();
                            data.streaming_text.push_str(&text);
                        }
                        self.ui.redraw(cx);
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
                    {
                        let mut data = CHAT_DATA.write().unwrap();
                        // Debug: compare backend accumulated text vs app streaming text
                        let backend_text: String = response
                            .message
                            .content
                            .iter()
                            .filter_map(|b| {
                                if let ContentBlock::Text { text } = b {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("");
                        log!(
                            "Complete: backend={} chars, app={} chars",
                            backend_text.len(),
                            data.streaming_text.len()
                        );
                        if backend_text != data.streaming_text {
                            log!(
                                "MISMATCH! Backend first 200: {:?}",
                                backend_text.chars().take(200).collect::<String>()
                            );
                            log!(
                                "MISMATCH! App first 200: {:?}",
                                data.streaming_text.chars().take(200).collect::<String>()
                            );
                        }
                        data.messages.push(response.message);
                        data.streaming_text.clear();
                        data.is_streaming = false;
                    }
                    self.current_request = None;

                    self.ui.view(ids!($cancel_button)).set_visible(cx, false);

                    let usage_str = format!(
                        "Tokens - in: {}, out: {}",
                        response.usage.input_tokens, response.usage.output_tokens
                    );
                    self.ui.label(ids!($status_label)).set_text(cx, &usage_str);

                    self.ui.redraw(cx);
                }
                AiEvent::Error { error, .. } => {
                    log!("AI Error: {}", error);
                    {
                        let mut data = CHAT_DATA.write().unwrap();
                        data.is_streaming = false;
                    }
                    self.current_request = None;

                    self.ui.view(ids!($cancel_button)).set_visible(cx, false);
                    self.ui
                        .label(ids!($status_label))
                        .set_text(cx, &format!("Error: {}", error));
                    self.ui.redraw(cx);
                }
            }
        }
    }
}
