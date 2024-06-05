use crate::makepad_live_id::*;
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::env;

const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_MODEL_ENV: &str = "OPENAI_MODEL";

live_design! {
    import makepad_widgets::theme_desktop_dark::*;

    App = {{App}} {
        ui: <Window> {body = {

            show_bg: true

            flow: Down,
            spacing: 20,
            align: {
                x: 0.5,
                y: 1.0
            },

            width: Fill,
            height: Fill

            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(#3, #1, self.pos.y);
                }
            }

            <ScrollXYView> {
                flow: Down,
                spacing: 20,
                align: {
                    x: 0.5,
                    y: 1.0
                },

                width: Fill,
                height: Fill

                message_label = <Label> {
                    width: 350,
                    height: Fit
                    draw_text: {
                        color: #f
                    },
                    text: "hi! how may I assist you today?",
                }
            }

            message_input = <TextInput> {
                text: "Hi!",
                empty_message: "Type a message...",
                width: 400,
                height: Fit
                draw_bg: {
                    color: #1
                }
            }

            send_button = <Button> {
                icon_walk: {margin: {left: 10}, width: 16, height: Fit}
                text: "send",
                margin: {
                    bottom: 10,
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
    conversation_history: Vec<Message>,
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
            let role_label = if message.role == "user" {
                "User:"
            } else {
                "Assistant:"
            };
            conversation_text.push_str(&format!("{}\n{}\n\n", role_label, message.content));
        }

        label.set_text_and_redraw(cx, &conversation_text);
    }

    // This performs an event-based HTTP request: it has no relationship with the response.
    // The response will be received and processed by AppMain's handle_event.
    fn send_message(&mut self, cx: &mut Cx, message: String) {
        let openai_base_url = env::var(OPENAI_BASE_URL_ENV)
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let openai_api_key = env::var(OPENAI_API_KEY_ENV).unwrap_or_else(|_| "".to_string());
        let openai_model = env::var(OPENAI_MODEL_ENV).unwrap_or_else(|_| "gpt-4o".to_string());

        if openai_api_key.is_empty() {
            eprintln!("Error: The OPENAI_API_KEY environment variable is not set.");
            std::process::exit(1);
        }

        // Add the user message to the conversation history
        self.conversation_history.push(Message {
            content: message.clone(),
            role: "user".to_string(),
        });

        // Update the gui
        self.update_message_label(cx);

        // Send the request
        let completion_url = format!("{}/chat/completions", openai_base_url);
        let request_id = live_id!(SendChatMessage);
        let mut request = HttpRequest::new(completion_url, HttpMethod::POST);

        request.set_header("Content-Type".to_string(), "application/json".to_string());
        if !openai_api_key.is_empty() {
            request.set_header(
                "Authorization".to_string(),
                format!("Bearer {}", openai_api_key),
            );
        }

        request.set_json_body(ChatPrompt {
            messages: self.conversation_history.clone(),
            model: openai_model,
            max_tokens: 1000,
        });

        cx.http_request(request_id, request);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let text_input = self.ui.text_input(id!(message_input));
        let send_button = self.ui.button(id!(send_button));

        if send_button.clicked(&actions) || text_input.returned(&actions).is_some() {
            self.send_message(cx, text_input.text());
            text_input.set_text_and_redraw(cx, "");
            text_input.set_cursor(0, 0);
        }
    }

    fn handle_network_responses(&mut self, cx: &mut Cx, responses: &NetworkResponsesEvent) {
        for event in responses {
            match &event.response {
                NetworkResponse::HttpResponse(response) => match event.request_id {
                    live_id!(SendChatMessage) => {
                        let label = self.ui.label(id!(message_label));
                        if response.status_code == 200 {
                            let chat_response = response.get_json_body::<ChatResponse>().unwrap();
                            let assistant_message =
                                chat_response.choices[0].message.content.clone();

                            self.conversation_history.push(Message {
                                content: assistant_message,
                                role: "assistant".to_string(),
                            });

                            self.update_message_label(cx);
                        } else {
                            label.set_text_and_redraw(cx, "Failed to connect with OpenAI");
                        }
                        label.redraw(cx);
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
    pub messages: Vec<Message>,
    pub model: String,
    pub max_tokens: i32,
}

#[derive(SerJson, DeJson, Clone)]
struct Message {
    pub content: String,
    pub role: String,
}

#[derive(SerJson, DeJson)]
struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i32,
    pub model: String,
    pub usage: Usage,
    pub choices: Vec<Choice>,
    pub system_fingerprint: Option<String>,
}

#[derive(SerJson, DeJson)]
pub struct Usage {
    prompt_tokens: i32,
    completion_tokens: i32,
    total_tokens: i32,
}

#[derive(SerJson, DeJson)]
struct Choice {
    message: Message,
    finish_reason: String,
    index: i32,
    logprobs: Option<String>,
}
