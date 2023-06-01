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
                    return mix(#7, #3, self.geom_pos.y);
                }
            }
                        
            message_input = <TextInput> {
                walk: {width: 100, height: 30},
                text: "Hi!"
            }
            message_label = <Label> {
                walk: {width: 200, height: 30},
                draw_label: {
                    color: #f
                },
                label: "hi! how may I assist you today?",
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
    
    #[rust] _user_message: String,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl App{
    // EVENT-BASED: sends message, has no relationship with response. Response will be received as an event
    // and processed by handle_event.
    fn send_message(cx: &mut Cx, ui: WidgetRef) {
        let completion_url = format!("{}/chat/completions", OPENAI_BASE_URL);
        let request_id = LiveId::from_str("SendChatMessage").unwrap();
        let mut request = HttpRequest::new(request_id, completion_url, Method::POST);
        
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_header("Authorization".to_string(), "Bearer <your-token>".to_string());
        
        let message = ui.get_text_input(id!(message_input)).get_text();
        request.set_body(ChatPrompt {
            messages: vec![ Message { content: message, role: "user".to_string() } ],
            model: "gpt-3.5-turbo".to_string(),
            max_tokens: 100
        });

        cx.http_request(request);
    }

    // ASYNC: sends message, awaits for response and updates the text value.
    async fn _send_message_async(_cx_ref: CxRef, _ui: WidgetRef){
        // simulate delay
        // std::thread::sleep(std::time::Duration::from_secs(4));
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
                            let chat_response = event.response.get_body::<ChatResponse>().unwrap();
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
            let label = self.ui.get_label(id!(message_label));

            makepad_error_log::log!("HTTP request error: {:?}", event.request_error);
            label.set_label("Failed to connect with OpenAI");
            label.redraw(cx);
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        if self.ui.get_button(id!(send_button)).clicked(&actions) {
            Self::send_message(cx, self.ui.clone()); // use cx.get_ref()?
        }
    }
}

#[derive(DeBin, SerBin, SerJson, DeJson, PartialEq, Debug)]
struct ChatPrompt {
    pub messages: Vec<Message>,
    pub model: String,
    pub max_tokens: i32
}

#[derive(DeBin, SerBin, SerJson, DeJson, PartialEq, Debug)]
struct Message {
    pub content: String,
    pub role: String
}

#[derive(DeBin, SerBin, SerJson, DeJson, PartialEq, Debug)]
struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i32,
    pub model: String,
    pub usage: Usage,
    pub choices: Vec<Choice>,
}

#[derive(DeBin, SerBin, SerJson, DeJson, PartialEq, Debug)]
pub struct Usage {
    prompt_tokens: i32,
    completion_tokens: i32,
    total_tokens: i32,
}

#[derive(DeBin, SerBin, SerJson, DeJson, PartialEq, Debug)]

struct Choice {
    message: Message,
    finish_reason: String,
    index: i32,
}
