use crate::makepad_live_id::*;
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::env;

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

live_design! {
    import makepad_widgets::theme_desktop_dark::*;

    App = {{App}} {
        ui: <Window> {
            body = {
                show_bg: true
                flow: Down,
                padding: {
                    top: 10
                        left: 100.0,
                    },

                width: Fill,
                height: Fill

                draw_bg: {
                    fn pixel(self) -> vec4 {
                        return mix(#3, #1, self.pos.y);
                    }
                }
            <ScrollYView>{
                flow: Down
                spacing: 20,
                height: Fill

                message_input = <TextInput> {
                    text: "Message"
                    width: 500,
                    height: Fit,
                    draw_bg: {
                        color: #1
                    }

                }

                send_button = <Button> {
                    icon_walk: {margin: {left: 10}, width: 16, height: Fit}
                    text: "send"
                }

                message_label = <Label> {
                    width: 300,
                    height: Fit
                    draw_text: {
                        color: #f
                        text_style:{font_size: 30}
                    },

                    text: r#"Output"#
                }
            }
        }}
    }
}

app_main!(App);

#[derive(Live, LiveHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    conversation_history: Vec<ChatMessage>,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl App {
    fn update_message_label(&mut self, cx: &mut Cx) {
        let label = self.ui.label(id!(message_label));
        let mut conversation_text = String::new();

        for message in &self.conversation_history {
            let role_label = if message.role == Some("user".to_string()) {
                "User:"
            } else {
                "Assistant:"
            };
            conversation_text.push_str(&format!(
                "{}\n{}\n\n",
                role_label,
                message.content.as_ref().unwrap()
            ));
        }

        label.set_text_and_redraw(cx, &conversation_text);
    }

    // This performs an event-based HTTP request: it has no relationship with the response.
    // The response will be received and processed by AppMain's handle_event.

    fn send_message(&self, cx: &mut Cx, message: String) {
        let request_id = live_id!(SendChatMessage);
        let completion_url = format!("{OPENAI_BASE_URL}/chat/completions");
        let mut request = HttpRequest::new(completion_url, HttpMethod::POST);
        request.set_is_streaming();
        let ai_key = env::var("OPENAI_API_KEY").unwrap();
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_header("Authorization".to_string(), format!("Bearer {ai_key}"));

        request.set_json_body(ChatPrompt {
            messages: vec![ChatMessage {
                content: Some(message),
                role: Some("user".to_string()),
                refusal: Some(JsonValue::Null),
            }],
            model: "gpt-4o".to_string(),
            max_tokens: 1000,
            stream: true,
        });
        self.ui
            .label(id!(message_label))
            .set_text_and_redraw(cx, "Answering:..\n");
        cx.http_request(request_id, request);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(id!(send_button)).clicked(&actions)
            || self
                .ui
                .text_input(id!(message_input))
                .returned(&actions)
                .is_some()
        {
            let user_prompt = self.ui.text_input(id!(message_input)).text();
            self.send_message(cx, user_prompt);
        }
    }

    fn handle_network_responses(&mut self, cx: &mut Cx, responses: &NetworkResponsesEvent) {
        let label = self.ui.label(id!(message_label));
        for event in responses {
            match &event.response {
                NetworkResponse::HttpStreamResponse(response) => {
                    let data = response.get_string_body().unwrap();
                    for data in data.split("\n\n") {
                        if let Some(data) = data.strip_prefix("data: ") {
                            if data != "[DONE]" {
                                match ChatResponse::deserialize_json(data) {
                                    Ok(chat_response) => {
                                        if let Some(content) = &chat_response.choices[0]
                                            .delta
                                            .as_ref()
                                            .unwrap()
                                            .content
                                        {
                                            let msg = format!("{}{}", label.text(), content);
                                            label.set_text_and_redraw(cx, &msg);
                                        }
                                    }
                                    Err(e) => {
                                        println!("JSon parse error {:?} {}", e, data);
                                    }
                                }
                            }
                        }
                    }
                }
                NetworkResponse::HttpStreamComplete => {
                    println!("Stream complete");
                }
                NetworkResponse::HttpResponse(response) => match event.request_id {
                    live_id!(SendChatMessage) => {
                        let label = self.ui.label(id!(message_label));
                        if response.status_code == 200 {
                            let chat_response = response.get_json_body::<ChatResponse>().unwrap();
                            label.set_text_and_redraw(
                                cx,
                                &chat_response.choices[0]
                                    .message
                                    .as_ref()
                                    .unwrap()
                                    .content
                                    .as_ref()
                                    .unwrap(),
                            );
                        } else {
                            label.set_text_and_redraw(cx, "Failed to connect with OpenAI");
                        }
                    }
                    _ => (),
                },
                NetworkResponse::HttpRequestError(error) => {
                    let label = self.ui.label(id!(message_label));
                    label.set_text_and_redraw(
                        cx,
                        &format!("Failed to connect with OpenAI {:?}", error),
                    );
                }
                _ => (),
            }
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(SerJson, DeJson, Clone)]
struct ChatPrompt {
    pub messages: Vec<ChatMessage>,
    pub model: String,
    pub max_tokens: i32,
    pub stream: bool,
}

#[derive(SerJson, DeJson, Clone)]
struct ChatMessage {
    pub content: Option<String>,
    pub role: Option<String>,
    pub refusal: Option<JsonValue>,
}

#[allow(unused)]
#[derive(DeJson)]
struct ChatResponse {
    id: String,
    object: String,
    created: i32,
    model: String,
    system_fingerprint: JsonValue,
    usage: Option<ChatUsage>,
    choices: Vec<ChatChoice>,
}

#[allow(unused)]
#[derive(DeJson)]
pub struct CompletionDetails {
    reasoning_tokens: i32,
}

#[allow(unused)]
#[derive(DeJson)]
pub struct ChatUsage {
    prompt_tokens: i32,
    completion_tokens: i32,
    total_tokens: i32,
    completion_tokens_details: CompletionDetails,
}

#[allow(unused)]
#[derive(DeJson)]
struct ChatChoice {
    message: Option<ChatMessage>,
    delta: Option<ChatMessage>,
    finish_reason: Option<String>,
    index: i32,
    logprobs: Option<String>,
}
