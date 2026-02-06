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
            selectable: true

            $User: RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 50 right: 8}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                flow: Overlay
                show_bg: true
                draw_bg +: {
                    color: #3a5a8a
                    radius: 8.0
                }

                $selectable: Markdown {
                    width: Fill
                    height: Fit
                    selectable: true
                    use_code_block_widget: true
                    body: ""
                    $code_block: View {
                        width: Fill
                        height: Fit
                        flow: Overlay
                        $code_view: CodeView {
                            keep_cursor_at_end: false
                            editor +: {
                                height: Fit
                                draw_bg +: { color: #1a1a2e }
                            }
                        }
                    }
                    $splash_block: View {
                        width: Fill
                        height: Fit
                        $splash_view: Splash {
                            width: Fill
                            height: Fit
                        }
                    }
                }

                View {
                    width: Fill
                    height: Fit
                    align: Align{x: 1.0}
                    $delete_button: ButtonFlat {
                        width: Fit
                        height: Fit
                        padding: Inset{top: 2 bottom: 2 left: 6 right: 6}
                        margin: Inset{top: 2 right: 2}
                        text: "x"
                        draw_text +: {
                            color: #888
                            text_style +: { font_size: 9 }
                        }
                    }
                }
            }

            $Assistant: RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 4 bottom: 4 left: 8 right: 50}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                flow: Overlay
                show_bg: true
                draw_bg +: {
                    color: #2a2a3a
                    radius: 8.0
                }

                RubberView {
                    width: Fill
                    height: Fit
                    smoothing: 0.3

                    $selectable: Markdown {
                        width: Fill
                        height: Fit
                        selectable: true
                        use_code_block_widget: true
                        body: ""
                        draw_text +: {
                            get_color: fn() {
                                let fade_chars = 50.0
                                let dist_from_end = self.total_chars - self.char_index
                                let t = clamp(dist_from_end / fade_chars, 0.0, 1.0)
                                let alpha = pow(t, 0.5)
                                return vec4(self.color.rgb, self.color.a * alpha)
                            }
                        }
                        $code_block: View {
                            width: Fill
                            height: Fit
                            flow: Overlay
                            $code_view: CodeView {
                                keep_cursor_at_end: true
                                editor +: {
                                    height: Fit
                                    draw_bg +: { color: #1a1a2e }
                                }
                            }
                        }
                        $splash_block: SolidView{
                            flow: Overlay
                            optimize: ViewOptimize.DrawList
                            width: Fill
                            height: Fit
                            $splash_view: Splash {
                                flow: Overlay
                                width: Fill
                                height: Fit
                            }
                        }
                    }
                }

                View {
                    width: Fill
                    height: Fit
                    align: Align{x: 1.0}
                    $delete_button: ButtonFlat {
                        width: Fit
                        height: Fit
                        padding: Inset{top: 2 bottom: 2 left: 6 right: 6}
                        margin: Inset{top: 2 right: 2}
                        text: "x"
                        draw_text +: {
                            color: #888
                            text_style +: { font_size: 9 }
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
                            width: 170
                            labels: ["Claude (ACP)" "Claude (API)" "Gemini" "Gemini Splash" "OpenAI"]
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
                        // Streaming message
                        let just_started = self.animating_msg != Some(item_id);
                        if just_started {
                            self.animating_msg = Some(item_id);
                        }

                        let (item_widget, _existed) =
                            list.item_with_existed(cx, item_id, id!($Assistant));
                        let text = if data.streaming_text.is_empty() {
                            "..."
                        } else {
                            &data.streaming_text
                        };
                        let mut markdown = item_widget.markdown(ids!($selectable));
                        markdown.set_text(cx, text);
                        if just_started {
                            markdown.reset_all_streaming_animations();
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
    GeminiSplash,
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
    #[rust]
    history_injected: bool,
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
            available_backends.push(BackendType::GeminiSplash);
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
        app.history_injected = false;
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
            BackendType::Gemini | BackendType::GeminiSplash => {
                Self::read_key_file("GOOGLE_API_KEY").map(|key| {
                    let backend = GeminiBackend::new(BackendConfig::Gemini {
                        api_key: key,
                        model: "gemini-2.0-flash".to_string(),
                    });
                    Box::new(StatelessBackendAdapter::new(Box::new(backend))) as Box<dyn Agent>
                })
            }
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

    fn system_prompt_for_backend(backend_type: BackendType) -> String {
        match backend_type {
            BackendType::GeminiSplash => {
                let splash_md = include_str!("../../../splash.md");
                format!(
                    r#"You are an AI agent that can create on-demand UI using Makepad's Splash scripting language.

You can answer questions normally using markdown. But when it makes sense to show something visually — a layout, a UI mockup, a styled card, a button arrangement, an animation, or anything graphical — you should embed a ```runsplash code block in your markdown response. The content inside a ```runsplash block is live Splash script that will be rendered as real interactive UI inline in the chat.

## How to use runsplash blocks

In your markdown output, write:

```runsplash
View{{
    flow: Down
    padding: 20
    spacing: 10
    Label{{text: "Hello from Splash!"}}
    Button{{text: "Click me"}}
}}
```

IMPORTANT: `use mod.prelude.widgets.*` is automatically prepended to every runsplash block — do NOT include it yourself. All widget names (View, Label, Button, etc.) are already in scope.

The block content is Splash script. It gets evaluated and rendered as a live widget tree. Do NOT wrap it in Root{{}} or Window{{}} — the content is placed directly inside a container.

## Let bindings for reusable components

You can define `let` bindings to create reusable widget templates. **`let` bindings must be defined ABOVE (before) the places where they are used.**

```runsplash
let MyCard = RoundedView{{
    width: Fill height: Fit
    padding: 15 flow: Down spacing: 8
    show_bg: true
    draw_bg.color: #334
    draw_bg.border_radius: 8.0
}}

View{{
    flow: Down spacing: 12 padding: 20
    MyCard{{
        Label{{text: "Card 1"}}
    }}
    MyCard{{
        Label{{text: "Card 2"}}
    }}
}}
```

## Syntax warnings

- Strings use double quotes only: `text: "Hello"`. No single quotes, no backticks.
- No commas between properties — they are whitespace-delimited.
- No semicolons.
- Every opening brace `{{` must have a matching closing brace `}}`.
- Property values that are widgets need the type name: `Label{{text: "hi"}}` not just `{{text: "hi"}}`.

## Splash Script Reference

Here is the complete Splash scripting manual. Use it to construct your UI responses:

{splash_md}

## Guidelines

- Use runsplash blocks for anything visual: UI mockups, styled cards, layouts, color palettes, shader demos, button groups, form layouts, etc.
- You can have multiple runsplash blocks in a single response, mixed with normal markdown text.
- Keep splash blocks focused — one concept per block when possible.
- Use `let` bindings at the top of a block to define reusable styled components, then instantiate them below.
- Use theme variables (theme.color_bg_app, theme.space_2, etc.) for consistent styling.
- You can use custom shaders with pixel: fn(){{}} for creative visual effects.
- For simple text answers, just use normal markdown without runsplash blocks.
- Be creative! Show off what Splash can do."#
                )
            }
            _ => "You are a helpful assistant. Be concise but thorough.".to_string(),
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
            self.history_injected = false;

            // Create session
            let config = SessionConfig {
                system_prompt: Some(Self::system_prompt_for_backend(backend_type)),
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
        self.history_injected = false;

        // Create new session for fresh conversation
        if let Some(agent) = &mut self.agent {
            let backend_type = self.active_backend.unwrap_or(BackendType::Gemini);
            let config = SessionConfig {
                system_prompt: Some(Self::system_prompt_for_backend(backend_type)),
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

        // Inject saved history on first prompt for stateless backends
        if !self.history_injected && agent.is_stateless() {
            let data = CHAT_DATA.read().unwrap();
            // Convert all messages except the one we just added
            let history: Vec<Message> = data.messages[..data.messages.len() - 1]
                .iter()
                .map(|m| match m.role {
                    ChatRole::User => Message::user(&m.text),
                    ChatRole::Assistant => Message::assistant(&m.text),
                })
                .collect();
            drop(data);
            if !history.is_empty() {
                agent.inject_history(session_id, history);
            }
            self.history_injected = true;
        }

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
            Some(BackendType::GeminiSplash) => "Active: Gemini Splash (UI Agent)",
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
            3 => Some(BackendType::GeminiSplash),
            4 => Some(BackendType::OpenAi),
            _ => None,
        }
    }

    fn index_from_backend_type(&self, backend_type: BackendType) -> Option<usize> {
        match backend_type {
            BackendType::ClaudeAcp => Some(0),
            BackendType::ClaudeApi => Some(1),
            BackendType::Gemini => Some(2),
            BackendType::GeminiSplash => Some(3),
            BackendType::OpenAi => Some(4),
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

        if self.ui.text_input(ids!($input)).escaped(actions) {
            self.cancel_request(cx);
        }

        if let Some(item) = self.ui.drop_down(ids!($backend_dropdown)).selected(actions) {
            if let Some(backend_type) = self.backend_type_from_index(item) {
                self.switch_backend(cx, backend_type);
            }
        }

        // Handle message deletion from delete buttons in portal list
        let chat_list = self.ui.widget(ids!($chat_list));
        let list = chat_list.portal_list(ids!($list));
        for (item_id, item) in list.items_with_actions(actions) {
            if item.button(ids!($delete_button)).pressed(actions) {
                let mut data = CHAT_DATA.write().unwrap();
                if item_id < data.messages.len() {
                    data.messages.remove(item_id);
                    data.save_to_disk();
                }
                drop(data);
                self.ui.redraw(cx);
            }
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        // Prefer Gemini Splash as default, fall back to first available
        let default_backend = if self.available_backends.contains(&BackendType::GeminiSplash) {
            Some(BackendType::GeminiSplash)
        } else {
            self.available_backends.first().copied()
        };
        if let Some(backend) = default_backend {
            self.switch_backend(cx, backend);
            // Set the dropdown to match
            if let Some(idx) = self.index_from_backend_type(backend) {
                self.ui.drop_down(ids!($backend_dropdown)).set_selected_item(cx, idx);
            }
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
                        let item_id = {
                            let mut data = CHAT_DATA.write().unwrap();
                            data.streaming_text.push_str(&text);
                            data.messages.len() // streaming item is at messages.len()
                        };
                        // Redraw the specific Splash widget that has DrawList optimization
                        let chat_list = self.ui.widget(ids!($chat_list));
                        let list = chat_list.portal_list(ids!($list));
                        if let Some((_template, item)) = list.get_item(item_id) {
                            // Clear query cache before searching (items are dynamically created)
                            item.clear_query_cache();
                            // Redraw the splash_view inside the markdown's splash_block
                            item.widget(ids!($splash_view)).redraw(cx);
                        }
                        cx.redraw_all();
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
                        cx.redraw_all();
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
                            cx.redraw_all();
                        }
                    AgentEvent::ToolRequest { .. } => {
                        // Not handling tools yet
                    }
                }
            }
        }
    }
}
