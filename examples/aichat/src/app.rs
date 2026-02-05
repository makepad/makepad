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
            smooth_tail_speed: 0.1
            selectable: true

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

                $selectable: Markdown {
                    width: Fill
                    height: Fit
                    selectable: true
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

                $selectable: Markdown {
                    width: Fill
                    height: Fit
                    selectable: true
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
                    color: #2a2a3a
                    radius: 8.0
                }

                $selectable: Markdown {
                    width: Fill
                    height: Fit
                    selectable: true
                    body: "..."

                    draw_text +: {
                        get_color: fn() {
                            // Fade in the last 50 characters with exponential curve
                            let fade_chars = 50.0
                            let dist_from_end = self.total_chars - self.char_index
                            let t = clamp(dist_from_end / fade_chars, 0.0, 1.0)
                            // Exponential: front quarter fades, rest is solid
                            let alpha = pow(t,0.5)
                            return vec4(self.color.rgb, self.color.a * alpha)
                        }
                    }
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
                            labels: ["Claude (ACP)" "Claude (API)" "Gemini" "OpenAI"]
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

                        $clear_button: Button {
                            text: "Clear"
                            width: 80
                        }
                    }

                    // Status bar
                    View {
                        width: Fill
                        height: Fit

                        $status_label: Label {
                            width: Fill
                            height: Fit
                            text: "Initializing..."
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

const CHAT_SAVE_PATH: &str = "aichat_history.json";

use makepad_widgets2::makepad_platform::makepad_micro_serde::*;

#[derive(SerJson, DeJson)]
struct SavedMessage {
    role: String,
    content: String,
}

#[derive(SerJson, DeJson, Default)]
struct SavedHistory {
    messages: Vec<SavedMessage>,
}

/// Chat message for display (simplified from AI Message type)
#[derive(Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ChatRole {
    User,
    Assistant,
}

pub struct ChatData {
    pub messages: Vec<ChatMessage>,
    pub streaming_text: String,
    pub is_streaming: bool,
}

impl ChatData {
    pub fn save_to_disk(&self) {
        let saved = SavedHistory {
            messages: self
                .messages
                .iter()
                .map(|m| SavedMessage {
                    role: match m.role {
                        ChatRole::User => "user".to_string(),
                        ChatRole::Assistant => "assistant".to_string(),
                    },
                    content: m.text.clone(),
                })
                .collect(),
        };
        let json = saved.serialize_json();
        let _ = std::fs::write(CHAT_SAVE_PATH, json);
    }

    pub fn load_from_disk() -> Vec<ChatMessage> {
        std::fs::read_to_string(CHAT_SAVE_PATH)
            .ok()
            .and_then(|s| SavedHistory::deserialize_json(&s).ok())
            .map(|saved| {
                saved
                    .messages
                    .into_iter()
                    .map(|m| ChatMessage {
                        role: if m.role == "user" {
                            ChatRole::User
                        } else {
                            ChatRole::Assistant
                        },
                        text: m.content,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ChatList widget that uses PortalList to display messages
#[derive(Script, ScriptHook, Widget)]
pub struct ChatList {
    #[deref]
    view: View,
    #[rust]
    animating_msg: Option<usize>,
}

impl Widget for ChatList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let data = CHAT_DATA.read().unwrap();

        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                let msg_count = data.messages.len();
                let items_len = if data.is_streaming {
                    msg_count + 1
                } else {
                    msg_count
                };
                list.set_item_range(cx, 0, items_len);

                while let Some(item_id) = list.next_visible_item(cx) {
                    if data.is_streaming && item_id == msg_count {
                        let just_started = self.animating_msg != Some(item_id);
                        if just_started {
                            self.animating_msg = Some(item_id);
                        }

                        let (item_widget, existed) =
                            list.item_with_existed(cx, item_id, id!($Streaming));
                        let text = if data.streaming_text.is_empty() {
                            "..."
                        } else {
                            &data.streaming_text
                        };
                        let mut markdown = item_widget.markdown(ids!($selectable));
                        markdown.set_text(cx, text);
                        // Reset animation on first draw, then keep animating
                        if just_started {
                            markdown.reset_streaming_animation();
                        } else {
                            markdown.start_streaming_animation();
                        }
                        item_widget.draw_all_unscoped(cx);
                        continue;
                    }

                    if let Some(msg) = data.messages.get(item_id) {
                        let is_animating = self.animating_msg == Some(item_id);
                        let template = match msg.role {
                            ChatRole::User => id!($User),
                            ChatRole::Assistant if is_animating => id!($Streaming),
                            ChatRole::Assistant => id!($Assistant),
                        };
                        let item_widget = list.item(cx, item_id, template);
                        let mut markdown = item_widget.markdown(ids!($selectable));
                        markdown.set_text(cx, &msg.text);

                        if is_animating {
                            markdown.stop_streaming_animation();
                        }

                        item_widget.draw_all_unscoped(cx);

                        if is_animating {
                            if markdown.is_streaming_animation_done() {
                                self.animating_msg = None;
                            }
                        }
                        continue;
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

/// Available backend types
#[derive(Clone, Copy, PartialEq, Eq)]
enum BackendType {
    ClaudeAcp,
    ClaudeApi,
    Gemini,
    OpenAi,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    agent: Option<Box<dyn Agent>>,
    #[rust]
    session_id: Option<SessionId>,
    #[rust]
    current_prompt: Option<PromptId>,
    #[rust]
    available_backends: Vec<BackendType>,
    #[rust]
    active_backend: Option<BackendType>,
}

impl App {
    fn read_key_file(path: &str) -> Option<String> {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets2::script_mod(vm);
        crate::makepad_code_editor2::script_mod(vm);

        let mut available_backends = vec![];

        // Check what's available
        if ClaudeAcpAgent::is_available() {
            available_backends.push(BackendType::ClaudeAcp);
        }

        if Self::read_key_file("ANTHROPIC_API_KEY").is_some() {
            available_backends.push(BackendType::ClaudeApi);
        }

        if Self::read_key_file("GOOGLE_API_KEY").is_some() {
            available_backends.push(BackendType::Gemini);
        }

        if Self::read_key_file("OPENAI_API_KEY").is_some() {
            available_backends.push(BackendType::OpenAi);
        }

        // Load saved chat history
        {
            let mut data = CHAT_DATA.write().unwrap();
            data.messages = ChatData::load_from_disk();
        }

        let mut app = App::from_script_mod(vm, self::script_mod);
        app.available_backends = available_backends;
        app.agent = None;
        app.session_id = None;
        app.current_prompt = None;
        app.active_backend = None;
        app
    }

    fn create_agent(&self, backend_type: BackendType) -> Option<Box<dyn Agent>> {
        match backend_type {
            BackendType::ClaudeAcp => {
                if ClaudeAcpAgent::is_available() {
                    Some(Box::new(ClaudeAcpAgent::new()))
                } else {
                    None
                }
            }
            BackendType::ClaudeApi => Self::read_key_file("ANTHROPIC_API_KEY").map(|key| {
                let backend = ClaudeBackend::new(BackendConfig::Claude {
                    api_key: Some(key),
                    oauth_token: None,
                    model: "claude-sonnet-4-5-20250929".to_string(),
                });
                Box::new(StatelessBackendAdapter::new(Box::new(backend))) as Box<dyn Agent>
            }),
            BackendType::Gemini => Self::read_key_file("GOOGLE_API_KEY").map(|key| {
                let backend = GeminiBackend::new(BackendConfig::Gemini {
                    api_key: key,
                    model: "gemini-2.0-flash".to_string(),
                });
                Box::new(StatelessBackendAdapter::new(Box::new(backend))) as Box<dyn Agent>
            }),
            BackendType::OpenAi => Self::read_key_file("OPENAI_API_KEY").map(|key| {
                let backend = OpenAiBackend::new(BackendConfig::OpenAI {
                    api_key: key,
                    model: "gpt-4o".to_string(),
                    base_url: None,
                    reasoning_effort: None,
                });
                Box::new(StatelessBackendAdapter::new(Box::new(backend))) as Box<dyn Agent>
            }),
        }
    }

    fn switch_backend(&mut self, cx: &mut Cx, backend_type: BackendType) {
        if self.active_backend == Some(backend_type) {
            return;
        }

        // Create new agent
        if let Some(agent) = self.create_agent(backend_type) {
            self.agent = Some(agent);
            self.active_backend = Some(backend_type);
            self.session_id = None;
            self.current_prompt = None;

            // Create session
            let config = SessionConfig {
                system_prompt: Some(
                    "You are a helpful assistant. Be concise but thorough.".to_string(),
                ),
                ..Default::default()
            };
            if let Some(agent) = &mut self.agent {
                self.session_id = Some(agent.create_session(cx, config));
            }

            self.update_status(cx);
        }
    }

    fn clear_chat(&mut self, cx: &mut Cx) {
        {
            let mut data = CHAT_DATA.write().unwrap();
            data.messages.clear();
            data.streaming_text.clear();
            data.is_streaming = false;
            data.save_to_disk();
        }

        // Create new session for fresh conversation
        if let Some(agent) = &mut self.agent {
            let config = SessionConfig {
                system_prompt: Some(
                    "You are a helpful assistant. Be concise but thorough.".to_string(),
                ),
                ..Default::default()
            };
            self.session_id = Some(agent.create_session(cx, config));
        }

        self.ui.redraw(cx);
    }

    fn send_message(&mut self, cx: &mut Cx) {
        let input = self.ui.text_input(ids!($input));
        let text = input.text();
        if text.trim().is_empty() {
            return;
        }

        let (agent, session_id) = match (&mut self.agent, self.session_id) {
            (Some(agent), Some(session_id)) => (agent, session_id),
            _ => {
                return;
            }
        };

        // Add user message to display
        let items_len = {
            let mut data = CHAT_DATA.write().unwrap();
            data.messages.push(ChatMessage {
                role: ChatRole::User,
                text: text.clone(),
            });
            data.streaming_text.clear();
            data.is_streaming = true;
            data.messages.len() + 1
        };
        input.set_text(cx, "");

        // Send prompt to agent
        let prompt_id = agent.send_prompt(cx, session_id, &text);
        self.current_prompt = Some(prompt_id);

        // Update UI
        self.ui.view(ids!($cancel_button)).set_visible(cx, true);

        let chat_list = self.ui.widget(ids!($chat_list));
        let list = chat_list.portal_list(ids!($list));
        list.set_tail_range(true);
        list.set_first_id_and_scroll(items_len.saturating_sub(1), 0.0);

        self.ui.redraw(cx);
    }

    fn cancel_request(&mut self, cx: &mut Cx) {
        if let (Some(agent), Some(prompt_id)) = (&mut self.agent, self.current_prompt.take()) {
            agent.cancel_prompt(cx, prompt_id);

            {
                let mut data = CHAT_DATA.write().unwrap();
                if !data.streaming_text.is_empty() {
                    let text = std::mem::take(&mut data.streaming_text);
                    data.messages.push(ChatMessage {
                        role: ChatRole::Assistant,
                        text,
                    });
                }
                data.is_streaming = false;
            }

            self.ui.view(ids!($cancel_button)).set_visible(cx, false);
            self.ui.redraw(cx);
        }
    }

    fn update_status(&self, cx: &mut Cx) {
        let status = match self.active_backend {
            Some(BackendType::ClaudeAcp) => "Active: Claude (ACP via Zed)",
            Some(BackendType::ClaudeApi) => "Active: Claude (API)",
            Some(BackendType::Gemini) => "Active: Gemini",
            Some(BackendType::OpenAi) => "Active: OpenAI",
            None => "No backend selected",
        };
        self.ui.label(ids!($status_label)).set_text(cx, status);
    }

    fn backend_type_from_index(&self, index: usize) -> Option<BackendType> {
        match index {
            0 => Some(BackendType::ClaudeAcp),
            1 => Some(BackendType::ClaudeApi),
            2 => Some(BackendType::Gemini),
            3 => Some(BackendType::OpenAi),
            _ => None,
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(ids!($send_button)).clicked(actions) {
            self.send_message(cx);
        }

        if self.ui.button(ids!($cancel_button)).clicked(actions) {
            self.cancel_request(cx);
        }

        if self.ui.button(ids!($clear_button)).clicked(actions) {
            self.clear_chat(cx);
        }

        if self.ui.text_input(ids!($input)).returned(actions).is_some() {
            self.send_message(cx);
        }

        if let Some(item) = self.ui.drop_down(ids!($backend_dropdown)).selected(actions) {
            if let Some(backend_type) = self.backend_type_from_index(item) {
                self.switch_backend(cx, backend_type);
            }
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        // Initialize with first available backend
        if let Some(&first_backend) = self.available_backends.first() {
            self.switch_backend(cx, first_backend);
        }
        self.update_status(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        // Handle agent events
        if let Some(agent) = &mut self.agent {
            for event in agent.handle_event(cx, event) {
                match event {
                    AgentEvent::SessionReady { .. } => {
                        self.update_status(cx);
                    }
                    AgentEvent::SessionError { error, .. } => {
                        self.ui
                            .label(ids!($status_label))
                            .set_text(cx, &format!("Error: {}", error));
                    }
                    AgentEvent::TextDelta { text, .. } => {
                        {
                            let mut data = CHAT_DATA.write().unwrap();
                            data.streaming_text.push_str(&text);
                        }
                        self.ui.redraw(cx);
                    }
                    AgentEvent::TurnComplete { .. } => {
                        {
                            let mut data = CHAT_DATA.write().unwrap();
                            let text = std::mem::take(&mut data.streaming_text);
                            if !text.is_empty() {
                                data.messages.push(ChatMessage {
                                    role: ChatRole::Assistant,
                                    text,
                                });
                            }
                            data.is_streaming = false;
                            data.save_to_disk();
                        }
                        self.current_prompt = None;
                        self.ui.view(ids!($cancel_button)).set_visible(cx, false);
                        self.ui.redraw(cx);
                    }
                    AgentEvent::PromptError { error, .. } => {
                        {
                            let mut data = CHAT_DATA.write().unwrap();
                            data.is_streaming = false;
                        }
                        self.current_prompt = None;
                        self.ui.view(ids!($cancel_button)).set_visible(cx, false);
                        self.ui
                            .label(ids!($status_label))
                            .set_text(cx, &format!("Error: {}", error));
                        self.ui.redraw(cx);
                    }
                    AgentEvent::ToolRequest { .. } => {
                        // Not handling tools yet
                    }
                }
            }
        }
    }
}
