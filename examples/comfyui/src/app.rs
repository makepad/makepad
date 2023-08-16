use crate::{makepad_live_id::*};
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::fs;

const COMFYUI_BASE_URL: &str = "192.168.1.59:8188";

live_design!{
    import makepad_widgets::button::Button;
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::label::Label;
    import makepad_widgets::image::Image;
    import makepad_widgets::text_input::TextInput;
    
    App = {{App}} {
        ui: <DesktopWindow> {
            
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
                icon_walk: {margin: {left: 10}, width: 16, height: Fit}
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
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.open_web_socket(cx);
    }
}
const CLIENT_ID: &'static str = "1234";

impl App {
    // This performs and event-based http request: it has no relationship with the response.
    // The response will be received and processed by AppMain's handle_event.
    fn send_prompt(&self, cx: &mut Cx, text_input: String) {
        let url = format!("http://{}/prompt", COMFYUI_BASE_URL);
        let mut request = HttpRequest::new(url, HttpMethod::POST);
        
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        
        let ws = fs::read_to_string("examples/comfyui/workspace1.json").unwrap();
        let ws = ws.replace("CLIENT_ID", CLIENT_ID);
        let ws = ws.replace("TEXT_INPUT", &text_input);
        let ws = ws.replace("KEYWORD_INPUT", "");
        let ws = ws.replace("NEGATIVE_INPUT", "");
        
        request.set_body(ws.as_bytes().to_vec());
        
        cx.http_request(live_id!(SendPrompt), request);
    }
    
    fn open_web_socket(&self, cx: &mut Cx) {
        let url = format!("ws://{}/ws?clientId={}", COMFYUI_BASE_URL, CLIENT_ID);
        let request = HttpRequest::new(url, HttpMethod::GET);
        cx.web_socket_open(live_id!(Socket), request);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.ui.draw_widget_all(&mut Cx2d::new(cx, event));
        }
        
        for event in event.network_responses(){
            match &event.response{
                NetworkResponse::WebSocketString(s) => {
                    log!("Got String {}", s);
                }
                NetworkResponse::WebSocketBinary(bin) => {
                    log!("Got Binary {}", bin.len());
                }
                e => {
                    log!("{:?}", e)
                }
            }
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        if self.ui.get_button(id!(send_button)).clicked(&actions) {
            let user_prompt = self.ui.get_text_input(id!(message_input)).get_text();
            self.send_prompt(cx, user_prompt);
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
