use crate::makepad_live_id::*;
use makepad_micro_serde::*;
use makepad_widgets::*;

const OPENAI_BASE_URL: &str = "https://makepad.nl/v1";

live_design!{
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
            
            message_label = <Label> {
                width: 300,
                height: Fit
                draw_text: {
                    color: #f
                },
                text: "hi! how may I assist you today?",
            }
            
            message_input = <TextInput> {
                text: "Hi!"
                width: 300,
                height: Fit
                draw_bg: {
                    color: #1
                }
            }
            
            send_button = <Button> {
                icon_walk: {margin: {left: 10}, width: 16, height: Fit}
                text: "send"
            }
        }}
    }
}

app_main!(App);

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl App {
    // This performs and event-based http request: it has no relationship with the response.
    // The response will be received and processed by AppMain's handle_event.
    fn send_message(cx: &mut Cx, message: String) {
        let completion_url = format!("{}/chat/completions", OPENAI_BASE_URL);
        let request_id = live_id!(SendChatMessage);
        let mut request = HttpRequest::new(completion_url, HttpMethod::POST);
        
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_header("Authorization".to_string(), "Bearer <your-token>".to_string());
        
        request.set_json_body(ChatPrompt {
            messages: vec![Message {content: message, role: "user".to_string()}],
            model: "gpt-3.5-turbo".to_string(),
            max_tokens: 100
        });
        
        cx.http_request(request_id, request);
    }
}

impl MatchEvent for App {

    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(send_button)).clicked(&actions) {
            let user_prompt = self.ui.text_input(id!(message_input)).text();
            Self::send_message(cx, user_prompt);
        }
    }
    
    fn handle_network_responses(&mut self, cx: &mut Cx, responses:&NetworkResponsesEvent ){
       for event in responses{
           match &event.response {
               NetworkResponse::HttpResponse(response) => {
                   let label = self.ui.label(id!(message_label));
                   match event.request_id {
                       live_id!(SendChatMessage) => {
                           if response.status_code == 200 {
                               let chat_response = response.get_json_body::<ChatResponse>().unwrap();
                               label.set_text_and_redraw(cx, &chat_response.choices[0].message.content);
                           } else {
                               label.set_text_and_redraw(cx, "Failed to connect with OpenAI");
                           }
                           label.redraw(cx);
                       },
                       _ => (),
                   }
               }
               NetworkResponse::HttpRequestError(error) => {
                   let label = self.ui.label(id!(message_label));
                   label.set_text_and_redraw(cx, &format!("Failed to connect with OpenAI {:?}", error));
               }
               _ => ()
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
