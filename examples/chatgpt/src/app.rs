use crate::{network::*, makepad_live_id::*};
use makepad_micro_serde::*;
use makepad_widgets::*;

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

live_design!{
    import makepad_widgets::button::Button;
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::label::Label;
    import makepad_widgets::frame::Image;
    import makepad_widgets::text_input::TextInput;
    
    App = {{App}} {
        ui: <DesktopWindow>{
            
            show_bg: true
            layout: {
                flow: Down,
                spacing: 20,
                align: {
                    x: 0.5,
                    y: 0.5
                }
            },
            walk: {
                width: Fill,
                height: Fill
            },
            draw_bg: {
                
                fn pixel(self) -> vec4 {
                    return mix(#3, #1, self.geom_pos.y);
                }
            }
            message_label = <Label> {
                walk: {width: 300, height: Fit},
                draw_label: {
                    color: #f
                },
                label: "hi! how may I assist you today?",
            }
            message_input = <TextInput> {
                text: "Hi!"
                walk: {width: 300, height: Fit},
                draw_bg: {
                    color: #1
                }
            }
            send_button = <Button> {
               icon_walk:{margin:{left:10}, width:16,height:Fit}
               label: "send"
            }
        }
    }
}

app_main!(App);

#[derive(Live)]

pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl App{
    // This performs and event-based http request: it has no relationship with the response. 
    // The response will be received and processed by AppMain's handle_event.
    fn send_message(cx: &mut Cx, message: String) {
        let completion_url = format!("{}/chat/completions", OPENAI_BASE_URL);
        let request_id = LiveId::from_str("SendChatMessage").unwrap();
        let mut request = HttpRequest::new(request_id, completion_url, Method::POST);
        
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_header("Authorization".to_string(), "Bearer <your-token>".to_string());
        
        request.set_body(ChatPrompt {
            messages: vec![ Message { content: message, role: "user".to_string() } ],
            model: "gpt-3.5-turbo".to_string(),
            max_tokens: 100
        });

        cx.http_request(request);
    }
}

impl AppMain for App{
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.ui.draw_widget_all(&mut Cx2d::new(cx, event));
        }

        if let Event::HttpResponse(event) = event { 
            let label = self.ui.get_label(id!(message_label));

            event.response.id.as_string(|id: Option<&str>| {
                match id {
                     Some("SendChatMessage") => {
                        if event.response.status_code == 200 {
                            let chat_response = event.response.get_json_body_as::<ChatResponse>().unwrap();
                            label.set_label(&chat_response.choices[0].message.content);
                        } else {
                            label.set_label("Failed to connect with OpenAI");
                        }
                        label.redraw(cx);
                    },
                    _ => (),
                }
            })
        } else if let Event::HttpRequestError(event) = event {
            crate::makepad_error_log::log!("Request failed {:?}", event);
            let label = self.ui.get_label(id!(message_label));

            label.set_label("Failed to connect with OpenAI");
            label.redraw(cx);
        } else if let Event::HttpResponseProgress(event) = event {
            crate::makepad_error_log::log!("Request progress {:?}", event);
        } else if let Event::HttpUploadProgress(event) = event {
            crate::makepad_error_log::log!("Upload progress {:?}", event);
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        if self.ui.get_button(id!(send_button)).clicked(&actions) {
            let user_prompt = self.ui.get_text_input(id!(message_input)).get_text();
            Self::send_message(cx, user_prompt);
        }
    }
}

#[derive(SerJson, DeJson)]
struct ChatPrompt {
    pub messages: Vec<Message>,
    pub model: String,
    pub max_tokens: i32
}

#[derive(SerJson, DeJson)]
struct Message {
    pub content: String,
    pub role: String
}

#[derive(SerJson, DeJson)]
struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i32,
    pub model: String,
    pub usage: Usage,
    pub choices: Vec<Choice>,
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
}
